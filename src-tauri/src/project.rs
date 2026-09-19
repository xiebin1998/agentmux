use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub work_dir: String,
    pub agent_cli_path: String,
    pub dingtalk_cli_path: String,
    pub im_platform: String,
    pub agent_platform: String,
    pub reply_enabled: bool,
    pub reply_timeout_ms: u64,
    pub reply_max_chars: usize,
    pub context_enabled: bool,
    pub context_message_limit: usize,
    pub context_max_chars: usize,
    /// 监听范围：只处理这些群（会话 id）。空 = 不限群。
    #[serde(default)]
    pub group_ids: Vec<String>,
    /// 监听范围：只处理这些人（open id，或单聊的会话 id）。空 = 不限人。
    #[serde(default)]
    pub member_ids: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
}

impl Project {
    pub fn new(
        name: String,
        work_dir: String,
        agent_cli_path: String,
        dingtalk_cli_path: String,
        im_platform: String,
        agent_platform: String,
    ) -> Self {
        let now = chrono::Local::now().to_rfc3339();
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name,
            work_dir,
            agent_cli_path,
            dingtalk_cli_path,
            im_platform,
            agent_platform,
            reply_enabled: false,
            reply_timeout_ms: 120_000,
            reply_max_chars: 500,
            context_enabled: true,
            context_message_limit: 50,
            context_max_chars: 8000,
            group_ids: Vec::new(),
            member_ids: Vec::new(),
            created_at: now.clone(),
            updated_at: now,
        }
    }
}

const SELECT_COLUMNS: &str = "id, name, work_dir, agent_cli_path, dingtalk_cli_path,
    im_platform, agent_platform, reply_enabled, reply_timeout_ms, reply_max_chars,
    context_enabled, context_message_limit, context_max_chars, created_at, updated_at,
    group_ids, member_ids";

/// 名单列存的是 JSON 数组文本；解析失败按空名单处理（不限制）。
fn parse_ids(raw: String) -> Vec<String> {
    serde_json::from_str::<Vec<String>>(&raw).unwrap_or_default()
}

fn ids_to_json(ids: &[String]) -> String {
    serde_json::to_string(ids).unwrap_or_else(|_| "[]".to_string())
}

fn row_to_project(row: &rusqlite::Row<'_>) -> rusqlite::Result<Project> {
    Ok(Project {
        id: row.get(0)?,
        name: row.get(1)?,
        work_dir: row.get(2)?,
        agent_cli_path: row.get(3)?,
        dingtalk_cli_path: row.get(4)?,
        im_platform: row.get(5)?,
        agent_platform: row.get(6)?,
        reply_enabled: row.get::<_, i32>(7)? != 0,
        reply_timeout_ms: row.get(8)?,
        reply_max_chars: row.get(9)?,
        context_enabled: row.get::<_, i32>(10)? != 0,
        context_message_limit: row.get(11)?,
        context_max_chars: row.get(12)?,
        created_at: row.get(13)?,
        updated_at: row.get(14)?,
        group_ids: parse_ids(row.get(15)?),
        member_ids: parse_ids(row.get(16)?),
    })
}

pub struct ProjectStore {
    db: Connection,
}

impl ProjectStore {
    pub fn new(data_dir: PathBuf) -> anyhow::Result<Self> {
        fs::create_dir_all(&data_dir)?;
        let db_path = data_dir.join("agentmux.db");
        let db = Connection::open(db_path)?;

        db.execute_batch(
            "CREATE TABLE IF NOT EXISTS projects (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                work_dir TEXT NOT NULL,
                agent_cli_path TEXT NOT NULL,
                dingtalk_cli_path TEXT NOT NULL,
                im_platform TEXT NOT NULL DEFAULT 'dingtalk',
                agent_platform TEXT NOT NULL DEFAULT 'qoder',
                reply_enabled INTEGER NOT NULL DEFAULT 0,
                reply_timeout_ms INTEGER NOT NULL DEFAULT 120000,
                reply_max_chars INTEGER NOT NULL DEFAULT 500,
                context_enabled INTEGER NOT NULL DEFAULT 1,
                context_message_limit INTEGER NOT NULL DEFAULT 50,
                context_max_chars INTEGER NOT NULL DEFAULT 8000,
                group_ids TEXT NOT NULL DEFAULT '[]',
                member_ids TEXT NOT NULL DEFAULT '[]',
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );",
        )?;

        for stmt in [
            "ALTER TABLE projects ADD COLUMN im_platform TEXT NOT NULL DEFAULT 'dingtalk'",
            "ALTER TABLE projects ADD COLUMN agent_platform TEXT NOT NULL DEFAULT 'qoder'",
            "ALTER TABLE projects ADD COLUMN group_ids TEXT NOT NULL DEFAULT '[]'",
            "ALTER TABLE projects ADD COLUMN member_ids TEXT NOT NULL DEFAULT '[]'",
        ] {
            let _ = db.execute(stmt, []);
        }

        Ok(Self { db })
    }

    pub fn create(&self, project: &Project) -> anyhow::Result<()> {
        self.db.execute(
            "INSERT INTO projects (id, name, work_dir, agent_cli_path, dingtalk_cli_path,
             im_platform, agent_platform, reply_enabled, reply_timeout_ms, reply_max_chars,
             context_enabled, context_message_limit, context_max_chars, created_at, updated_at,
             group_ids, member_ids)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
            params![
                project.id,
                project.name,
                project.work_dir,
                project.agent_cli_path,
                project.dingtalk_cli_path,
                project.im_platform,
                project.agent_platform,
                project.reply_enabled as i32,
                project.reply_timeout_ms,
                project.reply_max_chars,
                project.context_enabled as i32,
                project.context_message_limit,
                project.context_max_chars,
                project.created_at,
                project.updated_at,
                ids_to_json(&project.group_ids),
                ids_to_json(&project.member_ids),
            ],
        )?;
        Ok(())
    }

    pub fn list(&self) -> anyhow::Result<Vec<Project>> {
        let sql = format!(
            "SELECT {} FROM projects ORDER BY created_at DESC",
            SELECT_COLUMNS
        );
        let mut stmt = self.db.prepare(&sql)?;
        let rows = stmt.query_map([], row_to_project)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| anyhow::anyhow!("Failed to collect projects: {}", e))
    }

    pub fn get(&self, id: &str) -> anyhow::Result<Option<Project>> {
        let sql = format!("SELECT {} FROM projects WHERE id = ?1", SELECT_COLUMNS);
        let mut stmt = self.db.prepare(&sql)?;
        let result = stmt.query_row(params![id], row_to_project);

        match result {
            Ok(project) => Ok(Some(project)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn update(&self, project: &Project) -> anyhow::Result<()> {
        let now = chrono::Local::now().to_rfc3339();
        self.db.execute(
            "UPDATE projects SET name = ?1, work_dir = ?2, agent_cli_path = ?3,
             dingtalk_cli_path = ?4, im_platform = ?5, agent_platform = ?6,
             reply_enabled = ?7, reply_timeout_ms = ?8, reply_max_chars = ?9,
             context_enabled = ?10, context_message_limit = ?11, context_max_chars = ?12,
             updated_at = ?13, group_ids = ?14, member_ids = ?15
             WHERE id = ?16",
            params![
                project.name,
                project.work_dir,
                project.agent_cli_path,
                project.dingtalk_cli_path,
                project.im_platform,
                project.agent_platform,
                project.reply_enabled as i32,
                project.reply_timeout_ms,
                project.reply_max_chars,
                project.context_enabled as i32,
                project.context_message_limit,
                project.context_max_chars,
                now,
                ids_to_json(&project.group_ids),
                ids_to_json(&project.member_ids),
                project.id,
            ],
        )?;
        Ok(())
    }

    pub fn delete(&self, id: &str) -> anyhow::Result<()> {
        self.db
            .execute("DELETE FROM projects WHERE id = ?1", params![id])?;
        Ok(())
    }
}

#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn create_project(
    name: String,
    work_dir: String,
    agent_cli_path: String,
    dingtalk_cli_path: String,
    im_platform: Option<String>,
    agent_platform: Option<String>,
    group_ids: Option<Vec<String>>,
    member_ids: Option<Vec<String>>,
) -> Result<Project, String> {
    let data_dir = crate::config::data_dir();
    let store = ProjectStore::new(data_dir).map_err(|e| e.to_string())?;
    let mut project = Project::new(
        name,
        work_dir,
        agent_cli_path,
        dingtalk_cli_path,
        im_platform.unwrap_or_else(|| "dingtalk".to_string()),
        agent_platform.unwrap_or_else(|| "qoder".to_string()),
    );
    // 监听范围：留空 = 所有群、所有人。
    project.group_ids = group_ids.unwrap_or_default();
    project.member_ids = member_ids.unwrap_or_default();
    store.create(&project).map_err(|e| e.to_string())?;
    Ok(project)
}

#[tauri::command]
pub async fn list_projects() -> Result<Vec<Project>, String> {
    let data_dir = crate::config::data_dir();
    let store = ProjectStore::new(data_dir).map_err(|e| e.to_string())?;
    store.list().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_project(id: String) -> Result<Option<Project>, String> {
    let data_dir = crate::config::data_dir();
    let store = ProjectStore::new(data_dir).map_err(|e| e.to_string())?;
    store.get(&id).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn update_project(project: Project) -> Result<(), String> {
    let data_dir = crate::config::data_dir();
    let store = ProjectStore::new(data_dir).map_err(|e| e.to_string())?;
    store.update(&project).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn delete_project(id: String) -> Result<(), String> {
    let data_dir = crate::config::data_dir();
    let store = ProjectStore::new(data_dir).map_err(|e| e.to_string())?;
    store.delete(&id).map_err(|e| e.to_string())
}
