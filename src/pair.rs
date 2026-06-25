//! `smb-watch pair` — headless device pairing (Phase 2.5 / Issue #1)。
//!
//! ブラウザを持たない box (ohishi-data) が auth-worker から device credential を
//! 発行してもらうための RFC 8628 風フロー (auth-worker 側は #298)。
//!
//!   1. `POST {auth_url}/device/pair/start` で device_code(秘密) + user_code(短い) +
//!      承認 URL を得て、operator に URL と user_code を端末表示する。
//!   2. operator がスマホ等で承認 URL を開き auth-worker にログイン → user_code を承認。
//!   3. box は `POST {auth_url}/device/pair/token` を interval 間隔で poll。approved に
//!      なったら device_id + device_secret を受け取り、env ファイルに保管 (or stdout 表示)。
//!
//! device_secret は 1 度きりしか返らないので、取得した瞬間に保存する。

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tracing::{info, warn};

use crate::cli::PairArgs;

#[derive(Serialize)]
struct StartRequest<'a> {
    label: &'a str,
}

#[derive(Deserialize, Debug)]
pub struct StartResponse {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri_complete: String,
    pub expires_in: u64,
    pub interval: u64,
}

#[derive(Serialize)]
struct TokenRequest<'a> {
    device_code: &'a str,
}

#[derive(Deserialize, Debug)]
struct PollResponse {
    status: String,
    device_id: Option<String>,
    device_secret: Option<String>,
    tenant_id: Option<String>,
    label: Option<String>,
}

/// 発行された device credential (approved 時に 1 度だけ返る)。
#[derive(Debug, PartialEq, Eq)]
pub struct Credential {
    pub device_id: String,
    pub device_secret: String,
    pub tenant_id: String,
    pub label: String,
}

/// poll 1 回分の結果。
#[derive(Debug, PartialEq, Eq)]
pub enum PollOutcome {
    Pending,
    Approved(Credential),
    /// 承認前に有効期限が切れた / device_code が見つからない。
    Expired,
    /// 既に credential を受領済み (再 poll)。
    Consumed,
}

/// poll レスポンス JSON を `PollOutcome` に変換する純粋関数 (テスト可能)。
fn parse_poll_outcome(body: &str) -> Result<PollOutcome> {
    let r: PollResponse = serde_json::from_str(body).context("parsing pair token response")?;
    match r.status.as_str() {
        "approved" => {
            let device_id = r
                .device_id
                .filter(|s| !s.is_empty())
                .context("approved response missing device_id")?;
            let device_secret = r
                .device_secret
                .filter(|s| !s.is_empty())
                .context("approved response missing device_secret")?;
            Ok(PollOutcome::Approved(Credential {
                device_id,
                device_secret,
                tenant_id: r.tenant_id.unwrap_or_default(),
                label: r.label.unwrap_or_default(),
            }))
        }
        "pending" => Ok(PollOutcome::Pending),
        "consumed" => Ok(PollOutcome::Consumed),
        "expired" => Ok(PollOutcome::Expired),
        other => anyhow::bail!("unexpected pair token status: {}", other),
    }
}

/// `POST {auth_url}/device/pair/start` で pairing を開始する。
async fn start_pairing(
    client: &reqwest::Client,
    auth_url: &str,
    label: &str,
) -> Result<StartResponse> {
    let url = format!("{}/device/pair/start", auth_url.trim_end_matches('/'));
    let resp = client
        .post(&url)
        .json(&StartRequest { label })
        .send()
        .await
        .context("pair start request")?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("pair start failed (HTTP {}): {}", status, body.trim());
    }
    resp.json().await.context("parsing pair start response")
}

/// `POST {auth_url}/device/pair/token` を 1 回叩いて poll 結果を返す。
async fn poll_once(
    client: &reqwest::Client,
    auth_url: &str,
    device_code: &str,
) -> Result<PollOutcome> {
    let url = format!("{}/device/pair/token", auth_url.trim_end_matches('/'));
    let resp = client
        .post(&url)
        .json(&TokenRequest { device_code })
        .send()
        .await
        .context("pair token request")?;
    // pending=200 / approved=200 / consumed=410 / expired=410。body の status を正とする
    // ので 410 でも body を読んで分類する (5xx だけ transport error 扱い)。
    if resp.status().is_server_error() {
        anyhow::bail!("pair token server error (HTTP {})", resp.status());
    }
    let body = resp.text().await.context("reading pair token response")?;
    parse_poll_outcome(&body)
}

/// env ファイル本文の `KEY=...` 行を upsert する純粋関数。該当 key が無ければ末尾に追記、
/// あれば値を置換する。末尾改行を 1 つに正規化する。
fn upsert_env_var(content: &str, key: &str, value: &str) -> String {
    let prefix = format!("{}=", key);
    let mut replaced = false;
    let mut lines: Vec<String> = content
        .lines()
        .map(|line| {
            if line.trim_start().starts_with(&prefix) {
                replaced = true;
                format!("{}={}", key, value)
            } else {
                line.to_string()
            }
        })
        .collect();
    if !replaced {
        lines.push(format!("{}={}", key, value));
    }
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

/// credential を env ファイルに保存する (SMB_WATCH_DEVICE_ID / _SECRET を upsert、mode 0600)。
fn write_credential_env(path: &std::path::Path, cred: &Credential) -> Result<()> {
    let existing = std::fs::read_to_string(path).unwrap_or_default();
    let updated = upsert_env_var(&existing, "SMB_WATCH_DEVICE_ID", &cred.device_id);
    let updated = upsert_env_var(&updated, "SMB_WATCH_DEVICE_SECRET", &cred.device_secret);
    std::fs::write(path, updated).with_context(|| format!("writing {}", path.display()))?;
    set_file_mode_600(path)?;
    Ok(())
}

#[cfg(unix)]
fn set_file_mode_600(path: &std::path::Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .with_context(|| format!("chmod 600 {}", path.display()))
}

#[cfg(not(unix))]
fn set_file_mode_600(_path: &std::path::Path) -> Result<()> {
    Ok(())
}

/// approved になった credential を保存 (env-out 指定時) または stdout に表示する。
fn deliver_credential(cred: &Credential, args: &PairArgs) -> Result<()> {
    info!(
        "Paired: device_id={} tenant_id={} label={}",
        cred.device_id, cred.tenant_id, cred.label
    );
    match &args.env_out {
        Some(path) => {
            write_credential_env(path, cred)?;
            info!("Saved device credential to {}", path.display());
        }
        None => {
            // device_secret を log では出さず stdout にだけ出す。
            println!("\n# add these to /etc/smb-watch/smb-watch.env (chmod 600):");
            println!("SMB_WATCH_DEVICE_ID={}", cred.device_id);
            println!("SMB_WATCH_DEVICE_SECRET={}", cred.device_secret);
            println!();
        }
    }
    Ok(())
}

/// `smb-watch pair` 本体。start → URL/user_code 表示 → approved まで poll → 保存。
pub async fn run_pair(auth_url: &str, args: &PairArgs) -> Result<()> {
    let client = crate::uploader::build_client()?;
    let label = if args.label.is_empty() {
        "headless device".to_string()
    } else {
        args.label.clone()
    };

    let start = start_pairing(&client, auth_url, &label).await?;
    println!("\n==> デバイスのペアリングを開始しました");
    println!("    ブラウザで次の URL を開いて承認してください:");
    println!("      {}", start.verification_uri_complete);
    println!("    確認コード: {}", start.user_code);
    println!(
        "    (有効期限 {} 分、端末のコードと一致することを確認してください)\n",
        start.expires_in / 60
    );
    info!(
        "Waiting for approval (polling every {}s)...",
        start.interval
    );

    let deadline = std::time::Instant::now() + Duration::from_secs(start.expires_in);
    let interval = Duration::from_secs(start.interval.max(1));

    loop {
        if std::time::Instant::now() >= deadline {
            anyhow::bail!("pairing timed out before approval ({}s)", start.expires_in);
        }
        match poll_once(&client, auth_url, &start.device_code).await {
            Ok(PollOutcome::Approved(cred)) => {
                deliver_credential(&cred, args)?;
                return Ok(());
            }
            Ok(PollOutcome::Pending) => {}
            Ok(PollOutcome::Expired) => anyhow::bail!("pairing code expired before approval"),
            Ok(PollOutcome::Consumed) => {
                anyhow::bail!("pairing already consumed (credential issued elsewhere)")
            }
            Err(e) => warn!("poll error (will retry): {:#}", e),
        }
        tokio::time::sleep(interval).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pending() {
        assert_eq!(
            parse_poll_outcome(r#"{"status":"pending"}"#).unwrap(),
            PollOutcome::Pending
        );
    }

    #[test]
    fn parses_consumed_and_expired() {
        assert_eq!(
            parse_poll_outcome(r#"{"status":"consumed"}"#).unwrap(),
            PollOutcome::Consumed
        );
        assert_eq!(
            parse_poll_outcome(r#"{"status":"expired"}"#).unwrap(),
            PollOutcome::Expired
        );
    }

    #[test]
    fn parses_approved() {
        let body = r#"{"status":"approved","device_id":"d1","device_secret":"s1","tenant_id":"t1","label":"ohishi-data"}"#;
        let outcome = parse_poll_outcome(body).unwrap();
        assert_eq!(
            outcome,
            PollOutcome::Approved(Credential {
                device_id: "d1".into(),
                device_secret: "s1".into(),
                tenant_id: "t1".into(),
                label: "ohishi-data".into(),
            })
        );
    }

    #[test]
    fn approved_without_secret_is_error() {
        let body = r#"{"status":"approved","device_id":"d1"}"#;
        assert!(parse_poll_outcome(body).is_err());
    }

    #[test]
    fn approved_without_id_is_error() {
        let body = r#"{"status":"approved","device_secret":"s1"}"#;
        assert!(parse_poll_outcome(body).is_err());
    }

    #[test]
    fn unknown_status_is_error() {
        assert!(parse_poll_outcome(r#"{"status":"weird"}"#).is_err());
    }

    #[test]
    fn malformed_json_is_error() {
        assert!(parse_poll_outcome("{not json").is_err());
    }

    #[test]
    fn approved_defaults_optional_fields() {
        let body = r#"{"status":"approved","device_id":"d1","device_secret":"s1"}"#;
        let outcome = parse_poll_outcome(body).unwrap();
        assert_eq!(
            outcome,
            PollOutcome::Approved(Credential {
                device_id: "d1".into(),
                device_secret: "s1".into(),
                tenant_id: String::new(),
                label: String::new(),
            })
        );
    }

    #[test]
    fn upsert_appends_when_absent() {
        let out = upsert_env_var("SMB_USER=bob\n", "SMB_WATCH_DEVICE_ID", "abc");
        assert_eq!(out, "SMB_USER=bob\nSMB_WATCH_DEVICE_ID=abc\n");
    }

    #[test]
    fn upsert_replaces_when_present() {
        let out = upsert_env_var(
            "SMB_WATCH_DEVICE_ID=old\nSMB_USER=bob\n",
            "SMB_WATCH_DEVICE_ID",
            "new",
        );
        assert_eq!(out, "SMB_WATCH_DEVICE_ID=new\nSMB_USER=bob\n");
    }

    #[test]
    fn upsert_into_empty() {
        assert_eq!(upsert_env_var("", "K", "v"), "K=v\n");
    }

    #[test]
    fn upsert_does_not_match_key_prefix_substring() {
        // SMB_WATCH_DEVICE_ID should not match SMB_WATCH_DEVICE_ID_X
        let out = upsert_env_var("SMB_WATCH_DEVICE_IDX=keep\n", "SMB_WATCH_DEVICE_ID", "v");
        assert_eq!(out, "SMB_WATCH_DEVICE_IDX=keep\nSMB_WATCH_DEVICE_ID=v\n");
    }

    #[test]
    fn write_credential_env_roundtrip() {
        let dir = std::env::temp_dir().join(format!("smbw-pair-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("smb-watch.env");
        std::fs::write(&path, "SMB_USER=bob\n").unwrap();
        let cred = Credential {
            device_id: "dev-1".into(),
            device_secret: "sec-1".into(),
            tenant_id: "t".into(),
            label: "l".into(),
        };
        write_credential_env(&path, &cred).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("SMB_USER=bob"));
        assert!(content.contains("SMB_WATCH_DEVICE_ID=dev-1"));
        assert!(content.contains("SMB_WATCH_DEVICE_SECRET=sec-1"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        std::fs::remove_dir_all(&dir).ok();
    }
}
