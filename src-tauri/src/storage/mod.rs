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
    /// 会话名：群聊是群名、单聊是对方用户名。事件流里没有，靠 dws 会话列表补。
    pub name: String,
    /// group / direct / unknown，供界面打「群聊·单聊」标签。
    pub kind: String,
}

/// dws 会话列表查出的一条会话元信息。
#[derive(Debug, Clone)]
pub struct ConversationMeta {
    pub conversation_id: String,
    pub name: String,
    pub kind: String,
    pub name_known: bool,
}

/// 「指定群 / 指定人」的候选项：id 是会话 id 或 open id，其余字段只用于显示。
#[derive(Debug, Clone, Serialize)]
pub struct SourceCandidates {
    pub groups: Vec<crate::project::ScopeEntry>,
    pub people: Vec<crate::project::ScopeEntry>,
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
                last_context_ratio REAL,
                last_model TEXT,
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
            );

            -- 会话元信息：事件流里只有 conversation_id，没有名字也没有群/单聊标记，
            -- 靠 dws 的会话列表补上（详见 resolve 里的拉取逻辑）。
            CREATE TABLE IF NOT EXISTS conversations (
                conversation_id TEXT PRIMARY KEY,
                name TEXT NOT NULL DEFAULT '',
                kind TEXT NOT NULL DEFAULT 'unknown',
                name_known INTEGER NOT NULL DEFAULT 0,
                updated_at TEXT NOT NULL
            );",
        )?;

        // 旧库补列（已存在则忽略错误）。
        for stmt in [
            "ALTER TABLE events ADD COLUMN listen_kind TEXT NOT NULL DEFAULT ''",
            "ALTER TABLE events ADD COLUMN malformed INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE events ADD COLUMN raw TEXT NOT NULL DEFAULT ''",
            "ALTER TABLE events ADD COLUMN project_id TEXT NOT NULL DEFAULT ''",
            // 「删掉的会话」墓碑：记删除时的最大事件序号。之后又来了新消息
            // （序号更大）会话会自己回来，见 list_conversations。
            "ALTER TABLE conversations ADD COLUMN deleted_seq INTEGER",
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

        // 这两列**必须**在重建之后补：旧库迁移是把 sessions 整表重建的（显式列清单），
        // 放在迁移前补会被无声丢掉，表现是面板读占比时报 no such column。
        for stmt in [
            "ALTER TABLE sessions ADD COLUMN last_context_ratio REAL",
            "ALTER TABLE sessions ADD COLUMN last_model TEXT",
        ] {
            let _ = storage.db.execute(stmt, []);
        }
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

    /// 写入会话元信息（群名/用户名 + 群聊单聊）。以 dws 为准覆盖本地旧值。
    pub fn upsert_conversations(&self, items: &[ConversationMeta]) -> Result<usize> {
        let now = chrono::Local::now().to_rfc3339();
        let mut written = 0;
        for item in items {
            if item.conversation_id.trim().is_empty() {
                continue;
            }
            self.db.execute(
                "INSERT INTO conversations (conversation_id, name, kind, name_known, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(conversation_id) DO UPDATE SET
                    name = excluded.name,
                    kind = excluded.kind,
                    name_known = excluded.name_known,
                    updated_at = excluded.updated_at",
                params![
                    item.conversation_id,
                    item.name,
                    item.kind,
                    item.name_known as i32,
                    now
                ],
            )?;
            written += 1;
        }
        Ok(written)
    }

    /// 单条会话元信息（会话窗口用）。
    pub fn conversation_meta(&self, conversation_id: &str) -> Result<Option<ConversationMeta>> {
        let mut stmt = self.db.prepare(
            "SELECT conversation_id, name, kind, name_known FROM conversations
             WHERE conversation_id = ?1",
        )?;
        let mut rows = stmt.query(params![conversation_id])?;
        match rows.next()? {
            Some(row) => Ok(Some(ConversationMeta {
                conversation_id: row.get(0)?,
                name: row.get(1)?,
                kind: row.get(2)?,
                name_known: row.get::<_, i32>(3)? != 0,
            })),
            None => Ok(None),
        }
    }

    /// 删除会话：记下删除时该会话的最大事件序号当墓碑，并清掉它的 Agent 会话记录。
    ///
    /// 事件不删（归档与审计照旧）。之后又收到新消息（序号更大）会话会自己回来，
    /// 不会因为删过就永久收不到。
    pub fn delete_conversation(&self, conversation_id: &str) -> Result<()> {
        self.db.execute(
            "INSERT INTO conversations (conversation_id, name, kind, name_known, updated_at, deleted_seq)
             VALUES (
                ?1, '', 'unknown', 0, ?2,
                COALESCE((SELECT MAX(id) FROM events WHERE conversation_id = ?1), 0)
             )
             ON CONFLICT(conversation_id) DO UPDATE SET
                deleted_seq = excluded.deleted_seq,
                updated_at = excluded.updated_at",
            params![conversation_id, chrono::Local::now().to_rfc3339()],
        )?;
        // Agent 会话记录一起清掉：会话回来时从干净状态重新开始，不带旧上下文。
        self.db.execute(
            "DELETE FROM sessions WHERE conversation_id = ?1",
            params![conversation_id],
        )?;
        Ok(())
    }

    /// 建项目时「指定群 / 指定人」的候选名单。全部来自本地已有数据，不起 CLI 子进程。
    ///
    /// 人有两个来源：会话列表里的单聊（id 是会话 id）+ 历史事件的发送者（id 是 open id）。
    /// 过滤时两者都能命中，所以这里都收进来。工号/职位本机没有，留给钉钉搜索补。
    pub fn source_candidates(&self) -> Result<SourceCandidates> {
        let mut stmt = self.db.prepare(
            "SELECT conversation_id, name FROM conversations
             WHERE kind = ?1 AND conversation_id <> '' AND name <> ''",
        )?;
        let mut collect = |kind: &str| -> Result<Vec<crate::project::ScopeEntry>> {
            let rows = stmt.query_map(params![kind], |row| {
                Ok(crate::project::ScopeEntry::new(
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                ))
            })?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        };
        let mut groups = collect("group")?;

        let mut people = collect("direct")?;
        let mut sender_stmt = self.db.prepare(
            "SELECT sender_open_dingtalk_id, sender FROM events
             WHERE sender_open_dingtalk_id <> '' AND sender <> ''
             GROUP BY sender_open_dingtalk_id",
        )?;
        let senders = sender_stmt.query_map([], |row| {
            Ok(crate::project::ScopeEntry::new(
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
            ))
        })?;
        for sender in senders {
            people.push(sender?);
        }

        let dedupe = |mut items: Vec<crate::project::ScopeEntry>| {
            items.sort_by(|a, b| a.name.cmp(&b.name));
            items.dedup_by(|a, b| a.id == b.id);
            items.retain(|item| !item.id.trim().is_empty());
            items
        };
        groups = dedupe(groups);
        people = dedupe(people);

        Ok(SourceCandidates { groups, people })
    }

    /// 按 id 反查名字：群查会话表、人查历史发送人。
    ///
    /// 用途是给**旧数据补显示名** —— 早期版本的范围名单只存了 id，
    /// 打开编辑时不该只看到一个 id。查不到的就不返回，界面显示占位。
    pub fn resolve_scope_names(&self, ids: &[String]) -> Result<Vec<crate::project::ScopeEntry>> {
        let wanted: Vec<String> = ids
            .iter()
            .map(|id| id.trim().to_string())
            .filter(|id| !id.is_empty())
            .collect();
        if wanted.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders = vec!["?"; wanted.len()].join(",");
        let mut found: std::collections::HashMap<String, String> = std::collections::HashMap::new();

        let group_sql = format!(
            "SELECT conversation_id, name FROM conversations
             WHERE conversation_id IN ({}) AND name <> ''",
            placeholders
        );
        let mut stmt = self.db.prepare(&group_sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(wanted.iter()), |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (id, name) = row?;
            found.insert(id, name);
        }

        let people_sql = format!(
            "SELECT sender_open_dingtalk_id, sender FROM events
             WHERE sender_open_dingtalk_id IN ({}) AND sender <> ''
             GROUP BY sender_open_dingtalk_id",
            placeholders
        );
        let mut stmt = self.db.prepare(&people_sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(wanted.iter()), |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (id, name) = row?;
            found.entry(id).or_insert(name);
        }

        Ok(found
            .into_iter()
            .map(|(id, name)| crate::project::ScopeEntry::new(id, name))
            .collect())
    }

    /// 已知会话元的最近更新时间；界面据此判断要不要再拉一次。
    pub fn conversations_meta_updated_at(&self) -> Option<String> {
        self.db
            .query_row("SELECT MAX(updated_at) FROM conversations", [], |row| {
                row.get::<_, Option<String>>(0)
            })
            .ok()
            .flatten()
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
            " AND (e.project_id IS NULL OR e.project_id = '')"
        } else if project_id.map(|p| !p.is_empty()).unwrap_or(false) {
            " AND e.project_id = ?1"
        } else {
            ""
        };

        // 删掉的会话（deleted_seq >= 该会话最大事件序号）不再列出；
        // 删除之后又收到新消息的会自动回来 —— 序号比墓碑大。
        let sql = format!(
            "SELECT e.conversation_id,
                    COUNT(*) AS events,
                    MAX(e.received_at) AS last_received_at,
                    SUM(CASE WHEN e.reply_status = 'sent' THEN 1 ELSE 0 END) AS replied
             FROM events e
             LEFT JOIN conversations c ON c.conversation_id = e.conversation_id
             WHERE e.conversation_id <> ''{scope}
             GROUP BY e.conversation_id
             HAVING MAX(e.id) > COALESCE(MAX(c.deleted_seq), 0)
             ORDER BY last_received_at DESC",
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
            // 元信息可能还没拉到（比如刚收到第一条消息、还没刷新过会话列表），
            // 这时 name 为空、kind 为 unknown，界面自己决定怎么显示。
            let (name, kind): (String, String) = self
                .db
                .query_row(
                    "SELECT name, kind FROM conversations WHERE conversation_id = ?1",
                    params![conversation_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap_or_default();
            out.push(ConversationSummary {
                conversation_id,
                events,
                last_sender,
                last_received_at,
                replied,
                name,
                kind,
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

    /// 记下某会话最近一次生成时 Agent 回报的上下文占比与实际模型。
    /// 只改这两列：建档与换会话 id 是 `save_session` 的事。
    pub fn save_session_runtime(
        &self,
        project_id: &str,
        conversation_id: &str,
        ratio: Option<f64>,
        model: Option<&str>,
    ) -> Result<()> {
        let now = chrono::Local::now().to_rfc3339();
        self.db.execute(
            "UPDATE sessions SET last_context_ratio = ?3, last_model = ?4, updated_at = ?5
             WHERE project_id = ?1 AND conversation_id = ?2",
            params![project_id, conversation_id, ratio, model, now],
        )?;
        Ok(())
    }

    /// 读回会话最近一次回报的上下文占比与模型。落库是为了重启后还能显示。
    /// 会话不存在（未建档）时返回 None。
    pub fn session_runtime(
        &self,
        project_id: &str,
        conversation_id: &str,
    ) -> Result<Option<(Option<f64>, Option<String>)>> {
        let mut stmt = self.db.prepare(
            "SELECT last_context_ratio, last_model FROM sessions
             WHERE project_id = ?1 AND conversation_id = ?2",
        )?;

        let result = stmt.query_row(params![project_id, conversation_id], |row| {
            Ok((row.get(0)?, row.get(1)?))
        });

        match result {
            Ok(runtime) => Ok(Some(runtime)),
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

    /// 统计口径：传 project_id 只统计该项目的已落盘事件，不传则统计全部。
    pub fn get_stats(&self, project_id: Option<&str>) -> Result<Stats> {
        let scoped = project_id.filter(|p| !p.is_empty());
        let and = if scoped.is_some() {
            " AND project_id = ?1"
        } else {
            ""
        };
        // 带表别名的查询（会话数那条要 JOIN conversations）得用带别名的写法。
        let and_e = if scoped.is_some() {
            " AND e.project_id = ?1"
        } else {
            ""
        };

        let count = |where_clause: &str| -> Result<i64> {
            let sql = format!("SELECT COUNT(*) FROM events WHERE {}{}", where_clause, and);
            match scoped {
                Some(p) => Ok(self.db.query_row(&sql, params![p], |row| row.get(0))?),
                None => Ok(self.db.query_row(&sql, [], |row| row.get(0))?),
            }
        };

        Ok(Stats {
            total_events: count("1=1")?,
            malformed_events: count("malformed = 1")?,
            processed_events: count("processed = TRUE")?,
            replied_events: count("reply_status = 'sent'")?,
            failed_replies: count("reply_status = 'failed'")?,
            conversations: {
                // 与左树的口径一致：无会话标识的畸形事件不算会话，
                // 已经删掉的会话（墓碑 >= 最大事件序号）也不算。
                let sql = format!(
                    "SELECT COUNT(*) FROM (
                        SELECT e.conversation_id
                        FROM events e
                        LEFT JOIN conversations c ON c.conversation_id = e.conversation_id
                        WHERE e.conversation_id <> ''{}
                        GROUP BY e.conversation_id
                        HAVING MAX(e.id) > COALESCE(MAX(c.deleted_seq), 0)
                     )",
                    and_e
                );
                match scoped {
                    Some(p) => self.db.query_row(&sql, params![p], |row| row.get(0))?,
                    None => self.db.query_row(&sql, [], |row| row.get(0))?,
                }
            },
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

        // 占比与模型这两列旧库没有，靠补列加出来：读得回、写得进。
        assert_eq!(
            storage.session_runtime("", "cid-1").unwrap(),
            Some((None, None)),
            "旧库补列后应是「有会话但还没回报过」"
        );
        storage
            .save_session_runtime("", "cid-1", Some(0.42), Some("bailian/qwen3.7-plus-cp"))
            .unwrap();
        let runtime = storage.session_runtime("", "cid-1").unwrap().unwrap();
        assert_eq!(runtime.0, Some(0.42));
        assert_eq!(runtime.1.as_deref(), Some("bailian/qwen3.7-plus-cp"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 全新库也要带上这两列（CREATE 语句写错的话只有真跑起来才发现）。
    #[test]
    fn runtime_columns_exist_on_a_fresh_database() {
        let dir = std::env::temp_dir().join("agentmux-fresh-runtime-columns-test");
        let _ = std::fs::remove_dir_all(&dir);
        let storage = Storage::new(dir.clone()).unwrap();

        // 未建档的会话没有运行期记录
        assert_eq!(storage.session_runtime("p1", "cid-1").unwrap(), None);

        storage.save_session("p1", "cid-1", "sess-a", "D:\\a").unwrap();
        assert_eq!(
            storage.session_runtime("p1", "cid-1").unwrap(),
            Some((None, None)),
            "刚建档时还没回报过占比与模型"
        );

        // 建档走 save_session，运行期信息走 save_session_runtime，两者互不覆盖
        storage
            .save_session_runtime("p1", "cid-1", Some(0.83), Some("Qwen3.8-Max"))
            .unwrap();
        assert_eq!(
            storage.get_session("p1", "cid-1").unwrap().unwrap(),
            ("sess-a".to_string(), "D:\\a".to_string()),
            "写占比不该动会话 id 与工作目录"
        );
        let runtime = storage.session_runtime("p1", "cid-1").unwrap().unwrap();
        assert_eq!(runtime.0, Some(0.83));
        assert_eq!(runtime.1.as_deref(), Some("Qwen3.8-Max"));

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

    fn meta(conversation_id: &str, name: &str, kind: &str) -> ConversationMeta {
        ConversationMeta {
            conversation_id: conversation_id.to_string(),
            name: name.to_string(),
            kind: kind.to_string(),
            name_known: true,
        }
    }

    /// 删除会话：立刻从左树与统计里消失；之后再收到新消息要能自己回来
    /// （不能因为删过一次就永久收不到）。
    #[test]
    fn deleted_conversation_hides_until_a_new_event_arrives() {
        let dir = std::env::temp_dir().join("agentmux-delete-conversation-test");
        let _ = std::fs::remove_dir_all(&dir);
        let storage = Storage::new(dir.clone()).unwrap();

        storage
            .save_event(&sample_event("msg-1", "cid-1", "p1"))
            .unwrap();
        storage.save_session("p1", "cid-1", "sess-1", "D:\\a").unwrap();
        storage
            .upsert_conversations(&[meta("cid-1", "客服一群", "group")])
            .unwrap();

        assert_eq!(conversation_ids(&storage.list_conversations(Some("p1"), false).unwrap()).len(), 1);
        assert_eq!(storage.get_stats(Some("p1")).unwrap().conversations, 1);

        storage.delete_conversation("cid-1").unwrap();

        assert!(
            storage.list_conversations(Some("p1"), false).unwrap().is_empty(),
            "删掉的会话不应再出现在左树"
        );
        assert_eq!(
            storage.get_stats(Some("p1")).unwrap().conversations,
            0,
            "统计口径也要跟着少掉"
        );
        assert!(
            storage.get_session("p1", "cid-1").unwrap().is_none(),
            "删除会话应同时清掉 Agent 会话记录"
        );
        assert_eq!(
            storage.get_stats(Some("p1")).unwrap().total_events,
            1,
            "事件本身不删，归档与审计照旧"
        );
        assert_eq!(
            storage.conversation_meta("cid-1").unwrap().unwrap().name,
            "客服一群",
            "名字要留着，会话回来时还能显示"
        );

        // 新消息（序号更大）让它自己回来。
        storage
            .save_event(&sample_event("msg-2", "cid-1", "p1"))
            .unwrap();
        let after = storage.list_conversations(Some("p1"), false).unwrap();
        assert_eq!(
            conversation_ids(&after),
            vec!["cid-1".to_string()],
            "删除后又收到新消息，会话应重新出现"
        );
        assert_eq!(after[0].name, "客服一群");
        assert_eq!(storage.get_stats(Some("p1")).unwrap().conversations, 1);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 按 id 补名字：群的会话 id 与人的 open id 两种键都要能反查到。
    #[test]
    fn resolve_scope_names_fills_names_for_both_kinds() {
        let dir = std::env::temp_dir().join("agentmux-resolve-scope-names-test");
        let _ = std::fs::remove_dir_all(&dir);
        let storage = Storage::new(dir.clone()).unwrap();

        storage
            .upsert_conversations(&[meta("cid-group", "客服一群", "group")])
            .unwrap();
        storage
            .save_event(&sample_event("msg-1", "cid-group", "p1"))
            .unwrap();

        let found = storage
            .resolve_scope_names(&[
                "cid-group".to_string(),
                "open-other".to_string(),
                "查不到的 id".to_string(),
            ])
            .unwrap();
        let map: std::collections::HashMap<String, String> = found
            .into_iter()
            .map(|entry| (entry.id, entry.name))
            .collect();

        assert_eq!(map.get("cid-group").map(String::as_str), Some("客服一群"));
        assert_eq!(
            map.get("open-other").map(String::as_str),
            Some("同事"),
            "人的 open id 要能用历史发送人补出名字"
        );
        assert!(!map.contains_key("查不到的 id"), "查不到的不要编一个名字出来");

        assert!(storage.resolve_scope_names(&[]).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 建项目时给的候选名单：群来自会话列表，人同时来自单聊会话与历史发送人。
    #[test]
    fn source_candidates_cover_groups_and_people() {
        let dir = std::env::temp_dir().join("agentmux-source-candidates-test");
        let _ = std::fs::remove_dir_all(&dir);
        let storage = Storage::new(dir.clone()).unwrap();

        storage
            .upsert_conversations(&[
                meta("cid-group", "客服一群", "group"),
                meta("cid-direct", "李四", "direct"),
            ])
            .unwrap();
        storage
            .save_event(&sample_event("msg-1", "cid-group", "p1"))
            .unwrap();

        let candidates = storage.source_candidates().unwrap();
        let group_ids: Vec<&str> = candidates.groups.iter().map(|g| g.id.as_str()).collect();
        let people_ids: Vec<&str> = candidates.people.iter().map(|p| p.id.as_str()).collect();

        assert_eq!(group_ids, vec!["cid-group"], "群候选来自会话列表的 group");
        assert!(
            people_ids.contains(&"cid-direct"),
            "单聊会话应出现在人候选里: {:?}",
            people_ids
        );
        assert!(
            people_ids.contains(&"open-other"),
            "历史发送人的 open id 也要出现: {:?}",
            people_ids
        );

        let _ = std::fs::remove_dir_all(&dir);
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
