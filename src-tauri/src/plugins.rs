//! 内置适配器清单。
//!
//! 本版**只提供内置适配器**（编译进宿主的 Rust trait 实现），
//! 外部 manifest 插件（NDJSON over stdio）暂不提供，后续需要时再加回来。

use serde::Serialize;
use std::path::PathBuf;

use crate::resolve::{CliKind, PLATFORMS};

#[derive(Debug, Clone, Serialize)]
pub struct PluginInfo {
    pub id: String,
    pub name: String,
    /// "im" 或 "agent"
    pub kind: String,
    /// 恒为 "builtin"
    pub source: String,
    pub enabled: bool,
    /// low | medium | high
    pub risk: String,
    pub declared_capabilities: Vec<String>,
    /// ready | ...
    pub detect: String,
    pub detail: Option<String>,
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
                kind: kind.to_string(),
                source: "builtin".to_string(),
                enabled: !disabled.iter().any(|entry| entry == spec.id),
                // 内置适配器跑在宿主进程内，不引入外部进程风险。
                risk: "low".to_string(),
                declared_capabilities: match spec.kind {
                    CliKind::Im => vec!["read_history".to_string()],
                    CliKind::Agent => vec!["generate".to_string()],
                },
                detect: "ready".to_string(),
                detail: Some("内置 Rust trait 适配器，随宿主编译".to_string()),
            }
        })
        .collect()
}

#[tauri::command]
pub async fn list_plugins() -> Result<Vec<PluginInfo>, String> {
    Ok(builtin_plugins(&load_disabled()))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_plugins_cover_im_and_agent_tracks() {
        let plugins = builtin_plugins(&[]);
        assert!(plugins.iter().any(|p| p.kind == "im"));
        assert!(plugins.iter().any(|p| p.kind == "agent"));
        assert!(plugins.iter().all(|p| p.source == "builtin"));
        assert!(plugins.iter().all(|p| p.enabled));
        // 本版已移除 CodeGraph
        assert!(plugins.iter().all(|p| p.id != "codegraph"));
    }

    #[test]
    fn disabling_a_builtin_plugin_marks_it_disabled() {
        let plugins = builtin_plugins(&["dingtalk".to_string()]);
        let dingtalk = plugins.iter().find(|p| p.id == "dingtalk").unwrap();
        assert!(!dingtalk.enabled);
    }
}
