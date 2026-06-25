use anyhow::Result;
use std::time::SystemTime;
use tracing::info;

use crate::source::{Entry, FileSource};

/// ソースを再帰列挙し、`since` より新しい (mtime > since) ファイルだけを返す。
pub async fn find_changed_files(source: &mut FileSource, since: SystemTime) -> Result<Vec<Entry>> {
    let mut all = source.list_files().await?;
    all.retain(|e| e.mtime > since);
    all.sort_by(|a, b| a.id.cmp(&b.id));

    for e in &all {
        info!("Changed: {} (mtime: {:?})", e.id, e.mtime);
    }

    Ok(all)
}
