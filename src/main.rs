mod auth;
mod cli;
mod google_auth;
mod scanner;
#[cfg(windows)]
mod smb;
#[cfg(not(windows))]
mod smb_fs;
mod source;
mod state;
mod uploader;

use anyhow::Result;
use clap::Parser;
use std::collections::HashSet;
use std::time::SystemTime;
use tracing::{info, warn};

use crate::source::{file_name_of, FileSource};

#[tokio::main]
async fn main() -> Result<()> {
    let config = cli::Config::parse();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_new(&config.log_level)
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let scan_start = SystemTime::now();

    let mut source = FileSource::open(&config).await?;
    let result = run(&config, &mut source, scan_start).await;
    source.close().await;
    result
}

async fn run(config: &cli::Config, source: &mut FileSource, scan_start: SystemTime) -> Result<()> {
    let failed_list_path = state::failed_list_path(&config.state_file);

    // 1. Load previously failed files (retry candidates), pruning ones that no longer exist.
    let prev_failed = state::load_failed_list(&failed_list_path)?;
    let mut retry_candidates: Vec<String> = Vec::new();
    if !prev_failed.is_empty() {
        info!("{} file(s) pending retry from previous run", prev_failed.len());
        for id in prev_failed {
            if source.exists(&id).await {
                retry_candidates.push(id);
            }
        }
    }

    // 2. Resolve "since" threshold
    let since: SystemTime = if let Some(dt) = config.since {
        info!("Using --since override: {}", dt.to_rfc3339());
        SystemTime::from(dt)
    } else {
        state::read_last_run(&config.state_file)?
    };

    let changed = scanner::find_changed_files(source, since).await?;

    // 3. Merge: changed files + retries, deduplicated
    let retry_set: HashSet<String> = retry_candidates.into_iter().collect();
    let mut all_ids: Vec<String> = changed.into_iter().map(|e| e.id).collect();
    for id in &retry_set {
        if !all_ids.contains(id) {
            info!("Adding retry: {}", id);
            all_ids.push(id.clone());
        }
    }

    let files_found = all_ids.len();
    info!(
        "Found {} file(s) to process ({} new/changed + {} retries)",
        files_found,
        files_found - retry_set.len().min(files_found),
        retry_set.len().min(files_found),
    );

    let mut uploaded = 0usize;
    let mut new_failed: Vec<String> = Vec::new();

    if files_found == 0 {
        info!("No files to process");
    } else if config.dry_run {
        info!("Dry run mode: skipping uploads");
        for id in &all_ids {
            info!("  Would upload: {}", id);
        }
    } else {
        let client = uploader::build_client()?;

        // Google Device Flow → rust-alc-api で認証
        let id_token = google_auth::device_flow_get_id_token(
            &client,
            &config.google_client_id,
            &config.google_client_secret,
        )
        .await?;

        let auth_url = format!("{}/api/auth/google", config.alc_api_url.trim_end_matches('/'));
        let (token, tenant_id) = auth::login_with_google(&client, &auth_url, &id_token).await?;
        info!("Authenticated: tenant_id={}", tenant_id);

        let upload_url = format!("{}/api/files", config.alc_api_url.trim_end_matches('/'));

        for (i, id) in all_ids.iter().enumerate() {
            info!("Uploading {}/{}: {}", i + 1, files_found, id);
            let filename = file_name_of(id);
            match source.read(id).await {
                Ok(bytes) => {
                    match uploader::upload_bytes(&client, &upload_url, &filename, &bytes, &token)
                        .await
                    {
                        Ok(()) => uploaded += 1,
                        Err(e) => {
                            warn!("Failed: {}: {:#}", id, e);
                            new_failed.push(id.clone());
                        }
                    }
                }
                Err(e) => {
                    warn!("Failed to read {}: {:#}", id, e);
                    new_failed.push(id.clone());
                }
            }
        }

        if !new_failed.is_empty() {
            warn!("{} file(s) failed; will retry next run", new_failed.len());
        }
    }

    let failed_count = new_failed.len();

    // 4. Save updated failed list
    state::save_failed_list(&failed_list_path, &new_failed)?;

    // 5. Record run
    state::append_run_record(
        &config.state_file,
        &state::RunRecord {
            start: scan_start,
            end: SystemTime::now(),
            files_found,
            uploaded,
            failed: failed_count,
            dry_run: config.dry_run,
        },
    )?;

    Ok(())
}
