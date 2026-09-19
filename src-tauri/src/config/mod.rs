use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemeMode {
    Dark,
    Light,
    System,
}

impl Default for ThemeMode {
    fn default() -> Self {
        Self::System
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    /// 主题：深色 / 浅色 / 跟随系统。
    #[serde(default)]
    pub theme: ThemeMode,
    /// 选中的 IM 平台 id（注册表里的键），默认钉钉。
    #[serde(default = "default_im_platform")]
    pub im_platform: String,
    /// 选中的 Agent 平台 id。
    #[serde(default = "default_agent_platform")]
    pub agent_platform: String,
    /// 显式指定的 CLI 路径；为空时使用全局自动解析结果。
    #[serde(default)]
    pub im_cli_path: Option<String>,
    pub agent_cli_path: Option<String>,
    /// 自身在各 IM 平台上的身份 id（platform_id → open id），用于跳过自己发的消息。
    /// v1 之后 IM 不止钉钉，所以身份按平台存放，而不是一个全局值。
    #[serde(default)]
    pub im_identities: HashMap<String, String>,
    /// 旧字段：早期只支持钉钉时的自身身份。读取时作为 dingtalk 的回退。
    #[serde(default)]
    pub self_open_dingtalk_id: Option<String>,
    pub agent_cwd: Option<String>,
    /// Agent 启动参数覆盖（A2.2.4）。为空 = 用该平台的只读默认参数。
    #[serde(default)]
    pub agent_args: Option<Vec<String>>,
    /// 模型覆盖（-m）。空 = 用 CLI 默认模型。
    #[serde(default)]
    pub agent_model: Option<String>,
    pub reply_enabled: bool,
    pub reply_timeout_ms: u64,
    pub reply_max_chars: usize,
    pub context_enabled: bool,
    pub context_message_limit: usize,
    pub context_max_chars: usize,
    pub auto_compress: bool,
    /// 自动压缩阈值：上下文占预算的百分比（0-100）。界面用滑块设置。
    #[serde(default)]
    pub compress_trigger_percent: Option<u8>,
    #[serde(default)]
    pub compress_trigger_turns: Option<usize>,
    #[serde(default)]
    pub compress_trigger_chars: Option<usize>,
}

fn default_im_platform() -> String {
    "dingtalk".to_string()
}

fn default_agent_platform() -> String {
    "qoder".to_string()
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            theme: ThemeMode::default(),
            im_platform: default_im_platform(),
            agent_platform: default_agent_platform(),
            im_cli_path: None,
            agent_cli_path: None,
            im_identities: HashMap::new(),
            self_open_dingtalk_id: None,
            agent_cwd: None,
            agent_args: None,
            agent_model: None,
            reply_enabled: false,
            reply_timeout_ms: 120_000,
            reply_max_chars: 500,
            context_enabled: true,
            context_message_limit: 50,
            context_max_chars: 8000,
            auto_compress: false,
            compress_trigger_percent: None,
            compress_trigger_turns: None,
            compress_trigger_chars: None,
        }
    }
}

pub fn app_root() -> PathBuf {
    let base = std::env::var("APPDATA")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(base).join("agentmux")
}

/// 默认数据目录。
pub fn default_data_dir() -> PathBuf {
    app_root().join("data")
}

/// 记录数据目录改到哪了。**位置固定**，否则搬完就找不回来了。
fn location_file() -> PathBuf {
    app_root().join("location.json")
}

#[derive(Debug, Serialize, Deserialize)]
struct Location {
    data_dir: String,
    /// 换目录时记住「从哪搬」，否则连续搬两次会拿默认目录里的陈旧副本当源。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    previous: Option<String>,
}

fn configured_data_dir() -> Option<PathBuf> {
    let raw = fs::read_to_string(location_file()).ok()?;
    let parsed: Location = serde_json::from_str(&raw).ok()?;
    let path = PathBuf::from(parsed.data_dir.trim());
    if path.as_os_str().is_empty() {
        None
    } else {
        Some(path)
    }
}

/// 数据目录。可被用户改到别处（配置文件、数据库、归档都跟着走）。
pub fn data_dir() -> PathBuf {
    configured_data_dir().unwrap_or_else(default_data_dir)
}

/// 切换数据目录：只记录位置，搬迁在下次启动时做（那时数据库还没被打开，安全）。
pub fn persist_data_dir(path: &str) -> anyhow::Result<()> {
    let target = PathBuf::from(path.trim());
    if target.as_os_str().is_empty() {
        anyhow::bail!("数据目录不能为空");
    }
    let current = data_dir();
    fs::create_dir_all(&target)?;
    fs::create_dir_all(app_root())?;
    fs::write(
        location_file(),
        serde_json::to_string_pretty(&Location {
            data_dir: target.to_string_lossy().to_string(),
            previous: if current == target {
                None
            } else {
                Some(current.to_string_lossy().to_string())
            },
        })?,
    )?;
    Ok(())
}

/// 搬迁的源目录：优先用上一次的数据目录（连续换目录时它才是最新副本），
/// 没有记录时退回默认目录。
fn migrate_source() -> PathBuf {
    let previous = fs::read_to_string(location_file())
        .ok()
        .and_then(|raw| serde_json::from_str::<Location>(&raw).ok())
        .and_then(|parsed| parsed.previous)
        .map(|path| PathBuf::from(path.trim()))
        .filter(|path| !path.as_os_str().is_empty());

    pick_migrate_source(previous, default_data_dir())
}

/// 源目录选择：记录里的「上一次目录」里确实有数据库就用它，否则用默认目录。
fn pick_migrate_source(previous: Option<PathBuf>, fallback: PathBuf) -> PathBuf {
    match previous {
        Some(path) if path.join("agentmux.db").exists() => path,
        _ => fallback,
    }
}

/// 启动时的一次性搬迁：目标目录还没有数据库、而源目录有，就把源数据拷过去。
pub fn migrate_data_dir_on_startup() {
    let Some(target) = configured_data_dir() else {
        return;
    };
    let source = migrate_source();
    if target == source || target.join("agentmux.db").exists() || !source.join("agentmux.db").exists() {
        return;
    }

    let _ = copy_tree(&source, &target);
}

fn copy_tree(source: &PathBuf, target: &PathBuf) -> std::io::Result<()> {
    fs::create_dir_all(target)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let from = entry.path();
        let to = target.join(entry.file_name());
        if from.is_dir() {
            copy_tree(&from, &to)?;
        } else if !to.exists() {
            fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

pub fn config_path() -> PathBuf {
    // 配置文件跟数据目录放在一起，切换目录时一并搬走。
    data_dir().join("settings.json")
}

/// 老版本把 settings.json 放在 app_root 根下（数据目录之外）。改成「配置跟数据目录走」
/// 之后，若不认领这个老文件，升级上来的用户设置会被静默重置成默认值。
fn adopt_legacy_config() {
    let legacy = app_root().join("settings.json");
    let target = config_path();
    let _ = adopt_config_file(&legacy, &target);
}

/// 目标位置还没有配置、而老位置有，就把老配置复制过去。
fn adopt_config_file(legacy: &std::path::Path, target: &std::path::Path) -> std::io::Result<bool> {
    if target.exists() || !legacy.exists() {
        return Ok(false);
    }
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::copy(legacy, target)?;
    Ok(true)
}

pub fn load_config() -> anyhow::Result<AppConfig> {
    adopt_legacy_config();
    let path = config_path();
    if !path.exists() {
        return Ok(AppConfig::default());
    }
    let content = fs::read_to_string(path)?;
    let config: AppConfig = serde_json::from_str(&content)?;
    Ok(config)
}

pub fn save_config(config: &AppConfig) -> anyhow::Result<()> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let content = serde_json::to_string_pretty(config)?;
    fs::write(path, content)?;
    Ok(())
}

#[tauri::command]
pub async fn get_config() -> Result<AppConfig, String> {
    load_config().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn set_config(config: AppConfig) -> Result<(), String> {
    save_config(&config).map_err(|e| e.to_string())
}

#[derive(Debug, Clone, Serialize)]
pub struct DataPaths {
    /// 固定位置：只用来记录数据目录改到哪了（搬走后靠它找回来）
    pub app_root: String,
    pub data_dir: String,
    pub config_path: String,
    pub archive_dir: String,
    /// 当前是否在用默认数据目录
    pub is_default: bool,
}

fn paths_snapshot() -> DataPaths {
    let dir = data_dir();
    DataPaths {
        app_root: app_root().to_string_lossy().to_string(),
        config_path: config_path().to_string_lossy().to_string(),
        archive_dir: dir.join("archive").to_string_lossy().to_string(),
        is_default: dir == default_data_dir(),
        data_dir: dir.to_string_lossy().to_string(),
    }
}

#[tauri::command]
pub async fn data_paths() -> Result<DataPaths, String> {
    Ok(paths_snapshot())
}

/// 切换数据目录（配置文件、数据库、归档都跟着走）。
///
/// 这里只记录新位置；**实际搬迁在下次启动时做** —— 那时数据库还没被打开，
/// 复制文件才安全。重启后现有数据会一并出现在新目录。
#[tauri::command]
pub async fn set_data_dir(path: String) -> Result<DataPaths, String> {
    persist_data_dir(&path).map_err(|e| e.to_string())?;
    Ok(paths_snapshot())
}

/// 设置某个 IM 平台上的自身身份 id（用于跳过自己发的消息）。
/// 身份是**按平台**存的：以后接入别的 IM，各自有自己的身份。
#[tauri::command]
pub async fn set_im_identity(platform_id: String, identity: Option<String>) -> Result<(), String> {
    let mut config = load_config().unwrap_or_default();
    match identity.filter(|id| !id.trim().is_empty()) {
        Some(id) => {
            config.im_identities.insert(platform_id, id);
        }
        None => {
            config.im_identities.remove(&platform_id);
        }
    }
    save_config(&config).map_err(|e| e.to_string())
}

/// 把 settings.json 快照成回复引擎需要的配置。
/// Agent 工作目录留空时回退到程序配置目录下的专用子目录，而不是宿主任意目录。
///
/// **仅供手工实测的测试用**：真实链路走 `reply_settings_for_project`（按项目取），
/// 这里的 `enabled` 读的是全局字段，与项目级开关无关。
#[cfg(test)]
pub fn reply_settings() -> crate::reply::ReplySettings {
    let config = load_config().unwrap_or_default();

    let agent_cwd = config
        .agent_cwd
        .clone()
        .filter(|cwd| !cwd.trim().is_empty())
        .unwrap_or_else(|| data_dir().join("agent-cwd").to_string_lossy().to_string());

    crate::reply::ReplySettings {
        enabled: config.reply_enabled,
        agent_platform: config.agent_platform.clone(),
        agent_cli_path: config.agent_cli_path.clone(),
        agent_args: config
            .agent_args
            .clone()
            .filter(|args| !args.is_empty()),
        agent_model: config.agent_model.clone(),
        agent_cwd,
        timeout_ms: config.reply_timeout_ms,
        max_chars: config.reply_max_chars,
        self_open_id: config.self_open_dingtalk_id.clone(),
        context_enabled: config.context_enabled,
        context_message_limit: config.context_message_limit,
        context_max_chars: config.context_max_chars,
        auto_compress: config.auto_compress,
        compress_trigger_turns: config.compress_trigger_turns,
        compress_trigger_chars: config.compress_trigger_chars,
        compress_trigger_percent: config.compress_trigger_percent,
    }
}

/// IM CLI 的显式覆盖值；为空表示用全局自动解析结果。
pub fn configured_im_cli() -> Option<String> {
    load_config()
        .ok()
        .and_then(|config| config.im_cli_path)
        .filter(|path| !path.trim().is_empty())
}

/// 把「项目 + 全局设置」合并成该项目**实际生效**的回复设置。
///
/// **项目优先、全局兜底**：
/// - 工作目录必须用项目创建时指定的 `work_dir` —— 那是 Agent 的可见范围，
///   不能被全局值顶替；
/// - CLI、回复开关、超时、字数、上下文预算都取项目自己的；
/// - 自身身份按**项目所用的 IM 平台**取（不再是一个全局钉钉 id）；
/// - Agent 启动参数与压缩策略目前仍是全局项。
pub fn reply_settings_for_project(project: &crate::project::Project) -> crate::reply::ReplySettings {
    let global = load_config().unwrap_or_default();

    // 身份优先按平台查；旧的全局钉钉字段作为 dingtalk 的回退。
    let self_open_id = global
        .im_identities
        .get(&project.im_platform)
        .cloned()
        .filter(|id| !id.trim().is_empty())
        .or_else(|| {
            if project.im_platform == "dingtalk" {
                global
                    .self_open_dingtalk_id
                    .clone()
                    .filter(|id| !id.trim().is_empty())
            } else {
                None
            }
        });

    // 压缩阈值两条路：
    // 1. 百分比——Agent 回报过上下文占比后，直接按真实占比判定（自适应）；
    // 2. 字符数——由百分比按项目预算换算，只在还没有占比基线时兜底。
    let compress_trigger_percent = global.compress_trigger_percent.filter(|percent| *percent > 0);
    let compress_trigger_chars = compress_trigger_percent
        .map(|percent| {
            (project.context_max_chars as u64 * percent as u64 / 100).max(1) as usize
        })
        .or(global.compress_trigger_chars);

    crate::reply::ReplySettings {
        enabled: project.reply_enabled,
        agent_platform: project.agent_platform.clone(),
        agent_cli_path: Some(project.agent_cli_path.clone())
            .filter(|path| !path.trim().is_empty()),
        agent_args: global.agent_args.clone().filter(|args| !args.is_empty()),
        agent_model: global
            .agent_model
            .clone()
            .filter(|model| !model.trim().is_empty()),
        agent_cwd: project.work_dir.clone(),
        timeout_ms: project.reply_timeout_ms,
        max_chars: project.reply_max_chars,
        self_open_id,
        context_enabled: project.context_enabled,
        context_message_limit: project.context_message_limit,
        context_max_chars: project.context_max_chars,
        auto_compress: global.auto_compress,
        compress_trigger_turns: global.compress_trigger_turns,
        compress_trigger_chars,
        compress_trigger_percent,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::Project;

    fn sample_project(work_dir: &str) -> Project {
        Project::new(
            "示例项目".to_string(),
            work_dir.to_string(),
            r"C:\tools\qodercli\qodercli.exe".to_string(),
            r"C:\tools\dws\dws.exe".to_string(),
            "dingtalk".to_string(),
            "qoder".to_string(),
        )
    }

    /// 回归（问题 5）：驱动 Agent CLI 必须用**创建项目时指定的工作目录**，
    /// 不能被全局的 agent_cwd 顶替。
    #[test]
    fn project_settings_use_the_projects_work_dir() {
        let project = sample_project(r"D:\work\my-project");
        let settings = reply_settings_for_project(&project);

        assert_eq!(
            settings.agent_cwd, r"D:\work\my-project",
            "Agent 工作目录必须来自项目的 work_dir"
        );
        assert_eq!(
            settings.agent_cli_path.as_deref(),
            Some(r"C:\tools\qodercli\qodercli.exe")
        );
        assert_eq!(settings.agent_platform, "qoder");
        assert_eq!(settings.enabled, project.reply_enabled);
        assert_eq!(settings.max_chars, project.reply_max_chars);
        assert_eq!(settings.timeout_ms, project.reply_timeout_ms);
    }

    /// 两个不同项目解析出的工作目录必须各自独立。
    #[test]
    fn different_projects_resolve_to_their_own_work_dirs() {
        let a = reply_settings_for_project(&sample_project(r"D:\work\a"));
        let b = reply_settings_for_project(&sample_project(r"D:\work\b"));
        assert_eq!(a.agent_cwd, r"D:\work\a");
        assert_eq!(b.agent_cwd, r"D:\work\b");
    }

    /// 身份按**项目所用的 IM 平台**取；旧的全局钉钉字段作为回退。
    #[test]
    fn identity_comes_from_the_projects_im_platform() {
        let project = sample_project(r"D:\work\a"); // im_platform = "dingtalk"
        let settings = reply_settings_for_project(&project);
        let global = load_config().unwrap_or_default();

        let expected = global
            .im_identities
            .get("dingtalk")
            .cloned()
            .or_else(|| global.self_open_dingtalk_id.clone())
            .filter(|id| !id.trim().is_empty());

        assert_eq!(
            settings.self_open_id, expected,
            "钉钉项目的身份应取 im_identities[dingtalk]，缺失时回退旧字段"
        );
    }

    /// 压缩阈值由「上下文百分比」换算：80% × 预算。
    #[test]
    fn compression_threshold_is_derived_from_percentage() {
        let project = sample_project(r"D:\work\a");
        let settings = reply_settings_for_project(&project);
        let global = load_config().unwrap_or_default();

        match global.compress_trigger_percent.filter(|p| *p > 0) {
            Some(percent) => {
                let expected =
                    (project.context_max_chars as u64 * percent as u64 / 100).max(1) as usize;
                assert_eq!(settings.compress_trigger_chars, Some(expected));
            }
            None => assert_eq!(
                settings.compress_trigger_chars, global.compress_trigger_chars,
                "未设百分比时应沿用显式字符阈值"
            ),
        }
    }

    /// 回归：配置从 app_root 根下搬到数据目录里时，老文件必须被认领，
    /// 否则升级上来的用户会看到设置被重置成默认值。
    #[test]
    fn legacy_config_is_adopted_into_the_data_dir() {
        let root = std::env::temp_dir().join("agentmux-legacy-config-test");
        let _ = fs::remove_dir_all(&root);
        let legacy = root.join("settings.json");
        let target = root.join("data").join("settings.json");

        fs::create_dir_all(&root).unwrap();
        fs::write(&legacy, r#"{"theme":"light","agent_args":["--foo"]}"#).unwrap();

        assert!(adopt_config_file(&legacy, &target).unwrap(), "应认领老配置");
        assert_eq!(
            fs::read_to_string(&target).unwrap(),
            r#"{"theme":"light","agent_args":["--foo"]}"#
        );
        assert!(legacy.exists(), "老文件不删，保持可回退");

        // 已有目标配置时不再覆盖，避免把用户新设置冲掉。
        fs::write(&target, r#"{"theme":"dark"}"#).unwrap();
        assert!(!adopt_config_file(&legacy, &target).unwrap());
        assert_eq!(fs::read_to_string(&target).unwrap(), r#"{"theme":"dark"}"#);

        // 老文件不存在时不应凭空造配置。
        let absent = root.join("nope.json");
        let fresh = root.join("fresh").join("settings.json");
        assert!(!adopt_config_file(&absent, &fresh).unwrap());
        assert!(!fresh.exists());

        let _ = fs::remove_dir_all(&root);
    }

    /// 回归：连续换两次数据目录时，搬迁的源必须是「上一次的目录」，
    /// 否则第二次会把默认目录里的陈旧副本当成最新数据。
    #[test]
    fn chained_data_dir_moves_pick_the_previous_dir_as_source() {
        let root = std::env::temp_dir().join("agentmux-chained-move-test");
        let _ = fs::remove_dir_all(&root);
        let first = root.join("first");
        let stale_default = root.join("stale-default");
        fs::create_dir_all(&first).unwrap();
        fs::create_dir_all(&stale_default).unwrap();
        // 第一次搬走的目录里是最新库；默认目录留着的是搬走前的旧副本。
        fs::write(first.join("agentmux.db"), b"latest").unwrap();
        fs::write(stale_default.join("agentmux.db"), b"stale").unwrap();

        assert_eq!(
            pick_migrate_source(Some(first.clone()), stale_default.clone()),
            first,
            "有上一次目录且其中有库时，必须从它搬，不能退回默认目录"
        );

        // 上一次目录里没有库（比如被手工删了）→ 退回默认目录。
        let empty = root.join("empty");
        fs::create_dir_all(&empty).unwrap();
        assert_eq!(
            pick_migrate_source(Some(empty), stale_default.clone()),
            stale_default
        );

        // 从未换过目录 → 没有记录，用默认目录。
        assert_eq!(pick_migrate_source(None, stale_default.clone()), stale_default);

        let _ = fs::remove_dir_all(&root);
    }
}
