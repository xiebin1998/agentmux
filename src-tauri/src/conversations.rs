//! 会话元信息：群名 / 对方用户名 + 群聊单聊标记。
//!
//! 事件流（`dws event +listen-im` 的 NDJSON）里**只有 conversation_id**，
//! 没有名字也没有类型。实测（2026-09-19，dws）能补齐这两样的命令是
//! `dws chat +recent-conversations`：
//!
//! ```json
//! {"conversationId":"cidofTKV...==","name":"芬尼云业务异常通知",
//!  "nameKnown":true,"type":"group","latestMessageTime":"..."}
//! ```
//!
//! 其中 `type` 是 `group` / `direct`，`name` 群聊给群名、单聊给对方用户名，
//! 正好就是界面要显示的东西。

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use crate::project::ScopeEntry;
use crate::storage::ConversationMeta;

/// 一次最多拉多久以前的会话。窗口越大翻页越多、越慢，默认 24 小时足够覆盖
/// 「最近在聊的会话」；更早的会话靠库里已有的缓存名字继续显示。
pub const DEFAULT_LOOKBACK_HOURS: i64 = 24;

/// 解析 `+recent-conversations` 的输出。
///
/// 容忍噪声：真机上 stdout 可能先出现非 JSON 的提示行，所以从第一个 `{` 开始解析。
pub fn parse_recent_conversations(stdout: &str) -> Vec<ConversationMeta> {
    let Some(start) = stdout.find('{') else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&stdout[start..]) else {
        return Vec::new();
    };

    let Some(items) = value
        .get("data")
        .and_then(|data| data.get("conversations"))
        .and_then(|conversations| conversations.as_array())
    else {
        return Vec::new();
    };

    items
        .iter()
        .filter_map(|item| {
            let conversation_id = item.get("conversationId")?.as_str()?.trim().to_string();
            if conversation_id.is_empty() {
                return None;
            }
            Some(ConversationMeta {
                conversation_id,
                name: item
                    .get("name")
                    .and_then(|name| name.as_str())
                    .unwrap_or_default()
                    .trim()
                    .to_string(),
                kind: normalize_kind(item.get("type").and_then(|kind| kind.as_str())),
                name_known: item
                    .get("nameKnown")
                    .and_then(|known| known.as_bool())
                    .unwrap_or(false),
            })
        })
        .collect()
}

/// 把 dws 的类型值收敛成界面认识的三档。
fn normalize_kind(raw: Option<&str>) -> String {
    match raw.unwrap_or_default().trim().to_lowercase().as_str() {
        "group" => "group".to_string(),
        "direct" | "single" | "user" | "c2c" => "direct".to_string(),
        _ => "unknown".to_string(),
    }
}

/// 统一跑一次 dws：隐藏控制台 + 超时 + 失败时把 stderr 摘要带回来。
async fn run_dws(dws_path: &str, args: &[&str], timeout_secs: u64) -> anyhow::Result<String> {
    run_dws_in(dws_path, args, None, timeout_secs).await
}

/// 同 [`run_dws`]，但可以指定子进程的工作目录。
///
/// 附件下载**必须**带 cwd：`--output-dir` 只接受工作目录内的相对路径，dws 按
/// 自己的 cwd 解析它 —— 而那个「工作目录」必须正是 Agent 的可见范围，否则下
/// 载下来的文件 Agent 读不到。
pub(crate) async fn run_dws_in(
    dws_path: &str,
    args: &[&str],
    cwd: Option<&Path>,
    timeout_secs: u64,
) -> anyhow::Result<String> {
    let mut command = tokio::process::Command::new(dws_path);
    command
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    crate::process::hide_console(&mut command);

    let child = command.spawn()?;
    let output = match tokio::time::timeout(Duration::from_secs(timeout_secs), child.wait_with_output())
        .await
    {
        Ok(result) => result?,
        Err(_) => anyhow::bail!("dws {} 超时（{}s）", args.join(" "), timeout_secs),
    };

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!(
            "dws {} 失败 code={:?}: {}",
            args.join(" "),
            output.status.code(),
            stderr.trim().chars().take(200).collect::<String>()
        );
    }
    Ok(stdout)
}

/// 解析 `dws chat +chat-search` 的输出：按群名搜群。
///
/// 实测形状（2026-09-19）：
/// ```json
/// {"chats":[{"name":"四海饭堂沟通群","openConversationId":"cidoMkZ...==","memberCount":392}]}
/// ```
/// **没有名字的候选直接丢掉**：界面上只显示名字，没名字的条目选不出来也说不清是谁。
pub fn parse_group_search(stdout: &str) -> Vec<ScopeEntry> {
    let Some(start) = stdout.find('{') else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&stdout[start..]) else {
        return Vec::new();
    };
    let Some(items) = value.get("chats").and_then(|chats| chats.as_array()) else {
        return Vec::new();
    };

    items
        .iter()
        .filter_map(|item| {
            let id = item
                .get("openConversationId")
                .and_then(|id| id.as_str())?
                .trim()
                .to_string();
            let name = item
                .get("name")
                .and_then(|name| name.as_str())
                .unwrap_or_default()
                .trim()
                .to_string();
            if id.is_empty() || name.is_empty() {
                return None;
            }
            let members = item.get("memberCount").and_then(|count| count.as_i64());
            Some(ScopeEntry {
                id,
                name,
                code: String::new(),
                extra: members
                    .filter(|count| *count > 0)
                    .map(|count| format!("{}人", count))
                    .unwrap_or_default(),
            })
        })
        .collect()
}

/// 解析 `dws contact +search-user` 的输出：按姓名 / 工号搜人。
///
/// 实测形状（2026-09-19）：
/// ```json
/// {"data":{"users":[{"name":"谢斌","openDingTalkId":"Dq4c2...","userId":"53716",
///                    "title":"软件开发副高级工程师"}]}}
/// ```
/// `openDingTalkId` 就是事件里的 `sender_open_dingtalk_id`，能直接用于过滤。
/// 外部联系人可能只有 `openDingTalkId`、没有姓名 —— 按用户要求直接不列。
pub fn parse_people_search(stdout: &str) -> Vec<ScopeEntry> {
    let Some(start) = stdout.find('{') else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&stdout[start..]) else {
        return Vec::new();
    };
    let Some(items) = value
        .get("data")
        .and_then(|data| data.get("users"))
        .and_then(|users| users.as_array())
    else {
        return Vec::new();
    };

    items
        .iter()
        .filter_map(|item| {
            let id = item
                .get("openDingTalkId")
                .and_then(|id| id.as_str())?
                .trim()
                .to_string();
            let name = item
                .get("name")
                .and_then(|name| name.as_str())
                .unwrap_or_default()
                .trim()
                .to_string();
            if id.is_empty() || name.is_empty() {
                return None;
            }
            Some(ScopeEntry {
                id,
                name,
                code: item
                    .get("userId")
                    .and_then(|code| code.as_str())
                    .unwrap_or_default()
                    .trim()
                    .to_string(),
                extra: item
                    .get("title")
                    .and_then(|title| title.as_str())
                    .unwrap_or_default()
                    .trim()
                    .to_string(),
            })
        })
        .collect()
}

/// 按关键词搜群（`dws chat +chat-search`）。
pub async fn search_groups(
    dws_path: &str,
    query: &str,
    limit: usize,
) -> anyhow::Result<Vec<ScopeEntry>> {
    let limit = limit.clamp(1, 100).to_string();
    let stdout = run_dws(
        dws_path,
        &[
            "chat",
            "+chat-search",
            "--query",
            query,
            "--limit",
            &limit,
            "-f",
            "json",
        ],
        60,
    )
    .await?;
    Ok(parse_group_search(&stdout))
}

/// 按姓名 / 工号搜人（`dws contact +search-user`）。
pub async fn search_people(dws_path: &str, query: &str) -> anyhow::Result<Vec<ScopeEntry>> {
    let stdout = run_dws(
        dws_path,
        &["contact", "+search-user", "--query", query, "-f", "json"],
        60,
    )
    .await?;
    Ok(parse_people_search(&stdout))
}

/// 拉一次会话列表。`dws_path` 是解析出来的可执行文件路径。
pub async fn fetch_conversation_meta(
    dws_path: &str,
    lookback_hours: i64,
) -> anyhow::Result<Vec<ConversationMeta>> {
    let end = chrono::Local::now();
    let start = end - chrono::Duration::hours(lookback_hours.max(1));

    let stdout = run_dws(
        dws_path,
        &[
            "chat",
            "+recent-conversations",
            "--start",
            &start.format("%Y-%m-%d %H:%M:%S").to_string(),
            "--end",
            &end.format("%Y-%m-%d %H:%M:%S").to_string(),
            "-f",
            "json",
        ],
        120,
    )
    .await?;

    Ok(parse_recent_conversations(&stdout))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 真实抓到的 `dws chat +recent-conversations` 输出片段（2026-09-19 实测）。
    /// 群聊给群名、单聊给对方用户名，两种类型都要认出来。
    const REAL_SAMPLE: &str = r#"{"ok":true,"outcome":"success","data":{"complete":true,"count":4,"pagesFetched":11,"unknownTypeCount":0,"conversations":[
{"conversationId":"cidofTKVhp3g2r7HPhexlwsBA==","latestMessageTime":"2026-09-19T20:45:54+08:00","name":"芬尼云业务异常通知","nameKnown":true,"type":"group"},
{"conversationId":"cidzKrnnlMdDKVYA2THk41RdyrN/sO//wq","latestMessageTime":"2026-09-19T20:40:00+08:00","name":"胡汉吟（Maren）","nameKnown":true,"type":"direct"},
{"conversationId":"cidMhW71NIsc40vPpWYV1omSQ==","latestMessageTime":"2026-09-19T20:38:08+08:00","name":"谢斌,王超平,谢斌","nameKnown":true,"type":"group"},
{"conversationId":"","name":"空 id 要丢掉","nameKnown":true,"type":"group"}
]},"meta":{}}"#;

    #[test]
    fn parses_real_dws_conversation_list() {
        let items = parse_recent_conversations(REAL_SAMPLE);

        assert_eq!(items.len(), 3, "空 conversationId 应被丢掉");
        assert_eq!(items[0].kind, "group");
        assert_eq!(items[0].name, "芬尼云业务异常通知");
        assert_eq!(items[1].kind, "direct");
        assert_eq!(items[1].name, "胡汉吟（Maren）", "单聊名就是对方用户名");
        assert_eq!(items[2].conversation_id, "cidMhW71NIsc40vPpWYV1omSQ==");
        assert!(items.iter().all(|item| item.name_known));
    }

    /// 名称未提供时 dws 会回空串 + nameKnown=false，不能把它当成有效名字。
    #[test]
    fn unknown_name_is_marked_not_known() {
        let raw = r#"{"data":{"conversations":[{"conversationId":"cid-x","name":"","nameKnown":false,"type":"group"}]}}"#;
        let items = parse_recent_conversations(raw);

        assert_eq!(items.len(), 1);
        assert!(!items[0].name_known);
        assert!(items[0].name.is_empty());
    }

    /// 不认识的类型值归到 unknown，界面据此不显示标签而不是显示错的标签。
    #[test]
    fn unknown_type_falls_back_to_unknown() {
        let raw = r#"{"data":{"conversations":[{"conversationId":"cid-y","name":"X","nameKnown":true,"type":"space"}]}}"#;
        let items = parse_recent_conversations(raw);
        assert_eq!(items[0].kind, "unknown");
    }

    /// 前面有噪声行、或整体不是 JSON 时都不能 panic。
    #[test]
    fn tolerates_noise_and_garbage() {
        let noisy = format!("connecting to bus...\n{}", REAL_SAMPLE);
        assert_eq!(parse_recent_conversations(&noisy).len(), 3);

        assert!(parse_recent_conversations("not json at all").is_empty());
        assert!(parse_recent_conversations("").is_empty());
    }

    /// 真实抓到的 `dws chat +chat-search --query 饭堂` 输出（2026-09-19 实测）。
    const REAL_GROUP_SEARCH: &str = r#"{"chats":[{"channel":false,"createAt":"2026-03-05 08:14:43","extension":{"newCSpaceIdIM":"27331702980"},"groupType":"INTERNAL_GROUP","memberCount":392,"name":"四海饭堂沟通群","openConversationId":"cidoMkZ/4lcHQDv2mtKAOWgZQ==","ownerOpenDingtalkId":"DYqlNTWfxxDOZgWSQK78dCw2QKNgrgRiPK","title":"四海饭堂沟通群"},{"memberCount":0,"name":"","openConversationId":"cid-noname=="}],"complete":true,"count":2}"#;

    /// 真实抓到的 `dws contact +search-user --query 谢斌` 输出（2026-09-19 实测）。
    /// 关键是 `openDingTalkId`——事件里的 `sender_open_dingtalk_id` 就是它。
    const REAL_PEOPLE_SEARCH: &str = r#"{"ok":true,"outcome":"success","data":{"count":2,"users":[{"name":"谢斌","openDingTalkId":"Dq4c2eHMKEFlaF0M2ytLiiSyB1vJGJiPK5t","title":"软件开发副高级工程师","userId":"53716"}]}}"#;

    /// 外部联系人只回标识、没有姓名（实测胡汉吟如此）。
    const REAL_PEOPLE_SEARCH_WITHOUT_NAME: &str =
        r#"{"ok":true,"outcome":"success","data":{"count":1,"users":[{"openDingTalkId":"Dq4c2eHMKEFm5bLn6VrDO6M2naVkkDKPC"}]}}"#;

    #[test]
    fn parses_group_search_with_name_and_member_count() {
        let found = parse_group_search(REAL_GROUP_SEARCH);

        assert_eq!(found.len(), 1, "没名字的群候选要丢掉");
        assert_eq!(found[0].id, "cidoMkZ/4lcHQDv2mtKAOWgZQ==");
        assert_eq!(found[0].name, "四海饭堂沟通群", "界面显示的就是群名");
        assert_eq!(found[0].extra, "392人", "人数用于同名群之间区分");
        assert!(found[0].code.is_empty(), "群没有工号");
    }

    #[test]
    fn parses_people_search_with_code_and_title() {
        let noisy = format!("正在查询通讯录…\n{}", REAL_PEOPLE_SEARCH);
        let found = parse_people_search(&noisy);

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, "Dq4c2eHMKEFlaF0M2ytLiiSyB1vJGJiPK5t");
        assert_eq!(found[0].name, "谢斌");
        assert_eq!(found[0].code, "53716", "工号");
        assert_eq!(found[0].extra, "软件开发副高级工程师", "职位用于同名区分");
    }

    #[test]
    fn people_without_a_name_are_dropped() {
        assert!(
            parse_people_search(REAL_PEOPLE_SEARCH_WITHOUT_NAME).is_empty(),
            "只有 openDingTalkId 的外部联系人按用户要求直接不列"
        );
        assert!(parse_people_search("").is_empty());
        assert!(parse_group_search("not json").is_empty());
    }
}
