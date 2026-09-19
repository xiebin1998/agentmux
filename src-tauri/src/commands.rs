use tauri::ipc::Channel;
use tauri::State;

use crate::orchestrator::{ListenerStatus, ListenerUpdate};
use crate::providers::ListenKind;
use crate::storage::{ConversationSummary, EventQuery, EventRow, Stats};
use crate::AppState;

#[tauri::command]
pub async fn start_listener(
    state: State<'_, AppState>,
    kind: String,
    channel: Channel<ListenerUpdate>,
) -> Result<String, String> {
    let parsed = ListenKind::parse(&kind).ok_or_else(|| format!("未知监听类型: {}", kind))?;
    let orchestrator = state.orchestrator.lock().await;
    orchestrator
        .start_listener(parsed, Some(channel))
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

#[tauri::command]
pub async fn list_conversations(state: State<'_, AppState>) -> Result<Vec<ConversationSummary>, String> {
    let storage = state.storage.lock().await;
    storage.list_conversations().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn reset_conversation(
    state: State<'_, AppState>,
    conversation_id: String,
) -> Result<(), String> {
    let storage = state.storage.lock().await;
    storage
        .delete_session(&conversation_id)
        .map_err(|e| e.to_string())
}

#[derive(serde::Serialize)]
pub struct AgentSessionInfo {
    pub conversation_id: String,
    pub agent_session_id: String,
    pub agent_cwd: String,
}

/// A6.1.2：查看会话关联的 Agent 会话状态。未建档返回 None。
#[tauri::command]
pub async fn conversation_session(
    state: State<'_, AppState>,
    conversation_id: String,
) -> Result<Option<AgentSessionInfo>, String> {
    let storage = state.storage.lock().await;
    let found = storage
        .get_session(&conversation_id)
        .map_err(|e| e.to_string())?;

    Ok(found.map(|(agent_session_id, agent_cwd)| AgentSessionInfo {
        conversation_id,
        agent_session_id,
        agent_cwd,
    }))
}

/// 回复引擎当前真正生效的设置（含自动解析出来的 Agent CLI）。
/// 用于界面上「设置是否已生效」的如实展示，而不是只回显 settings.json。
#[derive(serde::Serialize)]
pub struct RuntimeSettings {
    pub reply_enabled: bool,
    pub agent_platform: String,
    pub agent_cli_path: Option<String>,
    pub agent_args: Option<Vec<String>>,
    pub agent_cwd: String,
    pub timeout_ms: u64,
    pub max_chars: usize,
    pub context_enabled: bool,
    pub context_message_limit: usize,
    pub context_max_chars: usize,
    pub auto_compress: bool,
    pub compress_trigger_turns: Option<usize>,
    pub compress_trigger_chars: Option<usize>,
    pub self_open_dingtalk_id: Option<String>,
    pub im_cli_path: Option<String>,
}

fn to_runtime(settings: crate::reply::ReplySettings, im_cli: Option<String>) -> RuntimeSettings {
    RuntimeSettings {
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
        compress_trigger_turns: settings.compress_trigger_turns,
        compress_trigger_chars: settings.compress_trigger_chars,
        self_open_dingtalk_id: settings.self_open_dingtalk_id,
        im_cli_path: im_cli,
    }
}

/// 只读：查看当前生效的运行期设置。
#[tauri::command]
pub async fn runtime_settings(state: State<'_, AppState>) -> Result<RuntimeSettings, String> {
    let orchestrator = state.orchestrator.lock().await;
    let settings = orchestrator.reply_settings().await;
    let im_cli = orchestrator.dws_path().await;
    Ok(to_runtime(settings, im_cli))
}

/// 保存 settings.json 之后调用：让回复开关与预算立即生效，无需重启监听。
#[tauri::command]
pub async fn apply_settings(state: State<'_, AppState>) -> Result<RuntimeSettings, String> {
    let orchestrator = state.orchestrator.lock().await;
    let settings = orchestrator.refresh_reply_settings().await;
    let im_cli = orchestrator.dws_path().await;
    Ok(to_runtime(settings, im_cli))
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

/// 手动压缩（A7.1.3）。前端负责二次确认，后端不再弹确认。
#[tauri::command]
pub async fn compress_now(
    state: State<'_, AppState>,
    conversation_id: String,
) -> Result<crate::storage::Summary, String> {
    let orchestrator = state.orchestrator.lock().await;
    orchestrator
        .compress_conversation(&conversation_id)
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
