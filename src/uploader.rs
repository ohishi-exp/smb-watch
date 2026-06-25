use anyhow::{Context, Result};
use serde::Deserialize;
use std::time::Duration;
use tracing::{info, warn};

#[derive(Deserialize, Debug)]
pub struct UploadResponse {
    pub uuid: String,
}

pub fn build_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()
        .context("Building HTTP client")
}

/// 読み込み済みバイト列を carins `/api/device-upload` に multipart で送る (Phase 2)。
/// `token` は auth-worker 発行の device JWT (Bearer)。carins が introspect 検証し
/// 検証済 tenant_id を X-Tenant-ID として rust-alc-api に注入する。
/// (ファイルの read は呼び出し側の `FileSource` が担い、ここは FS / SMB に依存しない)
pub async fn upload_bytes(
    client: &reqwest::Client,
    url: &str,
    filename: &str,
    bytes: &[u8],
    token: &str,
) -> Result<()> {
    let filename = filename.to_string();

    let mime = mime_guess::from_path(&filename)
        .first_or_octet_stream()
        .to_string();

    let part = reqwest::multipart::Part::bytes(bytes.to_vec())
        .file_name(filename.clone())
        .mime_str(&mime)
        .with_context(|| format!("building multipart part for {}", filename))?;
    let form = reqwest::multipart::Form::new().part("file", part);

    let response = client
        .post(url)
        .bearer_auth(token)
        .multipart(form)
        .send()
        .await
        .with_context(|| format!("POST to {}", url))?;

    let status = response.status();

    if !status.is_success() {
        let body = response
            .text()
            .await
            .unwrap_or_else(|_| "(unreadable body)".to_string());
        return Err(anyhow::anyhow!(
            "Upload failed with HTTP {}: {}",
            status,
            body.trim()
        ));
    }

    match response.json::<UploadResponse>().await {
        Ok(resp) => {
            info!("Uploaded {} -> uuid: {}", filename, resp.uuid);
        }
        Err(e) => {
            warn!("Uploaded {} but could not parse response: {}", filename, e);
        }
    }

    Ok(())
}
