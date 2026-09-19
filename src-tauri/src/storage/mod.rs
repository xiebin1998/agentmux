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

#[derive(Debug, Clone, Serialize)]
pub struct EventRow {
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
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                conversation_id TEXT UNIQUE NOT NULL,
                agent_session_id TEXT NOT NULL,
                agent_cwd TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
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
        ] {
            let _ = db.execute(stmt, []);
        }

        let archive_dir = data_dir.join("archive");
        fs::create_dir_all(&archive_dir)?;

        Ok(Self { db, archive_dir })
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
                message_id, conversation_id, sender, sender_open_dingtalk_id,
                content, create_time, received_at, listen_kind, malformed, raw
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                message_id,
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
            "SELECT message_id, conversation_id, sender, sender_open_dingtalk_id, content,
                    create_time, received_at, listen_kind, malformed, processed, reply_status, reply_text
             FROM events WHERE 1=1",
        );
        let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

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
                message_id: row.get(0)?,
                conversation_id: row.get(1)?,
                sender: row.get(2)?,
                sender_open_dingtalk_id: row.get(3)?,
                content: row.get(4)?,
                create_time: row.get(5)?,
                received_at: row.get(6)?,
                listen_kind: row.get(7)?,
                malformed: row.get::<_, i32>(8)? != 0,
                processed: row.get::<_, i32>(9)? != 0,
                reply_status: row.get(10)?,
                reply_text: row.get(11)?,
            })
        })?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| anyhow::anyhow!("Failed to collect events: {}", e))
    }

    pub fn list_conversations(&self) -> Result<Vec<ConversationSummary>> {
        let mut stmt = self.db.prepare(
            "SELECT conversation_id,
                    COUNT(*) AS events,
                    MAX(received_at) AS last_received_at,
                    SUM(CASE WHEN reply_status = 'sent' THEN 1 ELSE 0 END) AS replied
             FROM events
             GROUP BY conversation_id
             ORDER BY last_received_at DESC",
        )?;

        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })?;

        let mut out = Vec::new();
        for row in rows {
            let (conversation_id, events, last_received_at, replied) = row?;
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
        conversation_id: &str,
        agent_session_id: &str,
        agent_cwd: &str,
    ) -> Result<()> {
        let now = chrono::Local::now().to_rfc3339();
        self.db.execute(
            "INSERT INTO sessions (conversation_id, agent_session_id, agent_cwd, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(conversation_id) DO UPDATE SET
                agent_session_id = ?2,
                agent_cwd = ?3,
                updated_at = ?5",
            params![conversation_id, agent_session_id, agent_cwd, now, now],
        )?;
        Ok(())
    }

    pub fn get_session(&self, conversation_id: &str) -> Result<Option<(String, String)>> {
        let mut stmt = self
            .db
            .prepare("SELECT agent_session_id, agent_cwd FROM sessions WHERE conversation_id = ?1")?;

        let result = stmt.query_row(params![conversation_id], |row| Ok((row.get(0)?, row.get(1)?)));

        match result {
            Ok(session) => Ok(Some(session)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn delete_session(&self, conversation_id: &str) -> Result<()> {
        self.db.execute(
            "DELETE FROM sessions WHERE conversation_id = ?1",
            params![conversation_id],
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
