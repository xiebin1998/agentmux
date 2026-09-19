use serde::{Deserialize, Serialize};
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
    pub compress_trigger_turns: Option<usize>,
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
            compress_trigger_turns: None,
            compress_trigger_chars: None,
        }
    }
}

pub fn config_path() -> PathBuf {
    let app_data = std::env::var("APPDATA")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_else(|_| ".".to_string());

    PathBuf::from(app_data).join("agentmux").join("settings.json")
}

pub fn data_dir() -> PathBuf {
    let app_data = std::env::var("APPDATA")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_else(|_| ".".to_string());

    PathBuf::from(app_data).join("agentmux").join("data")
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
    pub config_path: String,
    pub data_dir: String,
    pub archive_dir: String,
}

#[tauri::command]
pub async fn data_paths() -> Result<DataPaths, String> {
    let dir = data_dir();
    Ok(DataPaths {
        config_path: config_path().to_string_lossy().to_string(),
        archive_dir: dir.join("archive").to_string_lossy().to_string(),
        data_dir: dir.to_string_lossy().to_string(),
    })
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
        self_open_dingtalk_id: config.self_open_dingtalk_id.clone(),
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
/// - 自身身份（openDingTalkId）是用户级信息，取全局；
/// - Agent 启动参数与压缩策略目前仍是全局项。
pub fn reply_settings_for_project(project: &crate::project::Project) -> crate::reply::ReplySettings {
    let global = load_config().unwrap_or_default();

    crate::reply::ReplySettings {
        enabled: project.reply_enabled,
        agent_platform: project.agent_platform.clone(),
        agent_cli_path: Some(project.agent_cli_path.clone())
            .filter(|path| !path.trim().is_empty()),
        agent_args: global.agent_args.clone().filter(|args| !args.is_empty()),
        agent_cwd: project.work_dir.clone(),
        timeout_ms: project.reply_timeout_ms,
        max_chars: project.reply_max_chars,
        self_open_dingtalk_id: global.self_open_dingtalk_id.clone(),
        context_enabled: project.context_enabled,
        context_message_limit: project.context_message_limit,
        context_max_chars: project.context_max_chars,
        auto_compress: global.auto_compress,
        compress_trigger_turns: global.compress_trigger_turns,
        compress_trigger_chars: global.compress_trigger_chars,
    }
}
