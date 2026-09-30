//! auth-worker の named entrypoint `SmbIngestEntrypoint` (service binding `AUTH_WORKER_INGEST`) への RPC。
//!
//! どちらのメソッドも失敗を throw せず `{status, body, contentType}` で返す (auth-worker
//! `src/smb-ingest-entrypoint.ts`)。tenant・通知の宛先は auth-worker 側で固定で、ここからは選べない。

use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use worker::Env;

const INGEST_BINDING: &str = "AUTH_WORKER_INGEST";

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(extends = js_sys::Object)]
    type SmbIngestRpc;

    #[wasm_bindgen(method, catch, js_name = ingestFile)]
    fn ingest_file(this: &SmbIngestRpc, input: JsValue) -> Result<js_sys::Promise, JsValue>;

    #[wasm_bindgen(method, catch)]
    fn notify(this: &SmbIngestRpc, text: &str) -> Result<js_sys::Promise, JsValue>;
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct IngestFileInput<'a> {
    filename: &'a str,
    content_type: &'a str,
    content_base64: &'a str,
}

/// RPC の戻り (`AlcRpcResult`)。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RpcResult {
    pub status: u16,
    pub body: String,
    #[allow(dead_code)]
    pub content_type: Option<String>,
}

impl RpcResult {
    pub(crate) fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

pub(crate) struct Ingest(SmbIngestRpc);

impl Ingest {
    pub(crate) fn from_env(env: &Env) -> worker::Result<Self> {
        Ok(Self(
            env.service(INGEST_BINDING)?.into_rpc::<SmbIngestRpc>(),
        ))
    }

    pub(crate) async fn ingest_file(
        &self,
        filename: &str,
        content_type: &str,
        content_base64: &str,
    ) -> worker::Result<RpcResult> {
        let input = serde_wasm_bindgen::to_value(&IngestFileInput {
            filename,
            content_type,
            content_base64,
        })?;
        await_result(self.0.ingest_file(input)).await
    }

    pub(crate) async fn notify(&self, text: &str) -> worker::Result<RpcResult> {
        await_result(self.0.notify(text)).await
    }
}

async fn await_result(call: Result<js_sys::Promise, JsValue>) -> worker::Result<RpcResult> {
    let value = JsFuture::from(call.map_err(js_error)?)
        .await
        .map_err(js_error)?;
    Ok(serde_wasm_bindgen::from_value(value)?)
}

fn js_error(e: JsValue) -> worker::Error {
    worker::Error::from(format!("rpc: {e:?}"))
}
