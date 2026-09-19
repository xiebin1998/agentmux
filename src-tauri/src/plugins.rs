//! 双轨插件：内置 Rust trait 适配器 + 外部 manifest 插件（NDJSON over stdio）。
//!
//! 外部插件协议 v1（最小可用）：
//!   宿主 spawn `command args...`，向 stdin 写一行
//!     {"type":"hello","protocol":1}
//!   插件需在 5s 内于 stdout 回一行
//!     {"type":"hello","protocol":1,"id":"<插件 id>"}
//!   超时/格式不符即判定握手失败，并在界面上如实报错。
//!
//! 只支持一个插件根目录（D-69）：`<data_dir>/plugins/<插件目录>/manifest.json`。

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

use crate::resolve::{CliKind, PLATFORMS};

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default = "default_protocol")]
    pub protocol: u32,
    /// "im" 或 "agent"
    pub kind: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub capabilities: Vec<String>,
}

fn default_protocol() -> u32 {
    1
}

#[derive(Debug, Clone, Serialize)]
pub struct PluginInfo {
    pub id: String,
    pub name: String,
    pub version: Option<String>,
    pub kind: String,
    /// builtin | external
    pub source: String,
    pub enabled: bool,
    /// low | medium | high
    pub risk: String,
    pub declared_capabilities: Vec<String>,
    pub protocol: u32,
    pub entry: Option<String>,
    pub path: Option<String>,
    /// ready | handshake_failed | not_scanned
    pub detect: String,
    pub detail: Option<String>,
}

pub fn plugin_root() -> PathBuf {
    crate::config::data_dir().join("plugins")
}

/// 按声明能力分级（D-67）。声明了写/执行/网络能力即为高危。
pub fn grade_risk(capabilities: &[String]) -> &'static str {
    const HIGH: [&str; 5] = ["send_message", "exec", "file_write", "network", "shell"];
    const MEDIUM: [&str; 2] = ["generate", "read_history"];

    let lower: Vec<String> = capabilities.iter().map(|c| c.to_lowercase()).collect();
    if lower.iter().any(|c| HIGH.contains(&c.as_str())) {
        return "high";
    }
    if lower.iter().any(|c| MEDIUM.contains(&c.as_str())) {
        return "medium";
    }
    "low"
}

/// 内置插件：来自平台注册表（IM / Agent 适配器）。
fn builtin_plugins(disabled: &[String]) -> Vec<PluginInfo> {
    PLATFORMS
        .iter()
        .map(|spec| {
            let kind = match spec.kind {
                CliKind::Im => "im",
                CliKind::Agent => "agent",
            };
            PluginInfo {
                id: spec.id.to_string(),
                name: spec.display.to_string(),
                version: None,
                kind: kind.to_string(),
                source: "builtin".to_string(),
                enabled: !disabled.iter().any(|d| d == spec.id),
                // 内置适配器运行在宿主进程内，不引入外部进程风险。
                risk: "low".to_string(),
                declared_capabilities: match spec.kind {
                    CliKind::Im => vec!["read_history".to_string()],
                    CliKind::Agent => vec!["generate".to_string()],
                },
                protocol: 1,
                entry: None,
                path: None,
                detect: "ready".to_string(),
                detail: Some("内置 Rust trait 适配器，随宿主编译".to_string()),
            }
        })
        .collect()
}

fn read_manifests(root: &Path) -> Vec<(Manifest, PathBuf, Option<String>)> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return out;
    };

    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let manifest_path = dir.join("manifest.json");
        if !manifest_path.is_file() {
            continue;
        }
        match std::fs::read_to_string(&manifest_path) {
            Ok(raw) => match serde_json::from_str::<Manifest>(&raw) {
                Ok(manifest) => out.push((manifest, dir, None)),
                Err(err) => {
                    // 保留解析失败的记录，让界面能如实显示报错（A8.2.3）。
                    let manifest = Manifest {
                        id: dir
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_else(|| "unknown".to_string()),
                        name: "（manifest.json 解析失败）".to_string(),
                        version: None,
                        protocol: 1,
                        kind: "unknown".to_string(),
                        command: String::new(),
                        args: Vec::new(),
                        capabilities: Vec::new(),
                    };
                    out.push((manifest, dir, Some(format!("manifest.json 解析失败: {}", err))));
                }
            },
            Err(err) => {
                let manifest = Manifest {
                    id: dir
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| "unknown".to_string()),
                    name: "（manifest.json 读取失败）".to_string(),
                    version: None,
                    protocol: 1,
                    kind: "unknown".to_string(),
                    command: String::new(),
                    args: Vec::new(),
                    capabilities: Vec::new(),
                };
                out.push((manifest, dir, Some(format!("manifest.json 读取失败: {}", err))));
            }
        }
    }

    out.sort_by(|a, b| a.0.id.cmp(&b.0.id));
    out
}

/// 与外部插件做一次 hello 握手，确认它真的能跑。
async fn handshake(manifest: &Manifest, cwd: &Path) -> (String, Option<String>) {
    if manifest.command.trim().is_empty() {
        return ("handshake_failed".to_string(), Some("manifest 未声明 command".to_string()));
    }

    let mut command = Command::new(&manifest.command);
    command
        .args(&manifest.args)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    crate::process::hide_console(&mut command);

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(err) => {
            return ("handshake_failed".to_string(), Some(format!("启动失败: {}", err)));
        }
    };

    let hello = format!("{{\"type\":\"hello\",\"protocol\":{}}}\n", manifest.protocol);
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(hello.as_bytes()).await;
        let _ = stdin.shutdown().await;
    }

    let stdout = child.stdout.take();
    let read = async {
        let Some(stdout) = stdout else {
            return Err("插件没有 stdout".to_string());
        };
        let mut lines = BufReader::new(stdout).lines();
        match lines.next_line().await {
            Ok(Some(line)) => Ok(line),
            Ok(None) => Err("插件未输出任何内容就退出了".to_string()),
            Err(err) => Err(format!("读取插件输出失败: {}", err)),
        }
    };

    let result = tokio::time::timeout(HANDSHAKE_TIMEOUT, read).await;
    let _ = child.kill().await;

    match result {
        Ok(Ok(line)) => match serde_json::from_str::<serde_json::Value>(&line) {
            Ok(json) => {
                let kind_ok = json.get("type").and_then(|v| v.as_str()) == Some("hello");
                let id_ok = json.get("id").and_then(|v| v.as_str()) == Some(manifest.id.as_str());
                if kind_ok && id_ok {
                    ("ready".to_string(), None)
                } else {
                    (
                        "handshake_failed".to_string(),
                        Some(format!("握手回包不匹配，收到: {}", truncate(&line, 200))),
                    )
                }
            }
            Err(err) => (
                "handshake_failed".to_string(),
                Some(format!("握手回包不是 JSON（{}）: {}", err, truncate(&line, 200))),
            ),
        },
        Ok(Err(err)) => ("handshake_failed".to_string(), Some(err)),
        Err(_) => (
            "handshake_failed".to_string(),
            Some(format!("{}s 内未收到握手回包", HANDSHAKE_TIMEOUT.as_secs())),
        ),
    }
}

fn truncate(input: &str, max: usize) -> String {
    if input.chars().count() <= max {
        return input.to_string();
    }
    input.chars().take(max).collect::<String>() + "…"
}

fn disabled_path() -> PathBuf {
    crate::config::data_dir().join("plugins.json")
}

fn load_disabled() -> Vec<String> {
    let Ok(raw) = std::fs::read_to_string(disabled_path()) else {
        return Vec::new();
    };
    serde_json::from_str::<Vec<String>>(&raw).unwrap_or_default()
}

fn save_disabled(disabled: &[String]) -> anyhow::Result<()> {
    let path = disabled_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, serde_json::to_string_pretty(disabled)?)?;
    Ok(())
}

/// 只列内置插件（快，不启动任何外部进程）。
#[tauri::command]
pub async fn list_plugins() -> Result<Vec<PluginInfo>, String> {
    Ok(builtin_plugins(&load_disabled()))
}

/// 连带扫描外部插件并逐个握手（A8.2.3）。由界面显式触发。
#[tauri::command]
pub async fn scan_plugins() -> Result<Vec<PluginInfo>, String> {
    let disabled = load_disabled();
    let mut plugins = builtin_plugins(&disabled);
    let root = plugin_root();

    for (manifest, dir, manifest_error) in read_manifests(&root) {
        let (detect, detail) = match manifest_error {
            Some(err) => ("handshake_failed".to_string(), Some(err)),
            None => handshake(&manifest, &dir).await,
        };

        plugins.push(PluginInfo {
            id: manifest.id.clone(),
            name: manifest.name,
            version: manifest.version,
            kind: manifest.kind,
            source: "external".to_string(),
            enabled: !disabled.iter().any(|d| d == &manifest.id),
            risk: grade_risk(&manifest.capabilities).to_string(),
            declared_capabilities: manifest.capabilities,
            protocol: manifest.protocol,
            entry: Some(format!(
                "{} {}",
                manifest.command,
                manifest.args.join(" ")
            ).trim().to_string()),
            path: Some(dir.to_string_lossy().to_string()),
            detect,
            detail,
        });
    }

    Ok(plugins)
}

#[tauri::command]
pub async fn set_plugin_enabled(id: String, enabled: bool) -> Result<(), String> {
    let mut disabled = load_disabled();
    disabled.retain(|entry| entry != &id);
    if !enabled {
        disabled.push(id);
    }
    save_disabled(&disabled).map_err(|e| e.to_string())
}

#[derive(Debug, Clone, Serialize)]
pub struct PluginEnv {
    pub root: String,
    pub manifest_example: String,
}

/// 插件根目录与 manifest 示例（A8.2.1 / A8.2.2）。
#[tauri::command]
pub async fn plugin_env() -> Result<PluginEnv, String> {
    let root = plugin_root();
    let _ = std::fs::create_dir_all(&root);

    let example = serde_json::json!({
        "id": "my-agent",
        "name": "My Agent",
        "version": "0.1.0",
        "protocol": 1,
        "kind": "agent",
        "command": "node",
        "args": ["index.js"],
        "capabilities": ["generate"]
    });

    Ok(PluginEnv {
        root: root.to_string_lossy().to_string(),
        manifest_example: serde_json::to_string_pretty(&example).unwrap_or_default(),
    })
}

/// 协议文档正文（A8.2.1，应用内页面）。
#[tauri::command]
pub async fn plugin_protocol_doc() -> Result<String, String> {
    Ok(include_str!("plugin_protocol.md").to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn risk_grading_follows_declared_capabilities() {
        assert_eq!(grade_risk(&["generate".to_string()]), "medium");
        assert_eq!(grade_risk(&["send_message".to_string()]), "high");
        assert_eq!(grade_risk(&["exec".to_string()]), "high");
        assert_eq!(grade_risk(&["network".to_string()]), "high");
        assert_eq!(grade_risk(&["mystery".to_string()]), "low");
        assert_eq!(grade_risk(&[]), "low");
    }

    #[test]
    fn builtin_plugins_cover_im_and_agent_tracks() {
        let plugins = builtin_plugins(&[]);
        assert!(plugins.iter().any(|p| p.kind == "im"));
        assert!(plugins.iter().any(|p| p.kind == "agent"));
        assert!(plugins.iter().all(|p| p.source == "builtin"));
        assert!(plugins.iter().all(|p| p.enabled));
    }

    #[test]
    fn disabling_a_builtin_plugin_marks_it_disabled() {
        let plugins = builtin_plugins(&["dingtalk".to_string()]);
        let dingtalk = plugins.iter().find(|p| p.id == "dingtalk").unwrap();
        assert!(!dingtalk.enabled);
    }

    fn manifest_with(command: &str, args: Vec<String>) -> Manifest {
        Manifest {
            id: "test-plugin".to_string(),
            name: "Test Plugin".to_string(),
            version: Some("0.0.1".to_string()),
            protocol: 1,
            kind: "agent".to_string(),
            command: command.to_string(),
            args,
            capabilities: vec!["generate".to_string()],
        }
    }

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("agentmux-plugin-test-{}", tag));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[tokio::test]
    async fn handshake_accepts_a_real_plugin_process() {
        let dir = temp_dir("ok");
        let script = r#"process.stdout.write(JSON.stringify({type:"hello",protocol:1,id:"test-plugin"})+"\n");"#;
        let manifest = manifest_with("node", vec!["-e".to_string(), script.to_string()]);

        let (detect, detail) = handshake(&manifest, &dir).await;
        assert_eq!(detect, "ready", "握手应通过，detail={:?}", detail);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn handshake_rejects_a_mismatched_id() {
        let dir = temp_dir("mismatch");
        let script = r#"process.stdout.write(JSON.stringify({type:"hello",protocol:1,id:"someone-else"})+"\n");"#;
        let manifest = manifest_with("node", vec!["-e".to_string(), script.to_string()]);

        let (detect, detail) = handshake(&manifest, &dir).await;
        assert_eq!(detect, "handshake_failed");
        assert!(
            detail.unwrap_or_default().contains("不匹配"),
            "应说明回包不匹配的原因"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn handshake_reports_missing_command_instead_of_hanging() {
        let dir = temp_dir("empty");
        let manifest = manifest_with("", Vec::new());

        let (detect, detail) = handshake(&manifest, &dir).await;
        assert_eq!(detect, "handshake_failed");
        assert!(detail.unwrap_or_default().contains("command"));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
