# workers/smb-ingest

社内の box で systemd timer から動いている smb-watch (native、リポジトリのルート) を置き換える
Cloudflare Worker (Refs #14)。SMB 共有の新しいファイルを読み、auth-worker 経由で carins に取り込み、
走行結果を LINE WORKS に通知する。

## 構成

| パス | 中身 |
|---|---|
| `logic/` | crate `smb-ingest-logic`。Worker に依存しない純粋ロジック (通知の判定と文面・差分抽出・since・サイズ上限・lease)。std のみで `cargo test` できる |
| `worker/` | crate `smb-ingest-worker` (cdylib)。`#[event(scheduled)]` だけを持つ Worker 本体と Durable Object `SmbIngestState` |
| `scripts/check-exposure.sh` | `worker/wrangler.toml` の公開範囲の検査 (CI で毎回) |
| `scripts/check-exposure-test.sh` | 上の陰性対照 |

```text
cron (平日 JST 9〜17 時の毎正時)
  └─ smb-ingest (scheduled のみ。fetch ハンドラ無し)
       ├─ DO SmbIngestState ("state" の 1 つだけ、SQLite): lease / watermark / 失敗一覧
       ├─ SMB_VPC (Workers VPC の VPC Service) ── Tunnel ── 社内の SMB サーバー
       │     smb2 (ohishi-exp/smb2、feature wasm) で NTLM・列挙・read
       └─ AUTH_WORKER_INGEST (service binding → auth-worker の SmbIngestEntrypoint)
             ingestFile({filename, contentType, contentBase64}) / notify(text)
```

1 run の流れ:

1. `lease_acquire` — 他の run が保持中なら何もしない。期限切れ (14 分) の lease が残っていたら
   前回が途中で止まった印として続行し、通知に「前回の run が途中で止まった」を足す
2. since = 保存した watermark、無ければ var `INITIAL_SINCE`。どちらも無ければ何も上げずに失敗を通知する
   (0 にフォールバックして全件を上げ直す事故を防ぐ)
3. SMB に繋ぎ (connect〜tree connect に 30 秒の上限)、`SMB_PATH` 配下を再帰列挙する
4. `mtime > since` のファイルと前回の失敗を、1 件ずつ read (60 秒の上限) → base64 → `ingestFile`。
   12 MiB 超・read の失敗や timeout・2xx 以外は失敗に積み、次の run で再試行する
5. `finish` — watermark を run の開始時刻に進め、失敗一覧を置き換え、lease を解放する。
   失敗が 1 件以上か成功が 1 件以上なら `notify` (検出 0 件は無音)

接続・列挙の失敗は run 全体の失敗として通知し、watermark と失敗一覧は動かさない。

## 設定 (値はこの repo に書かない)

| 種別 | 名前 | 中身 |
|---|---|---|
| secret | `SMB_USER` / `SMB_PASS` | NTLM の資格情報 |
| secret | `SMB_DOMAIN` | NTLM のドメイン (無ければ設定しない = 空) |
| secret | `SMB_SHARE` | 共有名 |
| secret | `SMB_PATH` | 共有内の起点ディレクトリ |
| var | `SOURCE_LABEL` | 通知の 2 行目 (出所表記)。社内の名前を入れない |
| var | `DRY_RUN` | `"0"` 以外 (既定 `"1"`) は dry-run: 列挙と「上げるはずの一覧」のログだけで、`ingestFile`・`notify`・watermark の更新をしない |
| var | `INITIAL_SINCE` | since の初期値 (RFC3339)。watermark が無い初回だけ使う。既定なし |
| binding | `SMB_VPC` | VPC Service (`service_id` は作成後に入れる) |
| binding | `SMB_INGEST_STATE` | Durable Object `SmbIngestState` |
| binding | `AUTH_WORKER_INGEST` | auth-worker の `SmbIngestEntrypoint` |

ローカル検証専用の `LOCAL_SMB_ADDR` (host:port) は `.dev.vars` にだけ置く (VPC を迂回して直接繋ぐ)。

ログと通知に出すのは件数と basename だけ。共有名・パスを含む full path はどこにも出さない
(smb2 のエラー文言に含まれる `SMB_SHARE` / `SMB_PATH` も伏せてから出す)。

## 公開範囲

外から届く口を持たない。`scripts/check-exposure.sh` が CI で毎回 `worker/wrangler.toml` を検査する:

- トップレベルに `workers_dev = false` と `preview_urls = false` が明示されている
- `route` / `routes` が無い、`env` 表が無い (トップレベルだけで運用する)
- SMB への口 `vpc_services` はトップレベルにある
- `vars` に `LOCAL_SMB_ADDR` が無い
- `service_id` がプレースホルダのままなら warning (fail にはしない)

Worker は fetch ハンドラを持たず、DO はこの Worker の binding からしか届かない。
`scripts/check-exposure-test.sh` は wrangler.toml を読んだ dict を 1 か所ずつ崩して書き戻し、各検査が exit 1 になることを確かめる。

## ビルドと検査

```sh
cargo test -p smb-ingest-logic                      # workers/smb-ingest で
bash scripts/check-exposure.sh worker/wrangler.toml
bash scripts/check-exposure-test.sh worker/wrangler.toml
cd worker
cargo install worker-build@0.8.7 --locked
worker-build --release
npx -y wrangler@4.144.0 deploy --dry-run            # login 不要
```

## ローカル検証 (wrangler dev + docker の samba)

auth-worker の binding はローカルに無いので、検証できるのは dry-run の経路 (接続・列挙・DO) まで。

1. ohishi-exp/smb2 の NTLM fixture (`crates/smb2/tests/docker/internal/smb-auth`、`testuser` / `testpass`、
   共有 `private`) を 127.0.0.1 のエフェメラルポートで起動し、ダミーファイルを置く
2. `worker/wrangler.toml` の `vpc_services` は `remote = true` なので、そのまま `wrangler dev` すると
   Cloudflare API に繋ぎにいく。ローカル専用のコピー (vpc_services と `[build]` を外し、`main` を
   ビルド済みの `worker/build/index.js` に向けたもの) を repo の外に作り、その隣に `.dev.vars` を置く:

   ```sh
   LOCAL_SMB_ADDR=127.0.0.1:<port>
   SMB_USER=testuser
   SMB_PASS=testpass
   SMB_SHARE=private
   SMB_PATH=<置いたディレクトリ>
   DRY_RUN=1
   INITIAL_SINCE=2020-01-01T00:00:00+09:00
   ```

3. `npx wrangler@4.144.0 dev --test-scheduled --port <port>` を起動し、
   `curl "http://127.0.0.1:<port>/__scheduled?cron=0+0-8+*+*+1-5"` を 2 回叩く。
   ログに件数と basename だけが出て full path が出ないこと、2 回目も同じ件数が出る
   (watermark が進んでいない) ことを見る

`worker/.dev.vars` は `.gitignore` 済み。

## 本番への切り替え

1. VPC Service (TCP、宛先は社内の SMB サーバーの 445) を作る
2. `worker/wrangler.toml` の `service_id` を入れる PR を merge する
3. secret を入れる: `SMB_USER` / `SMB_PASS` / `SMB_SHARE` / `SMB_PATH` (と要るなら `SMB_DOMAIN`)
4. `DRY_RUN = "1"` のままタグ `worker-smb-ingest-v*` を打って deploy する (`.github/workflows/worker-smb-ingest.yml`)
5. 数回ぶんの cron のログ (件数・basename) を box の smb-watch の結果と比較する
6. box の systemd timer を止める
7. `INITIAL_SINCE` を box の最終 run の時刻にして入れる
8. `DRY_RUN = "0"` にして deploy する (以後は watermark が since になる)
