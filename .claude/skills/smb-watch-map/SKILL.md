---
name: smb-watch-map
generated-from: smb-watch:d145ab6550fefca2d657c04b0ce39b3415c1cb98
paths: [workers/smb-ingest/, .github/workflows/worker-smb-ingest.yml]
description: ohishi-exp/smb-watch (SMB 共有の新ファイルを Cloudflare Worker `smb-ingest` が Workers VPC + Tunnel 経由で読み、auth-worker の Service Binding で carins に取り込み LINE WORKS に通知する) の構造ナビゲーション。ディレクトリ地図・設定 (vars / binding / Secrets Store)・wrangler dev の罠・ログの見方・リリース手順 (worker-smb-ingest-v* タグ)。トリガー:「smb-watch」「smb-ingest」「SMB 取り込み」「Workers VPC」「SMB_INGEST_SMB」「worker-smb-ingest」「wrangler tail smb-ingest」等。
---

# smb-watch-map — 構造ナビゲーション

box 版 (native) は退役済み。repo には Worker `workers/smb-ingest` だけがある。
詳細は `workers/smb-ingest/README.md`、退役の経緯は repo ルートの `README.md`。

## ディレクトリ地図

| パス | 役割 |
|---|---|
| `workers/smb-ingest/logic/` | crate `smb-ingest-logic`。純粋ロジック (通知の判定と文面・差分抽出・since・サイズ上限・lease)。`cargo test -p smb-ingest-logic` |
| `workers/smb-ingest/worker/` | crate `smb-ingest-worker` (cdylib)。`#[event(scheduled)]`・`#[event(fetch)]` (`POST /run`)・Durable Object `SmbIngestState`。`wrangler.toml` もここ |
| `workers/smb-ingest/scripts/` | `check-exposure.sh` (wrangler.toml の公開範囲検査) と `check-exposure-test.sh` (陰性対照) |
| `.github/workflows/worker-smb-ingest.yml` | repo 唯一の CI。job: `logic` (test + wasm32 build) / `worker` (build・公開範囲検査・`wrangler deploy --dry-run`) / `deploy` (タグ `worker-smb-ingest-v*` で本番 deploy) / `auto-merge` (PR のみ) |

pull_request は全 PR で走る (paths で絞らない)。docs だけの PR も検査を通って auto-merge される。

## 設定

| 種別 | 名前 | 中身 |
|---|---|---|
| Secrets Store | `SMB_INGEST_SMB` | JSON (`user` / `pass` / `domain` / `share` / `path`)。資格情報はここにだけ置く |
| var | `SOURCE_LABEL` | 通知の出所表記 |
| var | `DRY_RUN` | `"0"` 以外は dry-run。本番は `"0"` |
| var | `INITIAL_SINCE` | watermark が無い初回だけ使う since (RFC3339) |
| binding | `SMB_VPC` | Workers VPC の VPC Service |
| binding | `SMB_INGEST_STATE` | Durable Object `SmbIngestState` |
| binding | `AUTH_WORKER_INGEST` | auth-worker の `SmbIngestEntrypoint` (`ingestFile` / `notify`) |

public repo なのでホスト名・IP・Tunnel ID・共有名・パス・account ID・tenant UUID は書かない
(VPC の `service_id` と Secrets Store の `store_id` は可)。

## ローカル検証の罠

- `worker/wrangler.toml` のまま `wrangler dev` を打たない。`vpc_services` が `remote = true` で Cloudflare API に繋ぎにいく
- ローカルは README「ローカル検証」の手順で行う: repo 外にコピーを作り (`vpc_services` / `[build]` / `secrets_store_secrets` を外し、`main` を `worker/build/index.js` に向ける)、`.dev.vars` に `LOCAL_SMB_ADDR` / `LOCAL_SMB_CONFIG_JSON` を置く
- auth-worker の binding はローカルに無いので、検証できるのは dry-run の経路まで

## ログの見方

repo の外のディレクトリで実行する:

```sh
npx -y wrangler@4.144.0 tail smb-ingest --format pretty
```

Workers Logs (ダッシュボード / API) は org の token に権限が無く、読めない。
ログに出るのは件数と basename だけ。

## リリース手順

タグは `worker-smb-ingest-vX.Y.Z` だけ。`v*.*.*` は使わない。

```sh
gh api repos/ohishi-exp/smb-watch/git/refs \
  -f ref=refs/tags/worker-smb-ingest-vX.Y.Z -f sha=<main の SHA>
```

タグの push で `worker-smb-ingest.yml` の `deploy` job が走る (org secret `CLOUDFLARE_API_TOKEN` を使う)。
