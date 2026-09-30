//! ファイルソースの抽象化。
//!
//! ローカル FS (`--local-path` / Windows の `net use` マウント先) と Linux の
//! pure-Rust SMB 直アクセスを、scanner (mtime 比較) と uploader (read → upload) が
//! 同一 interface で扱えるようにする。
//!
//! `Entry.id` は「scanner が見つけ、uploader が read し、failed_files.txt に永続化する」
//! 際の識別子。ローカルでは絶対パス文字列、SMB では共有ルートからの相対パス
//! (forward-slash 区切り)。どちらも `read` / `exists` にそのまま渡せる。

use anyhow::{Context, Result};
use std::path::PathBuf;
use std::time::SystemTime;

use crate::cli::Config;

/// 走査で見つかった 1 ファイルのメタデータ。
#[derive(Debug, Clone)]
pub struct Entry {
    /// read / exists / 永続化に使う識別子 (local: 絶対パス, smb: 共有相対パス)。
    pub id: String,
    /// 最終更新時刻 (mtime 比較に使う)。
    pub mtime: SystemTime,
}

/// `Entry.id` からファイル名部分 (アップロード時の `filename`) を取り出す。
pub fn file_name_of(id: &str) -> String {
    id.rsplit(['/', '\\'])
        .find(|s| !s.is_empty())
        .unwrap_or(id)
        .to_string()
}

/// ローカル FS をソースにする実装。
/// Windows の `net use` マウント時は mount guard を保持し、close で unmount する。
pub struct LocalFs {
    root: PathBuf,
    #[cfg(windows)]
    mount: Option<crate::smb::SmbMount>,
}

impl LocalFs {
    pub fn local(root: PathBuf) -> Self {
        LocalFs {
            root,
            #[cfg(windows)]
            mount: None,
        }
    }

    #[cfg(windows)]
    pub fn mounted(root: PathBuf, mount: crate::smb::SmbMount) -> Self {
        LocalFs {
            root,
            mount: Some(mount),
        }
    }

    fn list_files(&self) -> Result<Vec<Entry>> {
        let mut out = Vec::new();
        for entry in walkdir::WalkDir::new(&self.root).follow_links(false) {
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    tracing::warn!("Skipping unreadable entry: {}", e);
                    continue;
                }
            };
            if !entry.file_type().is_file() {
                continue;
            }
            let mtime = match entry.metadata().ok().and_then(|m| m.modified().ok()) {
                Some(t) => t,
                None => {
                    tracing::warn!("Cannot read mtime for {}", entry.path().display());
                    continue;
                }
            };
            out.push(Entry {
                id: entry.path().to_string_lossy().into_owned(),
                mtime,
            });
        }
        Ok(out)
    }

    async fn read(&self, id: &str) -> Result<Vec<u8>> {
        tokio::fs::read(id)
            .await
            .with_context(|| format!("Reading file {}", id))
    }

    fn exists(&self, id: &str) -> bool {
        std::path::Path::new(id).is_file()
    }

    #[cfg(windows)]
    fn close(&self) {
        if let Some(mount) = &self.mount {
            if let Err(e) = mount.unmount() {
                tracing::warn!("Failed to unmount SMB share: {:#}", e);
            }
        }
    }
}

/// scanner / uploader が扱うファイルソース。
pub enum FileSource {
    Local(LocalFs),
    #[cfg(not(windows))]
    Smb(crate::smb_fs::SmbFs),
}

impl FileSource {
    /// Config に応じてソースを開く (SMB は接続 / Windows はマウント)。
    pub async fn open(config: &Config) -> Result<Self> {
        if let Some(local) = &config.local_path {
            tracing::info!("Local mode: monitoring {}", local.display());
            return Ok(FileSource::Local(LocalFs::local(local.clone())));
        }

        if config.smb_user.is_none() || config.smb_pass.is_none() {
            anyhow::bail!(
                "--smb-user and --smb-pass (or SMB_USER/SMB_PASS env vars) are required for SMB mode. \
                 Use --local-path for local mode."
            );
        }

        #[cfg(windows)]
        {
            let mount = crate::smb::SmbMount::mount(config)?;
            // Windows 固有のパス連結 (ドライブレター + バックスラッシュ) はここに閉じ込める。
            let root = PathBuf::from(format!("{}\\{}", mount.drive_letter, config.smb_path));
            Ok(FileSource::Local(LocalFs::mounted(root, mount)))
        }

        #[cfg(not(windows))]
        {
            let smb = crate::smb_fs::SmbFs::connect(config).await?;
            Ok(FileSource::Smb(smb))
        }
    }

    /// ルート配下の全ファイルを再帰列挙し、各エントリの id / mtime を返す。
    pub async fn list_files(&mut self) -> Result<Vec<Entry>> {
        match self {
            FileSource::Local(l) => l.list_files(),
            #[cfg(not(windows))]
            FileSource::Smb(s) => s.list_files().await,
        }
    }

    /// `id` のファイル内容を全読みする。
    pub async fn read(&mut self, id: &str) -> Result<Vec<u8>> {
        match self {
            FileSource::Local(l) => l.read(id).await,
            #[cfg(not(windows))]
            FileSource::Smb(s) => s.read(id).await,
        }
    }

    /// `id` がまだ存在するか (retry リストの剪定用)。
    pub async fn exists(&mut self, id: &str) -> bool {
        match self {
            FileSource::Local(l) => l.exists(id),
            #[cfg(not(windows))]
            FileSource::Smb(s) => s.exists(id).await,
        }
    }

    /// ソースを閉じる (Windows: unmount / Linux SMB: disconnect)。エラーは log のみ。
    pub async fn close(&mut self) {
        match self {
            #[cfg(windows)]
            FileSource::Local(l) => l.close(),
            #[cfg(not(windows))]
            FileSource::Local(_) => {}
            #[cfg(not(windows))]
            FileSource::Smb(s) => s.close().await,
        }
    }
}
