use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tracing::info;

#[derive(Serialize)]
struct TokenRequest<'a> {
    device_id: &'a str,
    device_secret: &'a str,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    tenant_id: String,
}

/// auth-worker `POST /device/token` で device credential を短命 device JWT に交換する
/// (Phase 2 / 案B)。Google 不要・無人。返り値は (device JWT, tenant_id)。
pub async fn get_device_jwt(
    client: &reqwest::Client,
    auth_url: &str,
    device_id: &str,
    device_secret: &str,
) -> Result<(String, String)> {
    let url = format!("{}/device/token", auth_url.trim_end_matches('/'));
    let resp = client
        .post(&url)
        .json(&TokenRequest {
            device_id,
            device_secret,
        })
        .send()
        .await
        .context("device token request")?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("device token failed (HTTP {}): {}", status, body.trim());
    }

    let tok: TokenResponse = resp.json().await.context("parsing device token response")?;
    info!("Device JWT acquired, tenant_id={}", tok.tenant_id);
    Ok((tok.access_token, tok.tenant_id))
}
