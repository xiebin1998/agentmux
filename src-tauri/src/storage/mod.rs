use anyhow::Result;
use rusqlite::{params, Connection};
use serde::Serialize;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

use crate::orchestrator::ChatEvent;

pub struct Storage {
    db: Connection,
    archive_dir: PathBuf,
}

fn table_has_column(db: &Connection, table: &str, column: &str) -> bool {
    let Ok(mut stmt) = db.prepare(&format!("PRAGMA table_info({})", table)) else {
        return false;
    };
    let Ok(rows) = stmt.query_map([], |row| row.get::<_, String>(1)) else {
        return false;
    };
    let names: Vec<String> = rows.flatten().collect();
    names.iter().any(|name| name == column)
}

#[derive(Debug, Clone, Serialize)]
pub struct EventRow {
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
    pub processed: bool,
    pub reply_status: Option<String>,
    pub reply_text: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Stats {
    pub total_events: i64,
    pub malformed_events: i64,
    pub processed_events: i64,
    pub replied_events: i64,
    pub failed_replies: i64,
    pub conversations: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConversationSummary {
    pub conversation_id: String,
    pub events: i64,
    pub last_sender: String,
    pub last_received_at: String,
    pub replied: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Summary {
    pub conversation_id: String,
    pub content: String,
    pub source_events: i64,
    pub updated_at: String,
}

#[derive(Debug, Clone, Default)]
pub struct EventQuery {
    pub limit: usize,
    pub offset: usize,
    /// 只看某个项目的事件。
    pub project_id: Option<String>,
    pub conversation_id: Option<String>,
    pub sender: Option<String>,
    pub keyword: Option<String>,
    pub malformed_only: bool,
    pub failed_only: bool,
    /// YYYY-MM-DD，按本地日期前缀比较（A4.2.1）。
    pub since_date: Option<String>,
    pub until_date: Option<String>,
}

impl Storage {
    pub fn new(data_dir: PathBuf) -> Result<Self> {
        fs::create_dir_all(&data_dir)?;
        let db_path = data_dir.join("agentmux.db");
        let db = Connection::open(db_path)?;

        db.execute_batch(
            "CREATE TABLE IF NOT EXISTS events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                message_id TEXT UNIQUE NOT NULL,
                project_id TEXT NOT NULL DEFAULT '',
                conversation_id TEXT NOT NULL,
                sender TEXT NOT NULL,
                sender_open_dingtalk_id TEXT NOT NULL,
                content TEXT NOT NULL,
                create_time TEXT NOT NULL,
                received_at TEXT NOT NULL,
                listen_kind TEXT NOT NULL DEFAULT '',
                malformed INTEGER NOT NULL DEFAULT 0,
                raw TEXT NOT NULL DEFAULT '',
                processed BOOLEAN DEFAULT FALSE,
                reply_status TEXT,
                reply_text TEXT,
                reply_sent_at TEXT
            );

            CREATE INDEX IF NOT EXISTS idx_events_conversation ON events(conversation_id);
            CREATE INDEX IF NOT EXISTS idx_events_received_at ON events(received_at);
            CREATE INDEX IF NOT EXISTS idx_events_processed ON events(processed);

            CREATE TABLE IF NOT EXISTS sessions (
                project_id TEXT NOT NULL DEFAULT '',
                conversation_id TEXT NOT NULL,
                agent_session_id TEXT NOT NULL,
                agent_cwd TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                PRIMARY KEY (project_id, conversation_id)
            );

            CREATE TABLE IF NOT EXISTS settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );

            -- 压缩摘要：v1 每个会话只保留最新一版（D-62）。
            CREATE TABLE IF NOT EXISTS summaries (
                conversation_id TEXT PRIMARY KEY,
                content TEXT NOT NULL,
                source_events INTEGER NOT NULL DEFAULT 0,
                updated_at TEXT NOT NULL
            );",
        )?;

        // 旧库补列（已存在则忽略错误）。
        for stmt in [
            "ALTER TABLE events ADD COLUMN listen_kind TEXT NOT NULL DEFAULT ''",
            "ALTER TABLE events ADD COLUMN malformed INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE events ADD COLUMN raw TEXT NOT NULL DEFAULT ''",
            "ALTER TABLE events ADD COLUMN project_id TEXT NOT NULL DEFAULT ''",
        ] {
            let _ = db.execute(stmt, []);
        }

        // project_id 可能刚刚才补上，索引必须在补列之后建，否则旧库会直接报
        // `no such column: project_id` 而启动失败。
        db.execute(
            "CREATE INDEX IF NOT EXISTS idx_events_project ON events(project_id)",
            [],
        )?;

        let archive_dir = data_dir.join("archive");
        fs::create_dir_all(&archive_dir)?;

        let storage = Self { db, archive_dir };
        storage.migrate_sessions_to_project_scoped();
        Ok(storage)
    }

    /// 旧库的 sessions 是 `conversation_id` 单列唯一，无法按项目隔离。
    /// 检测到旧结构时重建一次（幂等：只在缺 project_id 列时执行）。
    fn migrate_sessions_to_project_scoped(&self) {
        if table_has_column(&self.db, "sessions", "project_id") {
            return;
        }

        let _ = self.db.execute_batch(
            "CREATE TABLE sessions_v2 (
                project_id TEXT NOT NULL DEFAULT '',
                conversation_id TEXT NOT NULL,
                agent_session_id TEXT NOT NULL,
                agent_cwd TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                PRIMARY KEY (project_id, conversation_id)
             );
             INSERT OR IGNORE INTO sessions_v2
                (project_id, conversation_id, agent_session_id, agent_cwd, created_at, updated_at)
                SELECT '', conversation_id, agent_session_id, agent_cwd, created_at, updated_at
                FROM sessions;
             DROP TABLE sessions;
             ALTER TABLE sessions_v2 RENAME TO sessions;",
        );
    }

    pub fn archive_dir(&self) -> &PathBuf {
        &self.archive_dir
    }

    /// 返回 true 表示这是新事件（非重复）。仅在真正插入时才写归档。
    pub fn save_event(&self, event: &ChatEvent) -> Result<bool> {
        let message_id = if event.message_id.is_empty() {
            format!("malformed-{}", uuid::Uuid::new_v4())
        } else {
            event.message_id.clone()
        };

        let inserted = self.db.execute(
            "INSERT OR IGNORE INTO events (
                message_id, project_id, conversation_id, sender, sender_open_dingtalk_id,
                content, create_time, received_at, listen_kind, malformed, raw
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                message_id,
                event.project_id,
                event.conversation_id,
                event.sender,
                event.sender_open_dingtalk_id,
                event.content,
                event.create_time,
                event.received_at,
                event.listen_kind,
                event.malformed as i32,
                event.raw,
            ],
        )?;

        if inserted > 0 {
            self.append_to_archive(event)?;
        }

        Ok(inserted > 0)
    }

    fn append_to_archive(&self, event: &ChatEvent) -> Result<()> {
        let date = chrono::Local::now().format("%Y-%m-%d").to_string();
        let archive_file = self.archive_dir.join(format!("{}.ndjson", date));

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(archive_file)?;

        writeln!(file, "{}", serde_json::to_string(event)?)?;
        Ok(())
    }

    pub fn list_events(&self, query: &EventQuery) -> Result<Vec<EventRow>> {
        let mut sql = String::from(
            "SELECT project_id, message_id, conversation_id, sender, sender_open_dingtalk_id, content,
                    create_time, received_at, listen_kind, malformed, processed, reply_status, reply_text
             FROM events WHERE 1=1",
        );
        let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

        if let Some(project_id) = query.project_id.as_ref().filter(|s| !s.is_empty()) {
            sql.push_str(" AND project_id = ?");
            args.push(Box::new(project_id.clone()));
        }
        if let Some(conversation_id) = query.conversation_id.as_ref().filter(|s| !s.is_empty()) {
            sql.push_str(" AND conversation_id = ?");
            args.push(Box::new(conversation_id.clone()));
        }
        if let Some(sender) = query.sender.as_ref().filter(|s| !s.is_empty()) {
            sql.push_str(" AND sender LIKE ?");
            args.push(Box::new(format!("%{}%", sender)));
        }
        if let Some(keyword) = query.keyword.as_ref().filter(|s| !s.is_empty()) {
            sql.push_str(" AND content LIKE ?");
            args.push(Box::new(format!("%{}%", keyword)));
        }
        if query.malformed_only {
            sql.push_str(" AND malformed = 1");
        }
        if query.failed_only {
            sql.push_str(" AND reply_status = 'failed'");
        }
        if let Some(since) = query.since_date.as_ref().filter(|s| !s.is_empty()) {
            sql.push_str(" AND substr(received_at, 1, 10) >= ?");
            args.push(Box::new(since.clone()));
        }
        if let Some(until) = query.until_date.as_ref().filter(|s| !s.is_empty()) {
            sql.push_str(" AND substr(received_at, 1, 10) <= ?");
            args.push(Box::new(until.clone()));
        }

        sql.push_str(" ORDER BY received_at DESC LIMIT ? OFFSET ?");
        args.push(Box::new(query.limit.max(1) as i64));
        args.push(Box::new(query.offset as i64));

        let mut stmt = self.db.prepare(&sql)?;
        let params: Vec<&dyn rusqlite::ToSql> = args.iter().map(|a| a.as_ref()).collect();
        let rows = stmt.query_map(params.as_slice(), |row| {
            Ok(EventRow {
                project_id: row.get(0)?,
                message_id: row.get(1)?,
                conversation_id: row.get(2)?,
                sender: row.get(3)?,
                sender_open_dingtalk_id: row.get(4)?,
                content: row.get(5)?,
                create_time: row.get(6)?,
                received_at: row.get(7)?,
                listen_kind: row.get(8)?,
                malformed: row.get::<_, i32>(9)? != 0,
                processed: row.get::<_, i32>(10)? != 0,
                reply_status: row.get(11)?,
                reply_text: row.get(12)?,
            })
        })?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| anyhow::anyhow!("Failed to collect events: {}", e))
    }

    /// 会话汇总。
    ///
    /// - `project_id = Some(id)`：只统计该项目的会话（左树「项目下挂会话」）
    /// - `unassigned_only = true`：只统计**没有项目归属**的会话（升级前的历史数据）
    ///
    /// 会过滤掉 `conversation_id` 为空的行：那是缺会话标识的畸形事件，
    /// 在左树里会表现为一个点了没反应的「空会话」。
    pub fn list_conversations(
        &self,
        project_id: Option<&str>,
        unassigned_only: bool,
    ) -> Result<Vec<ConversationSummary>> {
        let scope = if unassigned_only {
            " AND (project_id IS NULL OR project_id = '')"
        } else if project_id.map(|p| !p.is_empty()).unwrap_or(false) {
            " AND project_id = ?1"
        } else {
            ""
        };

        let sql = format!(
            "SELECT conversation_id,
                    COUNT(*) AS events,
                    MAX(received_at) AS last_received_at,
                    SUM(CASE WHEN reply_status = 'sent' THEN 1 ELSE 0 END) AS replied
             FROM events
             WHERE conversation_id <> ''{}
             GROUP BY conversation_id
             ORDER BY last_received_at DESC",
            scope
        );

        let mut stmt = self.db.prepare(&sql)?;

        let map_row = |row: &rusqlite::Row<'_>| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
            ))
        };

        let collected: Vec<(String, i64, String, i64)> =
            if scope.contains("?1") {
                stmt.query_map(params![project_id], map_row)?
                    .collect::<Result<Vec<_>, _>>()?
            } else {
                stmt.query_map([], map_row)?.collect::<Result<Vec<_>, _>>()?
            };

        let mut out = Vec::new();
        for (conversation_id, events, last_received_at, replied) in collected {
            let last_sender: String = self
                .db
                .query_row(
                    "SELECT sender FROM events WHERE conversation_id = ?1 ORDER BY received_at DESC LIMIT 1",
                    params![conversation_id],
                    |row| row.get(0),
                )
                .unwrap_or_default();
            out.push(ConversationSummary {
                conversation_id,
                events,
                last_sender,
                last_received_at,
                replied,
            });
        }

        Ok(out)
    }

    /// 把某个会话（连同它的历史事件）归入项目。
    ///
    /// 用于升级前的历史数据：它们 `project_id` 为空，不属于任何项目，
    /// 在左树里看不到。返回被改写的会话数。
    pub fn assign_conversation(&self, conversation_id: &str, project_id: &str) -> Result<usize> {
        let changed = self.db.execute(
            "UPDATE events SET project_id = ?1
             WHERE conversation_id = ?2 AND (project_id IS NULL OR project_id = '')",
            params![project_id, conversation_id],
        )?;

        // 会话记录同样搬过去（旧记录原本挂在空项目下）。
        let _ = self.db.execute(
            "UPDATE sessions SET project_id = ?1
             WHERE conversation_id = ?2 AND (project_id IS NULL OR project_id = '')",
            params![project_id, conversation_id],
        );

        Ok(changed)
    }

    /// 取某会话最近若干条消息（最新在前），用于拼上下文。
    pub fn recent_messages(
        &self,
        conversation_id: &str,
        limit: usize,
    ) -> Result<Vec<(String, String)>> {
        let mut stmt = self.db.prepare(
            "SELECT sender, content FROM events
             WHERE conversation_id = ?1 AND malformed = 0
             ORDER BY received_at DESC LIMIT ?2",
        )?;

        let rows = stmt.query_map(params![conversation_id, limit as i64], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;

        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// v1 每个会话只保留最新一版摘要（D-62）。
    pub fn get_summary(&self, conversation_id: &str) -> Result<Option<Summary>> {
        let mut stmt = self.db.prepare(
            "SELECT conversation_id, content, source_events, updated_at
             FROM summaries WHERE conversation_id = ?1",
        )?;

        let result = stmt.query_row(params![conversation_id], |row| {
            Ok(Summary {
                conversation_id: row.get(0)?,
                content: row.get(1)?,
                source_events: row.get(2)?,
                updated_at: row.get(3)?,
            })
        });

        match result {
            Ok(summary) => Ok(Some(summary)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn save_summary(
        &self,
        conversation_id: &str,
        content: &str,
        source_events: i64,
    ) -> Result<()> {
        let now = chrono::Local::now().to_rfc3339();
        self.db.execute(
            "INSERT INTO summaries (conversation_id, content, source_events, updated_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(conversation_id) DO UPDATE SET
                content = ?2, source_events = ?3, updated_at = ?4",
            params![conversation_id, content, source_events, now],
        )?;
        Ok(())
    }

    pub fn delete_summary(&self, conversation_id: &str) -> Result<()> {
        self.db.execute(
            "DELETE FROM summaries WHERE conversation_id = ?1",
            params![conversation_id],
        )?;
        Ok(())
    }

    /// 该会话累计事件数，用于判断压缩阈值。
    pub fn count_events(&self, conversation_id: &str) -> Result<i64> {
        Ok(self.db.query_row(
            "SELECT COUNT(*) FROM events WHERE conversation_id = ?1",
            params![conversation_id],
            |row| row.get(0),
        )?)
    }

    pub fn mark_processed(
        &self,
        message_id: &str,
        status: &str,
        reply_text: Option<&str>,
    ) -> Result<()> {
        self.db.execute(
            "UPDATE events SET processed = TRUE, reply_status = ?1, reply_text = ?2, reply_sent_at = ?3
             WHERE message_id = ?4",
            params![
                status,
                reply_text,
                chrono::Local::now().to_rfc3339(),
                message_id,
            ],
        )?;
        Ok(())
    }

    pub fn get_unprocessed_events(&self, limit: usize) -> Result<Vec<EventRow>> {
        self.list_events(&EventQuery {
            limit,
            ..Default::default()
        })
    }

    pub fn save_session(
        &self,
        project_id: &str,
        conversation_id: &str,
        agent_session_id: &str,
        agent_cwd: &str,
    ) -> Result<()> {
        let now = chrono::Local::now().to_rfc3339();
        self.db.execute(
            "INSERT INTO sessions (project_id, conversation_id, agent_session_id, agent_cwd, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(project_id, conversation_id) DO UPDATE SET
                agent_session_id = ?3,
                agent_cwd = ?4,
                updated_at = ?6",
            params![project_id, conversation_id, agent_session_id, agent_cwd, now, now],
        )?;
        Ok(())
    }

    pub fn get_session(
        &self,
        project_id: &str,
        conversation_id: &str,
    ) -> Result<Option<(String, String)>> {
        let mut stmt = self.db.prepare(
            "SELECT agent_session_id, agent_cwd FROM sessions
             WHERE project_id = ?1 AND conversation_id = ?2",
        )?;

        let result = stmt.query_row(params![project_id, conversation_id], |row| {
            Ok((row.get(0)?, row.get(1)?))
        });

        match result {
            Ok(session) => Ok(Some(session)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn delete_session(&self, project_id: &str, conversation_id: &str) -> Result<()> {
        self.db.execute(
            "DELETE FROM sessions WHERE project_id = ?1 AND conversation_id = ?2",
            params![project_id, conversation_id],
        )?;
        Ok(())
    }

    pub fn get_setting(&self, key: &str) -> Result<Option<String>> {
        let mut stmt = self.db.prepare("SELECT value FROM settings WHERE key = ?1")?;
        let result = stmt.query_row(params![key], |row| row.get(0));

        match result {
            Ok(value) => Ok(Some(value)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.db.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = ?2",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn get_stats(&self) -> Result<Stats> {
        let count = |sql: &str| -> Result<i64> {
            Ok(self.db.query_row(sql, [], |row| row.get(0))?)
        };

        Ok(Stats {
            total_events: count("SELECT COUNT(*) FROM events")?,
            malformed_events: count("SELECT COUNT(*) FROM events WHERE malformed = 1")?,
            processed_events: count("SELECT COUNT(*) FROM events WHERE processed = TRUE")?,
            replied_events: count("SELECT COUNT(*) FROM events WHERE reply_status = 'sent'")?,
            failed_replies: count("SELECT COUNT(*) FROM events WHERE reply_status = 'failed'")?,
            conversations: count("SELECT COUNT(DISTINCT conversation_id) FROM events")?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 造一个**旧结构**的库：events 缺 project_id，sessions 是旧的 conversation_id 单列唯一。
    fn write_legacy_database(dir: &std::path::Path) {
        let db = Connection::open(dir.join("agentmux.db")).unwrap();
        db.execute_batch(
            "CREATE TABLE events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                message_id TEXT UNIQUE NOT NULL,
                conversation_id TEXT NOT NULL,
                sender TEXT NOT NULL,
                sender_open_dingtalk_id TEXT NOT NULL,
                content TEXT NOT NULL,
                create_time TEXT NOT NULL,
                received_at TEXT NOT NULL,
                listen_kind TEXT NOT NULL DEFAULT '',
                malformed INTEGER NOT NULL DEFAULT 0,
                raw TEXT NOT NULL DEFAULT '',
                processed BOOLEAN DEFAULT FALSE,
                reply_status TEXT, reply_text TEXT, reply_sent_at TEXT
             );

             INSERT INTO events
                (message_id, conversation_id, sender, sender_open_dingtalk_id, content, create_time, received_at)
             VALUES
                ('msg-1', 'cid-1', '甲', 'open-1', '老数据', '2026-09-18 10:00:00', '2026-09-18T10:00:00+08:00');

             CREATE TABLE sessions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                conversation_id TEXT UNIQUE NOT NULL,
                agent_session_id TEXT NOT NULL,
                agent_cwd TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
             );

             INSERT INTO sessions
                (conversation_id, agent_session_id, agent_cwd, created_at, updated_at)
             VALUES
                ('cid-1', 'sess-1', 'D:\\old-cwd', '2026-09-18T10:00:00+08:00', '2026-09-18T10:00:00+08:00');",
        )
        .unwrap();
    }

    /// 回归：旧库必须能正常打开并完成迁移。
    ///
    /// 曾经因为「先建 project_id 索引、后补列」，旧库启动时直接 panic：
    /// `no such column: project_id`。这个 bug 只有真正启动应用才会暴露。
    #[test]
    fn migrating_a_legacy_database_does_not_panic() {
        let dir = std::env::temp_dir().join("agentmux-legacy-schema-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        write_legacy_database(&dir);

        let storage = Storage::new(dir.clone()).expect("旧库应能迁移成功，而不是 panic");

        // 旧事件仍在，project_id 补成空串
        let rows = storage
            .list_events(&EventQuery {
                limit: 10,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(rows.len(), 1, "迁移不应丢数据");
        assert_eq!(rows[0].message_id, "msg-1");
        assert_eq!(rows[0].project_id, "", "旧事件迁移后项目应为空串");

        // 旧会话被搬进新结构，且可按 (项目, 会话) 读回
        let session = storage.get_session("", "cid-1").unwrap();
        assert!(session.is_some(), "旧会话应保留");
        assert_eq!(session.unwrap().0, "sess-1");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 项目维度可写入可读回；两个项目对同一会话各自独立。
    #[test]
    fn sessions_are_isolated_per_project() {
        let dir = std::env::temp_dir().join("agentmux-session-scope-test");
        let _ = std::fs::remove_dir_all(&dir);
        let storage = Storage::new(dir.clone()).unwrap();

        storage.save_session("p1", "cid-1", "sess-a", "D:\\a").unwrap();
        storage.save_session("p2", "cid-1", "sess-b", "D:\\b").unwrap();

        assert_eq!(storage.get_session("p1", "cid-1").unwrap().unwrap().0, "sess-a");
        assert_eq!(storage.get_session("p2", "cid-1").unwrap().unwrap().0, "sess-b");

        storage.delete_session("p1", "cid-1").unwrap();
        assert!(storage.get_session("p1", "cid-1").unwrap().is_none());
        assert!(
            storage.get_session("p2", "cid-1").unwrap().is_some(),
            "作废一个项目的会话不应影响另一个项目"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    fn sample_event(message_id: &str, conversation_id: &str, project_id: &str) -> ChatEvent {
        ChatEvent {
            project_id: project_id.to_string(),
            message_id: message_id.to_string(),
            conversation_id: conversation_id.to_string(),
            sender: "同事".to_string(),
            sender_open_dingtalk_id: "open-other".to_string(),
            content: "@我 看一下".to_string(),
            create_time: "2026-09-19T10:00:00+08:00".to_string(),
            received_at: "2026-09-19T10:00:01+08:00".to_string(),
            listen_kind: "at_me".to_string(),
            malformed: false,
            raw: "{}".to_string(),
        }
    }

    fn conversation_ids(rows: &[ConversationSummary]) -> Vec<String> {
        rows.iter().map(|row| row.conversation_id.clone()).collect()
    }

    /// 回归（问题 5）：升级前的历史会话 `project_id` 为空，既不归属任何项目，
    /// 又要能被看到并「归入」某个项目 —— 否则用户会觉得「我之前的会话不见了」。
    #[test]
    fn unassigned_conversations_are_visible_and_can_be_assigned() {
        let dir = std::env::temp_dir().join("agentmux-assign-conversation-test");
        let _ = std::fs::remove_dir_all(&dir);
        let storage = Storage::new(dir.clone()).unwrap();

        // 一条升级前的遗留会话（project_id 为空）+ 一条已归属 p1 的会话。
        storage
            .save_event(&sample_event("msg-old", "cid-old", ""))
            .unwrap();
        storage
            .save_event(&sample_event("msg-new", "cid-new", "p1"))
            .unwrap();

        // 未归类视图能看到遗留会话，且它不出现在任何项目下。
        let unassigned = storage.list_conversations(None, true).unwrap();
        assert_eq!(
            conversation_ids(&unassigned),
            vec!["cid-old".to_string()],
            "遗留会话应出现在「未归类」里"
        );
        assert_eq!(
            conversation_ids(&storage.list_conversations(Some("p1"), false).unwrap()),
            vec!["cid-new".to_string()],
            "未归属的会话不应混进项目列表"
        );

        // 归入 p1 之后，它从「未归类」消失、出现在 p1 下。
        let changed = storage.assign_conversation("cid-old", "p1").unwrap();
        assert_eq!(changed, 1, "应归类 1 条事件");

        assert!(
            storage.list_conversations(None, true).unwrap().is_empty(),
            "归类后不应再留在「未归类」里"
        );
        let mut p1 = conversation_ids(&storage.list_conversations(Some("p1"), false).unwrap());
        p1.sort();
        assert_eq!(p1, vec!["cid-new".to_string(), "cid-old".to_string()]);

        // 已有归属的会话不会被后来的归类改动（避免覆盖用户的选择）。
        assert_eq!(
            storage.assign_conversation("cid-new", "p2").unwrap(),
            0,
            "已归属会话不应被抢走"
        );
        let mut after = conversation_ids(&storage.list_conversations(Some("p1"), false).unwrap());
        after.sort();
        assert_eq!(after, vec!["cid-new".to_string(), "cid-old".to_string()]);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
