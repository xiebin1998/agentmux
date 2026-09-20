//! 判断当前模型是否真的能"看"图片。
//!
//! 为什么需要这个：CLI 没有能力查询接口（`--list-models` 只给名字，没有 vision 标记），
//! 也没有 vision 开关。而把图片路径交给一个不支持图片的模型，它**不一定会报错，
//! 更可能编** —— 实测这个模型在拿不到信息时会凭空列出不存在的文件。对一个以本人身份
//! 自动回复的机器人来说，"对没看过的图说得头头是道"是最坏的结果。
//!
//! 所以：探一次，按模型缓存；不支持就别把图片路径给出去。

use crate::reply::ReplySettings;

/// 探针图里写的校验码。**改图必须同步改这里**。
pub const PROBE_CODE: &str = "4821-7391";

/// 探针图（内置资源，避免运行时依赖任何绘图库）。
const PROBE_IMAGE: &[u8] = include_bytes!("../assets/vision-probe.png");

/// 探针图在临时工作目录里的文件名。
const PROBE_FILE: &str = "vision-probe.png";

/// 缓存键：按**配置的模型**区分（换模型要重新探）。
///
/// 用配置值而不是"上次实际用的模型名"：后者要探完才知道，那样就没法先查缓存。
/// 没配模型时用占位键，代价是别人改了 CLI 默认模型后缓存会过期 —— 很少见。
pub fn cache_key(configured_model: Option<&str>) -> String {
    match configured_model.map(str::trim).filter(|name| !name.is_empty()) {
        Some(model) => format!("vision_supported:{model}"),
        None => "vision_supported:<cli-default>".to_string(),
    }
}

/// 读缓存；`None` = 没探过这个模型。
pub fn cached(storage: &crate::storage::Storage, configured_model: Option<&str>) -> Option<bool> {
    storage
        .get_setting(&cache_key(configured_model))
        .ok()
        .flatten()
        .map(|value| value == "true")
}

/// 写回缓存。写失败不算错 —— 下次重探一遍而已。
pub fn remember(storage: &crate::storage::Storage, configured_model: Option<&str>, supported: bool) {
    let _ = storage.set_setting(&cache_key(configured_model), if supported { "true" } else { "false" });
}

/// 跑一次探针：把内置图放进一个临时工作目录，用**与回复完全相同**的参数问它图里写了什么。
///
/// 判据是**图上的校验码**：看不到图就不可能知道这串码，所以"回复里出现它"= 真能看图，
/// 比让模型自述可靠得多。探测失败（超时 / CLI 没配）一律按**不支持**处理 —— 宁可只说
/// "看不到"，也不要对着没读到的图乱讲。
pub async fn probe(settings: &ReplySettings) -> bool {
    let dir = std::env::temp_dir().join("agentmux-vision-probe");
    if std::fs::create_dir_all(&dir).is_err() {
        return false;
    }
    if std::fs::write(dir.join(PROBE_FILE), PROBE_IMAGE).is_err() {
        return false;
    }

    let mut probe_settings = settings.clone();
    // Agent 只能读它的工作目录，所以探针图必须落在 cwd 里。
    probe_settings.agent_cwd = dir.to_string_lossy().to_string();
    // 只问一张图，比整轮回复快得多；给足余量以防冷启动。
    probe_settings.timeout_ms = 60_000;
    if probe_settings.agent_cli_path.is_none() {
        probe_settings.agent_cli_path =
            crate::resolve::resolve_executable(&probe_settings.agent_platform).await;
        if probe_settings.agent_cli_path.is_none() {
            return false;
        }
    }

    let prompt = format!(
        "请用 Read 工具打开当前目录下的 {PROBE_FILE}，然后**只回答**图里写着的那串校验码，\
不要任何解释、不要复述问题。"
    );

    // 探针是一次内部校验，不推进度、不进会话窗口。
    match crate::reply::generate(&probe_settings, &prompt, None, false, None).await {
        Ok(generation) => generation.text.contains(PROBE_CODE),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_key_is_per_model() {
        assert_eq!(cache_key(Some("Qwen3.8-Max")), "vision_supported:Qwen3.8-Max");
        assert_eq!(cache_key(Some("  Qwen3.8-Max  ")), "vision_supported:Qwen3.8-Max");
        assert_eq!(cache_key(None), "vision_supported:<cli-default>");
        assert_eq!(cache_key(Some("   ")), "vision_supported:<cli-default>");
    }

    #[test]
    fn probe_image_is_embedded_and_non_empty() {
        assert!(PROBE_IMAGE.len() > 1000, "探针图应为真实 PNG，实际 {} 字节", PROBE_IMAGE.len());
        // PNG magic，确认是图不是占位文本
        assert_eq!(&PROBE_IMAGE[..4], b"\x89PNG");
    }

    /// 缓存读写走真实库（临时目录），并断言按模型区分。
    #[test]
    fn cache_round_trips_per_model() {
        let dir = std::env::temp_dir().join("agentmux-vision-cache-test");
        let _ = std::fs::remove_dir_all(&dir);
        let storage = crate::storage::Storage::new(dir.clone()).expect("建库");

        assert_eq!(cached(&storage, Some("m1")), None, "没探过应是 None");
        remember(&storage, Some("m1"), true);
        remember(&storage, Some("m2"), false);

        assert_eq!(cached(&storage, Some("m1")), Some(true));
        assert_eq!(cached(&storage, Some("m2")), Some(false), "不同模型互不影响");
        assert_eq!(cached(&storage, Some("m3")), None);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 真实环境：探针应当判定当前模型（`bailian/qwen3.7-plus-cp`）**支持**图片。
    ///
    /// 实测它能读出探针图上的校验码（早先用另一张写着 AMX-VISION-PROBE / 7391-KQTX
    /// 的图验证过）。默认 `#[ignore]`：会真调一次模型，约 10 秒。
    #[tokio::test]
    #[ignore]
    async fn real_probe_detects_vision_on_current_model() {
        let cli = crate::resolve::resolve_executable("qoder")
            .await
            .expect("本机应能解析到 qodercli");
        let settings = ReplySettings {
            agent_platform: "qoder".to_string(),
            agent_cli_path: Some(cli),
            ..Default::default()
        };

        assert!(
            probe(&settings).await,
            "当前模型实测能读图，探针不该判为不支持（判为不支持说明探针本身坏了）"
        );
    }
}