//! 校验 / 生成 Tauri 更新包的签名。
//!
//! 为什么需要它：应用内「立即更新」在验签失败时才报错，而那时候版本已经发出去了。
//! 这个工具把同一件事提前到构建期做完 —— 用 `minisign-verify`（应用运行时同一套库）
//! 校验 `xxx.exe` 与 `xxx.exe.sig` 是否配对；不配对时用 `minisign` 直接重签。
//!
//! 用法：
//!   sigcheck <tauri.conf.json 或 .key.pub> <安装包.sig> <安装包>      校单个
//!   sigcheck <tauri.conf.json 或 .key.pub> <目录>                校目录下所有 *.sig
//!   sigcheck sign <安装包>...                                   重签（密钥走环境变量）
//!   sigcheck keycheck <tauri.conf.json 或 .key.pub>             校私钥与公钥是否配套
//!
//! 为什么要 keycheck：签名里只带 8 字节 key id，公钥被抄错一个字符时 key id 不变，
//! 于是 CLI 的「私钥与 pubkey 不配套」警告、这里的 key id 比对全部放行，
//! 直到应用内更新验签才炸（2026-09 就是这么发出去几版的）。所以配套性要真签真验一次。
//!
//! 重签读环境变量，与 tauri CLI 同名（密钥不落命令行，免得进进程列表/日志）：
//!   TAURI_SIGNING_PRIVATE_KEY           私钥文件原文的 base64
//!   TAURI_SIGNING_PRIVATE_KEY_PASSWORD  口令（私钥没设口令就不用给）
//!
//! 失败时退出码非 0，便于 CI 直接拦下来。

use base64::Engine;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.first().map(String::as_str) == Some("sign") {
        let artifacts: Vec<PathBuf> = args[1..].iter().map(PathBuf::from).collect();
        if artifacts.is_empty() {
            eprintln!("用法: sigcheck sign <安装包>...");
            return ExitCode::from(2);
        }
        return match resign(&artifacts) {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                eprintln!("✗ 重签失败：{err}");
                ExitCode::FAILURE
            }
        };
    }

    if args.first().map(String::as_str) == Some("keycheck") {
        let Some(source) = args.get(1) else {
            eprintln!("用法: sigcheck keycheck <tauri.conf.json 或 .key.pub>");
            return ExitCode::from(2);
        };
        return match keycheck(source) {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                eprintln!("✗ 私钥与公钥不配套：{err}");
                ExitCode::FAILURE
            }
        };
    }

    let (pub_source, pairs) = match args.len() {
        3 => (args[0].clone(), vec![(PathBuf::from(&args[1]), PathBuf::from(&args[2]))]),
        2 => {
            let dir = PathBuf::from(&args[1]);
            (args[0].clone(), collect_pairs(&dir))
        }
        _ => {
            eprintln!("用法: sigcheck <conf 或 .key.pub> <安装包.sig> <安装包>");
            eprintln!("      sigcheck <conf 或 .key.pub> <目录>");
            eprintln!("      sigcheck sign <安装包>...");
            eprintln!("      sigcheck keycheck <conf 或 .key.pub>");
            return ExitCode::from(2);
        }
    };

    let public_key = match load_public_key(&pub_source) {
        Ok(key) => key,
        Err(err) => {
            eprintln!("公钥读取失败（{pub_source}）：{err}");
            return ExitCode::from(2);
        }
    };

    if pairs.is_empty() {
        eprintln!("没找到任何 *.sig：{}", args.last().unwrap());
        return ExitCode::from(2);
    }

    let mut failed = 0;
    for (sig_path, artifact) in &pairs {
        // .sig 资产是「minisign 文件原文的 base64」，先解一层再交给库解析。
        let sig_text = std::fs::read_to_string(sig_path).unwrap_or_default();
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(sig_text.trim())
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok());
        let signature = match decoded {
            Some(text) => minisign_verify::Signature::decode(&text),
            None => minisign_verify::Signature::decode(&sig_text),
        };
        let signature = match signature {
            Ok(signature) => signature,
            Err(err) => {
                eprintln!("✗ 签名解析失败 {}：{err:?}", sig_path.display());
                failed += 1;
                continue;
            }
        };

        let data = match std::fs::read(artifact) {
            Ok(data) => data,
            Err(err) => {
                eprintln!("✗ 读不到安装包 {}：{err}", artifact.display());
                failed += 1;
                continue;
            }
        };

        match public_key.verify(&data, &signature, true) {
            Ok(()) => println!(
                "✓ {} 与签名一致（{:.2} MB）",
                artifact.display(),
                data.len() as f64 / 1048576.0
            ),
            Err(err) => {
                eprintln!(
                    "✗ {} 与签名**不一致**（{:.2} MB，error={err:?}）—— 这个包应用内更新会验签失败",
                    artifact.display(),
                    data.len() as f64 / 1048576.0
                );
                if matches!(err, minisign_verify::Error::InvalidSignature) {
                    // key id 过了才轮到验签，这一档基本只有两种可能：产物被改过，
                    // 或 conf 里的 pubkey 与签名密钥不配套（公钥抄错一个字符，key id 照样相同）。
                    eprintln!(
                        "  ↳ key id 对得上但签名验不过：先跑 `sigcheck keycheck {}` 判是不是公钥写错了",
                        pub_source
                    );
                }
                failed += 1;
            }
        }
    }

    if failed > 0 {
        eprintln!("\n合计 {failed} 个不通过");
        ExitCode::FAILURE
    } else {
        println!("\n全部通过");
        ExitCode::SUCCESS
    }
}

/// 用同一把密钥给最终产物重签，写出 `<安装包>.sig`。
///
/// 这一步的价值是：签名覆盖的就是**最终落盘的那个文件**，签完由调用方立刻用
/// `minisign-verify` 复核，不依赖 bundler 内部的生成时序。
///
/// 注：v0.2.3/v0.2.4 曾判定「bundler 的 .sig 与产物不配对」，事后查明真因是
/// `tauri.conf.json` 里的 pubkey 被抄错了一个字符（签名本身一直是好的）。
/// 那类错现在由 `keycheck` 在构建前拦下。
fn resign(artifacts: &[PathBuf]) -> Result<(), String> {
    let secret_key = secret_key_from_env()?;

    for artifact in artifacts {
        let name = artifact
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("artifact")
            .to_string();
        let data = std::fs::read(artifact).map_err(|e| format!("读不到 {}：{e}", artifact.display()))?;
        let signature_box = minisign::sign(
            None,
            &secret_key,
            Cursor::new(data),
            Some(&format!("file:{name}")),
            Some("signature from agentmux release job"),
        )
        .map_err(|e| format!("签名失败：{e:?}"))?;

        // `.sig` 资产与 latest.json 的 signature 字段都是「签名文件原文的 base64」
        let content = base64::engine::general_purpose::STANDARD.encode(signature_box.to_string());
        let sig_path = PathBuf::from(format!("{}.sig", artifact.display()));
        std::fs::write(&sig_path, &content).map_err(|e| format!("写不到 {}：{e}", sig_path.display()))?;
        println!("已重签 {name}（签名 {} 字节）", content.len());
    }
    Ok(())
}

/// 目录下每一对 `xxx.sig` / `xxx`。**递归找**：bundle 里 nsis 与 msi 是并列子目录。
fn collect_pairs(dir: &Path) -> Vec<(PathBuf, PathBuf)> {
    let mut pairs = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().and_then(|ext| ext.to_str()) != Some("sig") {
                continue;
            }
            let Some(artifact) = path.file_stem().map(PathBuf::from) else {
                continue;
            };
            // file_stem 去掉了 .sig；安装包本体就是同目录下的同名文件
            let artifact = path.with_file_name(artifact);
            pairs.push((path, artifact));
        }
    }
    pairs.sort();
    pairs
}

/// 私钥（环境变量，与 tauri CLI 同名）。密钥不落命令行，免得进进程列表/日志。
fn secret_key_from_env() -> Result<minisign::SecretKey, String> {
    let key_b64 = std::env::var("TAURI_SIGNING_PRIVATE_KEY")
        .map_err(|_| "没有 TAURI_SIGNING_PRIVATE_KEY 环境变量".to_string())?;
    let password = std::env::var("TAURI_SIGNING_PRIVATE_KEY_PASSWORD").ok();

    // 与 tauri CLI 一致：环境变量里放的是私钥**文件原文的 base64**
    let key_text = String::from_utf8(
        base64::engine::general_purpose::STANDARD
            .decode(key_b64.trim())
            .map_err(|e| format!("私钥不是合法 base64：{e}"))?,
    )
    .map_err(|e| format!("私钥内容不是 UTF-8：{e}"))?;

    minisign::SecretKeyBox::from_string(&key_text)
        .map_err(|e| format!("私钥格式不对：{e:?}"))?
        .into_secret_key(password)
        .map_err(|e| format!("私钥解不开（口令错？）：{e:?}"))
}

/// 私钥与这份公钥是否**配套**，不配套时指出差在哪。
///
/// 按说去比 key id 就够了——但 key id 只有 8 字节，公钥正文抄错一个字符时它不变，
/// CLI 的警告与实际校验都会放行，一直要到用户点「立即更新」才炸。所以这里从私钥
/// 反推公钥（minisign 私钥里存着配套公钥），做逐字节比对。
fn keycheck(source: &str) -> Result<(), String> {
    let secret_key = secret_key_from_env()?;
    let want = minisign::PublicKey::from_secret_key(&secret_key)
        .map_err(|e| format!("私钥里没有公钥，无法反推：{e:?}"))?
        .to_bytes();

    let text = load_public_key_text(source)?;
    let got = minisign::PublicKeyBox::from_string(&text)
        .map_err(|e| format!("公钥格式不对：{e:?}"))?
        .into_public_key()
        .map_err(|e| format!("公钥解析失败：{e:?}"))?
        .to_bytes();

    let key_id = hex(&want[2..10]);
    if want[2..10] != got[2..10] {
        return Err(format!(
            "{source} 里是**另一把**密钥的公钥：私钥 key id {key_id}，这份 key id {}。请换回生成密钥时保存的 .key.pub",
            hex(&got[2..10])
        ));
    }
    if want != got {
        let at = want.iter().zip(&got).position(|(a, b)| a != b).unwrap_or(0);
        return Err(format!(
            "key id 相同（{key_id}）但公钥正文不一致：私钥反推出的第 {} 个字节是 0x{:02x}，{source} 里是 0x{:02x}。\
             多半是当初粘贴时改错了一个字符——请从保存的 .key.pub 整体重抄，不要手打",
            at - 10 + 1,
            want[at],
            got[at]
        ));
    }

    println!("✓ {source} 里的公钥与私钥配套（key id {key_id}）");
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn load_public_key(source: &str) -> Result<minisign_verify::PublicKey, String> {
    let text = load_public_key_text(source)?;
    // 文件原文是两行：注释 + box；取第二行的 box
    let box_line = text
        .lines()
        .nth(1)
        .ok_or("公钥内容不像 minisign 公钥文件")?
        .trim()
        .to_string();
    minisign_verify::PublicKey::from_base64(&box_line).map_err(|e| format!("{e:?}"))
}

/// 取公钥文件原文（两行：注释 + box）。配置里存的是它的 base64。
fn load_public_key_text(source: &str) -> Result<String, String> {
    let raw = std::fs::read_to_string(source).map_err(|e| e.to_string())?;
    // 两种形态：tauri.conf.json 里的 plugins.updater.pubkey，或直接的 .key.pub 文件
    let encoded = if source.ends_with(".pub") {
        raw.trim().to_string()
    } else {
        let json: serde_json::Value = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
        json["plugins"]["updater"]["pubkey"]
            .as_str()
            .ok_or("配置里没有 plugins.updater.pubkey")?
            .to_string()
    };

    let decoded = base64::engine::general_purpose::STANDARD
        .decode(encoded.trim())
        .map_err(|e| e.to_string())?;
    String::from_utf8(decoded).map_err(|e| e.to_string())
}