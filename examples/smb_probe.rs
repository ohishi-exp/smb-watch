//! SMB 疎通 probe (Issue #1 Phase 1 の最優先・前提条件)。
//!
//! 対象 SMB サーバに `smb2` crate で NTLM 接続 → 共有ルート配下を再帰列挙 →
//! 1 ファイル read が通ることを **実機 (SMB と同一 LAN 内)** で確認する。
//! ここで SMB dialect / 認証方式が合わなければ crate を `smb` (sspi ベース) に切替える。
//!
//! CCoW / LAN 外からは対象サーバ (<smb-host>) に到達できないため、手元の
//! LAN 内マシンで実行すること。
//!
//! 使い方:
//! ```sh
//! SMB_USER=xxx SMB_PASS=yyy \
//!   cargo run --example smb_probe -- \
//!   --smb-host <smb-host> --smb-share <共有> --smb-path <パス>
//! # 任意: --smb-domain WORKGROUP  RUST_LOG=smb2=debug で wire ログ
//! ```

#[cfg(windows)]
fn main() {
    eprintln!("smb_probe is Linux-only (Windows は net use マウントを使う)。");
}

#[cfg(not(windows))]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::time::Duration;

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    // --- 引数 / env ---
    let args: Vec<String> = std::env::args().collect();
    let arg = |name: &str, default: Option<&str>| -> String {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
            .or_else(|| default.map(str::to_string))
            .unwrap_or_else(|| {
                eprintln!("{} は必須です (既定値なし)。", name);
                std::process::exit(2);
            })
    };

    let host = arg("--smb-host", None);
    let share = arg("--smb-share", None);
    let path = arg("--smb-path", None);
    let domain = arg("--smb-domain", Some(""));
    let user = std::env::var("SMB_USER").unwrap_or_else(|_| {
        eprintln!("SMB_USER / SMB_PASS env vars が必要です。");
        std::process::exit(2);
    });
    let pass = std::env::var("SMB_PASS").unwrap_or_else(|_| {
        eprintln!("SMB_PASS env var が必要です。");
        std::process::exit(2);
    });

    let addr = if host.contains(':') {
        host.clone()
    } else {
        format!("{}:445", host)
    };
    let root = path.trim_matches(['/', '\\']).to_string();

    println!("=== SMB probe ===");
    println!("addr   : {}", addr);
    println!("share  : {}", share);
    println!("path   : '{}'", root);
    println!("user   : {}", user);
    println!("domain : '{}'", domain);
    println!();

    // --- 1. 接続 ---
    let cfg = smb2::ClientConfig {
        addr: addr.clone(),
        timeout: Duration::from_secs(30),
        username: user,
        password: pass,
        domain,
        auto_reconnect: true,
        compression: true,
        dfs_enabled: true,
        dfs_target_overrides: Default::default(),
    };
    let mut client = smb2::SmbClient::connect(cfg).await?;
    println!("[ok] connected + authenticated (NTLM)");

    let mut tree = client.connect_share(&share).await?;
    println!("[ok] tree connect: {}", share);

    // --- 2. 再帰列挙 ---
    let mut files: Vec<(String, u64)> = Vec::new();
    let mut dirs = 0usize;
    let mut stack = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        let entries = client.list_directory(&mut tree, &dir).await?;
        for e in entries {
            if e.name == "." || e.name == ".." {
                continue;
            }
            let p = if dir.is_empty() {
                e.name.clone()
            } else {
                format!("{}/{}", dir, e.name)
            };
            if e.is_directory {
                dirs += 1;
                stack.push(p);
            } else {
                files.push((p, e.size));
            }
        }
    }
    println!("[ok] 列挙: {} dir / {} file", dirs, files.len());
    for (p, size) in files.iter().take(20) {
        println!("       {:>12} bytes  {}", size, p);
    }
    if files.len() > 20 {
        println!("       ... ({} more)", files.len() - 20);
    }

    // --- 3. 1 ファイル read ---
    if let Some((p, size)) = files.first() {
        let data = client.read_file(&mut tree, p).await?;
        println!("[ok] read: {} ({} bytes, expected {})", p, data.len(), size);
        if data.len() as u64 != *size {
            println!("[warn] read サイズが stat と不一致 (truncated read?)");
        }
    } else {
        println!("[warn] ファイルが 0 件。read 検証はスキップ。");
    }

    client.disconnect_share(&tree).await?;
    println!();
    println!("=== probe 成功: smb2 crate で接続/列挙/read が通りました ===");
    Ok(())
}
