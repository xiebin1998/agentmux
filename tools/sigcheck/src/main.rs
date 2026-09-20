//! 校验 Tauri 更新包的签名是否**真的**覆盖了那个安装包。
//!
//! 为什么需要它：应用内「立即更新」在验签失败时才报错，而那时候版本已经发出去了。
//! 这个工具把同一件事提前到构建期做完 —— 用 `minisign-verify`（应用运行时同一套库）
//! 校验 `xxx.exe` 与 `xxx.exe.sig` 是否配对。
//!
//! 用法：
//!   sigcheck <tauri.conf.json 或 .key.pub> <安装包.sig> <安装包>      校单个
//!   sigcheck <tauri.conf.json 或 .key.pub> <目录>                校目录下所有 *.sig
//!
//! 失败时退出码非 0，并打印每一对的结论，便于 CI 直接拦下来。

use base64::Engine;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (pub_source, pairs) = match args.len() {
        3 => (args[0].clone(), vec![(PathBuf::from(&args[1]), PathBuf::from(&args[2]))]),
        2 => {
            let dir = PathBuf::from(&args[1]);
            (args[0].clone(), collect_pairs(&dir))
        }
        _ => {
            eprintln!("用法: sigcheck <conf 或 .key.pub> <安装包.sig> <安装包>");
            eprintln!("      sigcheck <conf 或 .key.pub> <目录>");
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

fn load_public_key(source: &str) -> Result<minisign_verify::PublicKey, String> {
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
    let text = String::from_utf8(decoded).map_err(|e| e.to_string())?;
    // 文件原文是两行：注释 + box；取第二行的 box
    let box_line = text
        .lines()
        .nth(1)
        .ok_or("公钥内容不像 minisign 公钥文件")?
        .trim()
        .to_string();
    minisign_verify::PublicKey::from_base64(&box_line).map_err(|e| format!("{e:?}"))
}