//! 会话元信息：群名 / 对方用户名 + 群聊单聊标记。
//!
//! 事件流（`dws event +listen-im` 的 NDJSON）里**只有 conversation_id**，
//! 没有名字也没有类型。实测（2026-09-19，dws）能补齐这两样的命令是
//! `dws chat +recent-conversations`：
//!
//! ```json
//! {"conversationId":"cidofTKV...==","name":"芬尼云业务异常通知",
//!  "nameKnown":true,"type":"group","latestMessageTime":"..."}
//! ```
//!
//! 其中 `type` 是 `group` / `direct`，`name` 群聊给群名、单聊给对方用户名，
//! 正好就是界面要显示的东西。

use std::process::Stdio;
use std::time::Duration;

use crate::storage::ConversationMeta;

/// 一次最多拉多久以前的会话。窗口越大翻页越多、越慢，默认 24 小时足够覆盖
/// 「最近在聊的会话」；更早的会话靠库里已有的缓存名字继续显示。
pub const DEFAULT_LOOKBACK_HOURS: i64 = 24;

/// 解析 `+recent-conversations` 的输出。
///
/// 容忍噪声：真机上 stdout 可能先出现非 JSON 的提示行，所以从第一个 `{` 开始解析。
pub fn parse_recent_conversations(stdout: &str) -> Vec<ConversationMeta> {
    let Some(start) = stdout.find('{') else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&stdout[start..]) else {
        return Vec::new();
    };

    let Some(items) = value
        .get("data")
        .and_then(|data| data.get("conversations"))
        .and_then(|conversations| conversations.as_array())
    else {
        return Vec::new();
    };

    items
        .iter()
        .filter_map(|item| {
            let conversation_id = item.get("conversationId")?.as_str()?.trim().to_string();
            if conversation_id.is_empty() {
                return None;
            }
            Some(ConversationMeta {
                conversation_id,
                name: item
                    .get("name")
                    .and_then(|name| name.as_str())
                    .unwrap_or_default()
                    .trim()
                    .to_string(),
                kind: normalize_kind(item.get("type").and_then(|kind| kind.as_str())),
                name_known: item
                    .get("nameKnown")
                    .and_then(|known| known.as_bool())
                    .unwrap_or(false),
            })
        })
        .collect()
}

/// 把 dws 的类型值收敛成界面认识的三档。
fn normalize_kind(raw: Option<&str>) -> String {
    match raw.unwrap_or_default().trim().to_lowercase().as_str() {
        "group" => "group".to_string(),
        "direct" | "single" | "user" | "c2c" => "direct".to_string(),
        _ => "unknown".to_string(),
    }
}

/// 拉一次会话列表。`dws_path` 是解析出来的可执行文件路径。
pub async fn fetch_conversation_meta(
    dws_path: &str,
    lookback_hours: i64,
) -> anyhow::Result<Vec<ConversationMeta>> {
    let end = chrono::Local::now();
    let start = end - chrono::Duration::hours(lookback_hours.max(1));

    let mut command = tokio::process::Command::new(dws_path);
    command
        .arg("chat")
        .arg("+recent-conversations")
        .arg("--start")
        .arg(start.format("%Y-%m-%d %H:%M:%S").to_string())
        .arg("--end")
        .arg(end.format("%Y-%m-%d %H:%M:%S").to_string())
        .arg("-f")
        .arg("json")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    crate::process::hide_console(&mut command);

    let child = command.spawn()?;
    let output = match tokio::time::timeout(Duration::from_secs(120), child.wait_with_output()).await
    {
        Ok(result) => result?,
        Err(_) => anyhow::bail!("拉取会话列表超时（120s）"),
    };

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!(
            "拉取会话列表失败 code={:?}: {}",
            output.status.code(),
            stderr.trim().chars().take(200).collect::<String>()
        );
    }

    Ok(parse_recent_conversations(&stdout))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 真实抓到的 `dws chat +recent-conversations` 输出片段（2026-09-19 实测）。
    /// 群聊给群名、单聊给对方用户名，两种类型都要认出来。
    const REAL_SAMPLE: &str = r#"{"ok":true,"outcome":"success","data":{"complete":true,"count":4,"pagesFetched":11,"unknownTypeCount":0,"conversations":[
{"conversationId":"cidofTKVhp3g2r7HPhexlwsBA==","latestMessageTime":"2026-09-19T20:45:54+08:00","name":"芬尼云业务异常通知","nameKnown":true,"type":"group"},
{"conversationId":"cidzKrnnlMdDKVYA2THk41RdyrN/sO//wq","latestMessageTime":"2026-09-19T20:40:00+08:00","name":"胡汉吟（Maren）","nameKnown":true,"type":"direct"},
{"conversationId":"cidMhW71NIsc40vPpWYV1omSQ==","latestMessageTime":"2026-09-19T20:38:08+08:00","name":"谢斌,王超平,谢斌","nameKnown":true,"type":"group"},
{"conversationId":"","name":"空 id 要丢掉","nameKnown":true,"type":"group"}
]},"meta":{}}"#;

    #[test]
    fn parses_real_dws_conversation_list() {
        let items = parse_recent_conversations(REAL_SAMPLE);

        assert_eq!(items.len(), 3, "空 conversationId 应被丢掉");
        assert_eq!(items[0].kind, "group");
        assert_eq!(items[0].name, "芬尼云业务异常通知");
        assert_eq!(items[1].kind, "direct");
        assert_eq!(items[1].name, "胡汉吟（Maren）", "单聊名就是对方用户名");
        assert_eq!(items[2].conversation_id, "cidMhW71NIsc40vPpWYV1omSQ==");
        assert!(items.iter().all(|item| item.name_known));
    }

    /// 名称未提供时 dws 会回空串 + nameKnown=false，不能把它当成有效名字。
    #[test]
    fn unknown_name_is_marked_not_known() {
        let raw = r#"{"data":{"conversations":[{"conversationId":"cid-x","name":"","nameKnown":false,"type":"group"}]}}"#;
        let items = parse_recent_conversations(raw);

        assert_eq!(items.len(), 1);
        assert!(!items[0].name_known);
        assert!(items[0].name.is_empty());
    }

    /// 不认识的类型值归到 unknown，界面据此不显示标签而不是显示错的标签。
    #[test]
    fn unknown_type_falls_back_to_unknown() {
        let raw = r#"{"data":{"conversations":[{"conversationId":"cid-y","name":"X","nameKnown":true,"type":"space"}]}}"#;
        let items = parse_recent_conversations(raw);
        assert_eq!(items[0].kind, "unknown");
    }

    /// 前面有噪声行、或整体不是 JSON 时都不能 panic。
    #[test]
    fn tolerates_noise_and_garbage() {
        let noisy = format!("connecting to bus...\n{}", REAL_SAMPLE);
        assert_eq!(parse_recent_conversations(&noisy).len(), 3);

        assert!(parse_recent_conversations("not json at all").is_empty());
        assert!(parse_recent_conversations("").is_empty());
    }
}
