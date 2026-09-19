use tauri::ipc::Channel;
use tauri::State;

use crate::orchestrator::{ListenerStatus, ListenerUpdate};
use crate::providers::ListenKind;
use crate::storage::{ConversationSummary, EventQuery, EventRow, Stats};
use crate::AppState;

fn data_dir() -> std::path::PathBuf {
    crate::config::data_dir()
}

fn load_project(project_id: &str) -> Result<crate::project::Project, String> {
    let store = crate::project::ProjectStore::new(data_dir()).map_err(|e| e.to_string())?;
    store
        .get(project_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("项目不存在: {}", project_id))
}

/// 解析某个项目实际生效的回复设置（必要时按平台自动解析 Agent CLI）。
async fn effective_settings(
    project: &crate::project::Project,
) -> crate::reply::ReplySettings {
    let mut settings = crate::config::reply_settings_for_project(project);
    if settings.enabled && settings.agent_cli_path.is_none() {
        settings.agent_cli_path =
            crate::resolve::resolve_executable(&settings.agent_platform).await;
    }
    settings
}

/// 拉一次会话元信息（群名/对方用户名 + 群聊单聊）。
///
/// 事件流里只有 conversation_id，名字和类型都得靠 dws 的会话列表补；
/// 返回这次写入的条数，界面据此提示「已刷新 N 个会话」。
#[tauri::command]
pub async fn refresh_conversation_meta(state: State<'_, AppState>) -> Result<usize, String> {
    let Some(dws_path) = crate::resolve::resolve_executable("dingtalk").await else {
        return Err("没解析到 dws，无法拉取会话列表".to_string());
    };
    let items = crate::conversations::fetch_conversation_meta(
        &dws_path,
        crate::conversations::DEFAULT_LOOKBACK_HOURS,
    )
    .await
    .map_err(|e| e.to_string())?;

    let storage = state.storage.lock().await;
    storage
        .upsert_conversations(&items)
        .map_err(|e| e.to_string())
}

/// 会话窗口要显示的一组信息：名称/类型 + 上下文用量 + 当前模型。
#[derive(serde::Serialize)]
pub struct ConversationDetails {
    pub conversation_id: String,
    pub name: String,
    pub kind: String,
    /// 上下文预算（字符，来自项目配置）。
    pub context_budget_chars: usize,
    /// 当前会话累计的字符数（上下文就是从这里取的）。
    pub context_used_chars: usize,
    pub context_message_limit: usize,
    /// Agent 最近一次回报的真实上下文占用比例（0~1）；没跑过则为 null。
    pub context_usage_ratio: Option<f64>,
    /// 压缩阈值百分比（滑块值）；未设置则为 null。
    pub compress_trigger_percent: Option<u8>,
    /// Agent 最近一次实际用的模型；没跑过则为 null。
    pub model: Option<String>,
    /// 配置里显式指定的模型（-m）；空 = 用 CLI 默认。
    pub model_override: Option<String>,
}

#[tauri::command]
pub async fn conversation_details(
    state: State<'_, AppState>,
    project_id: String,
    conversation_id: String,
) -> Result<ConversationDetails, String> {
    let project = load_project(&project_id)?;
    let settings = effective_settings(&project).await;

    let (meta, used_chars) = {
        let storage = state.storage.lock().await;
        let meta = storage
            .conversation_meta(&conversation_id)
            .map_err(|e| e.to_string())?;
        let rows = storage
            .recent_messages(&conversation_id, settings.context_message_limit)
            .unwrap_or_default();
        let used: usize = rows
            .iter()
            .map(|(sender, content)| sender.chars().count() + content.chars().count())
            .sum();
        (meta, used)
    };

    let (ratio, model) = {
        let orchestrator = state.orchestrator.lock().await;
        orchestrator
            .conversation_runtime(&conversation_id)
            .await
    };

    Ok(ConversationDetails {
        conversation_id: conversation_id.clone(),
        name: meta.as_ref().map(|m| m.name.clone()).unwrap_or_default(),
        kind: meta
            .as_ref()
            .map(|m| m.kind.clone())
            .unwrap_or_else(|| "unknown".to_string()),
        context_budget_chars: project.context_max_chars,
        context_used_chars: used_chars,
        context_message_limit: settings.context_message_limit,
        context_usage_ratio: ratio,
        compress_trigger_percent: settings.compress_trigger_percent,
        model,
        model_override: settings.agent_model,
    })
}

/// 列出 Agent CLI 支持切换的模型（qodercli --list-models）。
#[tauri::command]
pub async fn list_agent_models(
    state: State<'_, AppState>,
    project_id: String,
) -> Result<Vec<String>, String> {
    let project = load_project(&project_id)?;
    let settings = effective_settings(&project).await;
    let _ = state;
    Ok(crate::reply::list_available_models(&settings).await)
}

/// 设置模型覆盖。空字符串表示恢复「用 CLI 默认模型」。
#[tauri::command]
pub async fn set_agent_model(
    state: State<'_, AppState>,
    model: String,
) -> Result<Option<String>, String> {
    let _ = state;
    let trimmed = model.trim().to_string();
    let mut config = crate::config::load_config().unwrap_or_default();
    config.agent_model = if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    };
    crate::config::save_config(&config).map_err(|e| e.to_string())?;
    Ok(config.agent_model)
}

/// 启动某项目的一路监听（@我 / 单聊各自独立，可并发）。
#[tauri::command]
pub async fn start_listener(
    state: State<'_, AppState>,
    project_id: String,
    kind: String,
    channel: Channel<ListenerUpdate>,
) -> Result<String, String> {
    let parsed = ListenKind::parse(&kind).ok_or_else(|| format!("未知监听类型: {}", kind))?;
    let project = load_project(&project_id)?;
    let settings = effective_settings(&project).await;

    let orchestrator = state.orchestrator.lock().await;
    orchestrator
        .start_listener(project_id, parsed, settings, Some(channel))
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn stop_listener(state: State<'_, AppState>, id: String) -> Result<String, String> {
    let orchestrator = state.orchestrator.lock().await;
    orchestrator
        .stop_listener(&id)
        .await
        .map_err(|e| e.to_string())
}

/// 列出全部监听实例（含所属 project_id，界面按项目分组）。
#[tauri::command]
pub async fn listener_status(state: State<'_, AppState>) -> Result<Vec<ListenerStatus>, String> {
    let orchestrator = state.orchestrator.lock().await;
    Ok(orchestrator.get_all_listener_status().await)
}

#[tauri::command]
pub async fn listener_logs(
    state: State<'_, AppState>,
    limit: Option<usize>,
) -> Result<Vec<String>, String> {
    let orchestrator = state.orchestrator.lock().await;
    Ok(orchestrator.get_logs(limit.unwrap_or(500)).await)
}

#[tauri::command]
pub async fn clear_listener_logs(state: State<'_, AppState>) -> Result<(), String> {
    let orchestrator = state.orchestrator.lock().await;
    orchestrator.clear_logs().await;
    Ok(())
}

/// 强制重新解析 IM CLI（提供方页的「重新检测」会用到）。
#[tauri::command]
pub async fn reset_im_cli(state: State<'_, AppState>) -> Result<Option<String>, String> {
    let orchestrator = state.orchestrator.lock().await;
    orchestrator.set_dws_path(None).await;
    match orchestrator.ensure_dws_path().await {
        Ok(path) => Ok(Some(path)),
        Err(_) => Ok(None),
    }
}

/// 指定 IM CLI（用户在单选里选中某个检测结果时调用）。
#[tauri::command]
pub async fn set_im_cli(state: State<'_, AppState>, path: String) -> Result<(), String> {
    let orchestrator = state.orchestrator.lock().await;
    orchestrator.set_dws_path(Some(path)).await;
    Ok(())
}

#[tauri::command]
pub async fn current_im_cli(state: State<'_, AppState>) -> Result<Option<String>, String> {
    let orchestrator = state.orchestrator.lock().await;
    Ok(orchestrator.dws_path().await)
}

#[tauri::command]
pub async fn get_stats(state: State<'_, AppState>) -> Result<Stats, String> {
    let storage = state.storage.lock().await;
    storage.get_stats().map_err(|e| e.to_string())
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn list_events(
    state: State<'_, AppState>,
    limit: Option<usize>,
    offset: Option<usize>,
    project_id: Option<String>,
    conversation_id: Option<String>,
    sender: Option<String>,
    keyword: Option<String>,
    malformed_only: Option<bool>,
    failed_only: Option<bool>,
    since_date: Option<String>,
    until_date: Option<String>,
) -> Result<Vec<EventRow>, String> {
    // A4.2.1：查询跨度上限 31 天。
    if let (Some(since), Some(until)) = (since_date.as_deref(), until_date.as_deref()) {
        if let (Ok(since), Ok(until)) = (
            chrono::NaiveDate::parse_from_str(since, "%Y-%m-%d"),
            chrono::NaiveDate::parse_from_str(until, "%Y-%m-%d"),
        ) {
            let span = (until - since).num_days();
            if span > 31 {
                return Err(format!("查询跨度 {} 天，超过 31 天上限", span));
            }
            if span < 0 {
                return Err("结束日期早于开始日期".to_string());
            }
        }
    }

    let storage = state.storage.lock().await;
    storage
        .list_events(&EventQuery {
            limit: limit.unwrap_or(200),
            offset: offset.unwrap_or(0),
            project_id,
            conversation_id,
            sender,
            keyword,
            malformed_only: malformed_only.unwrap_or(false),
            failed_only: failed_only.unwrap_or(false),
            since_date,
            until_date,
        })
        .map_err(|e| e.to_string())
}

/// 会话汇总。
/// - 传 project_id：只列该项目的会话（左树「项目下挂会话」）
/// - unassigned = true：只列**没有项目归属**的历史会话（升级前的数据）
#[tauri::command]
pub async fn list_conversations(
    state: State<'_, AppState>,
    project_id: Option<String>,
    unassigned: Option<bool>,
) -> Result<Vec<ConversationSummary>, String> {
    let storage = state.storage.lock().await;
    storage
        .list_conversations(project_id.as_deref(), unassigned.unwrap_or(false))
        .map_err(|e| e.to_string())
}

/// 把没有归属的历史会话归入某个项目，让它出现在该项目的树里。
#[tauri::command]
pub async fn assign_conversation(
    state: State<'_, AppState>,
    conversation_id: String,
    project_id: String,
) -> Result<usize, String> {
    let storage = state.storage.lock().await;
    storage
        .assign_conversation(&conversation_id, &project_id)
        .map_err(|e| e.to_string())
}

#[derive(serde::Serialize)]
pub struct AgentSessionInfo {
    pub project_id: String,
    pub conversation_id: String,
    pub agent_session_id: String,
    pub agent_cwd: String,
}

/// A6.1.2：查看某项目下会话关联的 Agent 会话状态。未建档返回 None。
#[tauri::command]
pub async fn conversation_session(
    state: State<'_, AppState>,
    project_id: String,
    conversation_id: String,
) -> Result<Option<AgentSessionInfo>, String> {
    let storage = state.storage.lock().await;
    let found = storage
        .get_session(&project_id, &conversation_id)
        .map_err(|e| e.to_string())?;

    Ok(found.map(|(agent_session_id, agent_cwd)| AgentSessionInfo {
        project_id,
        conversation_id,
        agent_session_id,
        agent_cwd,
    }))
}

#[tauri::command]
pub async fn reset_conversation(
    state: State<'_, AppState>,
    project_id: String,
    conversation_id: String,
) -> Result<(), String> {
    let storage = state.storage.lock().await;
    storage
        .delete_session(&project_id, &conversation_id)
        .map_err(|e| e.to_string())
}

/// 该项目实际生效的运行期设置（供界面如实展示，而不是回显 settings.json）。
#[derive(serde::Serialize)]
pub struct RuntimeSettings {
    pub project_id: String,
    pub reply_enabled: bool,
    pub agent_platform: String,
    pub agent_cli_path: Option<String>,
    pub agent_args: Option<Vec<String>>,
    /// 实际驱动 Agent 的工作目录 —— 来自项目的 work_dir。
    pub agent_cwd: String,
    pub timeout_ms: u64,
    pub max_chars: usize,
    pub context_enabled: bool,
    pub context_message_limit: usize,
    pub context_max_chars: usize,
    pub auto_compress: bool,
    pub compress_trigger_percent: Option<u8>,
    pub compress_trigger_turns: Option<usize>,
    pub compress_trigger_chars: Option<usize>,
    pub self_open_id: Option<String>,
    pub im_cli_path: Option<String>,
}

fn to_runtime(
    project_id: String,
    settings: crate::reply::ReplySettings,
    im_cli: Option<String>,
    percent: Option<u8>,
) -> RuntimeSettings {
    RuntimeSettings {
        project_id,
        reply_enabled: settings.enabled,
        agent_platform: settings.agent_platform,
        agent_cli_path: settings.agent_cli_path,
        agent_args: settings.agent_args,
        agent_cwd: settings.agent_cwd,
        timeout_ms: settings.timeout_ms,
        max_chars: settings.max_chars,
        context_enabled: settings.context_enabled,
        context_message_limit: settings.context_message_limit,
        context_max_chars: settings.context_max_chars,
        auto_compress: settings.auto_compress,
        compress_trigger_percent: percent,
        compress_trigger_turns: settings.compress_trigger_turns,
        compress_trigger_chars: settings.compress_trigger_chars,
        self_open_id: settings.self_open_id,
        im_cli_path: im_cli,
    }
}

#[tauri::command]
pub async fn project_runtime_settings(
    state: State<'_, AppState>,
    project_id: String,
) -> Result<RuntimeSettings, String> {
    let project = load_project(&project_id)?;
    let settings = effective_settings(&project).await;
    let percent = crate::config::load_config()
        .ok()
        .and_then(|config| config.compress_trigger_percent);
    let im_cli = {
        let orchestrator = state.orchestrator.lock().await;
        orchestrator.dws_path().await
    };
    Ok(to_runtime(project_id, settings, im_cli, percent))
}

#[tauri::command]
pub async fn get_summary(
    state: State<'_, AppState>,
    conversation_id: String,
) -> Result<Option<crate::storage::Summary>, String> {
    let storage = state.storage.lock().await;
    storage
        .get_summary(&conversation_id)
        .map_err(|e| e.to_string())
}

/// 手动压缩（A7.1.3）。压缩要走 Agent，所以需要项目来决定用哪个 CLI 与工作目录。
#[tauri::command]
pub async fn compress_now(
    state: State<'_, AppState>,
    project_id: String,
    conversation_id: String,
) -> Result<crate::storage::Summary, String> {
    let project = load_project(&project_id)?;
    let settings = effective_settings(&project).await;

    let orchestrator = state.orchestrator.lock().await;
    orchestrator
        .compress_conversation(settings, &conversation_id)
        .await
        .map_err(|e| e.to_string())
}

/// 编辑摘要（A7.2.2）；v1 只保留最新一版。
#[tauri::command]
pub async fn update_summary(
    state: State<'_, AppState>,
    conversation_id: String,
    content: String,
) -> Result<(), String> {
    let storage = state.storage.lock().await;
    let source_events = storage
        .count_events(&conversation_id)
        .map_err(|e| e.to_string())?;
    storage
        .save_summary(&conversation_id, &content, source_events)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn delete_summary(
    state: State<'_, AppState>,
    conversation_id: String,
) -> Result<(), String> {
    let storage = state.storage.lock().await;
    storage
        .delete_summary(&conversation_id)
        .map_err(|e| e.to_string())
}
