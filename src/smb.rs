use anyhow::{anyhow, Context, Result};
use std::process::Command;
use tracing::{info, warn};

use crate::cli::Config;

pub struct SmbMount {
    pub drive_letter: String,
}

impl SmbMount {
    /// Mount the SMB share. Returns SmbMount that can be used to unmount.
    pub fn mount(config: &Config) -> Result<Self> {
        let drive = &config.drive_letter;
        let unc = format!("\\\\{}\\{}", config.smb_host, config.smb_share);

        // Check if already mounted
        if let Ok(current_unc) = query_drive(drive) {
            if current_unc.trim().eq_ignore_ascii_case(unc.trim()) {
                info!("Drive {} already mounted to {}, reusing", drive, unc);
                return Ok(SmbMount {
                    drive_letter: drive.clone(),
                });
            } else {
                warn!(
                    "Drive {} is mapped to {}, unmounting first",
                    drive, current_unc
                );
                unmount_drive(drive)?;
            }
        }

        // Build user argument: DOMAIN\user or just user
        let smb_user = config.smb_user.as_deref().unwrap_or("");
        let smb_pass = config.smb_pass.as_deref().unwrap_or("");
        let user_arg = if config.smb_domain.is_empty() {
            smb_user.to_string()
        } else {
            format!("{}\\{}", config.smb_domain, smb_user)
        };

        info!("Mounting {} -> {}", drive, unc);

        let output = Command::new("net")
            .args([
                "use",
                drive,
                &unc,
                smb_pass,
                &format!("/user:{}", user_arg),
                "/persistent:no",
            ])
            .output()
            .context("Failed to execute net use")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            return Err(anyhow!(
                "net use failed (exit {:?}):\nstdout: {}\nstderr: {}",
                output.status.code(),
                stdout.trim(),
                stderr.trim()
            ));
        }

        info!("SMB share mounted at {}", drive);
        Ok(SmbMount {
            drive_letter: drive.clone(),
        })
    }

    /// Unmount the SMB share.
    pub fn unmount(&self) -> Result<()> {
        unmount_drive(&self.drive_letter)
    }
}

fn query_drive(drive: &str) -> Result<String> {
    let output = Command::new("net")
        .args(["use", drive])
        .output()
        .context("Failed to execute net use query")?;

    if !output.status.success() {
        return Err(anyhow!("Drive {} is not mapped", drive));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    // Parse the UNC path from "net use Z:" output
    // Output contains lines like: "Remote name  \\server\share"
    for line in stdout.lines() {
        let line = line.trim();
        if line.starts_with("Remote name") || line.starts_with("リモート名") {
            if let Some(unc) = line.split_whitespace().last() {
                return Ok(unc.to_string());
            }
        }
    }

    Err(anyhow!("Could not parse UNC path from net use output"))
}

fn unmount_drive(drive: &str) -> Result<()> {
    info!("Unmounting {}", drive);

    let output = Command::new("net")
        .args(["use", drive, "/delete", "/yes"])
        .output()
        .context("Failed to execute net use /delete")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        return Err(anyhow!(
            "net use /delete failed (exit {:?}):\nstdout: {}\nstderr: {}",
            output.status.code(),
            stdout.trim(),
            stderr.trim()
        ));
    }

    info!("SMB share unmounted");
    Ok(())
}
