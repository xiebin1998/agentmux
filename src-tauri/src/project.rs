use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

/// 监听范围里的一个条目：`id` 用于过滤，其余字段只用于显示。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ScopeEntry {
    /// 群的会话 id，或人的 open id。
    pub id: String,
    /// 群名 / 姓名。老数据可能只有 id、没名字，界面先显示占位再补。
    #[serde(default)]
    pub name: String,
    /// 工号（人）。群为空。
    #[serde(default)]
    pub code: String,
    /// 区分信息：群是「42人」，人是职位。重名时靠它区分。
    #[serde(default)]
    pub extra: String,
}

impl ScopeEntry {
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            code: String::new(),
            extra: String::new(),
        }
    }
}

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
    pub group_ids: Vec<ScopeEntry>,
    /// 监听范围：只处理这些人（open id，或单聊的会话 id）。空 = 不限人。
    #[serde(default)]
    pub member_ids: Vec<ScopeEntry>,
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

    /// 界面上的数字框被清空会传 0，而 0 字上限会把每条回复都截成空串、
    /// 0 条上下文等于没上下文。落库前统一兜最小值。
    fn clamp_numeric_limits(&mut self) {
        self.reply_timeout_ms = self.reply_timeout_ms.max(1);
        self.reply_max_chars = self.reply_max_chars.max(1);
        self.context_message_limit = self.context_message_limit.max(1);
        self.context_max_chars = self.context_max_chars.max(1);
    }
}

const SELECT_COLUMNS: &str = "id, name, work_dir, agent_cli_path, dingtalk_cli_path,
    im_platform, agent_platform, reply_enabled, reply_timeout_ms, reply_max_chars,
    context_enabled, context_message_limit, context_max_chars, created_at, updated_at,
    group_ids, member_ids";

/// 名单列存的是 JSON 数组文本。兼容两种历史形态：
/// - 旧：`["cid-1"]`（只有 id，没名字）→ 补成只有 id 的条目；
/// - 新：`[{"id":"cid-1","name":"客服一群"}]`。
/// 解析失败按空名单处理（不限制范围）。
fn parse_ids(raw: String) -> Vec<ScopeEntry> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return Vec::new();
    };
    let Some(items) = value.as_array() else {
        return Vec::new();
    };

    items
        .iter()
        .filter_map(|item| {
            let id = match item {
                serde_json::Value::String(id) => id.trim().to_string(),
                serde_json::Value::Object(map) => map
                    .get("id")
                    .and_then(|id| id.as_str())
                    .unwrap_or_default()
                    .trim()
                    .to_string(),
                _ => return None,
            };
            if id.is_empty() {
                return None;
            }
            if let serde_json::Value::Object(map) = item {
                return Some(ScopeEntry {
                    id,
                    name: map
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    code: map
                        .get("code")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    extra: map
                        .get("extra")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                });
            }
            Some(ScopeEntry::new(id, ""))
        })
        .collect()
}

fn ids_to_json(ids: &[ScopeEntry]) -> String {
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

/// 创建项目时可一并落库的回复 / 上下文设置。`None` = 保持 `Project::new` 的默认值。
#[derive(Default, Clone, Copy)]
struct NewProjectSettings {
    reply_enabled: Option<bool>,
    reply_timeout_ms: Option<u64>,
    reply_max_chars: Option<usize>,
    context_enabled: Option<bool>,
    context_message_limit: Option<usize>,
    context_max_chars: Option<usize>,
}

impl NewProjectSettings {
    fn apply(self, project: &mut Project) {
        if let Some(value) = self.reply_enabled {
            project.reply_enabled = value;
        }
        if let Some(value) = self.reply_timeout_ms {
            project.reply_timeout_ms = value;
        }
        if let Some(value) = self.reply_max_chars {
            project.reply_max_chars = value;
        }
        if let Some(value) = self.context_enabled {
            project.context_enabled = value;
        }
        if let Some(value) = self.context_message_limit {
            project.context_message_limit = value;
        }
        if let Some(value) = self.context_max_chars {
            project.context_max_chars = value;
        }
    }
}

/// 创建项目。回复与上下文设置一并接收——之前这些参数不在签名里，
/// 界面勾了「启用自动回复」也会被静默丢掉，新项目永远是「未启用」。
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn create_project(
    name: String,
    work_dir: String,
    agent_cli_path: String,
    dingtalk_cli_path: String,
    im_platform: Option<String>,
    agent_platform: Option<String>,
    reply_enabled: Option<bool>,
    reply_timeout_ms: Option<u64>,
    reply_max_chars: Option<usize>,
    context_enabled: Option<bool>,
    context_message_limit: Option<usize>,
    context_max_chars: Option<usize>,
    group_ids: Option<Vec<ScopeEntry>>,
    member_ids: Option<Vec<ScopeEntry>>,
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
    NewProjectSettings {
        reply_enabled,
        reply_timeout_ms,
        reply_max_chars,
        context_enabled,
        context_message_limit,
        context_max_chars,
    }
    .apply(&mut project);
    // 监听范围：留空 = 所有群、所有人。
    project.group_ids = group_ids.unwrap_or_default();
    project.member_ids = member_ids.unwrap_or_default();
    project.clamp_numeric_limits();
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

/// 更新项目。保存后立刻把新设置热更新到该项目正在跑的监听上，
/// 否则「勾了启用自动回复」要手动停掉再启动监听才生效。
#[tauri::command]
pub async fn update_project(
    state: tauri::State<'_, crate::AppState>,
    mut project: Project,
) -> Result<(), String> {
    let data_dir = crate::config::data_dir();
    let store = ProjectStore::new(data_dir).map_err(|e| e.to_string())?;
    project.clamp_numeric_limits();
    store.update(&project).map_err(|e| e.to_string())?;

    let mut settings = crate::config::reply_settings_for_project(&project);
    if settings.enabled && settings.agent_cli_path.is_none() {
        settings.agent_cli_path = crate::resolve::resolve_executable(&settings.agent_platform).await;
    }
    let orchestrator = state.orchestrator.lock().await;
    orchestrator.apply_project_settings(&project.id, settings).await;
    Ok(())
}

/// 全局设置（模型、思考强度等）改完后，把新设置**重新解析并下发**到在跑的监听。
///
/// 不这么做的话，改动只落在 `settings.json` 里，在跑的监听还用启动时的快照 ——
/// 界面写着「下一条消息生效」其实是假的（得重启监听才生效）。返回被更新的监听数。
pub async fn push_settings_to_running_listeners(state: &crate::AppState) -> usize {
    let orchestrator = state.orchestrator.lock().await;

    match ProjectStore::new(crate::config::data_dir()).and_then(|store| store.list()) {
        Ok(projects) => orchestrator.push_project_settings(&projects).await,
        Err(err) => {
            // 写进「监听日志」而不是 stderr：GUI 下 stderr 没人看得见。
            orchestrator
                .push_global_log(&format!("全局设置未能下发到在跑的监听：{err}"))
                .await;
            0
        }
    }
}

/// **真删**一个项目：先把该项目下所有会话的消息清掉，再删项目记录。
    ///
    /// 不可恢复 —— 界面侧必须先让用户二次确认。先清消息再删记录：反过来的话
    /// 项目没了，事件上的 project_id 就成了孤儿，谁都认领不了。
    #[tauri::command]
    pub async fn delete_project(
        state: tauri::State<'_, crate::AppState>,
        id: String,
    ) -> Result<crate::storage::Purged, String> {
        let purged = {
            let storage = state.storage.lock().await;
            storage.purge_project(&id).map_err(|e| e.to_string())?
        };

        let data_dir = crate::config::data_dir();
        let store = ProjectStore::new(data_dir).map_err(|e| e.to_string())?;
        store.delete(&id).map_err(|e| e.to_string())?;
        Ok(purged)
    }

#[cfg(test)]
mod tests {
    use super::*;

    /// 早期的范围名单只存了 id（`["cid-1"]`），新格式存对象。
    /// 两种都要能读：旧数据不能因为升级就变成空范围（那会让过滤静默失效）。
    #[test]
    fn scope_ids_parse_both_legacy_and_new_shape() {
        let legacy = parse_ids(r#"["cid-1","open-2"]"#.to_string());
        assert_eq!(legacy.len(), 2);
        assert_eq!(legacy[0].id, "cid-1");
        assert!(legacy[0].name.is_empty(), "旧数据没有名字，等界面去补");

        let current = parse_ids(
            r#"[{"id":"cid-1","name":"客服一群","extra":"42人"},{"id":"open-2","name":"张三","code":"53716","extra":"工程师"}]"#
                .to_string(),
        );
        assert_eq!(current[0].name, "客服一群");
        assert_eq!(current[0].extra, "42人");
        assert_eq!(current[1].code, "53716");
        assert_eq!(current[1].extra, "工程师");
    }

    /// 脏数据不能让整个名单变成空：坏条目丢掉，好条目留着。
    #[test]
    fn scope_ids_skip_junk_without_losing_the_rest() {
        let parsed = parse_ids(r#"["cid-1",{"name":"没有 id"},"",42,null]"#.to_string());
        assert_eq!(parsed.len(), 1, "实际解析: {:?}", parsed);
        assert_eq!(parsed[0].id, "cid-1");

        assert!(parse_ids("不是 json".to_string()).is_empty());
        assert!(parse_ids("{}".to_string()).is_empty());
    }

    /// 创建项目时界面勾选的「启用自动回复」必须落到项目上。
    /// 以前 `create_project` 的签名里根本没有这些参数，界面的勾选被静默丢弃，
    /// 新项目永远是「未启用」——用户看到的就是「勾了却不回复」。
    #[test]
    fn creation_settings_override_project_defaults() {
        let mut project = Project::new(
            "test".to_string(),
            r"D:\work\test".to_string(),
            r"C:\tools\qodercli\qodercli.exe".to_string(),
            r"C:\tools\dws\dws.exe".to_string(),
            "dingtalk".to_string(),
            "qoder".to_string(),
        );
        assert!(!project.reply_enabled, "默认不开自动回复");

        NewProjectSettings {
            reply_enabled: Some(true),
            reply_max_chars: Some(300),
            ..Default::default()
        }
        .apply(&mut project);

        assert!(project.reply_enabled, "勾了就要落库为启用");
        assert_eq!(project.reply_max_chars, 300);
        assert_eq!(project.reply_timeout_ms, 120_000, "没传的项保持默认");
        assert!(project.context_enabled, "没传的项保持默认");
    }

    /// 数字框清空会传 0：0 字上限会把回复截成空串、0 条上下文等于没上下文。
    #[test]
    fn numeric_limits_are_clamped_to_at_least_one() {
        let mut project = Project::new(
            "test".to_string(),
            r"D:\work\test".to_string(),
            r"C:\tools\qodercli\qodercli.exe".to_string(),
            r"C:\tools\dws\dws.exe".to_string(),
            "dingtalk".to_string(),
            "qoder".to_string(),
        );
        project.reply_max_chars = 0;
        project.reply_timeout_ms = 0;
        project.context_message_limit = 0;
        project.context_max_chars = 0;

        project.clamp_numeric_limits();

        assert_eq!(project.reply_max_chars, 1);
        assert_eq!(project.reply_timeout_ms, 1);
        assert_eq!(project.context_message_limit, 1);
        assert_eq!(project.context_max_chars, 1);
    }

    /// 一个都不传（老调用方）时，项目仍按默认值创建，不能被 None 写成 0。
    #[test]
    fn creation_settings_without_options_keep_defaults() {
        let mut project = Project::new(
            "test".to_string(),
            r"D:\work\test".to_string(),
            r"C:\tools\qodercli\qodercli.exe".to_string(),
            r"C:\tools\dws\dws.exe".to_string(),
            "dingtalk".to_string(),
            "qoder".to_string(),
        );
        NewProjectSettings::default().apply(&mut project);

        assert!(!project.reply_enabled);
        assert_eq!(project.reply_timeout_ms, 120_000);
        assert_eq!(project.context_message_limit, 50);
        assert_eq!(project.context_max_chars, 8000);
    }

    /// 存回去的是对象数组，读回来要等价。
    #[test]
    fn scope_ids_round_trip() {
        let entries = vec![
            ScopeEntry {
                id: "cid-1".to_string(),
                name: "客服一群".to_string(),
                code: String::new(),
                extra: "42人".to_string(),
            },
            ScopeEntry::new("open-2", "张三"),
        ];
        let json = ids_to_json(&entries);
        assert!(json.starts_with("[{"), "新格式应是对象数组: {}", json);
        assert_eq!(parse_ids(json), entries);
    }
}
