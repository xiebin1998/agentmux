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
/// 退出时留给 dws 自行退订的时间，到点还没走就强杀。
const SHUTDOWN_GRACE: Duration = Duration::from_secs(3);

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

/// 日志缓冲里的一行。带上归属，界面才能按项目筛选：
/// 监听产生的日志归它所属项目，全局日志（如手动压缩）没有项目。
#[derive(Debug, Clone, Serialize)]
pub struct LogLine {
    pub project_id: String,
    pub kind: String,
    pub line: String,
}

struct LogBuffer {
    lines: VecDeque<LogLine>,
}

async fn push_log_line(logs: &Arc<Mutex<LogBuffer>>, project_id: &str, kind: &str, line: &str) {
    let stamped = format!("{} {}", chrono::Local::now().format("%H:%M:%S"), line);
    let mut logs = logs.lock().await;
    if logs.lines.len() >= LOG_BUFFER_LIMIT {
        logs.lines.pop_front();
    }
    logs.lines.push_back(LogLine {
        project_id: project_id.to_string(),
        kind: kind.to_string(),
        line: stamped,
    });
}

/// 终态：没有进程在跑、也不会自己再起来的状态。
fn is_terminal(state: &ListenerState) -> bool {
    matches!(state, ListenerState::Stopped | ListenerState::Abandoned)
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
    /// 每个会话最近一次生成时 CLI 报的**上下文占用比例**（0~1）。
    /// 自适应压缩的依据：比按字符估算准，因为它是 Agent 自己算的。
    last_context_ratio: Arc<Mutex<HashMap<String, f64>>>,
    /// 每个会话最近一次生成实际用的模型名，界面上要显示「当前用什么模型」。
    last_model: Arc<Mutex<HashMap<String, String>>>,
    /// 追踪用：每条消息的接收时刻，用来在日志里打「距收到多少毫秒」。
    trace_started: Arc<Mutex<HashMap<String, std::time::Instant>>>,
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
            last_context_ratio: Arc::new(Mutex::new(HashMap::new())),
            last_model: Arc::new(Mutex::new(HashMap::new())),
            trace_started: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// 缩短攒批窗口：端到端测试不该为生产用的 8 秒窗口白等。
    #[cfg(test)]
    pub fn set_reply_batch_window(&mut self, window: Duration) {
        self.reply_batch_window = window;
    }

    /// 某个会话的运行期观测值：Agent 最近回报的上下文占比与实际用的模型。
    /// 都是「最近一次生成」的快照，重启后为空——它们来自 CLI 回报，不是配置。
    pub async fn conversation_runtime(
        &self,
        conversation_id: &str,
    ) -> (Option<f64>, Option<String>) {
        let ratio = {
            let ratios = self.last_context_ratio.lock().await;
            ratios.get(conversation_id).copied()
        };
        let model = {
            let models = self.last_model.lock().await;
            models.get(conversation_id).cloned()
        };
        (ratio, model)
    }

    pub async fn push_global_log(&self, line: &str) {
        push_log_line(&self.logs, "", "", line).await;
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

        // 同一项目 + 同一种监听只保留一路：重复点「启动」直接复用已有实例。
        // 否则会出现两个进程同时订阅同一事件，界面上也多出一行。
        {
            let mut listeners = self.listeners.lock().await;
            if let Some(existing) = listeners.values().find(|task| {
                task.status.project_id == project_id
                    && task.status.kind == kind
                    && !is_terminal(&task.status.state)
            }) {
                return Ok(existing.status.id.clone());
            }
            // 终态实例（已停止/已放弃）不占位：重新启动时清掉，
            // 不然界面会同时挂着「已放弃」和新起的一行。
            listeners.retain(|_, task| {
                !(task.status.project_id == project_id
                    && task.status.kind == kind
                    && is_terminal(&task.status.state))
            });
        }

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
            last_context_ratio: self.last_context_ratio.clone(),
            last_model: self.last_model.clone(),
            trace_started: self.trace_started.clone(),
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

    /// 退出前把监听收干净：先对所有实例关 stdin（dws 收到 EOF 会自行退订退出），
    /// 等一小会儿仍活着的按 pid 强杀。
    ///
    /// 不这么做就会留下**孤儿 dws 进程**：它还在订阅同一事件，下次启动 App
    /// 就变成「一个事件被多个监听抢」，看起来就是监听重复。
    pub async fn shutdown_all_listeners(&self) {
        let snapshot: Vec<(Arc<Mutex<Option<ChildStdin>>>, Arc<Mutex<Option<u32>>>, Arc<AtomicBool>)> = {
            let listeners = self.listeners.lock().await;
            listeners
                .values()
                .map(|task| (task.stdin.clone(), task.pid.clone(), task.stop_requested.clone()))
                .collect()
        };

        for (stdin, _, stop_requested) in &snapshot {
            stop_requested.store(true, Ordering::SeqCst);
            if let Some(stdin) = stdin.lock().await.take() {
                drop(stdin);
            }
        }

        let deadline = Instant::now() + SHUTDOWN_GRACE;
        loop {
            let mut alive: Vec<u32> = Vec::new();
            for (_, pid, _) in &snapshot {
                if let Some(pid) = *pid.lock().await {
                    if process_alive(pid) {
                        alive.push(pid);
                    }
                }
            }
            if alive.is_empty() || Instant::now() >= deadline {
                for pid in alive {
                    kill_pid(pid).await;
                }
                break;
            }
            sleep(Duration::from_millis(200)).await;
        }
    }

    pub async fn get_all_listener_status(&self) -> Vec<ListenerStatus> {
        let listeners = self.listeners.lock().await;
        // 已停止的实例不再上报：否则「停止再启动」会在界面上留下两行，
        // 看起来像启动监听重复创建。
        let mut out: Vec<ListenerStatus> = listeners
            .values()
            .map(|t| t.status.clone())
            .filter(|s| s.state != ListenerState::Stopped)
            .collect();
        out.sort_by(|a, b| {
            a.project_id
                .cmp(&b.project_id)
                .then_with(|| a.kind.to_string().cmp(&b.kind.to_string()))
        });
        out
    }

    pub async fn get_logs(&self, limit: usize) -> Vec<LogLine> {
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
    last_context_ratio: Arc<Mutex<HashMap<String, f64>>>,
    last_model: Arc<Mutex<HashMap<String, String>>>,
    trace_started: Arc<Mutex<HashMap<String, std::time::Instant>>>,
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

        // 追踪起点：后面每一步都打「距收到多少毫秒」，哪一步后没有下一条
        // 就是卡在哪一步。
        if !event.message_id.is_empty() {
            self.trace_started
                .lock()
                .await
                .insert(event.message_id.clone(), std::time::Instant::now());
        }
        self.trace(
            &event.message_id,
            "① 收到事件",
            &format!(
                "监听={} kind={:?} 会话={} 发送者={}({}) 正文={}字",
                self.id,
                self.kind,
                if event.conversation_id.is_empty() {
                    "<空>"
                } else {
                    &event.conversation_id
                },
                event.sender,
                event.sender_open_dingtalk_id,
                event.content.chars().count()
            ),
        )
        .await;

        let dedupe_key = if event.message_id.is_empty() {
            format!("raw:{}", trimmed)
        } else {
            event.message_id.clone()
        };

        {
            let mut handled = self.handled.lock().await;
            if !handled.insert(dedupe_key) {
                self.trace(&event.message_id, "② 去重", " 判定为重复消息，丢弃")
                    .await;
                return;
            }
        }
        self.trace(&event.message_id, "② 去重", " 新消息，继续处理")
            .await;

        // ③ 范围过滤：项目指定了群/人名单时，只处理命中的消息；名单为空 = 不限制。
        // 不落盘也不推流 —— 不在这个项目的监听范围内，存下来只会污染会话与统计。
        if !event_in_scope(&self.reply.group_ids, &self.reply.member_ids, &event) {
            self.trace(
                &event.message_id,
                "③ 范围过滤",
                &format!(
                    " 未命中指定名单（群 {} 个 / 人 {} 个），丢弃不落盘",
                    self.reply.group_ids.len(),
                    self.reply.member_ids.len()
                ),
            )
            .await;
            self.trace_started.lock().await.remove(&event.message_id);
            return;
        }
        self.trace(&event.message_id, "③ 范围过滤", " 在监听范围内（或未限制），继续处理")
            .await;

        {
            let storage = self.storage.lock().await;
            if let Err(err) = storage.save_event(&event) {
                self.trace(&event.message_id, "④ 落盘", &format!(" 失败: {}", err))
                    .await;
                self.push_log(&format!("落盘失败: {}", err)).await;
            } else {
                self.trace(&event.message_id, "④ 落盘", " 成功（SQLite + ndjson）")
                    .await;
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
            self.trace(&event.message_id, "⑤ 推流", " 已推给界面（实时事件流）")
                .await;
        } else {
            self.trace(&event.message_id, "⑤ 推流", " 无界面通道，跳过")
                .await;
        }

        // 先落盘再推流再回复：回复失败不影响事件已经安全落盘。
        if self.reply.enabled {
            self.trace(
                &event.message_id,
                "⑥ 回复开关",
                " 本项目已启用自动回复，进入回复链路",
            )
            .await;
            let shared = self.clone();
            let event = event.clone();
            tokio::spawn(async move { shared.schedule_reply(event).await });
        } else {
            self.trace(
                &event.message_id,
                "⑥ 回复开关",
                " 本项目未启用自动回复，只记录不回复",
            )
            .await;
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
            self.trace(
                &event.message_id,
                "⑦ 单条判定",
                " 畸形或会话为空，跳过",
            )
            .await;
            self.finish_reply(&event, "skipped", None).await;
            return;
        }

        if let Some(self_id) = self.reply.self_open_id.as_ref() {
            if !self_id.is_empty() && *self_id == event.sender_open_dingtalk_id {
                self.trace(&event.message_id, "⑦ 单条判定", " 发送者是本人，跳过")
                    .await;
                self.push_log("跳过自己发送的消息").await;
                self.finish_reply(&event, "skipped", None).await;
                return;
            }
        }

        // 注意：正文剥离 @ 后为空（对方只 @ 了一下）**不跳过**，
        // 由 reply::build_prompt 用占位问句交给 Agent 自然回应。
        // 之前在这里直接 skip，用户看到的就是「收到消息但没回复」。
        if self.reply.agent_cli_path.is_none() {
            self.trace(
                &event.message_id,
                "⑦ 单条判定",
                " 未解析到 Agent CLI，无法生成回复",
            )
            .await;
            self.finish_reply(&event, "failed", Some("未解析到 Agent CLI，无法生成回复"))
                .await;
            return;
        }

        self.trace(
            &event.message_id,
            "⑦ 单条判定",
            " 通过（非畸形、非本人发送、Agent CLI 就绪）",
        )
        .await;

        let conversation = event.conversation_id.clone();
        let batch = {
            let mut batches = self.reply_batches.lock().await;
            let bucket = batches.entry(conversation.clone()).or_default();
            bucket.push(event);

            // 已经在等窗口的会话不再排新任务，这条会被那一次一起答掉。
            // 注意 HashSet::insert 返回 true 表示「新插入」——含义容易记反，
            // 记反的后果是第一条消息就被当成「已有窗口」，flush 永远不排，
            // 表现为「收到了但永远不回复」。
            let mut inflight = self.reply_inflight.lock().await;
            let first_in_window = inflight.insert(conversation.clone());
            let size = bucket.len();
            (size, first_in_window)
        };
        let (size, first_in_window) = batch;

        if !first_in_window {
            self.trace(
                &conversation,
                "⑧ 进攒批",
                &format!(
                    " 已有窗口在等，并入当前批（本批已 {} 条），窗口 {}ms",
                    size,
                    self.reply_batch_window.as_millis()
                ),
            )
            .await;
            return;
        }

        self.trace(
            &conversation,
            "⑧ 进攒批",
            &format!(
                " 开新窗口：本批第 {} 条，等 {}ms 收齐同会话消息",
                size,
                self.reply_batch_window.as_millis()
            ),
        )
        .await;

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

        self.trace(
            conversation,
            "⑨ 出批",
            &format!(
                " 窗口结束，本批 {} 条（message_id: {}）",
                batch.len(),
                batch
                    .iter()
                    .map(|item| item.message_id.chars().take(12).collect::<String>())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        )
        .await;

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

        // 这个会话上一次生成时 CLI 报的上下文占比，自适应压缩的依据。
        let last_ratio = {
            let ratios = self.last_context_ratio.lock().await;
            ratios.get(&event.conversation_id).copied()
        };
        let compress_percent = if reply.auto_compress {
            reply
                .compress_trigger_percent
                .map(|percent| format!("{}%", percent))
                .unwrap_or_else(|| "未设".to_string())
        } else {
            "压缩已关".to_string()
        };
        self.trace(
            &event.conversation_id,
            "⑩ 压缩判定",
            &format!(
                " 开关={} 阈值={} 上次占比={} 字符阈值={:?}",
                reply.auto_compress,
                compress_percent,
                last_ratio
                    .map(|ratio| format!("{:.2}%", ratio * 100.0))
                    .unwrap_or_else(|| "无基线".to_string()),
                reply.compress_trigger_chars
            ),
        )
        .await;

        // 自动压缩：失败只记日志，绝不阻塞本次回复（A7.2.3）。
        if compression_due_with(
            &self.storage,
            &self.compress_failures,
            &event.conversation_id,
            &reply,
            last_ratio,
        )
        .await
        {
            self.trace(&event.conversation_id, "⑩ 压缩判定", " → 触发压缩")
                .await;
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

        self.trace(
            &event.conversation_id,
            "⑪ 拉上下文",
            &format!(
                " 上下文={}（开关={} 上限 {} 条 / {} 字）",
                if reply.context_enabled {
                    format!("{} 条", context.len())
                } else {
                    "未启用".to_string()
                },
                reply.context_enabled,
                reply.context_message_limit,
                reply.context_max_chars
            ),
        )
        .await;

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
        let prompt_chars = prompt.chars().count();
        let generation_started = std::time::Instant::now();

        self.trace(
            &event.conversation_id,
            "⑫ 调 Agent",
            &format!(
                " CLI={} 会话模式={} prompt={}字 超时={}ms 工作目录={}",
                settings
                    .agent_cli_path
                    .as_deref()
                    .unwrap_or("<未配置>"),
                if resume { "resume 复用" } else { "新建会话" },
                prompt_chars,
                settings.timeout_ms,
                settings.agent_cwd
            ),
        )
        .await;

        let raw = match crate::reply::generate(&settings, &prompt, Some(&session_id), resume).await
        {
            Ok(raw) => raw,
            Err(err) => {
                self.trace(
                    &event.conversation_id,
                    "⑬ 生成失败",
                    &format!(
                        " 耗时 {}ms，错误: {}",
                        generation_started.elapsed().as_millis(),
                        err
                    ),
                )
                .await;
                self.finish_batch(&events, "failed", Some(&format!("生成失败: {}", err)))
                    .await;
                return;
            }
        };

        self.trace(
            &event.conversation_id,
            "⑬ 生成返回",
            &format!(
                " 耗时 {}ms 模型={} 上下文占比={} 输出={}字",
                generation_started.elapsed().as_millis(),
                raw.model.as_deref().unwrap_or("<CLI 未回报>"),
                raw.context_usage_ratio
                    .map(|ratio| format!("{:.2}%", ratio * 100.0))
                    .unwrap_or_else(|| "<CLI 未回报>".to_string()),
                raw.text.chars().count()
            ),
        )
        .await;

        // 记下这一轮的占比：下一轮压缩判定直接用，不再靠字符估算。
        if let Some(ratio) = raw.context_usage_ratio {
            self.last_context_ratio
                .lock()
                .await
                .insert(event.conversation_id.clone(), ratio);
        }
        // 记下实际用的模型：界面要显示「当前用什么模型」。
        if let Some(model) = raw.model.as_ref().filter(|name| !name.trim().is_empty()) {
            self.last_model
                .lock()
                .await
                .insert(event.conversation_id.clone(), model.clone());
        }

        if !resume {
            let storage = self.storage.lock().await;
            let _ = storage.save_session(
                &self.project_id,
                &event.conversation_id,
                &session_id,
                &settings.agent_cwd,
            );
        }

        let mut text = crate::reply::sanitize_reply(&raw.text, reply.max_chars);
        if text.is_empty() {
            self.trace(
                &event.conversation_id,
                "⑭ 清洗",
                &format!(
                    " → 空（原始 {} 字），判为失败",
                    raw.text.chars().count()
                ),
            )
            .await;
            self.finish_batch(&events, "failed", Some("清洗后回复为空")).await;
            return;
        }

        self.trace(
            &event.conversation_id,
            "⑭ 清洗",
            &format!(
                " {}字 → {}字（上限 {}）",
                raw.text.chars().count(),
                text.chars().count(),
                reply.max_chars
            ),
        )
        .await;

        // 会话是有记忆的：一旦某次按「我是编程助手」拒绝了，这条拒绝就留在会话里，
        // 之后 resume 同一会话会一直拒绝（实测）。识别到就换新会话重试一次，
        // 并把坏会话替换掉，让后续 resume 不再踩同一脚。
        if resume && crate::reply::looks_like_refusal(&text) {
            self.push_log(
                "本次回复像是在拒绝（旧会话里可能有拒绝惯性），换新会话重试一次",
            )
            .await;
            self.trace(
                &event.conversation_id,
                "⑮ 拒答重试",
                &format!(" 命中拒答特征，原文: {}", truncate(&text, 120)),
            )
            .await;

            let fresh_id = uuid::Uuid::new_v4().to_string();
            let retry_started = std::time::Instant::now();
            match crate::reply::generate(&settings, &prompt, Some(&fresh_id), false).await {
                Ok(fresh_raw) => {
                    let fresh_text = crate::reply::sanitize_reply(&fresh_raw.text, reply.max_chars);
                    if fresh_text.is_empty() {
                        self.push_log("新会话重试得到空回复，保留原回复").await;
                        self.trace(
                            &event.conversation_id,
                            "⑮ 拒答重试",
                            " 新会话返回空，保留原回复",
                        )
                        .await;
                    } else {
                        text = fresh_text;
                        let storage = self.storage.lock().await;
                        let _ = storage.save_session(
                            &self.project_id,
                            &event.conversation_id,
                            &fresh_id,
                            &settings.agent_cwd,
                        );
                        self.push_log("已切换到新会话").await;
                        self.trace(
                            &event.conversation_id,
                            "⑮ 拒答重试",
                            &format!(
                                " 成功，耗时 {}ms，已换成新会话 {}",
                                retry_started.elapsed().as_millis(),
                                &fresh_id[..8.min(fresh_id.len())]
                            ),
                        )
                        .await;
                    }
                }
                Err(err) => {
                    self.push_log(&format!("新会话重试失败，保留原回复: {}", err))
                        .await;
                    self.trace(
                        &event.conversation_id,
                        "⑮ 拒答重试",
                        &format!(" 失败: {}", err),
                    )
                    .await;
                }
            }
        }

        // 强制中文是 prompt 里的要求，模型有可能不遵守。这里只提醒不改写：
        // 硬拦会变成「静默不回」，比回一句英文更糟。
        if !crate::reply::has_cjk(&text) {
            self.push_log("提醒：本次回复不含中文，模型可能没有遵守「强制中文」要求")
                .await;
        }

        let send_started = std::time::Instant::now();
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
                self.trace(
                    &event.conversation_id,
                    "⑯ 发送",
                    &format!(
                        " 成功，耗时 {}ms，正文={}字",
                        send_started.elapsed().as_millis(),
                        text.chars().count()
                    ),
                )
                .await;
                self.finish_batch(&events, "sent", Some(&text)).await;
                self.trace(&event.conversation_id, "⑰ 台账", " 已标记 sent（批次内每条都写）")
                    .await;
                self.forget_trace(&events).await;
            }
            Err(err) => {
                self.trace(
                    &event.conversation_id,
                    "⑯ 发送",
                    &format!(" 失败，耗时 {}ms: {}", send_started.elapsed().as_millis(), err),
                )
                .await;
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

    /// 一批处理完就清掉追踪起点，避免这几十字节的记录无限攒着。
    async fn forget_trace(&self, events: &[ChatEvent]) {
        let mut started = self.trace_started.lock().await;
        for item in events {
            started.remove(&item.message_id);
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
        push_log_line(&self.logs, &self.project_id, &self.kind.to_string(), line).await;
        if let Some(channel) = &self.channel {
            let _ = channel.send(ListenerUpdate::Log {
                listener_id: self.id.clone(),
                line: format!("{} {}", chrono::Local::now().format("%H:%M:%S"), line),
            });
        }
    }

    /// 全链路追踪：从收到 @我 到发出回复，每个阶段一条带 message_id 与耗时的日志。
    ///
    /// 目的是让用户能一眼看出「卡在哪一步」：哪一步之后就没有下一条了，
    /// 就是卡住的位置。异常路径也都要留痕。
    async fn trace(&self, message_id: &str, stage: &str, detail: &str) {
        let short: String = if message_id.is_empty() {
            "<无id>".to_string()
        } else {
            message_id.chars().take(14).collect()
        };
        let elapsed = {
            let started = self.trace_started.lock().await;
            started.get(message_id).copied()
        }
        .map(|start| format!("+{}ms ", start.elapsed().as_millis()))
        .unwrap_or_default();

        self.push_log(&format!("[trace {}] {}{}{}", short, elapsed, stage, detail))
            .await;
    }
}

fn truncate(input: &str, max: usize) -> String {
    if input.chars().count() <= max {
        return input.to_string();
    }
    input.chars().take(max).collect::<String>() + "…"
}

/// 事件是否落在项目的监听范围内。
///
/// 两个名单都空 = 不限制（监听所有群、所有人）。只要指定了名单，命中其一就放行：
/// - 群名单按**会话 id** 命中（群聊会话、单聊会话都能配）；
/// - 人名单按**发送者 open id** 或**会话 id** 命中（前者来自历史发送人，后者来自会话列表里的单聊）。
///
/// 取并集而不是交集：指定「群 A + 人 B」表示 A 群和 B 人的消息都处理，
/// 交集会让「同时属于指定群又是指定人」这种组合几乎永远不成立。
pub fn event_in_scope(group_ids: &[String], member_ids: &[String], event: &ChatEvent) -> bool {
    if group_ids.is_empty() && member_ids.is_empty() {
        return true;
    }
    let conversation = event.conversation_id.as_str();
    if !conversation.is_empty()
        && (group_ids.iter().any(|id| id == conversation)
            || member_ids.iter().any(|id| id == conversation))
    {
        return true;
    }
    let sender = event.sender_open_dingtalk_id.as_str();
    !sender.is_empty() && member_ids.iter().any(|id| id == sender)
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
    let content = crate::reply::sanitize_reply(&raw.text, 4000);
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
    last_context_ratio: Option<f64>,
) -> bool {
    if !reply.auto_compress {
        return false;
    }
    if *failures.lock().await >= 3 {
        return false;
    }

    // 优先用 Agent 自己回报的上下文占比：这是真实占用，不用拿字符数猜。
    // 有基线时只用占比判定，不再叠加字符/轮次估算，避免两套规则互相打架。
    if let Some(percent) = reply.compress_trigger_percent.filter(|percent| *percent > 0) {
        if let Some(ratio) = last_context_ratio {
            return ratio * 100.0 >= percent as f64;
        }
        // 还没有基线（本项目这个会话第一次回复），退到字符估算。
    }

    if reply.compress_trigger_turns.is_none() && reply.compress_trigger_chars.is_none() {
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

    /// 监听范围过滤的判定规则（纯函数，不需要起进程）。
    ///
    /// 空名单 = 不限制；指定了名单就要求命中其一（群按会话 id、人按发送者 open id 或单聊会话 id）。
    #[test]
    fn scope_filter_keeps_only_listed_groups_and_people() {
        let event = |conversation: &str, sender: &str| ChatEvent {
            project_id: "p1".to_string(),
            message_id: "msg-1".to_string(),
            conversation_id: conversation.to_string(),
            sender: "同事".to_string(),
            sender_open_dingtalk_id: sender.to_string(),
            content: "@我 在吗".to_string(),
            create_time: String::new(),
            received_at: String::new(),
            listen_kind: "at-me".to_string(),
            malformed: false,
            raw: String::new(),
        };
        let ids = |items: &[&str]| items.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let none: Vec<String> = Vec::new();

        assert!(
            event_in_scope(&none, &none, &event("cid-any", "open-9")),
            "两个名单都空 = 监听所有群、所有人"
        );

        let groups = ids(&["cid-group"]);
        assert!(event_in_scope(&groups, &none, &event("cid-group", "open-9")));
        assert!(
            !event_in_scope(&groups, &none, &event("cid-other", "open-9")),
            "没在指定群里的事件应被丢弃"
        );

        let people = ids(&["open-1"]);
        assert!(event_in_scope(&none, &people, &event("cid-other", "open-1")));
        assert!(
            !event_in_scope(&none, &people, &event("cid-other", "open-2")),
            "不是指定人发的事件应被丢弃"
        );
        assert!(
            event_in_scope(&none, &people, &event("cid-direct-1", "open-1")),
            "单聊里指定人发来的也算命中"
        );
        assert!(
            event_in_scope(&none, &people, &event("open-1", "open-2")),
            "从会话列表挑的单聊（人名单里存的是会话 id）也要命中"
        );

        // 指定了群 + 人：命中其一即可（并集），不是要求同时满足。
        let both_groups = ids(&["cid-group"]);
        let both_people = ids(&["open-1"]);
        assert!(event_in_scope(&both_groups, &both_people, &event("cid-group", "open-2")));
        assert!(event_in_scope(&both_groups, &both_people, &event("cid-other", "open-1")));
        assert!(
            !event_in_scope(&both_groups, &both_people, &event("cid-other", "open-2")),
            "群和人都不命中才丢弃"
        );
    }

    /// 用桩起一路监听，跑一小会儿就停，返回（落盘条数, 日志）。
    async fn run_scoped_listener(
        node: String,
        data_name: &str,
        settings: ReplySettings,
    ) -> (usize, String) {
        let data_dir = std::env::temp_dir().join(data_name);
        let _ = std::fs::remove_dir_all(&data_dir);
        let storage = Arc::new(Mutex::new(Storage::new(data_dir.clone()).unwrap()));
        let orchestrator = Orchestrator::new(storage.clone());
        orchestrator.set_dws_path(Some(node)).await;

        let id = orchestrator
            .start_listener("p1".to_string(), ListenKind::AtMe, settings, None)
            .await
            .expect("应能启动监听");
        sleep(Duration::from_millis(900)).await;
        let _ = orchestrator.stop_listener(&id).await;

        let rows = storage
            .lock()
            .await
            .list_events(&EventQuery {
                limit: 50,
                ..Default::default()
            })
            .unwrap();
        let logs = orchestrator
            .get_logs(500)
            .await
            .iter()
            .map(|entry| entry.line.clone())
            .collect::<Vec<_>>()
            .join("\n");
        let _ = std::fs::remove_dir_all(&data_dir);
        (rows.len(), logs)
    }

    /// 指定了群/人名单后，不在范围内的事件必须在**落盘之前**就被丢掉：
    /// 存下来只会污染会话列表与统计，用户会以为「指定了范围却没生效」。
    #[tokio::test]
    async fn out_of_scope_events_are_dropped_before_storage() {
        let Some(node) = node_exe() else {
            eprintln!("跳过：环境里没有 node");
            return;
        };

        let stub_dir = std::env::temp_dir().join("agentmux-scope-stub");
        let _ = std::fs::remove_dir_all(&stub_dir);
        std::fs::create_dir_all(&stub_dir).unwrap();
        std::fs::write(stub_dir.join("event"), STUB).unwrap();

        let _cwd_guard = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let previous = std::env::current_dir().unwrap();
        std::env::set_current_dir(&stub_dir).unwrap();

        // 群名单写了一个不存在的会话 → 桩里的消息全都不该落盘。
        let (dropped_rows, dropped_logs) = run_scoped_listener(
            node.clone(),
            "agentmux-scope-dropped",
            ReplySettings {
                group_ids: vec!["cid-not-listen".to_string()],
                ..ReplySettings::default()
            },
        )
        .await;
        // 人名单命中桩里的发送者（open-1）→ 事件正常入库（3 条：2 正常 + 1 畸形，重复被去重）。
        let (kept_rows, _) = run_scoped_listener(
            node,
            "agentmux-scope-kept",
            ReplySettings {
                member_ids: vec!["open-1".to_string()],
                ..ReplySettings::default()
            },
        )
        .await;

        std::env::set_current_dir(previous).unwrap();

        assert_eq!(dropped_rows, 0, "不在指定群里的事件不应落盘");
        assert!(
            dropped_logs.contains("③ 范围过滤") && dropped_logs.contains("未命中指定名单"),
            "范围过滤必须留痕，否则用户查不出「为什么没收到」:\n{}",
            dropped_logs
        );
        assert_eq!(kept_rows, 3, "命中指定人的事件应照常落盘");
    }

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

        // 日志要带归属，界面才能按项目筛选（问题 3）。
        let logs = orchestrator.get_logs(500).await;
        assert!(!logs.is_empty(), "监听应产生日志");
        assert!(
            logs.iter().all(|entry| entry.project_id == "test-project"),
            "监听产生的日志应归到该项目，实际: {:?}",
            logs.iter().map(|e| (&e.project_id, &e.line)).collect::<Vec<_>>()
        );

        // 统计按项目分口径（问题 5）。
        let scoped = storage.lock().await.get_stats(Some("test-project")).unwrap();
        let unscoped = storage.lock().await.get_stats(None).unwrap();
        assert_eq!(scoped.total_events, 3, "本项目口径应为 3 条");
        assert_eq!(unscoped.total_events, 3, "只有这一个项目有数据");
        assert_eq!(scoped.conversations, 1, "无会话标识的畸形事件不计入会话");
        assert_eq!(
            storage
                .lock()
                .await
                .get_stats(Some("other-project"))
                .unwrap()
                .total_events,
            0,
            "别的项目口径应为 0"
        );

        let _ = std::fs::remove_dir_all(&stub_dir);
        let _ = std::fs::remove_dir_all(&data_dir);
    }

    /// 同一项目 + 同一种监听重复启动必须复用同一实例（问题 4）：
    /// 否则会起两个 dws 抢同一事件，界面上也会多出一行。
    #[tokio::test]
    async fn duplicate_start_reuses_the_same_listener() {
        let Some(node) = node_exe() else {
            eprintln!("跳过：环境里没有 node");
            return;
        };

        let stub_dir = std::env::temp_dir().join("agentmux-dedupe-stub");
        let _ = std::fs::remove_dir_all(&stub_dir);
        std::fs::create_dir_all(&stub_dir).unwrap();
        std::fs::write(stub_dir.join("event"), STUB).unwrap();

        let data_dir = std::env::temp_dir().join("agentmux-dedupe-data");
        let _ = std::fs::remove_dir_all(&data_dir);
        let storage = Arc::new(Mutex::new(Storage::new(data_dir.clone()).unwrap()));
        let orchestrator = Orchestrator::new(storage);
        // 必须把 dws 指到桩脚本上：否则会真的起 dws 订阅（抢真实事件、脏环境）。
        orchestrator.set_dws_path(Some(node)).await;

        let _cwd_guard = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let previous = std::env::current_dir().unwrap();
        std::env::set_current_dir(&stub_dir).unwrap();

        let start = |project: &'static str, kind: ListenKind| {
            let orchestrator = &orchestrator;
            async move {
                orchestrator
                    .start_listener(project.to_string(), kind, ReplySettings::default(), None)
                    .await
                    .expect("应能启动监听")
            }
        };

        let first = start("p1", ListenKind::AtMe).await;
        let second = start("p1", ListenKind::AtMe).await;
        let other_kind = start("p1", ListenKind::DirectMessage).await;
        let other_project = start("p2", ListenKind::AtMe).await;

        let statuses = orchestrator.get_all_listener_status().await;
        let same_scope = statuses
            .iter()
            .filter(|s| s.project_id == "p1" && s.kind == ListenKind::AtMe)
            .count();

        // 放弃掉的实例是终态，不该挡住重新启动，也不该和新起的那行同时挂在界面上。
        // 这里直接造一个「上一轮失败已放弃」的实例（没有进程，只有状态）。
        let abandoned_id = "p3-abandoned-instance".to_string();
        {
            let mut listeners = orchestrator.listeners.lock().await;
            listeners.insert(
                abandoned_id.clone(),
                ListenerTask {
                    status: ListenerStatus {
                        id: abandoned_id.clone(),
                        project_id: "p3".to_string(),
                        kind: ListenKind::AtMe,
                        state: ListenerState::Abandoned,
                        ready: false,
                        subscribe_id: None,
                        bus_pid: None,
                        attempts: MAX_ATTEMPTS,
                        last_error: Some("连续失败，已放弃".to_string()),
                        cli_path: None,
                        dropped_before_ready: 0,
                    },
                    stdin: Arc::new(Mutex::new(None)),
                    pid: Arc::new(Mutex::new(None)),
                    stop_requested: Arc::new(AtomicBool::new(false)),
                },
            );
        }
        let restarted_after_abandon = start("p3", ListenKind::AtMe).await;
        let p3_statuses: Vec<ListenerStatus> = orchestrator
            .get_all_listener_status()
            .await
            .into_iter()
            .filter(|s| s.project_id == "p3")
            .collect();

        let _ = orchestrator.stop_listener(&first).await;
        let _ = orchestrator.stop_listener(&other_kind).await;
        let _ = orchestrator.stop_listener(&other_project).await;
        let _ = orchestrator.stop_listener(&restarted_after_abandon).await;

        // 停掉之后旧实例不再占位，可以重新起一路（不能复用已停止的 id）。
        let restarted = start("p1", ListenKind::AtMe).await;
        let after_restart = orchestrator.get_all_listener_status().await;
        std::env::set_current_dir(previous).unwrap();

        assert_eq!(first, second, "重复启动应复用同一路监听");
        assert_ne!(first, other_kind, "同项目的另一类监听应各自独立");
        assert_ne!(first, other_project, "另一个项目的同类监听应各自独立");
        assert_eq!(same_scope, 1, "同项目同类型只应上报一路，实际: {:?}", statuses);
        assert_ne!(
            restarted_after_abandon, abandoned_id,
            "已放弃的实例不应被复用"
        );
        assert_eq!(
            p3_statuses.len(),
            1,
            "放弃后重启只应剩新起的一路，实际: {:?}",
            p3_statuses
        );
        assert_ne!(restarted, first, "已停止的实例不应被复用");
        assert_eq!(
            after_restart
                .iter()
                .filter(|s| s.project_id == "p1" && s.kind == ListenKind::AtMe)
                .count(),
            1,
            "重启后仍只应有一路，实际: {:?}",
            after_restart
        );

        let _ = orchestrator.stop_listener(&restarted).await;
        let _ = std::fs::remove_dir_all(&stub_dir);
        let _ = std::fs::remove_dir_all(&data_dir);
    }

    /// 退出时必须把监听子进程收干净：留成孤儿进程会继续订阅同一事件，
    /// 下次启动就成了「一个事件被多个监听抢」。
    #[tokio::test]
    async fn shutdown_closes_every_listener_process() {
        let Some(node) = node_exe() else {
            eprintln!("跳过：环境里没有 node");
            return;
        };

        let stub_dir = std::env::temp_dir().join("agentmux-shutdown-stub");
        let _ = std::fs::remove_dir_all(&stub_dir);
        std::fs::create_dir_all(&stub_dir).unwrap();
        std::fs::write(stub_dir.join("event"), STUB).unwrap();

        let data_dir = std::env::temp_dir().join("agentmux-shutdown-data");
        let _ = std::fs::remove_dir_all(&data_dir);
        let storage = Arc::new(Mutex::new(Storage::new(data_dir.clone()).unwrap()));
        let orchestrator = Orchestrator::new(storage);
        // 同上：这里要断言的是「子进程被杀掉」，用桩就够，不要碰真实 dws。
        orchestrator.set_dws_path(Some(node)).await;

        let _cwd_guard = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let previous = std::env::current_dir().unwrap();
        std::env::set_current_dir(&stub_dir).unwrap();

        let at_me = orchestrator
            .start_listener(
                "p1".to_string(),
                ListenKind::AtMe,
                ReplySettings::default(),
                None,
            )
            .await
            .unwrap();
        let direct = orchestrator
            .start_listener(
                "p1".to_string(),
                ListenKind::DirectMessage,
                ReplySettings::default(),
                None,
            )
            .await
            .unwrap();

        // 等两路子进程都真的起来了（bash 里 spawn 是异步的）。
        let mut pids: Vec<u32> = Vec::new();
        for _ in 0..60 {
            sleep(Duration::from_millis(100)).await;
            let handles = {
                let listeners = orchestrator.listeners.lock().await;
                [&at_me, &direct]
                    .iter()
                    .filter_map(|id| listeners.get(*id).map(|task| task.pid.clone()))
                    .collect::<Vec<_>>()
            };
            let mut current = Vec::new();
            for handle in handles {
                if let Some(pid) = *handle.lock().await {
                    current.push(pid);
                }
            }
            if current.len() == 2 {
                pids = current;
                break;
            }
        }
        assert_eq!(pids.len(), 2, "两路监听都应起出子进程");
        assert!(
            pids.iter().all(|pid| process_alive(*pid)),
            "监听起来后子进程应活着: {:?}",
            pids
        );

        orchestrator.shutdown_all_listeners().await;
        std::env::set_current_dir(previous).unwrap();

        for pid in &pids {
            assert!(!process_alive(*pid), "退出后不应残留子进程: pid {}", pid);
        }

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
        let logs = orchestrator.get_logs(500).await;
        let joined = logs
            .iter()
            .map(|entry| entry.line.clone())
            .collect::<Vec<_>>()
            .join("\n");
        // 失败时把全链路 trace 打出来，直接看出卡在哪一步
        // （加 --nocapture 可见）。这正是 trace 要解决的问题。
        if rows[0].reply_status.as_deref() != Some("sent") {
            eprintln!("---- 全链路 trace（回复未成功时打印）----\n{}", joined);
        }
        // 需求：从收到 @我 到发出回复，整条执行流都要有可查的日志。
        // 这里逐个阶段断言，少打一个点就算回归。
        for stage in [
            "① 收到事件",
            "② 去重",
            "③ 范围过滤",
            "④ 落盘",
            "⑤ 推流",
            "⑥ 回复开关",
            "⑦ 单条判定",
            "⑧ 进攒批",
            "⑨ 出批",
            "⑩ 压缩判定",
            "⑪ 拉上下文",
            "⑫ 调 Agent",
            "⑬ 生成返回",
            "⑭ 清洗",
            "⑯ 发送",
            "⑰ 台账",
        ] {
            assert!(
                joined.contains(stage),
                "全链路日志缺少阶段「{}」，实际日志:\n{}",
                stage,
                joined
            );
        }
        // 每条 trace 都要能定位到具体消息与耗时。
        assert!(joined.contains("[trace msg-stub-1]"), "trace 应带 message_id");
        assert!(joined.contains("+0ms"), "trace 应带「距收到多少毫秒」");

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

    /// 自适应压缩：Agent 回报过上下文占比后，就按**占比**判，不再看字符估算。
    /// 这是「压缩依据真实上下文占比」的核心回归。
    #[tokio::test]
    async fn compression_follows_the_reported_context_ratio() {
        let dir = std::env::temp_dir().join("agentmux-compress-ratio-test");
        let _ = std::fs::remove_dir_all(&dir);
        let storage = Arc::new(Mutex::new(Storage::new(dir.clone()).unwrap()));
        let failures = Arc::new(Mutex::new(0));

        // 阈值 75%，字符阈值设得极小，用来证明「有基线时不再走字符估算」。
        let settings = ReplySettings {
            auto_compress: true,
            compress_trigger_percent: Some(75),
            compress_trigger_chars: Some(1),
            ..Default::default()
        };

        // 字符阈值比的是「该会话实际累计的字符数」，所以先放一条消息进去。
        storage
            .lock()
            .await
            .save_event(&ChatEvent {
                project_id: "p".to_string(),
                message_id: "msg-ratio-1".to_string(),
                conversation_id: "cid-x".to_string(),
                sender: "同事".to_string(),
                sender_open_dingtalk_id: "open-1".to_string(),
                content: "你好呀".to_string(),
                create_time: "2026-09-19 20:00:00".to_string(),
                received_at: "2026-09-19T20:00:01+08:00".to_string(),
                listen_kind: "at_me".to_string(),
                malformed: false,
                raw: "{}".to_string(),
            })
            .unwrap();

        // 没有基线 → 退回字符估算（字符阈值 1，已累计 3 字，必然触发）。
        assert!(
            compression_due_with(&storage, &failures, "cid-x", &settings, None).await,
            "没有占比基线时应退回字符估算"
        );

        // 有基线且低于阈值 → 不触发；哪怕字符阈值早就超了。
        assert!(
            !compression_due_with(&storage, &failures, "cid-x", &settings, Some(0.30)).await,
            "占比 30% 低于阈值 75%，不该压缩"
        );

        // 有基线且达到阈值 → 触发。
        assert!(
            compression_due_with(&storage, &failures, "cid-x", &settings, Some(0.80)).await,
            "占比 80% 超过阈值 75%，应压缩"
        );

        // 关掉开关就一律不压。
        let off = ReplySettings {
            auto_compress: false,
            ..settings.clone()
        };
        assert!(
            !compression_due_with(&storage, &failures, "cid-x", &off, Some(0.99)).await,
            "压缩开关关闭时不该触发"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 桩 dws 的事件端：同一会话**分两次**发消息（间隔大于攒批窗口），
    /// 用来验证「第二轮才拿到占比、于是按占比触发压缩」。
    const RATIO_EVENT_STUB: &str = r#"
const READY = "[event] ready event_key=user_im_message_receive_at subscribe_id=subId-ratio bus_pid=1";
function evt(id) {
  return JSON.stringify({
    type: "user_im_message_receive_at",
    subscribe_id: "subId-ratio",
    message_id: id,
    conversation_id: "cid-ratio",
    sender: "Maren",
    sender_open_dingtalk_id: "open-other",
    content: "@我 你好",
    create_time: "2026-09-19 21:00:00"
  });
}
process.stderr.write(READY + "\n");
setTimeout(function () { process.stdout.write(evt("msg-r1") + "\n"); }, 100);
setTimeout(function () { process.stdout.write(evt("msg-r2") + "\n"); }, 1600);
process.stdin.resume();
process.stdin.on("end", function () { process.exit(0); });
"#;

    /// 桩 Agent CLI：**按 qodercli `-o json` 的真实形状**输出，且带噪声前缀行。
    /// 占比刻意给 0.85（高于测试里设的 75% 阈值），用来驱动自适应压缩。
    const RATIO_AGENT_STUB: &str = r#"
process.stdout.write("1 error loading agent configs. Use /agents to see details.\n");
process.stdout.write(JSON.stringify({
  type: "result",
  subtype: "success",
  result: "在的，还没吃呢，你吃了没？",
  usage: { input_tokens: 0, output_tokens: 0, context_usage_ratio: 0.85 },
  modelUsage: { "bailian/qwen3.7-plus-cp": { contextWindow: 0 } }
}) + "\n");
"#;

    /// 端到端验证「读到模型名与真实占比 → 据此触发压缩」整条链。
    /// 分段单测不够：必须证明占比真的从 CLI 输出流到了压缩判定里。
    #[tokio::test]
    async fn reported_context_ratio_flows_into_compression_decision() {
        let Some(node) = node_exe() else {
            eprintln!("跳过：环境里没有 node");
            return;
        };

        let base = std::env::temp_dir().join("agentmux-e2e-ratio");
        let _ = std::fs::remove_dir_all(&base);
        let stub_dir = base.join("stub");
        let work_dir = base.join("project-workdir");
        let data_dir = base.join("data");
        std::fs::create_dir_all(&stub_dir).unwrap();
        std::fs::create_dir_all(&work_dir).unwrap();

        std::fs::write(stub_dir.join("event"), RATIO_EVENT_STUB).unwrap();
        std::fs::write(stub_dir.join("chat"), APPEND_CHAT_STUB).unwrap();
        std::fs::write(stub_dir.join("agentstub"), RATIO_AGENT_STUB).unwrap();
        let agent_stub_path = stub_dir.join("agentstub").to_string_lossy().to_string();

        let storage = Arc::new(Mutex::new(Storage::new(data_dir.clone()).unwrap()));
        let mut orchestrator = Orchestrator::new(storage.clone());
        // 600ms 窗口：两条消息间隔 1500ms，会分成两批，第二轮才能用上第一轮的占比。
        orchestrator.set_reply_batch_window(Duration::from_millis(600));

        let settings = ReplySettings {
            enabled: true,
            agent_platform: "stub".to_string(),
            agent_cli_path: Some(node.clone()),
            agent_args: Some(vec![agent_stub_path]),
            agent_cwd: work_dir.to_string_lossy().to_string(),
            timeout_ms: 20_000,
            max_chars: 500,
            auto_compress: true,
            compress_trigger_percent: Some(75),
            ..Default::default()
        };
        orchestrator.set_dws_path(Some(node)).await;

        let _cwd_guard = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let previous = std::env::current_dir().unwrap();
        std::env::set_current_dir(&stub_dir).unwrap();

        let id = orchestrator
            .start_listener("proj-ratio".to_string(), ListenKind::AtMe, settings, None)
            .await
            .expect("应能启动监听");

        for _ in 0..150 {
            sleep(Duration::from_millis(100)).await;
            let rows = storage
                .lock()
                .await
                .list_events(&EventQuery {
                    limit: 10,
                    project_id: Some("proj-ratio".to_string()),
                    ..Default::default()
                })
                .unwrap();
            if rows.len() == 2 && rows.iter().all(|row| row.reply_status.is_some()) {
                break;
            }
        }

        let _ = orchestrator.stop_listener(&id).await;
        let logs = orchestrator.get_logs(500).await.iter().map(|entry| entry.line.clone()).collect::<Vec<_>>().join("\n");
        let ratio = {
            let map = orchestrator.last_context_ratio.lock().await;
            map.get("cid-ratio").copied()
        };
        std::env::set_current_dir(previous).unwrap();

        let rows = storage
            .lock()
            .await
            .list_events(&EventQuery {
                limit: 10,
                project_id: Some("proj-ratio".to_string()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(rows.len(), 2);

        // 1) 占比真的被解析出来了，并记到了这个会话上。
        assert_eq!(
            ratio,
            Some(0.85),
            "应把 CLI 回报的占比记到该会话，实际 {:?}；日志:\n{}",
            ratio,
            logs
        );

        // 2) 日志里能看见模型名与占比——用户要「读得到」的东西。
        assert!(
            logs.contains("模型=bailian/qwen3.7-plus-cp"),
            "日志应回报模型名，实际:\n{}",
            logs
        );
        assert!(
            logs.contains("上下文占比=85.00%"),
            "日志应回报上下文占比，实际:\n{}",
            logs
        );

        // 3) 第二轮按占比越阈值 → 真的触发了压缩。
        assert!(
            logs.contains("→ 触发压缩"),
            "占比 85% 超过阈值 75%，应触发压缩，实际:\n{}",
            logs
        );

        // 4) 两条都正常回复（压缩不阻塞回复）。
        for row in &rows {
            assert_eq!(
                row.reply_status.as_deref(),
                Some("sent"),
                "{} 应已回复",
                row.message_id
            );
        }

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
            logs.iter().any(|entry| entry.line.contains("[event] ready")),
            "日志里应保留 ready 原文"
        );

        let stopped = orchestrator.stop_listener(&id).await;
        assert!(
            matches!(stopped.as_deref(), Ok("stdin-eof") | Ok("killed")),
            "停机应走完阶梯，实际: {:?}",
            stopped
        );

        let after = orchestrator.get_all_listener_status().await;
        assert!(
            after.iter().all(|status| status.id != id),
            "停止后不应再上报该路监听（否则界面会残留一行），实际: {:?}",
            after
        );

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
