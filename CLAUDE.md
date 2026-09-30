# smb-watch

SMB 共有の新しいファイルを Cloudflare Worker が読み、carins に取り込む。
かつての box 版 (native) は退役済みで、repo には Worker だけが残っている。

## プロジェクト概要

| 項目 | 値 |
|---|---|
| 中身 | Cloudflare Worker `workers/smb-ingest` (workers-rs、`wasm32-unknown-unknown`) |
| 起動 | Cron (平日 JST 9〜17 時の毎正時) と、Service Binding 専用の `POST /run` (carins のボタン) |
| リリース | タグ `worker-smb-ingest-v*` (CI の deploy job) |

## 主なコマンド

ビルドと検査は `workers/smb-ingest/README.md` の「ビルドと検査」を参照
(`cargo test -p smb-ingest-logic`・`scripts/check-exposure*.sh`・`worker-build`)。

## 必ず守ること

1. SMB 資格情報は Secrets Store の `SMB_INGEST_SMB` (JSON 1 つ) にだけ置く。GitHub Actions / workflow / repo に載せない
2. public repo にホスト名・IP・Tunnel ID・共有名・パス・account ID・tenant UUID を書かない
   (VPC の `service_id` と Secrets Store の `store_id` は可)
3. `workers/smb-ingest/worker/wrangler.toml` のまま `wrangler dev` を打たない
   (`vpc_services` が remote で Cloudflare API に繋ぐ)。ローカル検証は README の手順 (repo 外のコピー + `LOCAL_SMB_*`)
4. タグは `worker-smb-ingest-v*` だけ。`v*.*.*` は使わない
5. GitHub Actions の Cloudflare token は org secret `CLOUDFLARE_API_TOKEN` を使い、repo 単位の secret を作らない

## 詳細

設定・ローカル検証の罠・ログの見方・リリース手順は `smb-watch-map` skill と
`workers/smb-ingest/README.md` を参照。
