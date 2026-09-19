//! 回复链路：调 Agent CLI 生成 → 清洗正文 → 用 dws 发回原会话。
//!
//! 行为对齐旧基准工程 `dingtalk-event-host`：
//! - 生成：prompt 走 stdin，非交互 + 只读工具白名单，会话参数必须追加在**末尾**
//!   （`--tools` 是变长参数，放后面才会停住，否则会话参数会被当成工具名吃掉）
//! - 发送：`dws chat +messages-send --group <会话> --text <正文> --uuid <原 message_id> --yes -f json`
//! - 清洗：剔除正文里所有 @、压平空白、按字符截断

use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

#[derive(Debug, Clone, serde::Serialize)]
pub struct ReplySettings {
    pub enabled: bool,
    pub agent_platform: String,
    pub agent_cli_path: Option<String>,
    /// 为空 = 用该平台默认的只读参数（A2.2.4 允许显式覆盖）。
    pub agent_args: Option<Vec<String>>,
    pub agent_cwd: String,
    pub timeout_ms: u64,
    pub max_chars: usize,
    /// 自身在该 IM 平台上的身份 id：用于跳过自己发的消息。
    /// 不叫 dingtalk id —— 以后 IM 不止钉钉。
    pub self_open_id: Option<String>,
    pub context_enabled: bool,
    pub context_message_limit: usize,
    pub context_max_chars: usize,
    pub auto_compress: bool,
    /// D-59：阈值未定，None = 不按该维度自动触发（不拍脑袋定值）。
    pub compress_trigger_turns: Option<usize>,
    pub compress_trigger_chars: Option<usize>,
}

impl Default for ReplySettings {
    fn default() -> Self {
        Self {
            enabled: false,
            agent_platform: "qoder".to_string(),
            agent_cli_path: None,
            agent_args: None,
            agent_cwd: String::new(),
            timeout_ms: 120_000,
            max_chars: 500,
            self_open_id: None,
            context_enabled: true,
            context_message_limit: 50,
            context_max_chars: 8000,
            auto_compress: false,
            compress_trigger_turns: None,
            compress_trigger_chars: None,
        }
    }
}

/// 各 Agent 平台的只读 + 非交互默认参数。新增平台在这里补一行。
pub fn default_agent_args(platform_id: &str) -> Vec<String> {
    match platform_id {
        "qoder" => vec![
            "-p".to_string(),
            "--tools".to_string(),
            "Read".to_string(),
            "Grep".to_string(),
            "Glob".to_string(),
        ],
        "claude" => vec!["-p".to_string()],
        // Codex 的默认参数标为待决：未实测，不做猜测。
        _ => vec!["-p".to_string()],
    }
}

/// 剔除 @ 提及（`@` 及其后连续非空白字符）、压平空白、按字符截断。
pub fn sanitize_reply(raw: &str, max_chars: usize) -> String {
    let mut without_mentions = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '@' {
            while let Some(next) = chars.peek() {
                if next.is_whitespace() || *next == '@' {
                    break;
                }
                chars.next();
            }
            continue;
        }
        without_mentions.push(ch);
    }

    let flattened = without_mentions
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");

    let flattened = flattened.trim();
    if flattened.chars().count() <= max_chars {
        return flattened.to_string();
    }
    flattened.chars().take(max_chars).collect()
}

/// 剥离 @ 之后正文为空时的占位问句。
///
/// 对方只 @ 了一下、没写具体内容时，**不要直接跳过**：用户看到的是「收到消息但
/// 没回复」。交给 Agent 自然回应更符合预期。
pub const EMPTY_MENTION_PLACEHOLDER: &str =
    "（对方只是 @ 了我，没有写具体内容；请自然打个招呼，并询问需要什么帮助）";

/// 回复生成的角色与硬约束。
///
/// 缺了这段时，Agent 会把来信当成一个「任务」来处理：面对「你好，吃晚饭了吗」这种
/// 正常寒暄，它回的是「I'm a software engineering assistant, not the right tool
/// for composing personal chat replies」——既拒绝了，还是英文。所以必须明确告诉它
/// 「你在替本人回消息，寒暄就是你的职责」，并强制中文。
pub const REPLY_PERSONA: &str = "\
你是本人在钉钉里的回复助手：请代替本人，在这个会话里回一条消息给对方。

必须遵守：
1. 一律使用简体中文回复，任何情况下都不要输出英文。
2. 日常寒暄、闲聊、问候、约时间、问事都属于你的职责范围，正常回应即可；\
不要说自己是 AI、助手或软件工程助手，不要以「这不属于我的职责」为由拒绝，也不要解释你在做什么。
3. 只输出要发出去的那句话本身：不要加引号、前缀、署名、括号说明或任何解释。
4. 像真人在钉钉里打字，一到两句话说完，不要长篇大论，不要列点。";

/// 一批消息合成一条回复的 prompt。
///
/// 对方在很短时间内连发 n 条时，逐条各回一条会刷屏；这里把它们并成一次回复，
/// 让 Agent 看到整批内容后只输出一条。**只有这一个入口**：单条也走这里，
/// 免得有人绕开 `REPLY_PERSONA` 拼 prompt。
pub fn build_prompt_for_batch(
    contents: &[String],
    summary: Option<&str>,
    context_lines: &[String],
    context_enabled: bool,
) -> String {
    let cleaned: Vec<String> = contents
        .iter()
        .map(|raw| sanitize_reply(raw, 4000))
        .filter(|text| !text.is_empty())
        .collect();

    let ask = match cleaned.len() {
        0 => format!(
            "请回复对方：\n{}",
            EMPTY_MENTION_PLACEHOLDER
        ),
        1 => format!("请回复对方这条消息：\n{}", cleaned[0]),
        n => {
            let listed = cleaned
                .iter()
                .enumerate()
                .map(|(index, text)| format!("{}. {}", index + 1, text))
                .collect::<Vec<_>>()
                .join("\n");
            format!(
                "对方在一两分钟内连着发了 {} 条消息，请**只回一条**，把这几条一起回应，\
不要逐条分别作答，也不要复述它们：\n{}",
                n, listed
            )
        }
    };

    let mut blocks: Vec<String> = vec![REPLY_PERSONA.to_string()];
    if let Some(summary) = summary.filter(|s| !s.trim().is_empty()) {
        blocks.push(format!("以下是该会话更早内容的摘要（不要复述）：\n{}", summary.trim()));
    }
    if context_enabled && !context_lines.is_empty() {
        blocks.push(format!(
            "以下是钉钉会话的最近消息，供你理解上下文（不要复述它们）：\n{}",
            context_lines.join("\n")
        ));
    }
    blocks.push(ask);

    blocks.join("\n\n")
}

/// 是否包含中日韩文字。用来兜底检查「强制中文」有没有被模型绕过去。
pub fn has_cjk(text: &str) -> bool {
    text.chars()
        .any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c))
}

/// 生成摘要用的 prompt。要求保留关键事实且不得编造。
pub fn build_summary_prompt(rows: &[(String, String)]) -> String {
    let transcript = rows
        .iter()
        .rev()
        .map(|(sender, content)| format!("{}: {}", sender, sanitize_reply(content, 800)))
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        "把下面这段钉钉会话记录压缩成一段简短摘要，保留关键事实、结论与待办，\
不要逐条复述，不要添加原文没有的信息：\n\n{}",
        transcript
    )
}

/// 组装上下文行：从最近往最旧取，累加到字符预算为止。
pub fn context_lines(
    rows: &[(String, String)],
    limit: usize,
    max_chars: usize,
) -> Vec<String> {
    let mut picked: Vec<String> = Vec::new();
    let mut budget = max_chars;

    for (sender, content) in rows.iter().rev().take(limit) {
        let line = format!("{}: {}", sender, sanitize_reply(content, 500));
        let cost = line.chars().count();
        if cost > budget {
            break;
        }
        budget -= cost;
        picked.push(line);
    }

    picked.reverse();
    picked
}

/// 调 Agent CLI 生成回复。返回 (纯文本, 可能出现的会话 id)。
pub async fn generate(
    settings: &ReplySettings,
    prompt: &str,
    session_id: Option<&str>,
    resume: bool,
) -> anyhow::Result<String> {
    let bin = settings
        .agent_cli_path
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("未配置 Agent CLI 路径"))?;

    let mut args = match settings.agent_args.as_ref() {
        Some(overridden) if !overridden.is_empty() => overridden.clone(),
        _ => default_agent_args(&settings.agent_platform),
    };
    // 会话参数必须追加在末尾，理由见模块头注释。
    if let Some(session_id) = session_id {
        args.push(if resume { "--resume" } else { "--session-id" }.to_string());
        args.push(session_id.to_string());
    }

    let mut command = Command::new(bin);
    command
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    crate::process::hide_console(&mut command);

    // 实测（2026-09-19）：从 Qoder agent 宿主里启动时，这些变量会把 CLI 强推到
    // SDK 模式，非交互生成直接报 `sdk_invalid_args` 而失败。宿主拉起的子进程
    // 不该继承宿主自己的 SDK 上下文。
    for key in [
        "QODER_AGENT_SDK_ENTRYPOINT",
        "CLAUDE_CODE_ENTRYPOINT",
        "AGENT_SDK_ENTRYPOINT",
    ] {
        command.env_remove(key);
    }

    if !settings.agent_cwd.is_empty() {
        command.current_dir(&settings.agent_cwd);
    }

    let mut child = command.spawn()?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(prompt.as_bytes()).await?;
        stdin.shutdown().await?;
    }

    // 并发读干管，避免子进程输出把管道缓冲写满后卡死。
    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();
    let stdout_task = tokio::spawn(async move { read_pipe(stdout_pipe).await });
    let stderr_task = tokio::spawn(async move { read_pipe(stderr_pipe).await });

    let status = match tokio::time::timeout(Duration::from_millis(settings.timeout_ms), child.wait())
        .await
    {
        Ok(result) => result?,
        Err(_) => {
            let _ = child.kill().await;
            anyhow::bail!("生成超时（{}ms）", settings.timeout_ms);
        }
    };

    let stdout = stdout_task.await.unwrap_or_default();
    let stderr = stderr_task.await.unwrap_or_default();

    if !status.success() {
        anyhow::bail!(
            "生成失败 code={:?}: {}",
            status.code(),
            truncate(stderr.trim(), 300)
        );
    }

    let stdout = stdout.trim().to_string();
    if stdout.is_empty() {
        anyhow::bail!("生成结果为空: {}", truncate(stderr.trim(), 300));
    }

    Ok(stdout)
}

async fn read_pipe<R>(pipe: Option<R>) -> String
where
    R: tokio::io::AsyncRead + Unpin,
{
    use tokio::io::AsyncReadExt;

    let Some(mut pipe) = pipe else {
        return String::new();
    };
    let mut buffer = String::new();
    let _ = pipe.read_to_string(&mut buffer).await;
    buffer
}

/// 用 dws 把回复发回原会话。uuid 取原 message_id，保证幂等。
pub async fn send(
    dws_path: &str,
    conversation_id: &str,
    text: &str,
    idempotency_key: &str,
) -> anyhow::Result<Option<String>> {
    let mut command = Command::new(dws_path);
    command
        .args([
            "chat",
            "+messages-send",
            "--group",
            conversation_id,
            "--text",
            text,
            "--uuid",
            idempotency_key,
            // confirmation=user_required：无人值守必须显式确认
            "--yes",
            "-f",
            "json",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    crate::process::hide_console(&mut command);

    let output = tokio::time::timeout(Duration::from_secs(30), command.output())
        .await
        .map_err(|_| anyhow::anyhow!("发送超时"))??;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("发送失败: {}", truncate(stderr.trim(), 300));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let message_id = serde_json::from_str::<serde_json::Value>(&stdout)
        .ok()
        .and_then(|json| {
            json.get("result")
                .and_then(|r| r.get("messageId"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        });

    Ok(message_id)
}

fn truncate(input: &str, max: usize) -> String {
    if input.chars().count() <= max {
        return input.to_string();
    }
    input.chars().take(max).collect::<String>() + "…"
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 单条消息的 prompt：测试里最常用，包一层省得每处都写数组字面量。
    fn one(content: &str, context: &[String], context_enabled: bool) -> String {
        build_prompt_for_batch(&[content.to_string()], None, context, context_enabled)
    }

    #[test]
    fn sanitize_strips_mentions_and_collapses_whitespace() {
        assert_eq!(sanitize_reply("@谢斌(谢斌) 你好   世界\n第二行", 500), "你好 世界 第二行");
        assert_eq!(sanitize_reply("@a@b 结果", 500), "结果");
    }

    #[test]
    fn sanitize_truncates_by_chars() {
        assert_eq!(sanitize_reply("一二三四五", 3), "一二三");
    }

    #[test]
    fn context_is_trimmed_to_budget_and_oldest_first() {
        let rows = vec![
            ("甲".to_string(), "最早".to_string()),
            ("乙".to_string(), "中间".to_string()),
            ("丙".to_string(), "最新".to_string()),
        ];
        let lines = context_lines(&rows, 2, 8000);
        assert_eq!(lines, vec!["乙: 中间".to_string(), "丙: 最新".to_string()]);
    }

    #[test]
    fn prompt_omits_context_when_disabled() {
        let prompt = one("@我 你好", &["甲: 旧消息".to_string()], false);
        assert!(!prompt.contains("旧消息"), "关掉上下文就不该带历史消息");
        assert!(prompt.contains("你好"), "来信正文必须在 prompt 里");
    }

    /// 回归：不写角色约束时，Agent 会把「你好，吃晚饭了吗」当成任务并拒绝，
    /// 而且用英文回。这段 persona 是强制中文 + 明确寒暄属于职责的唯一手段。
    #[test]
    fn prompt_always_requires_chinese_and_forbids_refusing() {
        let prompt = one("@我 你好，吃晚饭了吗", &[], false);

        assert!(prompt.starts_with(crate::reply::REPLY_PERSONA));
        assert!(prompt.contains("一律使用简体中文回复"), "必须强制中文");
        assert!(
            prompt.contains("不要说自己是 AI、助手或软件工程助手"),
            "必须禁止「我不是干这个的」式拒绝"
        );
        assert!(prompt.contains("日常寒暄"), "必须点明寒暄属于职责范围");
    }

    /// 多条消息合并成一条回复：一次给全部正文，并明确只回一条。
    #[test]
    fn batch_prompt_lists_all_messages_and_asks_for_a_single_reply() {
        let prompt = build_prompt_for_batch(
            &[
                "@我 你好".to_string(),
                "@我 在吗".to_string(),
                "@我 吃晚饭了吗".to_string(),
            ],
            None,
            &[],
            false,
        );

        assert!(prompt.contains("1. 你好"), "应列出第 1 条：{}", prompt);
        assert!(prompt.contains("2. 在吗"));
        assert!(prompt.contains("3. 吃晚饭了吗"));
        assert!(prompt.contains("只回一条"), "必须要求合并成一条");
        assert!(prompt.contains("连着发了 3 条消息"));
    }

    /// 单条消息不能出现「合并」措辞，否则模型会莫名其妙地说自己合并了消息。
    #[test]
    fn single_message_batch_prompt_reads_naturally() {
        let prompt = build_prompt_for_batch(&["@我 你好".to_string()], None, &[], false);

        assert!(prompt.contains("请回复对方这条消息"));
        assert!(!prompt.contains("只回一条"));
        assert!(prompt.ends_with("你好"));
    }

    #[test]
    fn bare_mention_gets_a_placeholder_instead_of_an_empty_question() {
        // 回归：裸 @ 曾经被上层直接跳过，导致「收到消息但没回复」
        let prompt = one("@谢斌 ", &[], false);
        assert!(!prompt.trim().is_empty(), "裸 @ 不应产生空 prompt");
        assert!(
            prompt.contains("只是 @ 了我"),
            "应使用占位说明，实际: {}",
            prompt
        );
    }

    #[test]
    fn prompt_includes_summary_even_when_context_off() {
        let prompt = build_prompt_for_batch(
            &["@我 进展如何".to_string()],
            Some("上轮结论：待定"),
            &[],
            false,
        );
        assert!(prompt.contains("上轮结论：待定"));
        assert!(prompt.ends_with("进展如何"));
    }

    #[test]
    fn summary_prompt_keeps_chronological_order() {
        // 输入约定：recent_messages 返回「最新在前」。
        let rows = vec![
            ("乙".to_string(), "第二".to_string()),
            ("甲".to_string(), "第一".to_string()),
        ];
        let prompt = build_summary_prompt(&rows);
        let first = prompt.find("第一").unwrap();
        let second = prompt.find("第二").unwrap();
        assert!(first < second, "摘要 prompt 应按时间正序排列（输入最新在前）");
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

    /// 实测过的坑：宿主自己的 SDK 环境变量不能泄漏给 Agent 子进程。
    #[tokio::test]
    async fn generate_does_not_leak_sdk_entrypoint_env() {
        let Some(node) = node_exe() else {
            eprintln!("跳过：环境里没有 node");
            return;
        };

        std::env::set_var("QODER_AGENT_SDK_ENTRYPOINT", "sdk-ts");

        let settings = ReplySettings {
            agent_platform: "stub".to_string(),
            agent_cli_path: Some(node),
            agent_args: Some(vec![
                "-e".to_string(),
                "process.stdout.write(String(process.env.QODER_AGENT_SDK_ENTRYPOINT))".to_string(),
            ]),
            agent_cwd: String::new(),
            timeout_ms: 20_000,
            ..Default::default()
        };

        let output = generate(&settings, "忽略这段 prompt", None, false)
            .await
            .expect("stub 生成应成功");

        std::env::remove_var("QODER_AGENT_SDK_ENTRYPOINT");
        assert_eq!(output, "undefined", "子进程不应继承 SDK entrypoint 变量");
    }

    /// 默认 `#[ignore]`：会真的调一次模型（耗时且产生用量），
    /// 需要时用 `cargo test -- --ignored real_qodercli` 显式跑。
    #[tokio::test]
    #[ignore]
    async fn real_qodercli_generates_text() {
        let Some(bin) = crate::resolve::resolve_executable("qoder").await else {
            eprintln!("跳过：没有找到 qodercli");
            return;
        };

        let settings = ReplySettings {
            agent_platform: "qoder".to_string(),
            agent_cli_path: Some(bin),
            agent_args: None,
            agent_cwd: std::env::temp_dir().to_string_lossy().to_string(),
            timeout_ms: 180_000,
            ..Default::default()
        };

        let output = generate(&settings, "只回答两个字：收到", None, false)
            .await
            .expect("真实 qodercli 应能非交互生成");

        assert!(
            output.contains("收到"),
            "真实生成结果应包含预期文本，实际: {}",
            output
        );
    }
}
