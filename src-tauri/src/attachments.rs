//! 被引用消息（引用回复）的解析与附件获取。
//!
//! 钉钉的事件载荷里带 `quoted_message`：对方引用某条消息再说话时，被引用的
//! 内容就在里面。正文只有一句「你看下这个」，真正要看的东西在引用里 —— 不取
//! 出来，Agent 就只能回一句「好的，我看看」。

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
}