use chrono::{DateTime, Utc};
use clap::{Args, Parser, Subcommand};

/// `--version` / `-V` 出力。deploy 検証で焼き込んだ commit を確認するため
/// BUILD_SHA / BUILD_TIME (build.rs が emit) を含める。
pub const VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    " (sha ",
    env!("BUILD_SHA"),
    ", built ",
    env!("BUILD_TIME"),
    ")"
);

#[derive(Parser, Debug)]
#[command(
    name = "smb-watch",
    version = VERSION,
    about = "Monitor SMB share and upload changed files via HTTP"
)]
pub struct Config {
    /// Subcommand (none = 通常の scan/upload run)
    #[command(subcommand)]
    pub command: Option<Command>,

    /// SMB server hostname or IP
    #[arg(long, default_value = "172.18.21.102")]
    pub smb_host: String,

    /// SMB share name
    #[arg(long, default_value = "共有")]
    pub smb_share: String,

    /// Subdirectory within the SMB share
    #[arg(long, default_value = "新車検証")]
    pub smb_path: String,

    /// SMB username (required for SMB mode, ignored in local mode)
    #[arg(long, env = "SMB_USER")]
    pub smb_user: Option<String>,

    /// SMB password (required for SMB mode, ignored in local mode)
    #[arg(long, env = "SMB_PASS", hide_env_values = true)]
    pub smb_pass: Option<String>,

    /// SMB domain (optional)
    #[arg(long, env = "SMB_DOMAIN", default_value = "")]
    pub smb_domain: String,

    /// auth-worker のベース URL (device JWT 発行 `/device/token`)
    #[arg(
        long,
        env = "SMB_WATCH_AUTH_URL",
        default_value = "https://auth.ippoan.org"
    )]
    pub auth_url: String,

    /// アップロード先 carins のベース URL (`/api/device-upload`)
    #[arg(
        long,
        env = "SMB_WATCH_UPLOAD_URL",
        default_value = "https://carins.ippoan.org"
    )]
    pub upload_url: String,

    /// Path to state file storing last run timestamp
    #[arg(long, default_value = "last_run.txt")]
    pub state_file: std::path::PathBuf,

    /// Windows drive letter to use for net use mount
    #[arg(long, default_value = "Z:")]
    pub drive_letter: String,

    /// Scan files but do not upload (dry run)
    #[arg(long, default_value_t = false)]
    pub dry_run: bool,

    /// Log level: error, warn, info, debug, trace
    #[arg(long, default_value = "info")]
    pub log_level: String,

    /// スキャン基準時刻を上書き。RFC3339形式 (例: 2026-02-10T00:00:00Z)。
    /// 指定すると last_run.txt より優先される。
    #[arg(long, value_name = "DATETIME", value_parser = parse_since)]
    pub since: Option<DateTime<Utc>>,

    /// device credential の ID (pairing で auth-worker から発行)
    #[arg(long, env = "SMB_WATCH_DEVICE_ID")]
    pub device_id: Option<String>,

    /// device credential の secret (pairing で 1 度だけ取得、`/etc/smb-watch` に 600 保管)
    #[arg(long, env = "SMB_WATCH_DEVICE_SECRET", hide_env_values = true)]
    pub device_secret: Option<String>,

    /// Local directory path to monitor (enables local mode, skips SMB mount)
    #[arg(long, value_name = "PATH")]
    pub local_path: Option<std::path::PathBuf>,
}

/// `smb-watch pair` の引数 (headless device pairing、Issue #1 Phase 2.5)。
#[derive(Args, Debug)]
pub struct PairArgs {
    /// 運用識別用ラベル (auth-worker の承認画面に表示)。空なら "headless device"。
    #[arg(long, default_value = "")]
    pub label: String,

    /// credential 保存先 env ファイル (該当行を upsert、mode 600)。
    /// 未指定なら stdout に表示するだけ。
    #[arg(long, value_name = "PATH")]
    pub env_out: Option<std::path::PathBuf>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// auth-worker とこの端末を pairing する (ブラウザ承認、Google 不要)。
    Pair(PairArgs),
}

fn parse_since(s: &str) -> std::result::Result<DateTime<Utc>, String> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| format!("Invalid RFC3339 datetime '{}': {}", s, e))
}
