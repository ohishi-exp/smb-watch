//! smb-ingest: SMB 共有の新しいファイルを auth-worker 経由で取り込む Worker (cron + Service Binding 専用の `POST /run`) (Refs ohishi-exp/smb-watch#14)。
//!
//! 社内の box で systemd timer から動いていた smb-watch (native) の置き換え。1 run の流れ:
//!
//! 1. DO `SmbIngestState` で lease を取る (保持中なら何もしない。期限切れ = 前回が途中で止まった印)
//! 2. since = 保存した watermark、無ければ var `INITIAL_SINCE` (どちらも無ければ何も上げず失敗通知)
//! 3. Workers VPC の binding `SMB_VPC` から SMB に繋ぎ、`SMB_PATH` 配下を再帰列挙する
//! 4. `mtime > since` と前回の失敗を 1 件ずつ read → base64 → auth-worker `ingestFile`
//! 5. DO に watermark (= run の開始時刻) と失敗一覧を書いて lease を解放し、必要なら `notify`
//!
//! `DRY_RUN` が `"0"` 以外 (既定 `"1"`) のときは列挙と「上げるはずの一覧」のログだけで、
//! `ingestFile`・`notify`・watermark の更新をしない (lease は解放する)。
//!
//! ログと通知に出すのは件数と basename だけ。共有名・パス (secret `SMB_INGEST_SMB` の share / path) を含む
//! full path はどこにも出さない (エラー文言からも伏せる)。

mod ingest;
mod state;
mod tcp;
mod transport;

use std::collections::HashMap;
use std::future::Future;
use std::pin::pin;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use futures_util::future::{select, Either};
use smb2::{ClientConfig, SmbClient, Tree};
use smb_ingest_logic::http::{self, ErrorKind, Reply};
use smb_ingest_logic::notify::{build_message, file_name_of, should_notify};
use smb_ingest_logic::plan::{candidates, Entry};
use smb_ingest_logic::since::resolve_since;
use smb_ingest_logic::size::too_large;
use worker::{
    console_error, console_log, event, Context, Date, Delay, Env, Request, Response,
    ScheduleContext, ScheduledEvent,
};

use crate::ingest::Ingest;
use crate::state::{Finished, Lease, LeaseAcquired, StateRpc};
use crate::tcp::TcpPort;
use crate::transport::{Route, WorkerSockets, VPC_NOMINAL_ADDR};

pub use crate::state::SmbIngestState;

/// connect〜tree connect の上限。SMB サーバーが NEGOTIATE に無言だと smb2 は期限切れにならないので自前で包む。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// 1 ディレクトリの列挙の上限。
const LIST_TIMEOUT: Duration = Duration::from_secs(60);
/// 1 ファイルの read の上限。
const READ_TIMEOUT: Duration = Duration::from_secs(60);

/// 通知の件名 (logic の `build_message` と同じ)。run 全体の失敗の文面でだけ使う。
const SUBJECT: &str = "[carins 車検証]";
/// lease が期限切れで残っていたときに通知へ足す一行。
const STALLED_NOTE: &str = "※ 前回の run が途中で止まった";
/// FILETIME (1601 起点・100ns) と UNIX 起点の差。
const FILETIME_UNIX_EPOCH: u64 = 116_444_736_000_000_000;

#[event(scheduled)]
async fn scheduled(_event: ScheduledEvent, env: Env, _ctx: ScheduleContext) {
    let dry_run = text(&env, "DRY_RUN").as_deref() != Some("0");
    match acquire(&env).await {
        Ok(Some(held)) => run(&env, held, dry_run).await,
        Ok(None) => {}
        Err(e) => console_error!("smb-ingest: run aborted: {e}"),
    }
}

/// fetch は Service Binding からだけ届く `POST /run` (route・workers.dev なし。同一アカウントで binding を
/// 宣言した worker は誰でも叩けるが、効果は run を早めることだけで、tenant は auth-worker の KV 固定)。
/// lease だけ同期で取り (保持中なら 409)、取れたら run 本体を `wait_until` で後ろに流して 202 を返す。
/// 応答に件数・ファイル名・共有名・パス・エラーの生文言を入れない。
#[event(fetch)]
async fn fetch(req: Request, env: Env, ctx: Context) -> worker::Result<Response> {
    let url = req.url()?;
    let route = http::route(req.method().as_ref(), url.path(), url.query());
    let force_dry_run = match route {
        http::Route::Run { force_dry_run } => force_dry_run,
        other => return respond(http::reply_for_unrouted(other).unwrap_or_else(http::busy)),
    };
    let dry_run = http::effective_dry_run(text(&env, "DRY_RUN").as_deref(), force_dry_run);
    match acquire(&env).await {
        Ok(None) => respond(http::busy()),
        Ok(Some(held)) => {
            ctx.wait_until(async move { run(&env, held, dry_run).await });
            respond(http::accepted(dry_run))
        }
        Err(e) => {
            console_error!("smb-ingest: manual run aborted: {e}");
            respond(http::error(ErrorKind::LeaseUnavailable))
        }
    }
}

fn respond(reply: Reply) -> worker::Result<Response> {
    let response = if reply.body.is_empty() {
        Response::empty()?
    } else {
        Response::from_body(worker::ResponseBody::Body(reply.body.into_bytes()))?
            .with_headers(json_headers())
    };
    Ok(response.with_status(reply.status))
}

fn json_headers() -> worker::Headers {
    let h = worker::Headers::new();
    let _ = h.set("content-type", "application/json");
    h
}

/// lease を取った run の文脈。
struct Held {
    lease: LeaseAcquired,
    now_ms: u64,
}

/// lease を取る。保持中なら `None` (ログを残す)。
async fn acquire(env: &Env) -> worker::Result<Option<Held>> {
    let now_ms = Date::now().as_millis();
    let lease: LeaseAcquired = state::call(env, &StateRpc::LeaseAcquire { now_ms }).await?;
    if lease.state == Lease::Held {
        console_log!("smb-ingest: another run holds the lease; skipping");
        return Ok(None);
    }
    Ok(Some(Held { lease, now_ms }))
}

/// lease を取れた後の run 本体。エラーはログに出して終わる。
async fn run(env: &Env, held: Held, dry_run: bool) {
    if let Err(e) = run_body(env, held, dry_run).await {
        console_error!("smb-ingest: run aborted: {e}");
    }
}

async fn run_body(env: &Env, held: Held, dry_run: bool) -> worker::Result<()> {
    let Held { lease, now_ms } = held;
    let label = text(env, "SOURCE_LABEL").unwrap_or_else(|| "smb-ingest".to_string());
    let stalled = lease.state == Lease::Expired;
    if stalled {
        console_log!("smb-ingest: previous run left an expired lease (it stopped midway)");
    }
    console_log!(
        "smb-ingest: start (dry_run={dry_run}, retry={})",
        lease.failed.len()
    );

    let config = load_config(env).await;
    let redact = config
        .as_ref()
        .map_or_else(|_| Redact(Vec::new()), Redact::from_config);
    let outcome = match config {
        Ok(config) => ingest_changed(env, &lease, dry_run, &redact, &config).await,
        Err(e) => Err(e),
    };

    // 失敗一覧・watermark は dry-run では触らない (lease の解放だけ)
    let finish = match &outcome {
        Ok(report) if !dry_run => StateRpc::Finish {
            watermark_ms: Some(now_ms),
            failed: report.failed.clone(),
        },
        _ => StateRpc::Finish {
            watermark_ms: None,
            failed: lease.failed.clone(),
        },
    };
    let _: Finished = state::call(env, &finish).await?;

    // 通知の 2 行目 (出所表記) に「前回が途中で止まった」を足す。build_message が全体の長さを保証する
    let label = if stalled {
        format!("{label}\n{STALLED_NOTE}")
    } else {
        label
    };
    let text = match outcome {
        Ok(report) => {
            console_log!(
                "smb-ingest: done (found={}, uploaded={}, failed={})",
                report.files_found,
                report.uploaded,
                report.failed.len()
            );
            (should_notify(report.files_found, report.uploaded, report.failed.len()) || stalled)
                .then(|| build_message(&label, report.files_found, report.uploaded, &report.failed))
        }
        Err(e) => {
            console_error!("smb-ingest: run failed: {}", redact.apply(&e.detail));
            Some(format!(
                "{SUBJECT} 失敗 (取り込み前に停止)\n{label}\n{}",
                e.reason
            ))
        }
    };
    match text {
        Some(_) if dry_run => console_log!("smb-ingest: dry-run; not notifying"),
        Some(text) => match Ingest::from_env(env)?.notify(&text).await {
            Ok(r) if r.is_success() => console_log!("smb-ingest: notified"),
            Ok(r) => console_error!(
                "smb-ingest: notify returned {}: {}",
                r.status,
                short(&r.body)
            ),
            Err(e) => console_error!("smb-ingest: notify failed: {e}"),
        },
        None => console_log!("smb-ingest: nothing to report"),
    }
    Ok(())
}

/// 1 run の結果。
struct Report {
    files_found: usize,
    uploaded: usize,
    /// 失敗した共有相対パス (次の run で再試行する)。
    failed: Vec<String>,
}

/// run 全体の失敗。`reason` は通知に出す定型文 (パス・共有名を含まない)、`detail` はログ用 (伏せてから出す)。
struct RunError {
    reason: &'static str,
    detail: String,
}

impl RunError {
    fn new(reason: &'static str, detail: impl std::fmt::Display) -> Self {
        Self {
            reason,
            detail: format!("{reason}: {detail}"),
        }
    }
}

/// Secrets Store の secret `SMB_INGEST_SMB` (JSON)。ローカルでは var `LOCAL_SMB_CONFIG_JSON` が代わりになる。
/// `Debug` は実装しない (値をログに出さない)。
#[derive(serde::Deserialize)]
struct SmbConfig {
    user: String,
    pass: String,
    /// 空文字もありうる (それ以外は空を許さない)。
    domain: String,
    share: String,
    path: String,
}

impl SmbConfig {
    /// JSON から読む。エラーに値を含めない (serde のメッセージは値を引用しうるので分類だけ返す)。
    fn parse(json: &str) -> Result<Self, RunError> {
        let config: Self = serde_json::from_str(json)
            .map_err(|e| RunError::new("SMB 設定が読めない", format!("{:?}", e.classify())))?;
        for (name, value) in [
            ("user", &config.user),
            ("pass", &config.pass),
            ("share", &config.share),
            ("path", &config.path),
        ] {
            if value.is_empty() {
                return Err(RunError::new("設定不足 (SMB_INGEST_SMB)", name));
            }
        }
        Ok(config)
    }

    /// 列挙の起点 (前後の区切りを落とした共有内パス)。
    fn root(&self) -> &str {
        self.path.trim_matches(['/', '\\'])
    }
}

async fn load_config(env: &Env) -> Result<SmbConfig, RunError> {
    // ローカル検証 (wrangler dev) だけ: Secrets Store が使えないので var LOCAL_SMB_CONFIG_JSON で代える。
    // 本番の vars には置かない (scripts/check-exposure.sh が検査する)
    if let Some(json) = text(env, "LOCAL_SMB_CONFIG_JSON") {
        return SmbConfig::parse(&json);
    }
    let json = env
        .secret_store("SMB_INGEST_SMB")
        .map_err(|e| RunError::new("SMB_INGEST_SMB の binding が無い", e))?
        .get()
        .await
        .map_err(|e| RunError::new("SMB_INGEST_SMB を読めない", e))?
        .ok_or_else(|| RunError::new("SMB_INGEST_SMB が空", "secret not found"))?;
    SmbConfig::parse(&json)
}

async fn ingest_changed(
    env: &Env,
    lease: &LeaseAcquired,
    dry_run: bool,
    redact: &Redact,
    settings: &SmbConfig,
) -> Result<Report, RunError> {
    let initial = match text(env, "INITIAL_SINCE") {
        None => None,
        Some(s) => Some(
            parse_rfc3339_ms(&s)
                .ok_or_else(|| RunError::new("INITIAL_SINCE が RFC3339 でない", "parse error"))?,
        ),
    };
    let since = resolve_since(lease.stored_since_ms, initial)
        .map_err(|e| RunError::new("since が未設定 (INITIAL_SINCE を入れる)", e))?;

    // dry-run では auth-worker を呼ばない。本番は binding が無ければ SMB に繋ぐ前に落とす
    let ingest = if dry_run {
        None
    } else {
        Some(Ingest::from_env(env).map_err(|e| RunError::new("AUTH_WORKER_INGEST が無い", e))?)
    };

    let (mut client, mut tree) = match timeout(CONNECT_TIMEOUT, connect(env, settings)).await {
        None => {
            return Err(RunError::new(
                "SMB 接続が 30 秒で終わらない",
                "connect timeout",
            ))
        }
        Some(r) => r?,
    };

    let result = scan_and_ingest(
        &mut client,
        &mut tree,
        settings,
        since,
        lease,
        ingest,
        redact,
    )
    .await;
    if let Err(e) = client.disconnect_share(&tree).await {
        console_log!(
            "smb-ingest: disconnect_share: {}",
            redact.apply(&e.to_string())
        );
    }
    result
}

async fn scan_and_ingest(
    client: &mut SmbClient,
    tree: &mut Tree,
    settings: &SmbConfig,
    since: u64,
    lease: &LeaseAcquired,
    ingest: Option<Ingest>,
    redact: &Redact,
) -> Result<Report, RunError> {
    let entries = list_files(client, tree, settings.root()).await?;
    let sizes: HashMap<&str, u64> = entries.iter().map(|e| (e.id.as_str(), e.size)).collect();
    let ids = candidates(&entries, since, &lease.failed);
    let files_found = ids.len();
    console_log!(
        "smb-ingest: listed {} file(s); {} to ingest",
        entries.len(),
        files_found
    );
    for id in &ids {
        console_log!("smb-ingest:   {}", file_name_of(id));
    }

    let Some(ingest) = ingest else {
        console_log!("smb-ingest: dry-run; not ingesting");
        return Ok(Report {
            files_found,
            uploaded: 0,
            failed: Vec::new(),
        });
    };

    let mut uploaded = 0usize;
    let mut failed = Vec::new();
    for id in ids {
        let name = file_name_of(&id);
        match ingest_one(
            client,
            tree,
            &ingest,
            &id,
            &name,
            sizes.get(id.as_str()).copied(),
        )
        .await
        {
            Ok(()) => uploaded += 1,
            Err(e) => {
                console_error!("smb-ingest: failed {name}: {}", redact.apply(&e));
                failed.push(id);
            }
        }
    }
    Ok(Report {
        files_found,
        uploaded,
        failed,
    })
}

async fn ingest_one(
    client: &mut SmbClient,
    tree: &mut Tree,
    ingest: &Ingest,
    id: &str,
    name: &str,
    size: Option<u64>,
) -> Result<(), String> {
    if let Some(size) = size.filter(|s| too_large(*s)) {
        return Err(format!("too large ({size} bytes)"));
    }
    let bytes = timeout(READ_TIMEOUT, client.read_file(tree, id))
        .await
        .ok_or("read timeout")?
        .map_err(|e| format!("read: {e}"))?;
    // 一覧の後に伸びたファイルもここで弾く
    if too_large(bytes.len() as u64) {
        return Err(format!("too large ({} bytes)", bytes.len()));
    }
    let content = base64::engine::general_purpose::STANDARD.encode(&bytes);
    drop(bytes);
    let content_type = mime_guess::from_path(name).first_or_octet_stream();
    let res = ingest
        .ingest_file(name, content_type.essence_str(), &content)
        .await
        .map_err(|e| format!("ingestFile: {e}"))?;
    if res.is_success() {
        Ok(())
    } else {
        Err(format!("ingestFile {}: {}", res.status, short(&res.body)))
    }
}

async fn connect(env: &Env, settings: &SmbConfig) -> Result<(SmbClient, Tree), RunError> {
    // ローカル検証 (wrangler dev) だけ: LOCAL_SMB_ADDR があれば直接繋ぐ。本番の vars には置かない
    // (scripts/check-exposure.sh が検査する)
    let (addr, route) = match text(env, "LOCAL_SMB_ADDR") {
        Some(addr) => (addr, Route::Direct),
        None => {
            let vpc = env
                .get_binding::<TcpPort>("SMB_VPC")
                .map_err(|e| RunError::new("SMB_VPC が無い", e))?;
            (VPC_NOMINAL_ADDR.to_string(), Route::Vpc(vpc))
        }
    };
    let config = ClientConfig {
        addr,
        username: settings.user.clone(),
        password: settings.pass.clone(),
        domain: settings.domain.clone(),
        transport_factory: Some(Arc::new(WorkerSockets::new(route))),
        ..Default::default()
    };
    let mut client = SmbClient::connect(config)
        .await
        .map_err(|e| RunError::new("SMB 接続に失敗", e))?;
    let tree = client
        .connect_share(&settings.share)
        .await
        .map_err(|e| RunError::new("共有への接続に失敗", e))?;
    Ok((client, tree))
}

/// `root` 配下を再帰列挙する (native `src/smb_fs.rs` の `list_files` と同じ順序・同じ id)。
async fn list_files(
    client: &mut SmbClient,
    tree: &mut Tree,
    root: &str,
) -> Result<Vec<Entry>, RunError> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_string()];
    while let Some(dir) = stack.pop() {
        let entries = timeout(LIST_TIMEOUT, client.list_directory(tree, &dir))
            .await
            .ok_or_else(|| RunError::new("一覧の取得が 60 秒で終わらない", "list timeout"))?
            .map_err(|e| RunError::new("一覧の取得に失敗", e))?;
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
            } else if let Some(mtime_ms) = filetime_to_unix_ms(e.modified.0) {
                out.push(Entry {
                    id: path,
                    mtime_ms,
                    size: e.size,
                });
            } else {
                console_log!(
                    "smb-ingest: invalid mtime, skipping {}",
                    file_name_of(&path)
                );
            }
        }
    }
    Ok(out)
}

/// FILETIME (100ns 単位・1601 起点) を UNIX ミリ秒に。UNIX 起点より前は `None`
/// (native は `to_system_time()` が `None` のとき skip する。wasm32 では `SystemTime` を避ける)。
fn filetime_to_unix_ms(filetime: u64) -> Option<u64> {
    filetime
        .checked_sub(FILETIME_UNIX_EPOCH)
        .map(|t| t / 10_000)
}

fn parse_rfc3339_ms(s: &str) -> Option<u64> {
    let dt = chrono::DateTime::parse_from_rfc3339(s.trim()).ok()?;
    u64::try_from(dt.timestamp_millis()).ok()
}

async fn timeout<F: Future>(limit: Duration, f: F) -> Option<F::Output> {
    match select(pin!(f), pin!(Delay::from(limit))).await {
        Either::Left((v, _)) => Some(v),
        Either::Right(_) => None,
    }
}

/// secret / var の文字列 (空は無いものとして扱う)。`.dev.vars` はローカルでは var として見える。
fn text(env: &Env, name: &str) -> Option<String> {
    env.secret(name)
        .map(|s| s.to_string())
        .or_else(|_| env.var(name).map(|v| v.to_string()))
        .ok()
        .filter(|s| !s.is_empty())
}

fn short(s: &str) -> String {
    s.chars().take(200).collect()
}

/// 共有名・パスをログから伏せる。
struct Redact(Vec<(String, &'static str)>);

impl Redact {
    fn from_config(config: &SmbConfig) -> Self {
        let path = config.root().to_string();
        let mut pairs = vec![
            (path.replace('/', "\\"), "<path>"),
            (path, "<path>"),
            (config.share.clone(), "<share>"),
        ];
        pairs.retain(|(s, _)| !s.is_empty());
        Self(pairs)
    }

    fn apply(&self, s: &str) -> String {
        self.0.iter().fold(s.to_string(), |acc, (needle, mask)| {
            acc.replace(needle.as_str(), mask)
        })
    }
}
