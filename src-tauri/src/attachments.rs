//! 被引用消息（引用回复）的解析与附件获取。
//!
//! 钉钉的事件载荷里带 `quoted_message`：对方引用某条消息再说话时，被引用的
//! 内容就在里面。正文只有一句「你看下这个」，真正要看的东西在引用里 —— 不取
//! 出来，Agent 就只能回一句「好的，我看看」。

use std::path::{Path, PathBuf};

use serde::Serialize;

/// 引用内容里代表「有资源可取」的前缀。钉钉把这类消息的正文渲染成
/// `[文件] 名字.xlsx` / `[图片] 截图.png` —— 要真正看懂，就得把资源取回来。
///
/// 只收这两类：图片 `Read` 能直接读，文件可以转成文本读。视频/语音取了也读不了，
/// 不在此列，免得白下载。
const ATTACHMENT_PREFIXES: &[&str] = &["[文件]", "[图片]"];

/// 从事件 raw JSON 里解析出的引用信息。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct QuotedRef {
    /// 被引用消息的 open message id，用来回头查它的资源（附件）。
    pub quoted_message_id: String,
    /// 被引用内容的原文，如 `[文件] 用例.xlsx`。
    pub text: String,
    /// 被引用的是不是文件消息。
    pub is_file: bool,
    /// 文件消息里的文件名（仅 is_file 时有值）。
    pub file_name: Option<String>,
}

/// 从事件原始 JSON 解析 `quoted_message`。
///
/// 没有引用、或引用缺 message_id（拿不到资源）时返回 None —— 调用方按
/// 「这条消息没有引用」处理即可。
pub fn quoted_from_raw(raw: &str) -> Option<QuotedRef> {
    let value: serde_json::Value = serde_json::from_str(raw).ok()?;
    let quoted = value.get("quoted_message")?.as_object()?;

    // 拿不到被引用消息的 id，就查不出它的资源 —— 按「没有引用」处理，
    // 免得后面为了一个空 id 白跑一次 dws。
    let quoted_message_id = quoted.get("message_id")?.as_str()?.trim().to_string();
    if quoted_message_id.is_empty() {
        return None;
    }

    let text = quoted
        .get("content")
        .and_then(|item| item.as_str())
        .unwrap_or_default()
        .trim()
        .to_string();

    let (is_file, file_name) = match ATTACHMENT_PREFIXES
        .iter()
        .find_map(|prefix| text.strip_prefix(prefix))
    {
        Some(rest) => {
            let name = rest.trim();
            (
                true,
                if name.is_empty() {
                    None
                } else {
                    Some(name.to_string())
                },
            )
        }
        None => (false, None),
    };

    Some(QuotedRef {
        quoted_message_id,
        text,
        is_file,
        file_name,
    })
}

/// 附件下载到工作目录后，需要写进提示词的信息。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Fetched {
    /// 工作目录内的相对路径（dws 的 `localPath`）。写进提示词的必须是相对路径：
    /// Agent 的 cwd 就是工作目录，相对路径能直接命中。
    pub rel_path: String,
    /// 文件大小。超限时只报「过大未读取」，不把内容塞进提示词。
    pub size_bytes: u64,
}

/// 附件统一落在工作目录的这个子目录下。
///
/// 必须在工作目录内 —— Agent 只能读工作目录，而且 dws 也强制 `--output-dir`
/// 不得绝对路径或 `..` 逃逸。写进用户项目目录是免不了的，实际路径会打进监听日志。
pub const ATTACHMENT_DIR: &str = ".agentmux/attachments";

/// 从 `dws chat +messages-mget` 的 JSON 输出里取出**下载成功**的附件。
///
/// 只看 `resourceDownloads.downloads`：那是逐资源的成功 ledger。失败项在
/// `failures` 里，由调用方按需记日志 —— 一个附件失败不该拖垮整条回复。
pub fn parse_downloads(stdout: &str) -> Vec<Fetched> {
    // dws 的 stdout 可能先出现非 JSON 的提示行，从第一个 `{` 开始解析 ——
    // 与 conversations.rs 解析 recent-conversations 的做法一致。
    let Some(start) = stdout.find('{') else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&stdout[start..]) else {
        return Vec::new();
    };

    value
        .get("resourceDownloads")
        .and_then(|downloads| downloads.get("downloads"))
        .and_then(|items| items.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    let rel_path = item.get("localPath").and_then(|v| v.as_str())?.trim();
                    if rel_path.is_empty() {
                        return None;
                    }
                    Some(Fetched {
                        rel_path: rel_path.to_string(),
                        size_bytes: item.get("sizeBytes").and_then(|v| v.as_u64()).unwrap_or(0),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// 把 dws 给的相对路径落到工作目录下，并确认它**确实没跑出工作目录**。
///
/// 用白名单：只接受全部由普通路径段组成的相对路径。绝对路径、盘符、任何
/// `..` 或 `.` 段一律拒绝。dws 自己也拦逃逸，这里再确认一次 —— 路径是上游
/// 返回的数据，不能只信它。
pub fn resolve_within(work_dir: &Path, rel_path: &str) -> Option<PathBuf> {
    let rel = rel_path.trim();
    if rel.is_empty() {
        return None;
    }

    let candidate = Path::new(rel);
    if candidate.is_absolute() {
        return None;
    }
    if candidate
        .components()
        .any(|part| !matches!(part, std::path::Component::Normal(_)))
    {
        return None;
    }

    Some(work_dir.join(candidate))
}

/// 把被引用消息的附件下到 `<work_dir>/.agentmux/attachments/`。
///
/// 传 `work_dir` 作 dws 的 cwd 是必需的：`--output-dir` 只接受工作目录内的
/// 相对路径，dws 按自己的 cwd 解析它。
///
/// 带 `--overwrite`：同一条消息被反复引用（或重试）时，`+messages-mget` 默认
/// 拒绝覆盖已有文件，会白跑一次。目标目录是我们自己管的 `.agentmux/attachments/`，
/// 覆盖是安全的，而且幂等。
pub async fn download_attachment(
    dws_path: &str,
    work_dir: &str,
    quoted_message_id: &str,
) -> anyhow::Result<Vec<Fetched>> {
    let work = Path::new(work_dir);
    let output_dir = format!("./{}", ATTACHMENT_DIR);
    let args = [
        "chat",
        "+messages-mget",
        "--msg-ids",
        quoted_message_id,
        "--download-resources",
        "--output-dir",
        output_dir.as_str(),
        "--overwrite",
        "-f",
        "json",
    ];

    let stdout = crate::conversations::run_dws_in(dws_path, &args, Some(work), 60).await?;

    Ok(parse_downloads(&stdout)
        .into_iter()
        .filter(|item| resolve_within(work, &item.rel_path).is_some())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 真实载荷（2026-09-20 取自 events.raw，会话 cidv2FVuOK1… 的第 40 条事件）。
    const REAL_QUOTED_FILE: &str = r#"{"type":"user_im_message_receive_o2o_all","message_id":"msgPiD9QsUfVOdGVQ1/gSkXLQ==","conversation_id":"cidv2FVuOK1BkpBYhO+HoLtoXEWQjApq7EF9yksKICvCVo=","sender":"谢斌","sender_open_dingtalk_id":"Dq4c2eHMKEFl0sTh6sXAniiMRfKJvYR13ii","content":"你好你看下这个测试用例咋样","create_time":"2026-09-20 12:15:26","quoted_message":{"message_id":"msgFnAefjZ5dKDfpuJZTPPFWg==","conversation_id":"cidv2FVuOK1BkpBYhO+HoLtoXEWQjApq7EF9yksKICvCVo=","sender":"null","sender_open_dingtalk_id":"","content":"[文件] 内部款-预警规则-测试用例（后端）.xlsx","create_time":"2026-09-18 09:41:36"}}"#;

    #[test]
    fn parses_quoted_file_from_real_payload() {
        let q = quoted_from_raw(REAL_QUOTED_FILE).expect("真实载荷里应有引用");

        assert_eq!(q.quoted_message_id, "msgFnAefjZ5dKDfpuJZTPPFWg==");
        assert!(q.is_file, "以 [文件] 开头应判定为文件");
        assert_eq!(
            q.file_name.as_deref(),
            Some("内部款-预警规则-测试用例（后端）.xlsx")
        );
        assert_eq!(q.text, "[文件] 内部款-预警规则-测试用例（后端）.xlsx");
    }

    #[test]
    fn no_quoted_message_yields_none() {
        let raw = r#"{"message_id":"msg1==","conversation_id":"cid1==","sender":"张三","content":"在吗"}"#;
        assert!(quoted_from_raw(raw).is_none(), "没有引用时应为 None");
    }

    #[test]
    fn quoted_text_is_not_a_file() {
        let raw = r#"{"message_id":"msg1==","conversation_id":"cid1==","content":"同意吗","quoted_message":{"message_id":"msg0==","conversation_id":"cid1==","content":"这个方案我觉得可以","create_time":"2026-09-20 10:00:00"}}"#;
        let q = quoted_from_raw(raw).expect("引用了文字也应有引用信息");

        assert_eq!(q.quoted_message_id, "msg0==");
        assert!(!q.is_file, "引用纯文字不是文件");
        assert_eq!(q.file_name, None);
        assert_eq!(q.text, "这个方案我觉得可以");
    }

    #[test]
    fn quoted_image_counts_as_file() {
        let raw = r#"{"message_id":"msg1==","conversation_id":"cid1==","quoted_message":{"message_id":"msg0==","content":"[图片] 截图.png","create_time":"2026-09-20 10:00:00"}}"#;
        let q = quoted_from_raw(raw).expect("引用图片也应有引用信息");

        assert!(q.is_file, "图片也是需要取回来的附件");
        assert_eq!(q.file_name.as_deref(), Some("截图.png"));
    }

    #[test]
    fn quoted_without_message_id_is_ignored() {
        let raw = r#"{"message_id":"msg1==","conversation_id":"cid1==","quoted_message":{"content":"[文件] 拿不到 id.xlsx"}}"#;
        assert!(
            quoted_from_raw(raw).is_none(),
            "拿不到被引用消息 id 就没法取资源，应忽略"
        );
    }

    #[test]
    fn malformed_raw_yields_none() {
        assert!(quoted_from_raw("").is_none());
        assert!(quoted_from_raw("not json").is_none());
        assert!(quoted_from_raw("[1,2,3]").is_none(), "非对象也应忽略");
    }

    /// 真实下载输出（2026-09-20 实测：`dws chat +messages-mget --msg-ids
    /// msgFnAefjZ5dKDfpuJZTPPFWg== --download-resources -f json` 的
    /// `resourceDownloads` 段，原样抄下来）。
    const REAL_DOWNLOAD_LEDGER: &str = r#"{
      "ok": true,
      "messages": [{"messageId":"msgFnAefjZ5dKDfpuJZTPPFWg==","sender":"谢斌"}],
      "resourceDownloads": {
        "deduplicatedCount": 0,
        "discoveredCount": 1,
        "downloadedCount": 1,
        "downloads": [
          {
            "localPath": ".agentmux/attachments/内部款-预警规则-测试用例（后端）.xlsx",
            "messageId": "",
            "resourceId": "Amq4vjg895nGlqqQIx2MbAl483kdP0wQ",
            "resourceType": "fileId",
            "sizeBytes": 1796166
          }
        ],
        "failedCount": 0,
        "failures": [],
        "ok": true,
        "partial": false,
        "requestedCount": 1
      }
    }"#;

    #[test]
    fn parses_real_download_ledger() {
        let items = parse_downloads(REAL_DOWNLOAD_LEDGER);

        assert_eq!(items.len(), 1);
        assert_eq!(
            items[0].rel_path,
            ".agentmux/attachments/内部款-预警规则-测试用例（后端）.xlsx"
        );
        assert_eq!(items[0].size_bytes, 1_796_166);
    }

    #[test]
    fn tolerates_leading_noise_before_json() {
        let noisy = format!("\u{4e0b}\u{8f7d}\u{4e2d}...\n{}", REAL_DOWNLOAD_LEDGER);
        assert_eq!(parse_downloads(&noisy).len(), 1, "dws 可能先打提示行");
    }

    #[test]
    fn partial_failure_keeps_successful_downloads() {
        let stdout = r#"{"resourceDownloads":{"ok":false,"partial":true,"failedCount":1,
            "failures":[{"resourceId":"x","reason":"boom"}],
            "downloads":[{"localPath":".agentmux/attachments/a.xlsx","sizeBytes":10}]}}"#;
        let items = parse_downloads(stdout);

        assert_eq!(items.len(), 1, "一个失败不该丢掉已成功的那个");
        assert_eq!(items[0].rel_path, ".agentmux/attachments/a.xlsx");
    }

    #[test]
    fn no_downloads_yields_empty() {
        assert!(parse_downloads(r#"{"resourceDownloads":{"downloads":[],"ok":true}}"#).is_empty());
        assert!(parse_downloads("").is_empty());
        assert!(parse_downloads("not json").is_empty());
    }

    #[test]
    fn resolve_within_keeps_paths_inside_work_dir() {
        let work = std::env::temp_dir().join("agentmux-attach-test");
        let got = resolve_within(&work, ".agentmux/attachments/a.xlsx").expect("工作目录内应通过");

        assert!(got.starts_with(&work), "拼出来的路径必须在工作目录下");
        assert!(got.ends_with("a.xlsx"));
    }

    #[test]
    fn resolve_within_rejects_escape() {
        let work = std::env::temp_dir().join("agentmux-attach-test");

        assert!(resolve_within(&work, "../evil.xlsx").is_none(), ".. 必须拒绝");
        assert!(
            resolve_within(&work, ".agentmux/../../evil.xlsx").is_none(),
            "藏在中间的 .. 也必须拒绝"
        );
        assert!(
            resolve_within(&work, r"C:\Windows\evil.xlsx").is_none(),
            "绝对路径必须拒绝"
        );
        assert!(resolve_within(&work, "/etc/passwd").is_none(), "绝对路径必须拒绝");
        assert!(resolve_within(&work, "").is_none(), "空路径必须拒绝");
    }

    /// 真实环境：把被引用消息的附件**真的**下到工作目录。
    ///
    /// 会连钉钉、会写盘，所以默认 `#[ignore]`（与项目里其它 real_* 测试一致）。
    /// 显式跑：`cargo test -- --ignored real_messages_mget_downloads_attachment`
    ///
    /// 这条是接线验证的关键 —— `--output-dir` 是相对路径、靠 dws 的 cwd 解析，
    /// 一旦 cwd 没传对，文件就落到别处，Agent 读不到。
    #[tokio::test]
    #[ignore]
    async fn real_messages_mget_downloads_attachment() {
        let dws = crate::resolve::resolve_executable("dingtalk")
            .await
            .expect("本机应能解析到 dws");
        let work = std::env::temp_dir().join("agentmux-attach-real");
        let _ = std::fs::remove_dir_all(&work);
        std::fs::create_dir_all(&work).expect("建临时工作目录");

        let fetched = download_attachment(
            &dws,
            work.to_str().unwrap(),
            // 实测过的一条文件消息（内部款-预警规则-测试用例（后端）.xlsx）
            "msgFnAefjZ5dKDfpuJZTPPFWg==",
        )
        .await
        .expect("下载调用本身不该失败");

        assert!(!fetched.is_empty(), "应至少下到一个附件，实际 {:?}", fetched);
        let abs = resolve_within(&work, &fetched[0].rel_path).expect("路径应落在工作目录内");
        assert!(
            abs.is_file(),
            "文件必须真的存在于工作目录下: {}",
            abs.display()
        );
        assert!(fetched[0].size_bytes > 0, "应记录文件大小");
    }
}