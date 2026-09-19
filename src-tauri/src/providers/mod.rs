use serde::{Deserialize, Serialize};
use std::process::Stdio;
use tokio::process::Command;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectReport {
    pub found: bool,
    pub exec_path: Option<String>,
    pub version: Option<String>,
    pub auth_state: AuthState,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum AuthState {
    LoggedIn,
    NotLoggedIn,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ListenKind {
    #[serde(rename = "at-me")]
    AtMe,
    #[serde(rename = "all-direct")]
    DirectMessage,
}

impl ListenKind {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "at-me" => Some(Self::AtMe),
            "all-direct" => Some(Self::DirectMessage),
            _ => None,
        }
    }
}

impl std::fmt::Display for ListenKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ListenKind::AtMe => write!(f, "at-me"),
            ListenKind::DirectMessage => write!(f, "all-direct"),
        }
    }
}

pub trait ImProvider: Send + Sync {
    fn id(&self) -> &'static str;
    fn detect(&self) -> impl std::future::Future<Output = anyhow::Result<DetectReport>> + Send;
}

pub trait AgentProvider: Send + Sync {
    fn id(&self) -> &'static str;
    fn supports_session_resume(&self) -> bool;
    fn detect(&self) -> impl std::future::Future<Output = anyhow::Result<DetectReport>> + Send;
}

// Dingtalk IM Provider
pub struct DingtalkProvider {
    pub exec_path: Option<String>,
}

impl DingtalkProvider {
    pub fn new() -> Self {
        Self { exec_path: None }
    }

    fn resolve_dws_path() -> Option<String> {
        // Check DWS_BIN env var first
        if let Ok(path) = std::env::var("DWS_BIN") {
            if std::path::Path::new(&path).exists() {
                return Some(path);
            }
        }

        // Check npm global prefix
        if let Ok(appdata) = std::env::var("APPDATA") {
            let vendor_path = std::path::Path::new(&appdata)
                .join("npm")
                .join("node_modules")
                .join("dingtalk-workspace-cli")
                .join("vendor")
                .join("dws.exe");
            if vendor_path.exists() {
                return Some(vendor_path.to_string_lossy().to_string());
            }
        }

        None
    }
}

impl ImProvider for DingtalkProvider {
    fn id(&self) -> &'static str {
        "dingtalk"
    }

    async fn detect(&self) -> anyhow::Result<DetectReport> {
        let exec_path = self.exec_path.clone().or_else(|| Self::resolve_dws_path());

        if exec_path.is_none() {
            return Ok(DetectReport {
                found: false,
                exec_path: None,
                version: None,
                auth_state: AuthState::Unknown,
                detail: Some("dws executable not found".to_string()),
            });
        }

        let path = exec_path.unwrap();

        // Check version
        let mut version_command = Command::new(&path);
        version_command
            .arg("version")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        crate::process::hide_console(&mut version_command);
        let version_output = version_command.output().await;

        let version = match version_output {
            Ok(output) if output.status.success() => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                // Parse "dws version v1.0.62 (commit, date)"
                stdout.lines().next().map(|s| s.to_string())
            }
            _ => None,
        };

        // Check auth status
        let mut auth_command = Command::new(&path);
        auth_command
            .args(["auth", "status", "-f", "json"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        crate::process::hide_console(&mut auth_command);
        let auth_output = auth_command.output().await;

        let auth_state = match auth_output {
            Ok(output) if output.status.success() => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(&stdout) {
                    if json.get("authenticated").and_then(|v| v.as_bool()).unwrap_or(false) {
                        AuthState::LoggedIn
                    } else {
                        AuthState::NotLoggedIn
                    }
                } else {
                    AuthState::Unknown
                }
            }
            _ => AuthState::Unknown,
        };

        Ok(DetectReport {
            found: true,
            exec_path: Some(path),
            version,
            auth_state,
            detail: None,
        })
    }
}

// Qoder Agent Provider
pub struct QoderProvider {
    pub exec_path: Option<String>,
}

impl QoderProvider {
    pub fn new() -> Self {
        Self { exec_path: None }
    }

    fn resolve_qoder_path() -> Option<String> {
        // Check common locations
        let home = std::env::var("USERPROFILE").ok()?;
        let qoder_path = std::path::Path::new(&home)
            .join(".qoder")
            .join("bin")
            .join("qodercli")
            .join("qodercli.exe");
        if qoder_path.exists() {
            return Some(qoder_path.to_string_lossy().to_string());
        }
        None
    }
}

impl AgentProvider for QoderProvider {
    fn id(&self) -> &'static str {
        "qoder"
    }

    fn supports_session_resume(&self) -> bool {
        true
    }

    async fn detect(&self) -> anyhow::Result<DetectReport> {
        let exec_path = self.exec_path.clone().or_else(|| Self::resolve_qoder_path());

        if exec_path.is_none() {
            return Ok(DetectReport {
                found: false,
                exec_path: None,
                version: None,
                auth_state: AuthState::Unknown,
                detail: Some("qodercli executable not found".to_string()),
            });
        }

        let path = exec_path.unwrap();

        // Check if executable exists
        let exists = std::path::Path::new(&path).exists();

        // For Qoder, we don't have a simple version/auth check
        // Just report if found
        Ok(DetectReport {
            found: exists,
            exec_path: if exists { Some(path) } else { None },
            version: None,
            auth_state: if exists { AuthState::LoggedIn } else { AuthState::Unknown },
            detail: if !exists { Some("executable not found".to_string()) } else { None },
        })
    }
}

#[tauri::command]
pub async fn detect_im_provider() -> Result<DetectReport, String> {
    let provider = DingtalkProvider::new();
    provider.detect().await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn detect_agent_provider() -> Result<DetectReport, String> {
    let provider = QoderProvider::new();
    provider.detect().await.map_err(|e| e.to_string())
}

/// 用平台注册表解析出的路径执行检测；未指定平台时取钉钉。
#[tauri::command]
pub async fn detect_platform(platform_id: String) -> Result<DetectReport, String> {
    let path = crate::resolve::resolve_executable(&platform_id).await;

    let Some(path) = path else {
        return Ok(DetectReport {
            found: false,
            exec_path: None,
            version: None,
            auth_state: AuthState::Unknown,
            detail: Some(format!("未在 PATH 或已知位置找到 {} 的可执行文件", platform_id)),
        });
    };

    if platform_id == "dingtalk" {
        let provider = DingtalkProvider { exec_path: Some(path) };
        return provider.detect().await.map_err(|e| e.to_string());
    }

    let provider = QoderProvider { exec_path: Some(path) };
    provider.detect().await.map_err(|e| e.to_string())
}
