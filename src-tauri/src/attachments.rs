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

/// 图片前缀。单独提出来是因为图片要**区别对待**：不做转换，且模型可能看不见。
const IMAGE_PREFIX: &str = "[图片]";

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
    /// 是不是图片。图片不做转换，但**模型未必有视觉能力**，提示词里要区别对待。
    pub is_image: bool,
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

    let (is_file, is_image, file_name) = match ATTACHMENT_PREFIXES
        .iter()
        .find_map(|prefix| text.strip_prefix(prefix).map(|rest| (*prefix, rest)))
    {
        Some((prefix, rest)) => {
            let name = rest.trim();
            (
                true,
                prefix == IMAGE_PREFIX,
                if name.is_empty() {
                    None
                } else {
                    Some(name.to_string())
                },
            )
        }
        None => (false, false, None),
    };

    Some(QuotedRef {
        quoted_message_id,
        text,
        is_file,
        file_name,
        is_image,
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

/// 附件大小上限。超过就只告知名字、不给路径 —— 免得 Agent 去读一个巨大文件把
/// 上下文撑爆，转换它也没有意义。
pub const MAX_ATTACHMENT_BYTES: u64 = 8 * 1024 * 1024;

/// 正文里是否**直接**带了媒体（不是引用）。返回 `Some(is_image)`；没有则 `None`。
///
/// 这类消息的资源同样能取回：**对消息自身**调 `+messages-mget --download-resources`
/// 即可，dws 会自己解析 mediaId —— 不用我们解析（实测落盘过 111KB 的真 PNG）。
pub fn inline_media_kind(content: &str) -> Option<bool> {
    if !content.contains("mediaId=") && !content.contains("fileId=") {
        return None;
    }
    Some(content.contains("[图片消息]"))
}

/// 写进提示词的一条附件说明。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AttachmentNote {
    /// 对方发来时叫什么。
    pub name: String,
    /// 工作目录内的相对路径；`None` = 取不到可读内容，原因见 `reason`。
    pub rel_path: Option<String>,
    /// 是不是图片。图片要单独措辞：模型未必具备视觉能力。
    pub is_image: bool,
    /// 路径是不是应用侧转换出来的（xlsx → CSV）。
    pub converted: bool,
    /// `rel_path` 为 `None` 时的原因，会写进提示词让 Agent 如实说明。
    pub reason: Option<String>,
}

/// `Read` 能直接读的文本类扩展名。
const TEXT_EXTENSIONS: &[&str] = &[
    "txt", "md", "markdown", "csv", "tsv", "json", "xml", "yaml", "yml", "log", "ini", "toml",
    "html", "htm", "pdf",
];

/// 图片扩展名。作为 quoted 前缀之外的兜底判断。
const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "bmp"];

fn name_of(rel_path: &str) -> String {
    rel_path
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(rel_path)
        .to_string()
}

fn extension_of(name: &str) -> String {
    name.rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .unwrap_or_default()
}

/// 把下载好的附件变成提示词能用的说明。
///
/// 图片与文本类直接给路径（`Read` 能读）；xlsx 由应用侧转成 CSV、给转换后的路径；
/// 其余二进制与超大文件只给名字加原因 —— 让 Agent 如实说"这个我读不了"，
/// 而不是对着一个它打不开的文件编内容。
pub fn to_notes(work_dir: &Path, fetched: &[Fetched], image_hint: bool) -> Vec<AttachmentNote> {
    fetched
        .iter()
        .map(|item| note_for(work_dir, item, image_hint))
        .collect()
}

fn note_for(work_dir: &Path, item: &Fetched, image_hint: bool) -> AttachmentNote {
    let name = name_of(&item.rel_path);
    let extension = extension_of(&name);
    let is_image = image_hint || IMAGE_EXTENSIONS.contains(&extension.as_str());

    let skipped = |reason: &str| AttachmentNote {
        name: name.clone(),
        rel_path: None,
        is_image,
        converted: false,
        reason: Some(reason.to_string()),
    };

    if item.size_bytes > MAX_ATTACHMENT_BYTES {
        return skipped("文件超过 8MB，未读取内容");
    }

    if is_image || TEXT_EXTENSIONS.contains(&extension.as_str()) {
        return AttachmentNote {
            name,
            rel_path: Some(item.rel_path.clone()),
            is_image,
            converted: false,
            reason: None,
        };
    }

    // xlsx 是二进制，Read 读不了；转成 CSV 再给路径。
    if extension == "xlsx" || extension == "xlsm" {
        let Some(absolute) = resolve_within(work_dir, &item.rel_path) else {
            return skipped("附件路径不合法，未读取");
        };
        let Some(csv) = std::fs::read(&absolute)
            .ok()
            .and_then(|bytes| crate::ooxml::xlsx_to_csv(&bytes).ok())
        else {
            return skipped("表格转换失败，未读取内容");
        };
        let csv_rel = format!("{}.csv", item.rel_path);
        let Some(csv_absolute) = resolve_within(work_dir, &csv_rel) else {
            return skipped("附件路径不合法，未读取");
        };
        if std::fs::write(&csv_absolute, csv).is_err() {
            return skipped("表格转换结果写盘失败，未读取内容");
        }
        return AttachmentNote {
            name,
            rel_path: Some(csv_rel),
            is_image: false,
            converted: true,
            reason: None,
        };
    }

    skipped("该格式无法转成文本，未读取内容")
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
        assert!(!q.is_image, "文件不是图片");
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
        assert!(q.is_image, "前缀是 [图片] 就应标记为图片");
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

    /// 正文里**直接发的**媒体（不是引用）—— 实测载荷形如
    /// `[图片消息](mediaId=@lQLPK…)这个新增的`。
    #[test]
    fn detects_inline_media_in_message_content() {
        assert_eq!(
            inline_media_kind(
                "[图片消息](mediaId=@lQLPKdVNvSKACLvNAxrNAjCw-eE_t9KKPG0KgPbB08mCAA)这个新增的，\"15%\" 用哪一个字段？"
            ),
            Some(true),
            "内联图片应被识别，且标记为图片"
        );
        assert_eq!(inline_media_kind("[文件消息](fileId=abc)看下"), Some(false));
        assert_eq!(inline_media_kind("普通文本，没有媒体"), None);
        assert_eq!(inline_media_kind(""), None);
    }

    /// 现场造一个最小 xlsx（只有一张表、无共享字符串），用于 to_notes 的转换测试。
    fn write_min_xlsx(path: &std::path::Path, rows: &str) {
        use std::io::Write;
        let mut buffer = std::io::Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut buffer);
            let options = zip::write::SimpleFileOptions::default();
            let parts = [
                (
                    "xl/workbook.xml",
                    r#"<workbook xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="S" sheetId="1" r:id="rId1"/></sheets></workbook>"#,
                ),
                (
                    "xl/_rels/workbook.xml.rels",
                    r#"<Relationships><Relationship Id="rId1" Type="worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#,
                ),
                (
                    "xl/worksheets/sheet1.xml",
                    &format!(r#"<worksheet><sheetData>{rows}</sheetData></worksheet>"#),
                ),
            ];
            for (name, body) in parts {
                writer.start_file(name, options).unwrap();
                writer.write_all(body.as_bytes()).unwrap();
            }
            writer.finish().unwrap();
        }
        std::fs::write(path, buffer.into_inner()).unwrap();
    }

    #[test]
    fn note_gives_path_for_image_and_text_unchanged() {
        let work = std::env::temp_dir().join("agentmux-note-test");
        let fetched = vec![
            Fetched {
                rel_path: ".agentmux/attachments/截图.png".into(),
                size_bytes: 1024,
            },
            Fetched {
                rel_path: ".agentmux/attachments/说明.md".into(),
                size_bytes: 2048,
            },
        ];

        let notes = to_notes(&work, &fetched, false);

        assert_eq!(notes.len(), 2);
        assert!(notes[0].is_image, "png 应识别为图片");
        assert_eq!(
            notes[0].rel_path.as_deref(),
            Some(".agentmux/attachments/截图.png"),
            "图片原样给路径，不做转换"
        );
        assert!(!notes[0].converted);
        assert!(!notes[1].is_image);
        assert_eq!(notes[1].rel_path.as_deref(), Some(".agentmux/attachments/说明.md"));
    }

    #[test]
    fn quoted_image_hint_marks_note_as_image() {
        // 前缀是 [图片] 但扩展名认不出来时，也要当图片
        let work = std::env::temp_dir().join("agentmux-note-test");
        let fetched = vec![Fetched {
            rel_path: ".agentmux/attachments/未知资源".into(),
            size_bytes: 10,
        }];

        let notes = to_notes(&work, &fetched, true);

        assert!(notes[0].is_image);
    }

    #[test]
    fn note_converts_xlsx_to_csv_and_points_at_the_csv() {
        let work = std::env::temp_dir().join("agentmux-note-xlsx-test");
        let _ = std::fs::remove_dir_all(&work);
        let dir = work.join(".agentmux/attachments");
        std::fs::create_dir_all(&dir).unwrap();
        write_min_xlsx(
            &dir.join("用例.xlsx"),
            r#"<row r="1"><c r="A1" t="inlineStr"><is><t>编号</t></is></c><c r="B1" t="inlineStr"><is><t>结论</t></is></c></row>
               <row r="2"><c r="A2" t="inlineStr"><is><t>TC-001</t></is></c><c r="B2"><v>1</v></c></row>"#,
        );

        let fetched = vec![Fetched {
            rel_path: ".agentmux/attachments/用例.xlsx".into(),
            size_bytes: 4096,
        }];
        let notes = to_notes(&work, &fetched, false);

        assert!(notes[0].converted, "xlsx 应由应用侧转成 CSV");
        assert_eq!(
            notes[0].rel_path.as_deref(),
            Some(".agentmux/attachments/用例.xlsx.csv"),
            "提示词里给的应是转换后的路径"
        );
        assert_eq!(notes[0].name, "用例.xlsx", "名字仍是对方发来的原名");

        let csv = std::fs::read_to_string(dir.join("用例.xlsx.csv")).expect("CSV 应落盘");
        assert_eq!(csv, "编号,结论\nTC-001,1");

        let _ = std::fs::remove_dir_all(&work);
    }

    #[test]
    fn oversized_and_unconvertible_attachments_get_no_path() {
        let work = std::env::temp_dir().join("agentmux-note-test");
        let fetched = vec![
            Fetched {
                rel_path: ".agentmux/attachments/巨大.xlsx".into(),
                size_bytes: MAX_ATTACHMENT_BYTES + 1,
            },
            Fetched {
                rel_path: ".agentmux/attachments/文档.docx".into(),
                size_bytes: 1024,
            },
        ];

        let notes = to_notes(&work, &fetched, false);

        assert!(notes[0].rel_path.is_none(), "超限不给路径");
        assert!(notes[0].reason.as_deref().unwrap().contains("8MB"));
        assert!(notes[1].rel_path.is_none(), "docx 转不了，不给路径");
        assert!(
            notes[1].reason.is_some(),
            "必须给原因，好让 Agent 如实说明读不了"
        );
        // 没有路径也要保留名字：Agent 至少能说清"哪个文件我读不了"
        assert_eq!(notes[1].name, "文档.docx");
    }

    /// 真实环境：正文里**直接发的图片**也能取回（不是引用）。
    ///
    /// 用实测过的一条图片消息（2026-09-20 的 id=45，对方发的截图）。默认 `#[ignore]`。
    ///
    /// **会连钉钉，可能瞬时失败**：实测出现过一次 1.16s 的快速失败（此前刚连续打过
    /// 几次 mget，疑似服务端限流/瞬时错误），随后连跑 5 次都通过。失败时重跑一次即可，
    /// 不要据此判定功能坏了。
    #[tokio::test]
    #[ignore]
    async fn real_inline_image_downloads() {
        let dws = crate::resolve::resolve_executable("dingtalk")
            .await
            .expect("本机应能解析到 dws");
        let work = std::env::temp_dir().join("agentmux-inline-real");
        let _ = std::fs::remove_dir_all(&work);
        std::fs::create_dir_all(&work).expect("建临时工作目录");

        let fetched = download_attachment(&dws, work.to_str().unwrap(), "msg5CpqfQItPEd0VaZcWbchXQ==")
            .await
            .expect("下载调用本身不该失败");

        assert!(!fetched.is_empty(), "应取回那张图，实际 {fetched:?}");
        let abs = resolve_within(&work, &fetched[0].rel_path).expect("路径应在工作目录内");
        assert!(abs.is_file(), "图片必须真的落盘: {}", abs.display());
        assert!(
            fetched[0].rel_path.to_ascii_lowercase().ends_with(".png"),
            "应是 PNG，实际 {}",
            fetched[0].rel_path
        );
        assert!(fetched[0].size_bytes > 0);
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