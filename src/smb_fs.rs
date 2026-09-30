//! Linux 向け pure-Rust SMB 直アクセス (`smb2` crate)。
//!
//! cifs マウントを使わず、NTLM (`--smb-user` / `--smb-pass` / `--smb-domain`) で
//! 共有に接続し、共有ルートからの相対パスで再帰列挙 / read / stat する。
//!
//! `smb2` は no C deps / no FFI で musl static cross-compile が崩れない (Issue #1 の選定)。
//! 認証方式 (SMB dialect / NTLM) が実機で合わない場合は `smb` crate (sspi ベース) へ
//! 切替える方針。crate を差し替えてもこの module の外側 (FileSource) は変えない。

use anyhow::{Context, Result};
use std::time::Duration;
use tracing::{info, warn};

use crate::cli::Config;
use crate::source::Entry;

pub struct SmbFs {
    client: smb2::SmbClient,
    tree: smb2::Tree,
    /// 共有ルートからの走査開始相対パス (例: `新車検証`)。ルート直下なら空文字列。
    root: String,
}

impl SmbFs {
    /// 共有に接続し、tree connect する。
    pub async fn connect(config: &Config) -> Result<Self> {
        let host = config.smb_host.trim();
        // host に明示ポートが無ければ SMB 既定の 445 を付ける。
        let addr = if host.contains(':') {
            host.to_string()
        } else {
            format!("{}:445", host)
        };

        let user = config.smb_user.clone().unwrap_or_default();
        let pass = config.smb_pass.clone().unwrap_or_default();

        info!(
            "Connecting to SMB //{}/{} as {}",
            addr,
            config.smb_share,
            if user.is_empty() { "guest" } else { &user }
        );

        let cfg = smb2::ClientConfig {
            addr: addr.clone(),
            timeout: Duration::from_secs(30),
            username: user,
            password: pass,
            domain: config.smb_domain.clone(),
            // 無人 daemon 向け: 接続断時に自動再接続する。
            auto_reconnect: true,
            compression: true,
            dfs_enabled: true,
            dfs_target_overrides: Default::default(),
        };

        let mut client = smb2::SmbClient::connect(cfg)
            .await
            .with_context(|| format!("SMB connect to {}", addr))?;

        let tree = client
            .connect_share(&config.smb_share)
            .await
            .with_context(|| format!("SMB connect_share '{}'", config.smb_share))?;

        let root = config.smb_path.trim_matches(['/', '\\']).to_string();
        info!("SMB connected, scanning under '{}'", root);

        Ok(SmbFs { client, tree, root })
    }

    /// 共有ルート配下を再帰列挙し、ファイルの相対パス + mtime を返す。
    pub async fn list_files(&mut self) -> Result<Vec<Entry>> {
        let mut out = Vec::new();
        let mut stack = vec![self.root.clone()];

        while let Some(dir) = stack.pop() {
            let entries = self
                .client
                .list_directory(&mut self.tree, &dir)
                .await
                .with_context(|| format!("Listing SMB dir '{}'", dir))?;

            for e in entries {
                // SMB の query_directory は "." / ".." を返すので除外する。
                if e.name == "." || e.name == ".." {
                    continue;
                }
                let path = if dir.is_empty() {
                    e.name.clone()
                } else {
                    format!("{}/{}", dir, e.name)
                };

                if e.is_directory {
                    stack.push(path);
                } else {
                    match e.modified.to_system_time() {
                        Some(mtime) => out.push(Entry { id: path, mtime }),
                        None => warn!("SMB entry has invalid mtime, skipping: {}", path),
                    }
                }
            }
        }

        Ok(out)
    }

    /// `id` (共有相対パス) のファイルを全読みする。
    pub async fn read(&mut self, id: &str) -> Result<Vec<u8>> {
        self.client
            .read_file(&mut self.tree, id)
            .await
            .with_context(|| format!("Reading SMB file '{}'", id))
    }

    /// `id` が存在するか。stat 成功なら true、失敗 (不在 / エラー) なら false。
    pub async fn exists(&mut self, id: &str) -> bool {
        self.client.stat(&mut self.tree, id).await.is_ok()
    }

    /// 共有を切断する。エラーは log のみ。
    pub async fn close(&mut self) {
        if let Err(e) = self.client.disconnect_share(&self.tree).await {
            warn!("Failed to disconnect SMB share: {:#}", e);
        }
    }
}
