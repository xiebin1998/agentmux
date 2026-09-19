//! 平台 CLI 自动解析。
//!
//! 目标：用户不需要手输 CLI 路径——所有候选都从全局 PATH 与已知安装位置扫出来，
//! 前端只做单选。新增平台只需在 `PLATFORMS` 注册一行。

use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

use crate::providers::AuthState;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CliKind {
    Im,
    Agent,
}

impl CliKind {
    fn parse(raw: &str) -> Option<Self> {
        match raw {
            "im" => Some(Self::Im),
            "agent" => Some(Self::Agent),
            _ => None,
        }
    }
}

pub struct PlatformSpec {
    pub id: &'static str,
    pub display: &'static str,
    pub kind: CliKind,
    /// 在 PATH 里查找的可执行文件名（不含扩展名）。
    pub commands: &'static [&'static str],
    /// 除 PATH 之外的已知安装位置。
    pub fallbacks: fn() -> Vec<String>,
    /// 取版本号的子命令。
    pub version_args: &'static [&'static str],
}

fn no_fallback() -> Vec<String> {
    Vec::new()
}

fn dingtalk_fallbacks() -> Vec<String> {
    let mut out = Vec::new();

    if let Ok(appdata) = std::env::var("APPDATA") {
        let base = Path::new(&appdata)
            .join("npm")
            .join("node_modules")
            .join("dingtalk-workspace-cli");
        out.push(base.join("vendor").join("dws.exe").to_string_lossy().to_string());
        out.push(base.join("dws.exe").to_string_lossy().to_string());
    }

    // 插件市场缓存是带版本号的目录，扫一层，避免写死版本。
    if let Ok(home) = std::env::var("USERPROFILE") {
        let flavour = Path::new(&home)
            .join(".qoder")
            .join("plugins")
            .join("cache")
            .join("qoder-marketplace")
            .join("dingtalk");
        if let Ok(entries) = std::fs::read_dir(&flavour) {
            for entry in entries.flatten() {
                let cand = entry.path().join("bin").join("dws");
                if cand.is_file() {
                    out.push(cand.to_string_lossy().to_string());
                }
            }
        }
    }

    out
}

fn qoder_fallbacks() -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(home) = std::env::var("USERPROFILE") {
        out.push(
            Path::new(&home)
                .join(".qoder")
                .join("bin")
                .join("qodercli")
                .join("qodercli.exe")
                .to_string_lossy()
                .to_string(),
        );
        out.push(
            Path::new(&home)
                .join(".qoder")
                .join("bin")
                .join("qoder")
                .to_string_lossy()
                .to_string(),
        );
    }
    out
}

fn npm_global_fallbacks() -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(appdata) = std::env::var("APPDATA") {
        let npm = Path::new(&appdata).join("npm");
        for name in ["claude.cmd", "codex.cmd", "codegraph.cmd"] {
            let cand = npm.join(name);
            if cand.is_file() {
                out.push(cand.to_string_lossy().to_string());
            }
        }
    }
    out
}

/// 平台注册表。新增一个 IM / Agent 平台 = 在这里加一行。
pub const PLATFORMS: &[PlatformSpec] = &[
    PlatformSpec {
        id: "dingtalk",
        display: "钉钉",
        kind: CliKind::Im,
        commands: &["dws", "dws.exe"],
        fallbacks: dingtalk_fallbacks,
        version_args: &["version"],
    },
    PlatformSpec {
        id: "qoder",
        display: "Qoder CLI",
        kind: CliKind::Agent,
        commands: &["qodercli", "qodercli.exe"],
        fallbacks: qoder_fallbacks,
        version_args: &["--version"],
    },
    PlatformSpec {
        id: "claude",
        display: "Claude Code",
        kind: CliKind::Agent,
        commands: &["claude", "claude.exe"],
        fallbacks: npm_global_fallbacks,
        version_args: &["--version"],
    },
    PlatformSpec {
        id: "codex",
        display: "Codex CLI",
        kind: CliKind::Agent,
        commands: &["codex", "codex.exe"],
        fallbacks: no_fallback,
        version_args: &["--version"],
    },
    PlatformSpec {
        id: "codegraph",
        display: "CodeGraph",
        kind: CliKind::Agent,
        commands: &["codegraph", "codegraph.exe"],
        fallbacks: no_fallback,
        version_args: &["--version"],
    },
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliCandidate {
    pub path: String,
    /// 来源：PATH / known（已知安装位置）。
    pub source: String,
    /// direct | via_cmd | unsupported —— 见 [`Launch`]。
    pub launch_mode: String,
    pub version: Option<String>,
    pub auth_state: AuthState,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlatformCandidates {
    pub platform_id: String,
    pub display: String,
    pub kind: CliKind,
    pub candidates: Vec<CliCandidate>,
}

impl PlatformCandidates {
    /// 推荐路径只从**能启动**的候选里挑：unsupported 的选了也起不来。
    pub fn recommended_path(&self) -> Option<String> {
        self.candidates
            .iter()
            .find(|c| launch_kind(&c.path) != Launch::Unsupported)
            .map(|c| c.path.clone())
    }
}

fn windows_extensions() -> Vec<String> {
    if !cfg!(windows) {
        return vec![String::new()];
    }
    let mut exts = vec![".exe".to_string(), ".cmd".to_string(), ".bat".to_string(), ".ps1".to_string()];
    if let Ok(pathext) = std::env::var("PATHEXT") {
        for raw in pathext.split(';') {
            let ext = raw.trim().to_ascii_lowercase();
            if !ext.is_empty() && !exts.iter().any(|e| e.eq_ignore_ascii_case(&ext)) {
                exts.push(ext);
            }
        }
    }
    exts
}

/// 候选可执行文件在本机能否被直接启动。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Launch {
    /// 可直接 spawn（`.exe`）
    Direct,
    /// 需要 `cmd /C` 才能跑（`.cmd` / `.bat`）
    ViaCmd,
    /// 本机无法直接启动。
    ///
    /// `.ps1` 用 `cmd /C` **不会执行，而是被文件关联"打开"**（实测：会弹出
    /// 编辑器/ISE）；无扩展名的 sh 脚本同理。这类候选**绝不 spawn**。
    Unsupported,
}

impl Launch {
    pub fn as_str(self) -> &'static str {
        match self {
            Launch::Direct => "direct",
            Launch::ViaCmd => "via_cmd",
            Launch::Unsupported => "unsupported",
        }
    }

    fn rank(self) -> u8 {
        match self {
            Launch::Direct => 0,
            Launch::ViaCmd => 1,
            Launch::Unsupported => 2,
        }
    }
}

pub fn launch_kind(path: &str) -> Launch {
    let ext = Path::new(path)
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase());

    #[cfg(windows)]
    {
        match ext.as_deref() {
            Some("exe") => Launch::Direct,
            Some("cmd") | Some("bat") => Launch::ViaCmd,
            _ => Launch::Unsupported,
        }
    }

    #[cfg(not(windows))]
    {
        let _ = ext;
        Launch::Direct
    }
}

/// 非「可直接启动」的都算包装脚本，界面上要如实标注。
pub fn is_wrapper_path(path: &str) -> bool {
    launch_kind(path) != Launch::Direct
}

fn normalize(path: &str) -> String {
    path.replace('\\', "/").to_ascii_lowercase()
}

fn scan_path(name: &str) -> Vec<String> {
    let mut out = Vec::new();
    let path_var = std::env::var("PATH").unwrap_or_default();
    let sep = if cfg!(windows) { ';' } else { ':' };

    for dir in path_var.split(sep) {
        if dir.trim().is_empty() {
            continue;
        }
        let base = Path::new(dir.trim().trim_matches('"'));
        for ext in windows_extensions() {
            let cand = if ext.is_empty() {
                base.join(name)
            } else {
                base.join(format!("{}{}", name, ext))
            };
            if cand.is_file() {
                out.push(cand.to_string_lossy().to_string());
            }
        }
    }

    out
}

async fn probe_version(path: &str, args: &[&str]) -> Option<String> {
    let mut command = match launch_kind(path) {
        Launch::Direct => Command::new(path),
        Launch::ViaCmd => {
            let mut c = Command::new("cmd");
            c.arg("/C").arg(path);
            c
        }
        // 绝不 spawn：`.ps1` 会被文件关联打开，无扩展名脚本也跑不起来。
        Launch::Unsupported => return None,
    };

    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // 探测可能超时（有些 CLI 会等输入），超时后必须连带杀掉子进程，
        // 否则会留下孤儿子进程把测试/宿主卡住。
        .kill_on_drop(true);

    crate::process::hide_console(&mut command);

    let output = tokio::time::timeout(Duration::from_secs(3), command.output())
        .await
        .ok()?
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // `dws version` 是多行 kv（Version: v1.0.62 ...），其余 CLI 一般单行。
        if let Some(rest) = line.strip_prefix("Version:") {
            return Some(rest.trim().to_string());
        }
        return Some(line.to_string());
    }
    None
}

async fn probe_auth(path: &str) -> AuthState {
    let mut command = match launch_kind(path) {
        Launch::Direct => Command::new(path),
        Launch::ViaCmd => {
            let mut c = Command::new("cmd");
            c.arg("/C").arg(path);
            c
        }
        Launch::Unsupported => return AuthState::Unknown,
    };

    command
        .args(["auth", "status", "-f", "json"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    crate::process::hide_console(&mut command);

    let output = match tokio::time::timeout(Duration::from_secs(10), command.output()).await {
        Ok(Ok(output)) if output.status.success() => output,
        _ => return AuthState::Unknown,
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    match serde_json::from_str::<serde_json::Value>(&stdout) {
        Ok(json) => {
            if json.get("authenticated").and_then(|v| v.as_bool()).unwrap_or(false) {
                AuthState::LoggedIn
            } else {
                AuthState::NotLoggedIn
            }
        }
        Err(_) => AuthState::Unknown,
    }
}

pub async fn list_platform(kind: CliKind) -> Vec<PlatformCandidates> {
    let mut result = Vec::new();

    for spec in PLATFORMS.iter().filter(|s| s.kind == kind) {
        let mut seen: Vec<String> = Vec::new();
        let mut candidates: Vec<CliCandidate> = Vec::new();

        let mut push = |path: String, source: &str| {
            let key = normalize(&path);
            if seen.iter().any(|s| s == &key) {
                return;
            }
            seen.push(key);
            let launch_mode = launch_kind(&path).as_str().to_string();
            candidates.push(CliCandidate {
                path,
                source: source.to_string(),
                launch_mode,
                version: None,
                auth_state: AuthState::Unknown,
                detail: None,
            });
        };

        for name in spec.commands {
            for path in scan_path(name) {
                push(path, "PATH");
            }
        }
        for path in (spec.fallbacks)() {
            if Path::new(&path).is_file() {
                push(path, "known");
            }
        }

        // 能直接启动的优先，其次是 .cmd，无法启动的排最后。
        candidates.sort_by_key(|c| launch_kind(&c.path).rank());

        for candidate in candidates.iter_mut() {
            candidate.version = probe_version(&candidate.path, spec.version_args).await;
            if candidate.version.is_none() {
                candidate.detail = Some(
                    match launch_kind(&candidate.path) {
                        Launch::Unsupported => {
                            "本机无法直接启动（.ps1 / 无扩展名脚本），未探测".to_string()
                        }
                        _ => "无法获取版本（可能不可执行）".to_string(),
                    },
                );
            }
            if spec.id == "dingtalk" {
                candidate.auth_state = probe_auth(&candidate.path).await;
            }
        }

        result.push(PlatformCandidates {
            platform_id: spec.id.to_string(),
            display: spec.display.to_string(),
            kind: spec.kind,
            candidates,
        });
    }

    result
}

/// 解析出某个平台应当使用的可执行文件绝对路径。优先真实可执行文件。
pub async fn resolve_executable(platform_id: &str) -> Option<String> {
    let spec = PLATFORMS.iter().find(|s| s.id == platform_id)?;
    let platforms = list_platform(spec.kind).await;
    platforms
        .into_iter()
        .find(|p| p.platform_id == platform_id)
        .and_then(|p| p.recommended_path())
}

#[tauri::command]
pub async fn list_cli_platforms(kind: Option<String>) -> Result<Vec<PlatformCandidates>, String> {
    let kinds: Vec<CliKind> = match kind.as_deref() {
        Some(raw) => vec![CliKind::parse(raw).ok_or_else(|| format!("未知的 kind: {}", raw))?],
        None => vec![CliKind::Im, CliKind::Agent],
    };

    let mut all = Vec::new();
    for kind in kinds {
        all.extend(list_platform(kind).await);
    }
    Ok(all)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn dingtalk_is_resolved_from_global_install() {
        let platforms = list_platform(CliKind::Im).await;
        let dingtalk = platforms
            .iter()
            .find(|p| p.platform_id == "dingtalk")
            .expect("钉钉平台必须注册在注册表里");

        assert!(
            !dingtalk.candidates.is_empty(),
            "应能从全局 PATH / 已知位置解析到 dws"
        );

        let best = &dingtalk.candidates[0];
        assert_eq!(
            best.launch_mode, "direct",
            "首选候选应可直接启动，实际选中的是 {}",
            best.path
        );
        assert!(best.version.is_some(), "首选候选应能取到版本号");
        assert_eq!(
            best.auth_state,
            AuthState::LoggedIn,
            "实测环境应为已登录"
        );
    }

    #[test]
    fn launch_kind_classifies_windows_script_types() {
        assert_eq!(launch_kind(r"C:\x\dws.exe"), Launch::Direct);
        assert_eq!(launch_kind(r"C:\x\claude.cmd"), Launch::ViaCmd);
        assert_eq!(launch_kind(r"C:\x\claude.bat"), Launch::ViaCmd);
        // 这两类用 cmd /C 会被文件关联"打开"，绝不能当可执行文件对待
        assert_eq!(launch_kind(r"C:\x\claude.ps1"), Launch::Unsupported);
        assert_eq!(launch_kind(r"C:\x\bin\dws"), Launch::Unsupported);

        assert!(!is_wrapper_path(r"C:\x\dws.exe"));
        assert!(is_wrapper_path(r"C:\x\claude.cmd"));
        assert!(is_wrapper_path(r"C:\x\claude.ps1"));
    }

    /// 回归：曾经对所有候选都跑 `cmd /C <path> --version`，导致 `.ps1`
    /// 被文件关联打开、弹出编辑器。这类候选必须完全不 spawn。
    #[tokio::test]
    async fn script_wrappers_are_never_probed() {
        for kind in [CliKind::Im, CliKind::Agent] {
            for platform in list_platform(kind).await {
                for candidate in platform.candidates {
                    if launch_kind(&candidate.path) != Launch::Unsupported {
                        continue;
                    }
                    assert!(
                        candidate.version.is_none(),
                        "不应探测无法启动的候选: {}",
                        candidate.path
                    );
                    assert_eq!(candidate.auth_state, AuthState::Unknown);
                    assert!(
                        candidate
                            .detail
                            .as_deref()
                            .unwrap_or_default()
                            .contains("无法直接启动"),
                        "应如实说明未探测的原因: {:?}",
                        candidate.detail
                    );
                }
            }
        }
    }

    #[tokio::test]
    async fn agent_platforms_are_scanned_and_extensible() {
        let platforms = list_platform(CliKind::Agent).await;
        assert!(
            platforms.len() >= 2,
            "Agent 平台注册表应是可扩展的多平台列表，实际 {}",
            platforms.len()
        );

        let qoder = platforms
            .iter()
            .find(|p| p.platform_id == "qoder")
            .expect("Qoder 平台应在注册表里");
        assert!(!qoder.candidates.is_empty(), "应能解析到 qodercli");
        assert!(qoder.recommended_path().is_some());
    }
}
