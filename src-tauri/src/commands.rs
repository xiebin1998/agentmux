use tauri::ipc::Channel;
use tauri::{Emitter, Manager, State};

use crate::orchestrator::{ListenerStatus, ListenerUpdate, LogLine};
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
    pub context_message_limit: usize,
    /// Agent 最近一次回报的真实上下文占用比例（0~1）；没跑过则为 null。
    pub context_usage_ratio: Option<f64>,
    /// 模型的上下文窗口（token）。相当于「会话最大能有这么大」。
    /// 非 qoder 平台没有这个概念，为 null。
    pub context_window_tokens: Option<u64>,
    /// 窗口是哪来的：`session` = 从 Agent 会话文件读到的真实值，
    /// `default` = 读不到、用了实测默认值，`none` = 该平台不适用。
    pub context_window_source: String,
    /// 已用 token（≈ 占比 × 窗口）。CLI 只回占比、不回 token 数，所以只能换算。
    pub context_used_tokens: Option<u64>,
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

    let (meta, session_id, ratio, model) = {
        let storage = state.storage.lock().await;
        let meta = storage
            .conversation_meta(&conversation_id)
            .map_err(|e| e.to_string())?;
        let session = storage
            .get_session(&project_id, &conversation_id)
            .map_err(|e| e.to_string())?;
        // 从库里读：这轮数值是回复时落下来的，重启后照样在。
        let runtime = storage
            .session_runtime(&project_id, &conversation_id)
            .map_err(|e| e.to_string())?
            .unwrap_or((None, None));
        (
            meta,
            session.map(|(agent_session_id, _cwd)| agent_session_id),
            runtime.0,
            runtime.1,
        )
    };

    // 窗口：只有 qoder 的会话文件格式是已知的；读不到就退回实测默认值。
    let (context_window_tokens, context_window_source) =
        match (settings.agent_platform.as_str(), session_id.as_deref()) {
            ("qoder", Some(session_id)) => {
                match crate::reply::read_context_window_tokens(session_id) {
                    Some(window) => (Some(window), "session".to_string()),
                    None => (
                        Some(crate::reply::DEFAULT_CONTEXT_WINDOW_TOKENS),
                        "default".to_string(),
                    ),
                }
            }
            _ => (None, "none".to_string()),
        };

    // 绝对大小只能换算：实测 `-o json` 里的 token 字段恒为 0，有值的只有占比。
    let context_used_tokens = match (ratio, context_window_tokens) {
        (Some(ratio), Some(window)) => Some((ratio * window as f64).round() as u64),
        _ => None,
    };

    Ok(ConversationDetails {
        conversation_id: conversation_id.clone(),
        name: meta.as_ref().map(|m| m.name.clone()).unwrap_or_default(),
        kind: meta
            .as_ref()
            .map(|m| m.kind.clone())
            .unwrap_or_else(|| "unknown".to_string()),
        context_message_limit: settings.context_message_limit,
        context_usage_ratio: ratio,
        context_window_tokens,
        context_window_source,
        context_used_tokens,
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
    let trimmed = model.trim().to_string();
    let mut config = crate::config::load_config().unwrap_or_default();
    config.agent_model = if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    };
    crate::config::save_config(&config).map_err(|e| e.to_string())?;
    // 保存即热更新到在跑的监听，别让界面那句「下一条消息生效」变成空话。
    crate::project::push_settings_to_running_listeners(&state).await;
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
) -> Result<Vec<LogLine>, String> {
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

/// 事件与回复统计。传 project_id 只统计该项目，不传统计全部。
#[tauri::command]
pub async fn get_stats(
    state: State<'_, AppState>,
    project_id: Option<String>,
) -> Result<Stats, String> {
    let storage = state.storage.lock().await;
    storage
        .get_stats(project_id.as_deref())
        .map_err(|e| e.to_string())
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

/// 删除会话：从左树与统计里去掉（记墓碑），并清掉它的 Agent 会话记录。
/// 事件不删；之后收到新消息会话会自己回来。
#[tauri::command]
pub async fn delete_conversation(
    state: State<'_, AppState>,
    conversation_id: String,
) -> Result<(), String> {
    let storage = state.storage.lock().await;
    storage
        .delete_conversation(&conversation_id)
        .map_err(|e| e.to_string())
}

/// 建项目时「指定群 / 指定人」的候选名单（来自会话列表与历史发送人）。
#[tauri::command]
pub async fn list_source_candidates(
    state: State<'_, AppState>,
) -> Result<crate::storage::SourceCandidates, String> {
    let storage = state.storage.lock().await;
    storage.source_candidates().map_err(|e| e.to_string())
}

/// 按关键词去钉钉搜「群」或「人」。
///
/// 这是**显式动作**：只有用户点「搜索」才会起 dws 子进程。
/// `kind` 取 `group`（`dws chat +chat-search`）或 `member`（`dws contact +search-user`）。
#[tauri::command]
pub async fn search_scope_candidates(
    kind: String,
    query: String,
) -> Result<Vec<crate::project::ScopeEntry>, String> {
    let keyword = query.trim();
    if keyword.is_empty() {
        return Ok(Vec::new());
    }

    let Some(dws_path) = crate::resolve::resolve_executable("dingtalk").await else {
        return Err("没解析到 dws，无法搜索（可在「运行总览」点重新检测）".to_string());
    };

    match kind.as_str() {
        "group" => crate::conversations::search_groups(&dws_path, keyword, 20)
            .await
            .map_err(|e| e.to_string()),
        "member" => crate::conversations::search_people(&dws_path, keyword)
            .await
            .map_err(|e| e.to_string()),
        other => Err(format!("未知的搜索类型: {}", other)),
    }
}

/// 按 id 反查名字（群查会话表、人查历史发送人），给旧数据补上显示名。
#[tauri::command]
pub async fn resolve_scope_names(
    state: State<'_, AppState>,
    ids: Vec<String>,
) -> Result<Vec<crate::project::ScopeEntry>, String> {
    let storage = state.storage.lock().await;
    storage.resolve_scope_names(&ids).map_err(|e| e.to_string())
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

/// 一次更新检查的结果。
#[derive(serde::Serialize)]
pub struct UpdateInfo {
    /// 当前正在运行的版本。
    pub current_version: String,
    /// 远端可用的版本。
    pub version: String,
    /// 发布说明（Release 正文）。
    pub notes: Option<String>,
    /// 发布时间（latest.json 的 pub_date）。
    pub date: Option<String>,
}

/// 把更新相关的失败记进监听日志。
///
/// 更新检查**故意不打扰用户**（离线是常态），但「怎么一直不提示更新」必须有地方查，
/// 所以原因写进日志：被墙、签名配置不对、endpoint 写错，都会在这里现形。
async fn log_update(app: &tauri::AppHandle, line: &str) {
    let orchestrator = app.state::<AppState>().orchestrator.clone();
    orchestrator.lock().await.push_global_log(line).await;
}

/// 检查有没有新版本。**失败一律当作「没有更新」**，返回 Ok(None)。
#[tauri::command]
pub async fn check_update(app: tauri::AppHandle) -> Result<Option<UpdateInfo>, String> {
    use tauri_plugin_updater::UpdaterExt;

    let updater = match app.updater() {
        Ok(updater) => updater,
        Err(err) => {
            log_update(&app, &format!("检查更新不可用: {}", err)).await;
            return Ok(None);
        }
    };

    match updater.check().await {
        Ok(Some(update)) => Ok(Some(UpdateInfo {
            current_version: update.current_version.clone(),
            version: update.version.clone(),
            notes: update.body.clone(),
            date: update.date.map(|date| date.to_string()),
        })),
        Ok(None) => Ok(None),
        Err(err) => {
            log_update(&app, &format!("检查更新失败: {}", err)).await;
            Ok(None)
        }
    }
}

/// 下载并安装新版本。
///
/// **装之前必须先把监听收干净**：Windows 上安装器一启动就让本进程退出，**不走**
/// `RunEvent::ExitRequested`，留下的 `dws` 子进程会变成孤儿继续订阅、抢走事件。
#[tauri::command]
pub async fn install_update(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    use tauri_plugin_updater::UpdaterExt;

    let updater = app.updater().map_err(|e| e.to_string())?;
    let update = updater
        .check()
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "已经没有可用更新".to_string())?;

    {
        let orchestrator = state.orchestrator.lock().await;
        orchestrator.shutdown_all_listeners().await;
    }

    // 进度只在整数百分比变化时才发，否则一块一块地刷屏。
    let handle = app.clone();
    let mut last_percent = u64::MAX;
    update
        .download_and_install(
            move |downloaded, total| {
                let Some(total) = total.filter(|total| *total > 0) else {
                    return;
                };
                let percent = downloaded as u64 * 100 / total;
                if percent == last_percent {
                    return;
                }
                last_percent = percent;
                let _ = handle.emit("update-progress", percent);
            },
            {
                let handle = app.clone();
                move || {
                    // 下载完了，接下来是安装：安装器会接管并重启应用。
                    let _ = handle.emit("update-installing", ());
                }
            },
        )
        .await
        .map_err(|e| e.to_string())?;

    Ok(())
}
