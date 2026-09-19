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
    fs::create_dir_all(&target)?;
    fs::create_dir_all(app_root())?;
    fs::write(
        location_file(),
        serde_json::to_string_pretty(&Location {
            data_dir: target.to_string_lossy().to_string(),
        })?,
    )?;
    Ok(())
}

/// 启动时的一次性搬迁：目标目录还没有数据库、而旧目录有，就把旧数据拷过去。
pub fn migrate_data_dir_on_startup() {
    let Some(target) = configured_data_dir() else {
        return;
    };
    let source = default_data_dir();
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

pub fn load_config() -> anyhow::Result<AppConfig> {
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

    // 压缩阈值：滑块给的是「上下文占预算的百分比」，换算成字符数交给压缩判定。
    let compress_trigger_chars = global
        .compress_trigger_percent
        .filter(|percent| *percent > 0)
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
}
