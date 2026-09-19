use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tauri::ipc::Channel;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{ChildStdin, Command};
use tokio::sync::Mutex;
use tokio::time::{sleep, Instant};

use crate::providers::ListenKind;
use crate::reply::ReplySettings;
use crate::storage::Storage;

const READY_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_ATTEMPTS: u32 = 5;
const BACKOFF_SECS: [u64; 5] = [5, 10, 20, 40, 60];
const LOG_BUFFER_LIMIT: usize = 5000;
const READY_BUFFER_LIMIT: usize = 1000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ListenerState {
    Stopped,
    Starting,
    Running,
    BackingOff,
    Abandoned,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListenerStatus {
    pub id: String,
    /// 该监听属于哪个项目。
    pub project_id: String,
    pub kind: ListenKind,
    pub state: ListenerState,
    pub ready: bool,
    pub subscribe_id: Option<String>,
    pub bus_pid: Option<u32>,
    pub attempts: u32,
    pub last_error: Option<String>,
    pub cli_path: Option<String>,
    pub dropped_before_ready: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatEvent {
    /// 归属于哪个项目（哪个项目的监听收到的就归谁）。
    pub project_id: String,
    pub message_id: String,
    pub conversation_id: String,
    pub sender: String,
    pub sender_open_dingtalk_id: String,
    pub content: String,
    pub create_time: String,
    pub received_at: String,
    pub listen_kind: String,
    pub malformed: bool,
    pub raw: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ListenerUpdate {
    Status { status: ListenerStatus },
    Event { event: ChatEvent },
    Log { listener_id: String, line: String },
}

struct ListenerTask {
    status: ListenerStatus,
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    pid: Arc<Mutex<Option<u32>>>,
    stop_requested: Arc<AtomicBool>,
}

struct LogBuffer {
    lines: VecDeque<String>,
}

pub struct Orchestrator {
    listeners: Arc<Mutex<HashMap<String, ListenerTask>>>,
    handled: Arc<Mutex<HashSet<String>>>,
    logs: Arc<Mutex<LogBuffer>>,
    storage: Arc<Mutex<Storage>>,
    dws_path: Arc<Mutex<Option<String>>>,
    /// 回复全局串行：一次只处理一条（与旧基准工程一致）。
    reply_lock: Arc<Mutex<()>>,
    /// 按会话攒批：窗口内收到的多条消息合并成一条回复，避免连发 n 条就回 n 条。
    /// 放在 Orchestrator 上而不是每个 listener 上：同一会话可能被同一个项目的
    /// 「@我」和「单聊」两路监听同时看到，各自攒批会重复回复。
    reply_batches: Arc<Mutex<HashMap<String, Vec<ChatEvent>>>>,
    /// 已在等窗口的会话，避免同一会话排多个 flush 任务。
    reply_inflight: Arc<Mutex<HashSet<String>>>,
    /// 攒批窗口长度，见 `DEFAULT_REPLY_BATCH_WINDOW`。
    reply_batch_window: Duration,
}

/// 攒批窗口：收到第一条后等这么久，把期间到达的同会话消息并成一次回复。
/// 太短收不齐（对方连着打字），太长显得反应慢。
const DEFAULT_REPLY_BATCH_WINDOW: Duration = Duration::from_secs(8);

impl Orchestrator {
    pub fn new(storage: Arc<Mutex<Storage>>) -> Self {
        Self {
            listeners: Arc::new(Mutex::new(HashMap::new())),
            handled: Arc::new(Mutex::new(HashSet::new())),
            logs: Arc::new(Mutex::new(LogBuffer { lines: VecDeque::new() })),
            storage,
            dws_path: Arc::new(Mutex::new(None)),
            reply_lock: Arc::new(Mutex::new(())),
            reply_batches: Arc::new(Mutex::new(HashMap::new())),
            reply_inflight: Arc::new(Mutex::new(HashSet::new())),
            reply_batch_window: DEFAULT_REPLY_BATCH_WINDOW,
        }
    }

    /// 缩短攒批窗口：端到端测试不该为生产用的 8 秒窗口白等。
    #[cfg(test)]
    pub fn set_reply_batch_window(&mut self, window: Duration) {
        self.reply_batch_window = window;
    }

    pub async fn push_global_log(&self, line: &str) {
        let stamped = format!("{} {}", chrono::Local::now().format("%H:%M:%S"), line);
        let mut logs = self.logs.lock().await;
        if logs.lines.len() >= LOG_BUFFER_LIMIT {
            logs.lines.pop_front();
        }
        logs.lines.push_back(stamped);
    }

    /// 压缩某会话的上下文并落盘为最新一版摘要（A7.1.3 / A7.2.1）。
    /// 压缩要走 Agent，因此需要调用方传入该会话所属项目的回复设置。
    pub async fn compress_conversation(
        &self,
        settings: ReplySettings,
        conversation_id: &str,
    ) -> anyhow::Result<crate::storage::Summary> {
        let summary = compress_with(&self.storage, settings, conversation_id).await?;
        self.push_global_log(&format!("已压缩会话 {} 的上下文", conversation_id))
            .await;
        Ok(summary)
    }

    /// 全局自动解析 IM CLI；settings.json 里的显式覆盖优先。结果缓存，直到显式重设。
    pub async fn ensure_dws_path(&self) -> anyhow::Result<String> {
        if let Some(path) = self.dws_path.lock().await.clone() {
            return Ok(path);
        }
        if let Some(override_path) = crate::config::configured_im_cli() {
            *self.dws_path.lock().await = Some(override_path.clone());
            return Ok(override_path);
        }
        let resolved = crate::resolve::resolve_executable("dingtalk")
            .await
            .ok_or_else(|| anyhow::anyhow!("未检测到 dws 可执行文件，请先在提供方页执行重新检测"))?;
        *self.dws_path.lock().await = Some(resolved.clone());
        Ok(resolved)
    }

    pub async fn set_dws_path(&self, path: Option<String>) {
        *self.dws_path.lock().await = path;
    }

    pub async fn dws_path(&self) -> Option<String> {
        self.dws_path.lock().await.clone()
    }

    /// 启动一路监听。**监听属于某个项目**：收到的事件归该项目，回复用该项目的设置
    /// （工作目录、CLI、开关等）。`reply` 由调用方按项目解析后传入。
    pub async fn start_listener(
        &self,
        project_id: String,
        kind: ListenKind,
        reply: ReplySettings,
        channel: Option<Channel<ListenerUpdate>>,
    ) -> anyhow::Result<String> {
        let path = self.ensure_dws_path().await?;
        let id = uuid::Uuid::new_v4().to_string();

        let stdin = Arc::new(Mutex::new(None));
        let pid = Arc::new(Mutex::new(None));
        let stop_requested = Arc::new(AtomicBool::new(false));

        let task = ListenerTask {
            status: ListenerStatus {
                id: id.clone(),
                project_id: project_id.clone(),
                kind,
                state: ListenerState::Starting,
                ready: false,
                subscribe_id: None,
                bus_pid: None,
                attempts: 0,
                last_error: None,
                cli_path: Some(path.clone()),
                dropped_before_ready: 0,
            },
            stdin: stdin.clone(),
            pid: pid.clone(),
            stop_requested: stop_requested.clone(),
        };

        {
            let mut listeners = self.listeners.lock().await;
            listeners.insert(id.clone(), task);
        }

        let shared = Shared {
            id: id.clone(),
            project_id,
            kind,
            path,
            listeners: self.listeners.clone(),
            handled: self.handled.clone(),
            logs: self.logs.clone(),
            storage: self.storage.clone(),
            channel,
            stdin,
            pid,
            stop_requested,
            pending: Arc::new(Mutex::new(VecDeque::new())),
            reply,
            reply_lock: self.reply_lock.clone(),
            reply_batches: self.reply_batches.clone(),
            reply_inflight: self.reply_inflight.clone(),
            reply_batch_window: self.reply_batch_window,
            compress_failures: Arc::new(Mutex::new(0)),
            malformed_streak: Arc::new(Mutex::new(0)),
        };

        tokio::spawn(async move { shared.run().await });

        Ok(id)
    }

    pub async fn stop_listener(&self, id: &str) -> anyhow::Result<String> {
        let (stdin, pid, stop_requested) = {
            let listeners = self.listeners.lock().await;
            let task = listeners
                .get(id)
                .ok_or_else(|| anyhow::anyhow!("监听不存在: {}", id))?;
            (
                task.stdin.clone(),
                task.pid.clone(),
                task.stop_requested.clone(),
            )
        };

        stop_requested.store(true, Ordering::SeqCst);

        // 停机阶梯：先关 stdin（让 dws 自行退订退出），超时再按 pid 强杀。
        if let Some(stdin) = stdin.lock().await.take() {
            drop(stdin);
        }

        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            let alive = pid.lock().await.map(|p| process_alive(p)).unwrap_or(false);
            if !alive {
                self.mark_stopped(id).await;
                return Ok("stdin-eof".to_string());
            }
            sleep(Duration::from_millis(250)).await;
        }

        if let Some(pid) = *pid.lock().await {
            kill_pid(pid).await;
        }

        self.mark_stopped(id).await;
        Ok("killed".to_string())
    }

    async fn mark_stopped(&self, id: &str) {
        let mut listeners = self.listeners.lock().await;
        if let Some(task) = listeners.get_mut(id) {
            task.status.state = ListenerState::Stopped;
            task.status.ready = false;
        }
    }

    pub async fn get_all_listener_status(&self) -> Vec<ListenerStatus> {
        let listeners = self.listeners.lock().await;
        let mut out: Vec<ListenerStatus> = listeners.values().map(|t| t.status.clone()).collect();
        out.sort_by(|a, b| a.kind.to_string().cmp(&b.kind.to_string()));
        out
    }

    pub async fn get_logs(&self, limit: usize) -> Vec<String> {
        let logs = self.logs.lock().await;
        let skip = logs.lines.len().saturating_sub(limit);
        logs.lines.iter().skip(skip).cloned().collect()
    }

    pub async fn clear_logs(&self) {
        let mut logs = self.logs.lock().await;
        logs.lines.clear();
    }
}

/// 单路监听的运行循环共享句柄。
#[derive(Clone)]
struct Shared {
    id: String,
    /// 该监听所属项目：收到的事件按此归属，回复按此项目的设置执行。
    project_id: String,
    kind: ListenKind,
    path: String,
    listeners: Arc<Mutex<HashMap<String, ListenerTask>>>,
    handled: Arc<Mutex<HashSet<String>>>,
    logs: Arc<Mutex<LogBuffer>>,
    storage: Arc<Mutex<Storage>>,
    channel: Option<Channel<ListenerUpdate>>,
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    pid: Arc<Mutex<Option<u32>>>,
    stop_requested: Arc<AtomicBool>,
    pending: Arc<Mutex<VecDeque<String>>>,
    /// 本项目的回复设置（启动监听时解析一次；改设置需重启该项目监听）。
    reply: ReplySettings,
    reply_lock: Arc<Mutex<()>>,
    reply_batches: Arc<Mutex<HashMap<String, Vec<ChatEvent>>>>,
    reply_inflight: Arc<Mutex<HashSet<String>>>,
    reply_batch_window: Duration,
    compress_failures: Arc<Mutex<u32>>,
    /// 连续畸形事件计数：仅在连续出现时提示（D-40）。
    malformed_streak: Arc<Mutex<u32>>,
}

impl Shared {
    async fn run(&self) {
        let mut attempt: u32 = 0;

        loop {
            attempt += 1;
            self.patch_status(|s| {
                s.state = ListenerState::Starting;
                s.ready = false;
                s.attempts = attempt;
            })
            .await;

            let args = vec![
                "event".to_string(),
                "+listen-im".to_string(),
                "--kind".to_string(),
                self.kind.to_string(),
            ];

            let mut command = Command::new(&self.path);
            command
                .args(&args)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            crate::process::hide_console(&mut command);

            let mut child = match command.spawn() {
                Ok(child) => child,
                Err(err) => {
                    let message = format!("启动 dws 失败: {}", err);
                    self.push_log(&message).await;
                    self.patch_status(move |s| {
                        s.last_error = Some(message);
                    })
                    .await;
                    if attempt >= MAX_ATTEMPTS {
                        self.abandon().await;
                        return;
                    }
                    self.backoff(attempt).await;
                    continue;
                }
            };

            *self.pid.lock().await = child.id();
            *self.stdin.lock().await = child.stdin.take();

            let ready = Arc::new(AtomicBool::new(false));
            let stdout = child.stdout.take();
            let stderr = child.stderr.take();

            let stdout_task = stdout.map(|stdout| {
                let shared = self.clone();
                let ready = ready.clone();
                tokio::spawn(async move {
                    let mut lines = BufReader::new(stdout).lines();
                    while let Ok(Some(line)) = lines.next_line().await {
                        shared.on_stdout_line(line, &ready).await;
                    }
                })
            });

            let stderr_task = stderr.map(|stderr| {
                let shared = self.clone();
                let ready = ready.clone();
                tokio::spawn(async move {
                    let mut lines = BufReader::new(stderr).lines();
                    while let Ok(Some(line)) = lines.next_line().await {
                        shared.on_stderr_line(line, &ready).await;
                    }
                })
            });

            let ready_wait = {
                let ready = ready.clone();
                async move {
                    let deadline = Instant::now() + READY_TIMEOUT;
                    while Instant::now() < deadline {
                        if ready.load(Ordering::SeqCst) {
                            return true;
                        }
                        sleep(Duration::from_millis(200)).await;
                    }
                    ready.load(Ordering::SeqCst)
                }
            };

            tokio::select! {
                _ = child.wait() => {}
                became_ready = ready_wait => {
                    if became_ready {
                        let _ = child.wait().await;
                    } else {
                        self.patch_status(|s| {
                            s.last_error = Some(format!("{}s 内未收到就绪信号", READY_TIMEOUT.as_secs()));
                        }).await;
                        self.push_log("就绪超时，本轮放弃").await;
                        let _ = child.kill().await;
                    }
                }
            }

            if let Some(task) = stdout_task {
                task.abort();
            }
            if let Some(task) = stderr_task {
                task.abort();
            }
            *self.stdin.lock().await = None;
            *self.pid.lock().await = None;
            self.pending.lock().await.clear();

            if self.stop_requested.load(Ordering::SeqCst) {
                self.patch_status(|s| {
                    s.state = ListenerState::Stopped;
                    s.ready = false;
                })
                .await;
                return;
            }

            if attempt >= MAX_ATTEMPTS {
                self.abandon().await;
                return;
            }

            self.backoff(attempt).await;
        }
    }

    async fn backoff(&self, attempt: u32) {
        let secs = BACKOFF_SECS[(attempt as usize - 1).min(BACKOFF_SECS.len() - 1)];
        self.patch_status(|s| {
            s.state = ListenerState::BackingOff;
            s.ready = false;
        })
        .await;
        self.push_log(&format!(
            "监听退出，{}s 后重试（第 {}/{} 次）",
            secs, attempt, MAX_ATTEMPTS
        ))
        .await;
        sleep(Duration::from_secs(secs)).await;
    }

    async fn abandon(&self) {
        self.patch_status(|s| {
            s.state = ListenerState::Abandoned;
            s.ready = false;
        })
        .await;
        self.push_log(&format!("连续失败 {} 次，已放弃该路监听", MAX_ATTEMPTS))
            .await;
    }

    async fn on_stderr_line(&self, line: String, ready: &Arc<AtomicBool>) {
        self.push_log(&line).await;

        if line.contains("[event] ready") && !ready.swap(true, Ordering::SeqCst) {
            let (subscribe_id, bus_pid) = parse_ready_line(&line);

            self.patch_status(move |s| {
                s.state = ListenerState::Running;
                s.ready = true;
                s.subscribe_id = subscribe_id;
                s.bus_pid = bus_pid;
                s.last_error = None;
            })
            .await;

            let buffered: Vec<String> = {
                let mut pending = self.pending.lock().await;
                pending.drain(..).collect()
            };
            for line in buffered {
                self.handle_event_line(&line).await;
            }
        }
    }

    async fn on_stdout_line(&self, line: String, ready: &Arc<AtomicBool>) {
        if !ready.load(Ordering::SeqCst) {
            let mut pending = self.pending.lock().await;
            if pending.len() < READY_BUFFER_LIMIT {
                pending.push_back(line);
            } else {
                drop(pending);
                self.bump_dropped().await;
            }
            return;
        }

        self.handle_event_line(&line).await;
    }

    async fn bump_dropped(&self) {
        let mut listeners = self.listeners.lock().await;
        if let Some(task) = listeners.get_mut(&self.id) {
            task.status.dropped_before_ready += 1;
        }
    }

    async fn handle_event_line(&self, line: &str) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return;
        }

        let mut event = match parse_event_line(trimmed, self.kind) {
            Some(event) => event,
            None => {
                self.push_log(&format!("丢弃非法事件行: {}", truncate(trimmed, 200)))
                    .await;
                return;
            }
        };
        // 归属到「哪个项目的监听收到的」，左树与统计都按这个分。
        event.project_id = self.project_id.clone();

        let dedupe_key = if event.message_id.is_empty() {
            format!("raw:{}", trimmed)
        } else {
            event.message_id.clone()
        };

        {
            let mut handled = self.handled.lock().await;
            if !handled.insert(dedupe_key) {
                return;
            }
        }

        {
            let storage = self.storage.lock().await;
            if let Err(err) = storage.save_event(&event) {
                self.push_log(&format!("落盘失败: {}", err)).await;
            }
        }

        if event.malformed {
            // D-40：单条畸形只记录，连续出现才提示，避免偶发噪声刷屏。
            let streak = {
                let mut streak = self.malformed_streak.lock().await;
                *streak += 1;
                *streak
            };
            self.push_log(&format!(
                "收到畸形事件（缺 message_id/conversation_id）: {}",
                truncate(trimmed, 200)
            ))
            .await;
            if streak >= 3 {
                self.push_log(&format!(
                    "连续 {} 条畸形事件，请检查 dws 输出格式是否变更（A4.2.3）",
                    streak
                ))
                .await;
            }
        } else {
            *self.malformed_streak.lock().await = 0;
        }

        if let Some(channel) = &self.channel {
            let _ = channel.send(ListenerUpdate::Event {
                event: event.clone(),
            });
        }

        // 先落盘再推流再回复：回复失败不影响事件已经安全落盘。
        if self.reply.enabled {
            let shared = self.clone();
            let event = event.clone();
            tokio::spawn(async move { shared.schedule_reply(event).await });
        } else {
            // 项目没开自动回复时，此前什么都不写，界面只看到「收到消息但没回复」
            // 却查不到原因（用户实际反馈过）。把原因记进台账，让它可见。
            let shared = self.clone();
            let event = event.clone();
            tokio::spawn(async move {
                shared
                    .finish_reply(&event, "skipped", Some("该项目未启用自动回复"))
                    .await
            });
        }
    }

    /// 回复入口：先做单条判定，再按会话攒批等窗口。
    ///
    /// 之前是「来一条答一条」，对方连发 n 条就会回 n 条。现在同会话的消息在
    /// `REPLY_BATCH_WINDOW` 内攒成一批，只生成并发送一条回复。
    async fn schedule_reply(&self, event: ChatEvent) {
        if event.malformed || event.conversation_id.is_empty() {
            self.finish_reply(&event, "skipped", None).await;
            return;
        }

        if let Some(self_id) = self.reply.self_open_id.as_ref() {
            if !self_id.is_empty() && *self_id == event.sender_open_dingtalk_id {
                self.push_log("跳过自己发送的消息").await;
                self.finish_reply(&event, "skipped", None).await;
                return;
            }
        }

        // 注意：正文剥离 @ 后为空（对方只 @ 了一下）**不跳过**，
        // 由 reply::build_prompt 用占位问句交给 Agent 自然回应。
        // 之前在这里直接 skip，用户看到的就是「收到消息但没回复」。
        if self.reply.agent_cli_path.is_none() {
            self.finish_reply(&event, "failed", Some("未解析到 Agent CLI，无法生成回复"))
                .await;
            return;
        }

        let conversation = event.conversation_id.clone();
        {
            let mut batches = self.reply_batches.lock().await;
            batches
                .entry(conversation.clone())
                .or_default()
                .push(event);
            // 已经有窗口在等这个会话：这条会被那一次一起答掉，不再排新任务。
            let mut inflight = self.reply_inflight.lock().await;
            if !inflight.insert(conversation.clone()) {
                return;
            }
        }

        let shared = self.clone();
        tokio::spawn(async move { shared.flush_reply_batch(&conversation).await });
    }

    /// 等一个静默窗口，把这一批消息合成一条回复发出去。
    async fn flush_reply_batch(&self, conversation: &str) {
        tokio::time::sleep(self.reply_batch_window).await;

        let batch = self
            .reply_batches
            .lock()
            .await
            .remove(conversation)
            .unwrap_or_default();

        // 先摘标记再处理：处理期间新到的消息会自己排下一次窗口，
        // 不会卡在「有标记但没人处理」而永远不回。
        self.reply_inflight.lock().await.remove(conversation);

        if batch.is_empty() {
            return;
        }

        if batch.len() > 1 {
            self.push_log(&format!(
                "收到 {} 条消息，合并成一条回复（会话 {}）",
                batch.len(),
                conversation
            ))
            .await;
        }

        self.reply_to_batch(batch).await;
    }

    /// 回复链路：判定 → 拉上下文 → 生成 → 清洗 → 发送 → 记账。
    async fn reply_to_batch(&self, events: Vec<ChatEvent>) {
        // 本项目的回复设置快照（启动监听时已解析）。
        let reply = self.reply.clone();

        let Some(event) = events.first().cloned() else {
            return;
        };
        let Some(agent_cli) = reply.agent_cli_path.clone() else {
            for item in &events {
                self.finish_reply(item, "failed", Some("未解析到 Agent CLI，无法生成回复"))
                    .await;
            }
            return;
        };

        // 全局串行：一次只处理一批回复。
        let _guard = self.reply_lock.lock().await;

        // 自动压缩：失败只记日志，绝不阻塞本次回复（A7.2.3）。
        if compression_due_with(
            &self.storage,
            &self.compress_failures,
            &event.conversation_id,
            &reply,
        )
        .await
        {
            match compress_with(&self.storage, reply.clone(), &event.conversation_id).await {
                Ok(_) => *self.compress_failures.lock().await = 0,
                Err(err) => {
                    let count = {
                        let mut failures = self.compress_failures.lock().await;
                        *failures += 1;
                        *failures
                    };
                    self.push_log(&format!("自动压缩失败（不影响本次回复）: {}", err))
                        .await;
                    if count >= 3 {
                        self.push_log("自动压缩连续失败 3 次，已暂停自动压缩（A7.2.3）")
                            .await;
                    }
                }
            }
        }

        let context = if reply.context_enabled {
            let rows = {
                let storage = self.storage.lock().await;
                storage
                    .recent_messages(&event.conversation_id, reply.context_message_limit * 4)
                    .unwrap_or_default()
            };
            crate::reply::context_lines(
                &rows,
                reply.context_message_limit,
                reply.context_max_chars,
            )
        } else {
            Vec::new()
        };

        // 这一批里对方说的所有内容：多条并成一条回复时都要交给 Agent 看。
        let contents: Vec<String> = events.iter().map(|item| item.content.clone()).collect();

        let prompt = match self
            .storage
            .lock()
            .await
            .get_summary(&event.conversation_id)
            .ok()
            .flatten()
        {
            Some(summary) => crate::reply::build_prompt_for_batch(
                &contents,
                Some(&summary.content),
                &context,
                reply.context_enabled,
            ),
            None => {
                crate::reply::build_prompt_for_batch(&contents, None, &context, reply.context_enabled)
            }
        };

        // 会话按 (项目, conversation_id) 建档：首次 --session-id，之后 --resume。
        let existing = {
            let storage = self.storage.lock().await;
            storage
                .get_session(&self.project_id, &event.conversation_id)
                .ok()
                .flatten()
        };
        let (session_id, resume) = match existing {
            Some((session_id, _cwd)) => (session_id, true),
            None => (uuid::Uuid::new_v4().to_string(), false),
        };

        let mut settings = reply.clone();
        settings.agent_cli_path = Some(agent_cli);

        let raw = match crate::reply::generate(&settings, &prompt, Some(&session_id), resume).await
        {
            Ok(raw) => raw,
            Err(err) => {
                self.finish_batch(&events, "failed", Some(&format!("生成失败: {}", err)))
                    .await;
                return;
            }
        };

        if !resume {
            let storage = self.storage.lock().await;
            let _ = storage.save_session(
                &self.project_id,
                &event.conversation_id,
                &session_id,
                &settings.agent_cwd,
            );
        }

        let text = crate::reply::sanitize_reply(&raw, reply.max_chars);
        if text.is_empty() {
            self.finish_batch(&events, "failed", Some("清洗后回复为空")).await;
            return;
        }

        // 强制中文是 prompt 里的要求，模型有可能不遵守。这里只提醒不改写：
        // 硬拦会变成「静默不回」，比回一句英文更糟。
        if !crate::reply::has_cjk(&text) {
            self.push_log("提醒：本次回复不含中文，模型可能没有遵守「强制中文」要求")
                .await;
        }

        match crate::reply::send(
            &self.path,
            &event.conversation_id,
            &text,
            &event.message_id,
        )
        .await
        {
            Ok(_) => {
                self.push_log(&format!("已回复会话 {}", event.conversation_id))
                    .await;
                self.finish_batch(&events, "sent", Some(&text)).await;
            }
            Err(err) => {
                self.finish_batch(&events, "failed", Some(&format!("{} | 正文: {}", err, text)))
                    .await;
            }
        }
    }

    /// 一批消息共用一个结果。必须逐条写台账，否则这批里除第一条外的消息
    /// 会一直停在「没状态」上，界面看起来又变成「收到了但没回复」。
    async fn finish_batch(&self, events: &[ChatEvent], status: &str, text: Option<&str>) {
        for item in events {
            self.finish_reply(item, status, text).await;
        }
    }

    async fn finish_reply(&self, event: &ChatEvent, status: &str, text: Option<&str>) {
        if !event.message_id.is_empty() {
            let storage = self.storage.lock().await;
            if let Err(err) = storage.mark_processed(&event.message_id, status, text) {
                drop(storage);
                self.push_log(&format!("回复台账写入失败: {}", err)).await;
                return;
            }
        }

        match status {
            "failed" => {
                self.push_log(&format!(
                    "回复失败 message_id={} 原因={}",
                    event.message_id,
                    text.unwrap_or("未知")
                ))
                .await
            }
            "skipped" => {
                // 有原因才写日志：绝大多数跳过是正常的「自己发的消息」，
                // 全记会淹掉监听日志。带原因的跳过要留痕，否则查不出为什么没回复。
                if let Some(reason) = text {
                    self.push_log(&format!(
                        "跳过回复 message_id={} 原因={}",
                        event.message_id, reason
                    ))
                    .await
                }
            }
            _ => {}
        }
    }

    async fn patch_status<F>(&self, f: F)
    where
        F: FnOnce(&mut ListenerStatus),
    {
        let status = {
            let mut listeners = self.listeners.lock().await;
            match listeners.get_mut(&self.id) {
                Some(task) => {
                    f(&mut task.status);
                    Some(task.status.clone())
                }
                None => None,
            }
        };

        if let (Some(status), Some(channel)) = (status, self.channel.as_ref()) {
            let _ = channel.send(ListenerUpdate::Status { status });
        }
    }

    async fn push_log(&self, line: &str) {
        let stamped = format!("{} {}", chrono::Local::now().format("%H:%M:%S"), line);
        {
            let mut logs = self.logs.lock().await;
            if logs.lines.len() >= LOG_BUFFER_LIMIT {
                logs.lines.pop_front();
            }
            logs.lines.push_back(stamped.clone());
        }
        if let Some(channel) = &self.channel {
            let _ = channel.send(ListenerUpdate::Log {
                listener_id: self.id.clone(),
                line: stamped,
            });
        }
    }
}

fn truncate(input: &str, max: usize) -> String {
    if input.chars().count() <= max {
        return input.to_string();
    }
    input.chars().take(max).collect::<String>() + "…"
}

/// 压缩某会话：调 Agent 生成摘要并落盘为最新一版。
async fn compress_with(
    storage: &Arc<Mutex<Storage>>,
    mut settings: ReplySettings,
    conversation_id: &str,
) -> anyhow::Result<crate::storage::Summary> {
    let agent_cli = settings
        .agent_cli_path
        .clone()
        .ok_or_else(|| anyhow::anyhow!("未解析到 Agent CLI，无法生成摘要"))?;
    settings.agent_cli_path = Some(agent_cli);

    let rows = {
        let storage = storage.lock().await;
        storage.recent_messages(conversation_id, 200)?
    };
    if rows.is_empty() {
        anyhow::bail!("该会话暂无可压缩的事件");
    }

    let prompt = crate::reply::build_summary_prompt(&rows);
    // 摘要走独立会话，避免污染回复所用的 Agent 会话。
    let raw = crate::reply::generate(&settings, &prompt, None, false).await?;
    let content = crate::reply::sanitize_reply(&raw, 4000);
    if content.is_empty() {
        anyhow::bail!("生成的摘要为空");
    }

    let storage = storage.lock().await;
    let total = storage.count_events(conversation_id)?;
    storage.save_summary(conversation_id, &content, total)?;
    storage
        .get_summary(conversation_id)?
        .ok_or_else(|| anyhow::anyhow!("摘要写入后读取失败"))
}

/// 是否该自动压缩。阈值留空 = 该维度不触发（D-59 未定值，不拍脑袋）。
async fn compression_due_with(
    storage: &Arc<Mutex<Storage>>,
    failures: &Arc<Mutex<u32>>,
    conversation_id: &str,
    reply: &ReplySettings,
) -> bool {
    if !reply.auto_compress {
        return false;
    }
    if reply.compress_trigger_turns.is_none() && reply.compress_trigger_chars.is_none() {
        return false;
    }
    if *failures.lock().await >= 3 {
        return false;
    }

    let (total, summarized, chars) = {
        let storage = storage.lock().await;
        let total = storage.count_events(conversation_id).unwrap_or(0);
        let summarized = storage
            .get_summary(conversation_id)
            .ok()
            .flatten()
            .map(|summary| summary.source_events)
            .unwrap_or(0);
        let rows = storage.recent_messages(conversation_id, 500).unwrap_or_default();
        let chars: usize = rows.iter().map(|(_, content)| content.chars().count()).sum();
        (total, summarized, chars)
    };

    let pending_turns = (total - summarized).max(0) as usize;
    if let Some(limit) = reply.compress_trigger_turns {
        if limit > 0 && pending_turns >= limit {
            return true;
        }
    }
    if let Some(limit) = reply.compress_trigger_chars {
        if limit > 0 && chars >= limit {
            return true;
        }
    }
    false
}

fn process_alive(pid: u32) -> bool {
    #[cfg(windows)]
    {
        use std::process::Command as StdCommand;
        let mut command = StdCommand::new("tasklist");
        command
            .args(["/FI", &format!("PID eq {}", pid), "/NH"]);
        crate::process::hide_console_std(&mut command);
        let output = command.output();
        match output {
            Ok(output) => String::from_utf8_lossy(&output.stdout).contains(&pid.to_string()),
            Err(_) => false,
        }
    }
    #[cfg(not(windows))]
    {
        std::path::Path::new(&format!("/proc/{}", pid)).exists()
    }
}

async fn kill_pid(pid: u32) {
    #[cfg(windows)]
    {
        let mut command = Command::new("taskkill");
        command
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        crate::process::hide_console(&mut command);
        let _ = command.status().await;
    }
    #[cfg(not(windows))]
    {
        let _ = Command::new("kill")
            .args(["-9", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;
    }
}

fn parse_ready_line(line: &str) -> (Option<String>, Option<u32>) {
    let mut subscribe_id = None;
    let mut bus_pid = None;

    for part in line.split_whitespace() {
        if let Some(value) = part.strip_prefix("subscribe_id=") {
            subscribe_id = Some(value.to_string());
        } else if let Some(value) = part.strip_prefix("bus_pid=") {
            bus_pid = value.parse().ok();
        }
    }

    (subscribe_id, bus_pid)
}

fn parse_event_line(line: &str, kind: ListenKind) -> Option<ChatEvent> {
    let raw: serde_json::Value = serde_json::from_str(line).ok()?;
    if !raw.is_object() {
        return None;
    }

    let get_str = |key: &str| -> String {
        raw.get(key)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string()
    };

    let message_id = get_str("message_id");
    let conversation_id = get_str("conversation_id");

    Some(ChatEvent {
        project_id: String::new(),
        malformed: message_id.is_empty() || conversation_id.is_empty(),
        message_id,
        conversation_id,
        sender: get_str("sender"),
        sender_open_dingtalk_id: get_str("sender_open_dingtalk_id"),
        content: get_str("content"),
        create_time: get_str("create_time"),
        received_at: chrono::Local::now().to_rfc3339(),
        listen_kind: kind.to_string(),
        raw: line.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::EventQuery;

    /// 两个端到端测试都要临时改**进程级**工作目录（node 按相对名 `event` / `chat`
    /// 找桩脚本）。进程 cwd 是全局的，并行跑就会互相踩：一个测试改了 cwd，
    /// 另一个测试的 `chat` 桩就找不到了，表现为偶发的「发送失败」。
    /// 用一把锁把「改 cwd 的窗口」串起来，否则这是测试环境问题而非产品缺陷。
    static CWD_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn node_exe() -> Option<String> {
        let path_var = std::env::var("PATH").ok()?;
        for dir in std::env::split_paths(&path_var) {
            for name in ["node.exe", "node"] {
                let candidate = dir.join(name);
                if candidate.is_file() {
                    return Some(candidate.to_string_lossy().to_string());
                }
            }
        }
        None
    }

    /// 假 dws：stderr 打 ready，stdout 打事件；复刻真实 dws 的输出分工。
    const STUB: &str = r#"
const READY = "[event] ready event_key=user_im_message_receive_at subscribe_id=subId-stub bus_pid=1";
function evt(id, over) {
  return JSON.stringify(Object.assign({
    type: "user_im_message_receive_at",
    event_id: id,
    subscribe_id: "subId-stub",
    message_id: "msg-" + id,
    conversation_id: "cid-1",
    sender: "张三",
    sender_open_dingtalk_id: "open-1",
    content: "消息 " + id,
    create_time: "2026-09-19T00:00:00Z"
  }, over || {}));
}
process.stderr.write(READY + "\n");
setTimeout(function () {
  process.stdout.write(evt("1") + "\n");
  process.stdout.write(evt("2") + "\n");
  process.stdout.write(evt("3", { message_id: "", conversation_id: "" }) + "\n");
  process.stdout.write(evt("1") + "\n");
}, 200);
process.stdin.resume();
process.stdin.on("end", function () { process.exit(0); });
"#;

    #[tokio::test]
    async fn stub_listener_runs_end_to_end() {
        let Some(node) = node_exe() else {
            eprintln!("跳过：环境里没有 node");
            return;
        };

        let stub_dir = std::env::temp_dir().join("agentmux-orchestrator-stub");
        let _ = std::fs::remove_dir_all(&stub_dir);
        std::fs::create_dir_all(&stub_dir).unwrap();
        // 文件名必须是 `event`：宿主会追加 "event +listen-im --kind at-me" 这几个参数。
        std::fs::write(stub_dir.join("event"), STUB).unwrap();

        let data_dir = std::env::temp_dir().join("agentmux-orchestrator-data");
        let _ = std::fs::remove_dir_all(&data_dir);
        let storage = Arc::new(Mutex::new(Storage::new(data_dir.clone()).unwrap()));

        let orchestrator = Orchestrator::new(storage.clone());
        orchestrator.set_dws_path(Some(node)).await;

        // 改 cwd 前拿锁，直到本测试恢复 cwd 为止（见 CWD_LOCK 的说明）。
        let _cwd_guard = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let previous = std::env::current_dir().unwrap();
        std::env::set_current_dir(&stub_dir).unwrap();

        let id = orchestrator
            .start_listener(
                "test-project".to_string(),
                ListenKind::AtMe,
                ReplySettings::default(),
                None,
            )
            .await
            .expect("应能启动监听");

        let mut ready = false;
        for _ in 0..60 {
            sleep(Duration::from_millis(100)).await;
            if orchestrator
                .get_all_listener_status()
                .await
                .iter()
                .any(|status| status.ready)
            {
                ready = true;
                break;
            }
        }

        // 给事件落盘留出时间，然后恢复工作目录。
        sleep(Duration::from_millis(600)).await;
        let stop_result = orchestrator.stop_listener(&id).await;
        std::env::set_current_dir(previous).unwrap();

        assert!(ready, "应通过 stderr 的 ready 行完成就绪门控");
        assert!(stop_result.is_ok(), "停机应成功: {:?}", stop_result);

        let rows = storage
            .lock()
            .await
            .list_events(&EventQuery {
                limit: 50,
                ..Default::default()
            })
            .unwrap();

        // 2 条正常 + 1 条畸形（缺 id 也保留）+ 重复的 msg-1 被去重。
        assert_eq!(rows.len(), 3, "实际落盘: {:?}", rows.iter().map(|r| &r.message_id).collect::<Vec<_>>());
        assert_eq!(
            rows.iter().filter(|row| row.malformed).count(),
            1,
            "畸形事件应被标记而不是丢弃"
        );
        assert_eq!(
            rows.iter().filter(|row| row.message_id == "msg-1").count(),
            1,
            "跨监听去重应拦住重复的 message_id"
        );

        // 项目归属（问题 2/3 的数据基础）：哪个项目的监听收到就归哪个项目。
        assert!(
            rows.iter().all(|row| row.project_id == "test-project"),
            "事件应归属到发起监听的项目，实际: {:?}",
            rows.iter().map(|r| (&r.message_id, &r.project_id)).collect::<Vec<_>>()
        );

        // 左树「项目 → 会话」靠这个查询：本项目能看到会话，别的项目看不到。
        // 注意 stub 里有一条畸形事件（conversation_id 为空），它**不该**成为会话。
        let mine = storage
            .lock()
            .await
            .list_conversations(Some("test-project"), false)
            .unwrap();
        assert_eq!(mine.len(), 1, "本项目应只看到 cid-1 这一个会话，实际 {:?}", mine);
        assert_eq!(mine[0].conversation_id, "cid-1");
        assert_eq!(
            mine[0].events, 2,
            "畸形事件（无会话标识）不应计入会话，实际 {:?}",
            mine[0]
        );

        let others = storage
            .lock()
            .await
            .list_conversations(Some("other-project"), false)
            .unwrap();
        assert!(others.is_empty(), "别的项目不应看到这个会话，实际 {:?}", others);

        let _ = std::fs::remove_dir_all(&stub_dir);
        let _ = std::fs::remove_dir_all(&data_dir);
    }

    /// 桩 dws 的收事件端：stderr 打 ready，stdout 打一条真实形态的事件。
    const EVENT_STUB: &str = r#"
const READY = "[event] ready event_key=user_im_message_receive_at subscribe_id=subId-stub bus_pid=1";
const EVT = JSON.stringify({
  type: "user_im_message_receive_at",
  subscribe_id: "subId-stub",
  message_id: "msg-stub-1",
  conversation_id: "cid-1",
  sender: "张三",
  sender_open_dingtalk_id: "open-1",
  content: "@我 你好",
  create_time: "2026-09-19 18:00:00"
});
process.stderr.write(READY + "\n");
setTimeout(function () { process.stdout.write(EVT + "\n"); }, 200);
process.stdin.resume();
process.stdin.on("end", function () { process.exit(0); });
"#;

    /// 桩 dws 的发送端：把收到的参数原样记到 cwd 下的 send-record.json，并回一个 messageId。
    const CHAT_STUB: &str = r#"
const fs = require("fs");
fs.writeFileSync("send-record.json", JSON.stringify(process.argv.slice(2)));
process.stdout.write(JSON.stringify({ result: { messageId: "sent-stub-1" } }));
"#;

    /// 桩 Agent CLI：回显自己的工作目录，用来证明宿主用的是**项目的工作目录**。
    ///
    /// 必须是脚本文件而不是 `node -e`：宿主会在参数末尾追加 `--session-id <uuid>`，
    /// 而 `node -e "code" --session-id x` 会被 node 当成自己的非法选项直接报错
    /// （真实 CLI 是二进制，会把它们当脚本参数吃掉）。
    const AGENT_STUB: &str = r#"
process.stdout.write("收到 " + process.cwd());
"#;

    /// 项目级**完整回复闭环**（问题 1/2/3/5 的联合验证），全程用桩：
    /// 桩 dws 收事件 → 落盘（带 project_id）→ 桩 Agent 生成（断言 cwd 是**项目工作目录**）
    /// → 剔除 @ → 桩 dws 发送（断言发出的会话与幂等键）→ 台账标记 sent。
    #[tokio::test]
    async fn project_scoped_reply_loop_runs_end_to_end() {
        let Some(node) = node_exe() else {
            eprintln!("跳过：环境里没有 node");
            return;
        };

        let base = std::env::temp_dir().join("agentmux-e2e-scoped");
        let _ = std::fs::remove_dir_all(&base);
        let stub_dir = base.join("stub");
        let work_dir = base.join("project-workdir");
        let data_dir = base.join("data");
        std::fs::create_dir_all(&stub_dir).unwrap();
        std::fs::create_dir_all(&work_dir).unwrap();

        // 把 node 当作「dws」：它按第一个参数找脚本，所以 event / chat 两个桩都能被接住。
        std::fs::write(stub_dir.join("event"), EVENT_STUB).unwrap();
        std::fs::write(stub_dir.join("chat"), CHAT_STUB).unwrap();
        std::fs::write(stub_dir.join("agentstub"), AGENT_STUB).unwrap();
        // Agent 用绝对路径：它的 cwd 是**项目工作目录**（不是 stub 目录），
        // 相对路径会找不到脚本。真实场景同样是指向 CLI 的绝对路径。
        let agent_stub_path = stub_dir.join("agentstub").to_string_lossy().to_string();

        let storage = Arc::new(Mutex::new(Storage::new(data_dir.clone()).unwrap()));
        let mut orchestrator = Orchestrator::new(storage.clone());
        // 单条消息的场景不必等生产用的 8 秒攒批窗口。
        orchestrator.set_reply_batch_window(Duration::from_millis(200));

        let node_for_agent = node.clone();
        orchestrator.set_dws_path(Some(node)).await;

        // 模拟「创建项目时指定的工作目录」：Agent 必须在这个目录里被驱动。
        let settings = ReplySettings {
            enabled: true,
            agent_platform: "stub".to_string(),
            agent_cli_path: Some(node_for_agent),
            agent_args: Some(vec![agent_stub_path]),
            agent_cwd: work_dir.to_string_lossy().to_string(),
            timeout_ms: 20_000,
            max_chars: 500,
            ..Default::default()
        };

        // 改 cwd 前拿锁，直到本测试恢复 cwd 为止（见 CWD_LOCK 的说明）。
        let _cwd_guard = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let previous = std::env::current_dir().unwrap();
        std::env::set_current_dir(&stub_dir).unwrap();

        let id = orchestrator
            .start_listener("proj-A".to_string(), ListenKind::AtMe, settings, None)
            .await
            .expect("应能启动监听");

        // 等回复台账变成终态（生成与发送都要走桩进程）。
        let mut rows = Vec::new();
        for _ in 0..160 {
            sleep(Duration::from_millis(250)).await;
            rows = storage
                .lock()
                .await
                .list_events(&EventQuery {
                    limit: 10,
                    project_id: Some("proj-A".to_string()),
                    ..Default::default()
                })
                .unwrap();
            if rows.iter().any(|row| row.reply_status.is_some()) {
                break;
            }
        }

        let _ = orchestrator.stop_listener(&id).await;
        let record = std::fs::read_to_string(stub_dir.join("send-record.json")).ok();
        std::env::set_current_dir(previous).unwrap();

        assert_eq!(rows.len(), 1, "应只落盘 1 条事件，实际 {:?}", rows);
        let row = &rows[0];
        assert_eq!(row.project_id, "proj-A", "事件应归属到发起监听的项目");
        assert_eq!(row.conversation_id, "cid-1");
        assert_eq!(
            row.reply_status.as_deref(),
            Some("sent"),
            "回复应真实发出，台账正文: {:?}",
            row.reply_text
        );

        let reply = row.reply_text.clone().unwrap_or_default();
        assert!(reply.contains("收到"), "回复应是 Agent 的输出，实际: {}", reply);
        assert!(
            reply.contains(&work_dir.file_name().unwrap().to_string_lossy().to_string()),
            "Agent 必须在**项目的工作目录**里运行（回显 cwd），实际: {}",
            reply
        );
        assert!(!reply.contains('@'), "回复正文里的 @ 应被剔除，实际: {}", reply);

        // 断言真正发出去的参数：回到原会话、用原 message_id 做幂等键。
        let record = record.expect("桩 dws 应记录到一次发送调用");
        assert!(record.contains("+messages-send"), "应调用发送: {}", record);
        assert!(record.contains("cid-1"), "应发回原会话: {}", record);
        assert!(
            record.contains("msg-stub-1"),
            "幂等键应用原 message_id: {}",
            record
        );

        // 左树的数据来源：会话按项目聚合，且统计到 1 条已回复。
        let mine = storage
            .lock()
            .await
            .list_conversations(Some("proj-A"), false)
            .unwrap();
        assert_eq!(mine.len(), 1);
        assert_eq!(mine[0].replied, 1, "应统计到 1 条已回复");

        let _ = std::fs::remove_dir_all(&base);
    }

    /// 桩 dws 的事件端：**同一会话连发 3 条**，模拟「一次性收到 n 个 @我」。
    const BURST_EVENT_STUB: &str = r#"
const READY = "[event] ready event_key=user_im_message_receive_at subscribe_id=subId-burst bus_pid=1";
function evt(id, text) {
  return JSON.stringify({
    type: "user_im_message_receive_at",
    subscribe_id: "subId-burst",
    message_id: id,
    conversation_id: "cid-burst",
    sender: "Maren",
    sender_open_dingtalk_id: "open-other",
    content: text,
    create_time: "2026-09-19 20:00:00"
  });
}
process.stderr.write(READY + "\n");
setTimeout(function () { process.stdout.write(evt("msg-b1", "@我 你好") + "\n"); }, 100);
setTimeout(function () { process.stdout.write(evt("msg-b2", "@我 在吗") + "\n"); }, 250);
setTimeout(function () { process.stdout.write(evt("msg-b3", "@我 吃晚饭了吗") + "\n"); }, 400);
process.stdin.resume();
process.stdin.on("end", function () { process.exit(0); });
"#;

    /// 桩 dws 的发送端：**追加**记录每一次发送，这样才能数出到底发了几条。
    const APPEND_CHAT_STUB: &str = r#"
const fs = require("fs");
fs.appendFileSync("send-log.jsonl", JSON.stringify(process.argv.slice(2)) + "\n");
process.stdout.write(JSON.stringify({ result: { messageId: "sent-burst" } }));
"#;

    /// 桩 Agent CLI：把 stdin 收到的 prompt 原样落到**项目工作目录**，再回一句中文。
    /// 落盘是为了断言「这几条消息都被交给了 Agent」，而不只是宿主内部合并了。
    const PROMPT_AGENT_STUB: &str = r#"
const fs = require("fs");
let prompt = "";
process.stdin.on("data", function (chunk) { prompt += chunk; });
process.stdin.on("end", function () {
  fs.writeFileSync("agent-prompt.txt", prompt);
  process.stdout.write("你好呀，吃过啦，你吃了吗？");
});
"#;

    /// 回归（问题 2）：一次性连收 n 条时，只生成并发送**一条**回复，
    /// 且这批里的每条事件都要落到 sent（否则界面又会显示成「收到了没回复」）。
    #[tokio::test]
    async fn burst_messages_are_merged_into_a_single_reply() {
        let Some(node) = node_exe() else {
            eprintln!("跳过：环境里没有 node");
            return;
        };

        let base = std::env::temp_dir().join("agentmux-e2e-burst");
        let _ = std::fs::remove_dir_all(&base);
        let stub_dir = base.join("stub");
        let work_dir = base.join("project-workdir");
        let data_dir = base.join("data");
        std::fs::create_dir_all(&stub_dir).unwrap();
        std::fs::create_dir_all(&work_dir).unwrap();

        std::fs::write(stub_dir.join("event"), BURST_EVENT_STUB).unwrap();
        std::fs::write(stub_dir.join("chat"), APPEND_CHAT_STUB).unwrap();
        std::fs::write(stub_dir.join("agentstub"), PROMPT_AGENT_STUB).unwrap();
        let agent_stub_path = stub_dir.join("agentstub").to_string_lossy().to_string();

        let storage = Arc::new(Mutex::new(Storage::new(data_dir.clone()).unwrap()));
        let mut orchestrator = Orchestrator::new(storage.clone());
        // 生产默认是 8 秒，测试里缩短到 800ms：远大于 3 条事件的间隔（150ms），
        // 又不用白等。
        orchestrator.set_reply_batch_window(Duration::from_millis(800));

        let settings = ReplySettings {
            enabled: true,
            agent_platform: "stub".to_string(),
            agent_cli_path: Some(node.clone()),
            agent_args: Some(vec![agent_stub_path]),
            agent_cwd: work_dir.to_string_lossy().to_string(),
            timeout_ms: 20_000,
            max_chars: 500,
            ..Default::default()
        };
        orchestrator.set_dws_path(Some(node)).await;

        let _cwd_guard = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let previous = std::env::current_dir().unwrap();
        std::env::set_current_dir(&stub_dir).unwrap();

        let id = orchestrator
            .start_listener("proj-burst".to_string(), ListenKind::AtMe, settings, None)
            .await
            .expect("应能启动监听");

        // 事件落盘 → 攒批 → 生成 → 发送需要点时间，轮询等这一批收尾。
        let send_log = stub_dir.join("send-log.jsonl");
        for _ in 0..120 {
            sleep(Duration::from_millis(100)).await;
            let rows = storage
                .lock()
                .await
                .list_events(&EventQuery {
                    limit: 10,
                    project_id: Some("proj-burst".to_string()),
                    ..Default::default()
                })
                .unwrap();
            if rows.len() == 3 && rows.iter().all(|row| row.reply_status.is_some()) {
                break;
            }
        }

        let _ = orchestrator.stop_listener(&id).await;
        let prompt = std::fs::read_to_string(work_dir.join("agent-prompt.txt")).ok();
        let sends = std::fs::read_to_string(&send_log).ok();
        std::env::set_current_dir(previous).unwrap();

        let rows = storage
            .lock()
            .await
            .list_events(&EventQuery {
                limit: 10,
                project_id: Some("proj-burst".to_string()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(rows.len(), 3, "三条消息都应落盘");

        // 只发一条：这是「合并回复」的核心证据。
        let sends = sends.expect("桩 dws 应记录到发送调用");
        let send_count = sends.lines().filter(|line| !line.trim().is_empty()).count();
        assert_eq!(
            send_count, 1,
            "一批 3 条消息只应发出 1 条回复，实际 {} 条：{}",
            send_count, sends
        );

        // 三条都标记 sent，且共用同一条回复正文。
        for row in &rows {
            assert_eq!(
                row.reply_status.as_deref(),
                Some("sent"),
                "{} 也应标记为已回复，否则界面显示成「收到了没回复」",
                row.message_id
            );
            assert_eq!(
                row.reply_text.as_deref(),
                Some("你好呀，吃过啦，你吃了吗？"),
                "同一批应共用同一条回复"
            );
        }

        // Agent 必须看到全部三条内容，否则合并就是假的。
        let prompt = prompt.expect("桩 Agent 应收到 prompt");
        assert!(prompt.contains("你好"), "prompt 缺少第 1 条：{}", prompt);
        assert!(prompt.contains("在吗"), "prompt 缺少第 2 条：{}", prompt);
        assert!(prompt.contains("吃晚饭了吗"), "prompt 缺少第 3 条：{}", prompt);
        assert!(prompt.contains("只回一条"), "应要求合并成一条回复");
        assert!(
            prompt.contains("一律使用简体中文回复"),
            "必须带强制中文的角色约束"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn ready_line_parses_subscribe_id_and_bus_pid() {
        let line = "[event] ready event_key=user_im_message_receive_at bus_pid=22304 subscribe_id=subId-3a58";
        let (subscribe_id, bus_pid) = parse_ready_line(line);
        assert_eq!(subscribe_id.as_deref(), Some("subId-3a58"));
        assert_eq!(bus_pid, Some(22304));
    }

    #[test]
    fn non_object_lines_are_rejected() {
        assert!(parse_event_line("[1,2,3]", ListenKind::AtMe).is_none());
        assert!(parse_event_line("not json", ListenKind::AtMe).is_none());
    }

    /// 用**真实 dws 抓包**验证解析与落盘。
    ///
    /// 来源：参考工程 `data/session-listen.ndjson`，是旧宿主在真实监听窗口里
    /// 从 dws stdout 原样落下的行（stderr 的 ready 行在同目录 `.err` 里）。
    #[tokio::test]
    async fn real_captured_dws_payloads_parse_and_store() {
        let capture =
            std::path::Path::new(r"D:\workSpase\idea\dingtalk-event-host\data\session-listen.ndjson");
        if !capture.is_file() {
            eprintln!("跳过：环境里没有真实 dws 抓包文件");
            return;
        }

        let content = std::fs::read_to_string(capture).unwrap();
        let lines: Vec<&str> = content
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect();
        assert!(!lines.is_empty(), "抓包文件不应为空");

        let data_dir = std::env::temp_dir().join("agentmux-real-capture-test");
        let _ = std::fs::remove_dir_all(&data_dir);
        let storage = Storage::new(data_dir.clone()).unwrap();

        let mut stored = 0;
        for line in &lines {
            let event = parse_event_line(line, ListenKind::AtMe)
                .unwrap_or_else(|| panic!("真实抓包行应能解析: {}", line));

            // 真实载荷的字段形态：message_id 以 msg 开头，conversation_id 以 cid 开头。
            assert!(
                event.message_id.starts_with("msg"),
                "message_id 形态不符: {}",
                event.message_id
            );
            assert!(
                event.conversation_id.starts_with("cid"),
                "conversation_id 形态不符: {}",
                event.conversation_id
            );
            assert!(!event.sender.is_empty(), "应解析出发送人");
            assert!(!event.content.is_empty(), "应解析出正文");
            assert_eq!(event.listen_kind, "at-me");
            assert!(!event.malformed, "真实抓包里不应出现畸形事件");

            if storage.save_event(&event).unwrap() {
                stored += 1;
            }
        }

        assert!(stored >= 1, "应至少落盘 1 条真实事件");

        // 同一批真实载荷再走一遍，应全部判重（跨监听去重生效）。
        let mut duplicated = 0;
        for line in &lines {
            let event = parse_event_line(line, ListenKind::AtMe).unwrap();
            if !storage.save_event(&event).unwrap() {
                duplicated += 1;
            }
        }
        assert_eq!(duplicated, stored, "二次解析应全部判重");

        let _ = std::fs::remove_dir_all(&data_dir);
    }

    /// 默认 `#[ignore]`：会真的连钉钉建订阅（跑完自动精准退订）。
    /// 跑法：`cargo test -- --ignored real_dws`。
    ///
    /// 这条补的是「宿主自己的代码 + 真实 dws」这一环：桩测试证明了代码正确、
    /// 命令行窗口证明了 dws 能 ready，但没有证明**宿主驱动真实 dws** 也能就绪并干净退出。
    #[tokio::test]
    #[ignore]
    async fn real_dws_listener_reaches_ready_and_stops_cleanly() {
        let data_dir = std::env::temp_dir().join("agentmux-real-dws-test");
        let _ = std::fs::remove_dir_all(&data_dir);
        let storage = Arc::new(Mutex::new(Storage::new(data_dir.clone()).unwrap()));

        let orchestrator = Orchestrator::new(storage);
        // 不显式设路径：走全局自动解析，顺带验证解析结果真能被 spawn。
        let id = match orchestrator
            .start_listener(
                "test-project".to_string(),
                ListenKind::AtMe,
                ReplySettings::default(),
                None,
            )
            .await
        {
            Ok(id) => id,
            Err(err) => {
                eprintln!("跳过：{}", err);
                return;
            }
        };

        let mut ready = false;
        for _ in 0..300 {
            sleep(Duration::from_millis(100)).await;
            if orchestrator
                .get_all_listener_status()
                .await
                .iter()
                .any(|status| status.ready)
            {
                ready = true;
                break;
            }
        }

        let status = orchestrator.get_all_listener_status().await;
        assert!(ready, "真实 dws 应在 30s 内就绪，实际状态: {:?}", status);
        assert!(
            status[0].subscribe_id.is_some(),
            "就绪后必须拿到 subscribe_id（精准退订要用）"
        );
        assert!(status[0].bus_pid.is_some(), "就绪后应有 bus_pid");

        let logs = orchestrator.get_logs(200).await;
        assert!(
            logs.iter().any(|line| line.contains("[event] ready")),
            "日志里应保留 ready 原文"
        );

        let stopped = orchestrator.stop_listener(&id).await;
        assert!(
            matches!(stopped.as_deref(), Ok("stdin-eof") | Ok("killed")),
            "停机应走完阶梯，实际: {:?}",
            stopped
        );

        let after = orchestrator.get_all_listener_status().await;
        assert_eq!(after[0].state, ListenerState::Stopped);
        assert!(!after[0].ready);

        let _ = std::fs::remove_dir_all(&data_dir);
    }

    /// 默认 `#[ignore]`：真端到端 —— 宿主自己的代码 + 真实 dws + **真实消息**。
    /// 跑法：`cargo test -- --ignored real_dws_receives_live_event`，
    /// 运行期间请从另一个账号 @ 一下本人。
    #[tokio::test]
    #[ignore]
    async fn real_dws_receives_live_event() {
        let data_dir = std::env::temp_dir().join("agentmux-live-event-test");
        let _ = std::fs::remove_dir_all(&data_dir);
        let storage = Arc::new(Mutex::new(Storage::new(data_dir.clone()).unwrap()));

        let orchestrator = Orchestrator::new(storage.clone());
        let id = match orchestrator
            .start_listener(
                "test-project".to_string(),
                ListenKind::AtMe,
                ReplySettings::default(),
                None,
            )
            .await
        {
            Ok(id) => id,
            Err(err) => {
                eprintln!("跳过：{}", err);
                return;
            }
        };

        // 最多等 150s，等一条真实消息进来。
        let mut rows = Vec::new();
        for _ in 0..300 {
            sleep(Duration::from_millis(500)).await;
            rows = storage
                .lock()
                .await
                .list_events(&EventQuery {
                    limit: 10,
                    ..Default::default()
                })
                .unwrap();
            if !rows.is_empty() {
                break;
            }
        }

        let _ = orchestrator.stop_listener(&id).await;
        let _ = std::fs::remove_dir_all(&data_dir);

        assert!(
            !rows.is_empty(),
            "150s 内没有收到真实消息；请在这段时间里从另一账号 @ 一下本人"
        );

        let event = &rows[0];
        assert!(event.message_id.starts_with("msg"), "message_id: {}", event.message_id);
        assert!(
            event.conversation_id.starts_with("cid"),
            "conversation_id: {}",
            event.conversation_id
        );
        assert!(!event.sender.is_empty(), "应解析出发送人");
        assert!(!event.malformed, "真实事件不应被判为畸形");
        assert_eq!(event.listen_kind, "at-me");

        eprintln!(
            "真实事件已落盘：sender={} conversation={} content={}",
            event.sender, event.conversation_id, event.content
        );
    }

    /// 默认 `#[ignore]`：真端到端**含自动回复** —— 真实 dws + 真实消息 + 真实 Agent 生成 + 真实发送。
    /// 跑法：`cargo test -- --ignored real_dws_replies_to_live_message`。
    ///
    /// 前置：settings.json 里 `reply_enabled=true` 且 agent_cli_path 指向真实 CLI。
    /// 会往被 @ 的会话里真发一条消息。
    #[tokio::test]
    #[ignore]
    async fn real_dws_replies_to_live_message() {
        let data_dir = std::env::temp_dir().join("agentmux-live-reply-test");
        let _ = std::fs::remove_dir_all(&data_dir);
        let storage = Arc::new(Mutex::new(Storage::new(data_dir.clone()).unwrap()));

        let orchestrator = Orchestrator::new(storage.clone());
        let mut settings = crate::config::reply_settings();
        if settings.enabled && settings.agent_cli_path.is_none() {
            settings.agent_cli_path =
                crate::resolve::resolve_executable(&settings.agent_platform).await;
        }
        if !settings.enabled {
            eprintln!("跳过：settings.json 里 reply_enabled 不是 true");
            return;
        }
        eprintln!(
            "生效配置：enabled={} cli={:?} cwd={} timeout={}ms max_chars={}",
            settings.enabled,
            settings.agent_cli_path,
            settings.agent_cwd,
            settings.timeout_ms,
            settings.max_chars
        );
        let id = match orchestrator
            .start_listener("test-project".to_string(), ListenKind::AtMe, settings, None)
            .await
        {
            Ok(id) => id,
            Err(err) => {
                eprintln!("跳过：{}", err);
                return;
            }
        };

        // 生成 + 发送需要时间，最多等 900s，给人工触发留足时间。
        let mut replied = Vec::new();
        for _ in 0..1800 {
            sleep(Duration::from_millis(500)).await;
            let rows = storage
                .lock()
                .await
                .list_events(&EventQuery {
                    limit: 20,
                    ..Default::default()
                })
                .unwrap();
            replied = rows
                .into_iter()
                .filter(|row| row.reply_status.is_some())
                .collect();
            if !replied.is_empty() {
                break;
            }
        }

        let _ = orchestrator.stop_listener(&id).await;
        let _ = std::fs::remove_dir_all(&data_dir);

        assert!(
            !replied.is_empty(),
            "180s 内没有产生回复台账；请在这段时间里从另一账号 @ 一下本人"
        );

        let row = &replied[0];
        eprintln!(
            "回复台账：status={:?} text={:?}",
            row.reply_status, row.reply_text
        );
        assert_eq!(
            row.reply_status.as_deref(),
            Some("sent"),
            "回复应真实发出，失败原文: {:?}",
            row.reply_text
        );
        let text = row.reply_text.clone().unwrap_or_default();
        assert!(!text.is_empty(), "已发送的回复正文不应为空");
        assert!(
            !text.contains('@'),
            "回复正文里的 @ 应已被剔除，实际: {}",
            text
        );
    }
}
