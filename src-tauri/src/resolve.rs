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
        for name in ["claude.cmd", "codex.cmd"] {
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
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliCandidate {
    /// 命令名（用户在终端里敲的那个），如 `dws` / `qodercli`。界面主显示这个。
    pub name: String,
    /// 内部用于 spawn 的可执行文件路径；解析不到时为空。
    pub path: String,
    /// 来源：PATH / known（已知安装位置）/ not_found。
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
    /// 该平台的启动命令名。
    pub command: String,
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

/// 按命令名走 PATH 探测，等价于用户在终端里敲 `<命令> <args>`。
async fn probe_by_name(name: &str, args: &[&str]) -> Option<String> {
    if name.trim().is_empty() {
        return None;
    }

    let mut command = if cfg!(windows) {
        // 经 cmd 才能用上 PATH + PATHEXT（`dws` 实际是 `dws.cmd`）
        let mut c = Command::new("cmd");
        c.arg("/C").arg(name);
        c
    } else {
        Command::new(name)
    };

    command.args(args);
    run_probe(command).await
}

async fn probe_auth_by_name(name: &str) -> AuthState {
    let mut command = if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.arg("/C").arg(name);
        c
    } else {
        Command::new(name)
    };

    command.args(["auth", "status", "-f", "json"]);

    match run_probe_raw(command, 10).await {
        Some(stdout) => match serde_json::from_str::<serde_json::Value>(&stdout) {
            Ok(json) => {
                if json.get("authenticated").and_then(|v| v.as_bool()).unwrap_or(false) {
                    AuthState::LoggedIn
                } else {
                    AuthState::NotLoggedIn
                }
            }
            Err(_) => AuthState::Unknown,
        },
        None => AuthState::Unknown,
    }
}

async fn run_probe_raw(mut command: Command, timeout_secs: u64) -> Option<String> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // 超时必须连带杀掉子进程，否则会留下孤儿子进程把宿主/测试卡住。
        .kill_on_drop(true);

    crate::process::hide_console(&mut command);

    let output = tokio::time::timeout(Duration::from_secs(timeout_secs), command.output())
        .await
        .ok()?
        .ok()?;

    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).to_string())
}

async fn run_probe(command: Command) -> Option<String> {
    let stdout = run_probe_raw(command, 3).await?;

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

/// 解析出一个**能直接 spawn** 的路径。
///
/// PATH 上按命令名命中的往往是脚本（`.ps1`）或包装脚本（`.cmd`），Windows 下都
/// 不能不带 shell 直接启动，所以启动这一步仍需要解析到真实可执行文件。
/// 这一步只服务于「启动」，不暴露给用户。
fn resolve_spawnable(spec: &PlatformSpec) -> Option<(String, &'static str)> {
    let mut hits: Vec<(String, &'static str)> = Vec::new();

    for name in spec.commands {
        for path in scan_path(name) {
            hits.push((path, "PATH"));
        }
    }
    for path in (spec.fallbacks)() {
        if Path::new(&path).is_file() {
            hits.push((path, "known"));
        }
    }

    let mut seen: Vec<String> = Vec::new();
    hits.retain(|(path, _)| {
        let key = normalize(path);
        if seen.contains(&key) {
            return false;
        }
        seen.push(key);
        launch_kind(path) != Launch::Unsupported
    });

    // 可直接启动的优先；同为 direct 时 PATH 命中优先于已知位置。
    hits.sort_by_key(|(path, source)| {
        (launch_kind(path).rank(), if *source == "PATH" { 0 } else { 1 })
    });

    hits.into_iter().next()
}

pub async fn list_platform(kind: CliKind) -> Vec<PlatformCandidates> {
    let mut result = Vec::new();

    for spec in PLATFORMS.iter().filter(|s| s.kind == kind) {
        // 用户在终端里敲的就是命令名；检测也按命令名走 PATH，不暴露目录。
        let command = spec
            .commands
            .first()
            .map(|c| c.trim_end_matches(".exe").to_string())
            .unwrap_or_default();

        let version = probe_by_name(&command, spec.version_args).await;
        let auth_state = if spec.id == "dingtalk" {
            probe_auth_by_name(&command).await
        } else {
            AuthState::Unknown
        };

        let spawnable = resolve_spawnable(spec);

        let (path, source, launch_mode) = match spawnable {
            Some((path, source)) => {
                let mode = launch_kind(&path).as_str().to_string();
                (path, source.to_string(), mode)
            }
            None => (
                String::new(),
                "not_found".to_string(),
                Launch::Unsupported.as_str().to_string(),
            ),
        };

        let detail = if version.is_some() {
            None
        } else if path.is_empty() {
            Some(format!("未在 PATH 中找到 `{}`", command))
        } else {
            Some(format!(
                "`{}` 未能返回版本，但已解析到可启动文件",
                command
            ))
        };

        result.push(PlatformCandidates {
            platform_id: spec.id.to_string(),
            display: spec.display.to_string(),
            kind: spec.kind,
            command: command.clone(),
            candidates: vec![CliCandidate {
                name: command,
                path,
                source,
                launch_mode,
                version,
                auth_state,
                detail,
            }],
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
        assert_eq!(dingtalk.command, "dws", "钉钉平台的启动命令名应为 dws");
        assert_eq!(best.name, "dws", "候选对外以命令名标识");
        assert_eq!(
            best.launch_mode, "direct",
            "内部解析出的启动文件应可直接执行，实际是 {}",
            best.path
        );
        assert!(best.version.is_some(), "`dws version` 应能取到版本号");
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
    }

    /// 回归：曾经对所有 PATH 命中都跑 `cmd /C <path> --version`，导致 `.ps1`
    /// 被文件关联打开、弹出编辑器。现在只保留**能启动**的候选，脚本类不再进入列表。
    #[tokio::test]
    async fn candidates_never_include_unlaunchable_scripts() {
        for kind in [CliKind::Im, CliKind::Agent] {
            for platform in list_platform(kind).await {
                assert!(
                    !platform.command.is_empty(),
                    "{} 应带启动命令名",
                    platform.platform_id
                );
                for candidate in platform.candidates {
                    assert_eq!(candidate.name, platform.command);
                    assert!(
                        !candidate.path.to_ascii_lowercase().ends_with(".ps1"),
                        "绝不应把 .ps1 当作可启动候选: {}",
                        candidate.path
                    );
                    if candidate.path.is_empty() {
                        continue;
                    }
                    assert_ne!(
                        launch_kind(&candidate.path),
                        Launch::Unsupported,
                        "只应保留能直接启动的候选: {}",
                        candidate.path
                    );
                }
            }
        }
    }

    /// 检测按命令名走 PATH（等价于终端里敲 `qodercli --version`），不暴露目录。
    #[tokio::test]
    async fn detection_is_command_name_based() {
        let agents = list_platform(CliKind::Agent).await;
        let qoder = agents
            .iter()
            .find(|p| p.platform_id == "qoder")
            .expect("Qoder 平台应在注册表里");
        assert_eq!(qoder.command, "qodercli");
        assert_eq!(qoder.candidates[0].name, "qodercli");
        assert!(
            qoder.candidates[0].version.is_some(),
            "`qodercli --version` 应能取到版本"
        );
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
