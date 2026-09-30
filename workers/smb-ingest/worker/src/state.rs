//! 走行の状態を持つ Durable Object `SmbIngestState` (SQLite) と、Worker からそれを呼ぶ小さな RPC。
//!
//! インスタンスは名前 `"state"` 固定の 1 つだけ。表は 2 つ:
//! - `kv(key, value)` … `watermark_ms` (次の run の since) と `lease_until_ms` (走行中の印)
//! - `failed(id)` … 前回失敗した共有相対パス (次の run で再試行する)
//!
//! Worker → DO は DO の fetch に JSON を 1 本投げる形で、口は `lease_acquire` と `finish` の 2 つだけ。
//! DO はこの Worker の binding からしか届かない (Worker 自身に fetch ハンドラは無い)。

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use smb_ingest_logic::lease::{lease_state, LeaseState, LEASE_MS};
use worker::{
    durable_object, DurableObject, Env, Method, Request, RequestInit, Response, Result, SqlStorage,
    SqlStorageValue, State,
};

/// wrangler.toml の `[[durable_objects.bindings]]` の name。
const STATE_BINDING: &str = "SMB_INGEST_STATE";
/// DO のインスタンス名 (1 つだけ)。
const STATE_NAME: &str = "state";

const KEY_WATERMARK: &str = "watermark_ms";
const KEY_LEASE: &str = "lease_until_ms";

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub(crate) enum StateRpc {
    /// lease を取りにいく。`free` / `expired` なら `now_ms + LEASE_MS` まで確保して返す。
    LeaseAcquire { now_ms: u64 },
    /// run の終わり。`watermark_ms` があれば進め、失敗一覧を置き換え、lease を解放する。
    Finish {
        watermark_ms: Option<u64>,
        failed: Vec<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Lease {
    Free,
    Held,
    Expired,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct LeaseAcquired {
    pub state: Lease,
    pub stored_since_ms: Option<u64>,
    pub failed: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Finished {}

#[durable_object]
pub struct SmbIngestState {
    sql: SqlStorage,
}

impl DurableObject for SmbIngestState {
    fn new(state: State, _env: Env) -> Self {
        Self {
            sql: state.storage().sql(),
        }
    }

    async fn fetch(&self, mut req: Request) -> Result<Response> {
        if req.method() != Method::Post {
            return Response::error("method not allowed", 405);
        }
        let rpc: StateRpc = req.json().await?;
        self.migrate()?;
        match rpc {
            StateRpc::LeaseAcquire { now_ms } => Response::from_json(&self.lease_acquire(now_ms)?),
            StateRpc::Finish {
                watermark_ms,
                failed,
            } => {
                self.finish(watermark_ms, &failed)?;
                Response::from_json(&Finished {})
            }
        }
    }
}

#[derive(Deserialize)]
struct ValueRow {
    value: String,
}

#[derive(Deserialize)]
struct IdRow {
    id: String,
}

impl SmbIngestState {
    fn migrate(&self) -> Result<()> {
        self.sql.exec(
            "CREATE TABLE IF NOT EXISTS kv (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
            None,
        )?;
        self.sql.exec(
            "CREATE TABLE IF NOT EXISTS failed (id TEXT PRIMARY KEY)",
            None,
        )?;
        Ok(())
    }

    fn get_u64(&self, key: &str) -> Result<Option<u64>> {
        let rows: Vec<ValueRow> = self
            .sql
            .exec(
                "SELECT value FROM kv WHERE key = ?",
                vec![SqlStorageValue::from(key)],
            )?
            .to_array()?;
        match rows.first() {
            None => Ok(None),
            Some(row) => row
                .value
                .parse::<u64>()
                .map(Some)
                .map_err(|e| worker::Error::from(format!("kv {key} is not a number: {e}"))),
        }
    }

    fn put_u64(&self, key: &str, value: u64) -> Result<()> {
        self.sql.exec(
            "INSERT INTO kv (key, value) VALUES (?, ?) \
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            vec![
                SqlStorageValue::from(key),
                SqlStorageValue::from(value.to_string()),
            ],
        )?;
        Ok(())
    }

    fn lease_acquire(&self, now_ms: u64) -> Result<LeaseAcquired> {
        let state = match lease_state(self.get_u64(KEY_LEASE)?, now_ms) {
            LeaseState::Free => Lease::Free,
            LeaseState::Held => Lease::Held,
            LeaseState::Expired => Lease::Expired,
        };
        if state != Lease::Held {
            self.put_u64(KEY_LEASE, now_ms + LEASE_MS)?;
        }
        let failed: Vec<IdRow> = self
            .sql
            .exec("SELECT id FROM failed ORDER BY id", None)?
            .to_array()?;
        Ok(LeaseAcquired {
            state,
            stored_since_ms: self.get_u64(KEY_WATERMARK)?,
            failed: failed.into_iter().map(|r| r.id).collect(),
        })
    }

    fn finish(&self, watermark_ms: Option<u64>, failed: &[String]) -> Result<()> {
        if let Some(ms) = watermark_ms {
            self.put_u64(KEY_WATERMARK, ms)?;
        }
        self.sql.exec("DELETE FROM failed", None)?;
        for id in failed {
            self.sql.exec(
                "INSERT OR IGNORE INTO failed (id) VALUES (?)",
                vec![SqlStorageValue::from(id.as_str())],
            )?;
        }
        self.sql.exec(
            "DELETE FROM kv WHERE key = ?",
            vec![SqlStorageValue::from(KEY_LEASE)],
        )?;
        Ok(())
    }
}

/// Worker 側: DO へ RPC を 1 本投げて応答を受ける。
pub(crate) async fn call<T: DeserializeOwned>(env: &Env, rpc: &StateRpc) -> Result<T> {
    let stub = env.durable_object(STATE_BINDING)?.get_by_name(STATE_NAME)?;
    let mut init = RequestInit::new();
    init.with_method(Method::Post)
        .with_body(Some(serde_json::to_string(rpc)?.into()));
    // ホスト名は名目 (DO の fetch は binding 経由で、外へは出ない)
    let req = Request::new_with_init("https://smb-ingest-state/rpc", &init)?;
    let mut res = stub.fetch_with_request(req).await?;
    if res.status_code() != 200 {
        return Err(worker::Error::from(format!(
            "state rpc failed: {} {}",
            res.status_code(),
            res.text().await.unwrap_or_default()
        )));
    }
    res.json().await
}
