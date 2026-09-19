//! 导入旧版 `dingtalk-event-host` 的数据（A9.2.2）。
//!
//! 只导**事件 / 回复台账 / 会话**三类，不导入旧版日志（D-78）。
//! 旧版 `received_at` 是 UTC（`...Z`），这里统一转成本地时间，
//! 否则按本地日期前缀做的检索会跨时区错位。

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::orchestrator::ChatEvent;
use crate::storage::Storage;

#[derive(Debug, Default, Serialize)]
pub struct ImportReport {
    pub events_imported: u32,
    pub events_duplicated: u32,
    pub bad_lines: u32,
    pub replies_sent: u32,
    pub replies_skipped: u32,
    pub replies_unmatched: u32,
    pub sessions_imported: u32,
    pub errors: Vec<String>,
}

#[derive(Deserialize)]
struct LegacyReply {
    message_id: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    reply: Option<String>,
}

#[derive(Deserialize)]
struct LegacySession {
    #[serde(rename = "sessionId")]
    session_id: String,
    #[serde(default)]
    cwd: String,
}

fn to_local_rfc3339(raw: &str) -> String {
    match chrono::DateTime::parse_from_rfc3339(raw) {
        Ok(parsed) => parsed.with_timezone(&chrono::Local).to_rfc3339(),
        Err(_) => raw.to_string(),
    }
}

fn listen_kind_from_type(event_type: &str) -> &'static str {
    if event_type.contains("_at") {
        "at-me"
    } else if event_type.contains("o2o") {
        "all-direct"
    } else {
        "unknown"
    }
}

fn record_files(root: &Path) -> Vec<PathBuf> {
    let dir = root.join("records");
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };

    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().map(|e| e == "ndjson").unwrap_or(false))
        .collect();
    files.sort();
    files
}

fn line_to_event(line: &str) -> Option<ChatEvent> {
    let raw: serde_json::Value = serde_json::from_str(line).ok()?;
    if !raw.is_object() {
        return None;
    }

    let get = |key: &str| -> String {
        raw.get(key)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string()
    };

    let message_id = get("message_id");
    let conversation_id = get("conversation_id");
    let received_at = raw
        .get("received_at")
        .and_then(|v| v.as_str())
        .map(to_local_rfc3339)
        .unwrap_or_else(|| chrono::Local::now().to_rfc3339());

    Some(ChatEvent {
        malformed: message_id.is_empty() || conversation_id.is_empty(),
        message_id,
        conversation_id,
        sender: get("sender"),
        sender_open_dingtalk_id: get("sender_open_dingtalk_id"),
        content: get("content"),
        create_time: get("create_time"),
        received_at,
        listen_kind: listen_kind_from_type(&get("type")).to_string(),
        raw: line.to_string(),
    })
}

fn import_events(storage: &Storage, root: &Path, report: &mut ImportReport) {
    for file in record_files(root) {
        let Ok(content) = std::fs::read_to_string(&file) else {
            report
                .errors
                .push(format!("读取失败: {}", file.to_string_lossy()));
            continue;
        };

        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            match line_to_event(line) {
                Some(event) => match storage.save_event(&event) {
                    Ok(true) => report.events_imported += 1,
                    Ok(false) => report.events_duplicated += 1,
                    Err(err) => report.errors.push(format!("写入事件失败: {}", err)),
                },
                None => report.bad_lines += 1,
            }
        }
    }
}

fn import_replies(storage: &Storage, root: &Path, report: &mut ImportReport) {
    let path = root.join("replies.ndjson");
    let Ok(content) = std::fs::read_to_string(&path) else {
        return;
    };

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let entry: LegacyReply = match serde_json::from_str(line) {
            Ok(entry) => entry,
            Err(_) => {
                report.bad_lines += 1;
                continue;
            }
        };

        // 旧版把「自己发的」记成 self-sent；新版语义是 skipped。
        let (status, text) = match entry.status.as_str() {
            "sent" => ("sent", entry.reply.as_deref()),
            _ => ("skipped", None),
        };

        match storage.mark_processed(&entry.message_id, status, text) {
            Ok(()) => {
                if status == "sent" {
                    report.replies_sent += 1;
                } else {
                    report.replies_skipped += 1;
                }
            }
            Err(err) => report.errors.push(format!("写回复台账失败: {}", err)),
        }
    }
}

fn import_sessions(storage: &Storage, root: &Path, report: &mut ImportReport) {
    let path = root.join("sessions.json");
    let Ok(content) = std::fs::read_to_string(&path) else {
        return;
    };

    let parsed: serde_json::Value = match serde_json::from_str(&content) {
        Ok(parsed) => parsed,
        Err(err) => {
            report.errors.push(format!("sessions.json 解析失败: {}", err));
            return;
        }
    };

    let Some(map) = parsed.as_object() else {
        report.errors.push("sessions.json 顶层不是对象".to_string());
        return;
    };

    for (conversation_id, value) in map {
        match serde_json::from_value::<LegacySession>(value.clone()) {
            Ok(session) => match storage.save_session(
                conversation_id,
                &session.session_id,
                &session.cwd,
            ) {
                Ok(()) => report.sessions_imported += 1,
                Err(err) => report.errors.push(format!("写会话失败: {}", err)),
            },
            Err(err) => report
                .errors
                .push(format!("会话 {} 结构不符: {}", conversation_id, err)),
        }
    }
}

/// 从旧版数据目录导入。path 指向旧工程的 `data` 目录。
#[tauri::command]
pub async fn import_legacy(
    state: tauri::State<'_, crate::AppState>,
    path: String,
) -> Result<ImportReport, String> {
    let root = PathBuf::from(path.trim());
    if !root.is_dir() {
        return Err(format!("目录不存在: {}", root.to_string_lossy()));
    }

    let storage = state.storage.lock().await;
    let mut report = ImportReport::default();

    import_events(&storage, &root, &mut report);
    import_replies(&storage, &root, &mut report);
    import_sessions(&storage, &root, &mut report);

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_timestamps_are_converted_to_local() {
        let converted = to_local_rfc3339("2026-09-18T07:48:21.253Z");
        assert_ne!(converted, "2026-09-18T07:48:21.253Z", "应转换成带本地偏移的时间");
        assert!(
            !converted.ends_with('Z'),
            "转换后不应仍是 UTC 字面量，否则按本地日期检索会错位"
        );
    }

    #[test]
    fn listen_kind_is_derived_from_legacy_event_type() {
        assert_eq!(
            listen_kind_from_type("user_im_message_receive_at"),
            "at-me"
        );
        assert_eq!(
            listen_kind_from_type("user_im_message_receive_o2o_all"),
            "all-direct"
        );
        assert_eq!(listen_kind_from_type("something_else"), "unknown");
    }

    #[test]
    fn legacy_record_line_maps_into_an_event() {
        let line = r#"{"received_at":"2026-09-18T07:48:21.253Z","type":"user_im_message_receive_at","message_id":"m1","conversation_id":"c1","sender":"甲","sender_open_dingtalk_id":"open1","content":"@我 你好","create_time":"2026-09-18 15:48:20"}"#;
        let event = line_to_event(line).expect("应能解析");
        assert_eq!(event.message_id, "m1");
        assert_eq!(event.listen_kind, "at-me");
        assert!(!event.malformed);
        assert_eq!(event.raw, line);
    }

    #[test]
    fn line_without_ids_is_flagged_malformed_not_dropped() {
        let line = r#"{"received_at":"2026-09-18T07:48:21.253Z","type":"x","content":"无 id"}"#;
        let event = line_to_event(line).expect("畸形事件也应保留");
        assert!(event.malformed);
    }

    /// 用真实的旧版数据跑一遍导入 + 去重。旧版目录不在时自动跳过。
    #[test]
    fn imports_real_legacy_data_and_dedupes_on_second_run() {
        let legacy = Path::new(r"D:\workSpase\idea\dingtalk-event-host\data");
        if !legacy.is_dir() {
            eprintln!("跳过：环境里没有旧版数据目录");
            return;
        }

        let dir = std::env::temp_dir().join("agentmux-legacy-import-test");
        let _ = std::fs::remove_dir_all(&dir);
        let storage = Storage::new(dir.clone()).expect("临时库应能创建");

        let mut first = ImportReport::default();
        import_events(&storage, legacy, &mut first);
        import_replies(&storage, legacy, &mut first);
        import_sessions(&storage, legacy, &mut first);

        assert!(first.events_imported > 0, "应导入事件，实际 {:?}", first);
        assert_eq!(first.bad_lines, 0, "不应有坏行，实际 {:?}", first);
        assert!(
            first.replies_sent + first.replies_skipped > 0,
            "应导入回复台账，实际 {:?}",
            first
        );
        assert!(first.sessions_imported > 0, "应导入会话，实际 {:?}", first);

        // 再导一次：事件应全部判重，不重复入库。
        let mut second = ImportReport::default();
        import_events(&storage, legacy, &mut second);
        assert_eq!(second.events_imported, 0, "二次导入不应新增事件");
        assert!(second.events_duplicated > 0, "二次导入应识别为重复");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
