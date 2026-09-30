//! smb-ingest Worker の純粋ロジック。依存は std のみ (wasm32-unknown-unknown でビルドできる)。
//!
//! 時刻はすべて呼び出し側が渡す UNIX ミリ秒 (`u64`)。この crate は現在時刻を取得しない。

pub mod lease;
pub mod notify;
pub mod plan;
pub mod since;
pub mod size;
