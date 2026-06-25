use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use tracing::info;

/// Read the last run timestamp from the last line of the state file.
pub fn read_last_run(path: &Path) -> Result<SystemTime> {
    if !path.exists() {
        info!(
            "No state file found at {}, will upload all files",
            path.display()
        );
        return Ok(SystemTime::UNIX_EPOCH);
    }

    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Reading state file {}", path.display()))?;

    let last_line = content.lines().filter(|l| !l.trim().is_empty()).last();

    let last_line = match last_line {
        Some(l) => l,
        None => {
            info!("State file is empty, will upload all files");
            return Ok(SystemTime::UNIX_EPOCH);
        }
    };

    // First field is the start timestamp
    let start_ts = last_line.split('\t').next().unwrap_or("").trim();

    let dt = DateTime::parse_from_rfc3339(start_ts)
        .with_context(|| format!("Parsing last run timestamp: {:?}", start_ts))?;

    let time = SystemTime::from(dt);
    info!("Last run: {}", dt.to_rfc3339());
    Ok(time)
}

pub struct RunRecord {
    pub start: SystemTime,
    pub end: SystemTime,
    pub files_found: usize,
    pub uploaded: usize,
    pub failed: usize,
    pub dry_run: bool,
}

/// Append a run record as a tab-separated line to the state file.
/// Format: start_ts\tend_ts\tfiles_found\tuploaded\tfailed\tstatus
pub fn append_run_record(path: &Path, record: &RunRecord) -> Result<()> {
    let start_dt: DateTime<Utc> = record.start.into();
    let end_dt: DateTime<Utc> = record.end.into();

    let status = if record.dry_run { "dry-run" } else { "ok" };

    let line = format!(
        "{}\t{}\t{}\t{}\t{}\t{}\n",
        start_dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        end_dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        record.files_found,
        record.uploaded,
        record.failed,
        status,
    );

    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Creating directory {}", parent.display()))?;
        }
    }

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("Opening state file {}", path.display()))?;

    file.write_all(line.as_bytes())
        .with_context(|| format!("Writing to state file {}", path.display()))?;

    info!(
        "Run recorded: start={} end={} found={} uploaded={} failed={} status={}",
        start_dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        end_dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        record.files_found,
        record.uploaded,
        record.failed,
        status,
    );

    Ok(())
}

/// Returns the path for the failed files list (alongside state_file).
pub fn failed_list_path(state_file: &Path) -> PathBuf {
    state_file.with_file_name("failed_files.txt")
}

/// Load the list of previously failed file ids (local path or SMB-relative path).
pub fn load_failed_list(path: &Path) -> Result<Vec<String>> {
    if !path.exists() {
        return Ok(vec![]);
    }
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Reading failed list {}", path.display()))?;
    let ids = content
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .map(|l| l.to_string())
        .collect();
    Ok(ids)
}

/// Save the list of failed file ids, overwriting the previous list.
/// Passing an empty slice deletes the file.
pub fn save_failed_list(path: &Path, failed: &[String]) -> Result<()> {
    if failed.is_empty() {
        if path.exists() {
            std::fs::remove_file(path)
                .with_context(|| format!("Removing failed list {}", path.display()))?;
            info!("All retries succeeded, removed {}", path.display());
        }
        return Ok(());
    }

    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Creating directory {}", parent.display()))?;
        }
    }

    let content = failed.join("\n") + "\n";

    std::fs::write(path, content)
        .with_context(|| format!("Writing failed list {}", path.display()))?;

    info!(
        "{} file(s) remain in retry list {}",
        failed.len(),
        path.display()
    );
    Ok(())
}
