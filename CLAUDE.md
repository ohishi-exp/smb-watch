# smb-watch

SMB 共有フォルダを監視し、変更されたファイルを HTTP でアップロードするツール。
Windows / Linux 両対応 (Issue #1 で Windows → Linux 無人運用に移行中)。

## プロジェクト概要

| 項目 | 値 |
|---|---|
| バイナリ名 | `smb-watch` (Windows: `smb-watch.exe`) |
| ターゲット | `x86_64-pc-windows-msvc` / `x86_64-unknown-linux-musl` |
| 非同期ランタイム | Tokio |
| TLS | rustls（OpenSSL 不要） |

### SMB アクセス方式 (OS 別、Phase 1)

ファイルソースは `src/source.rs` の `FileSource` で抽象化され、scanner (mtime 比較) と
uploader (read → アップロード) が同一 interface でローカル FS / SMB を扱う。

| 環境 | 接続方式 | 実装 |
|---|---|---|
| Windows | `net use` でドライブにマウント → ローカル FS として走査 | `src/smb.rs` (`#[cfg(windows)]`) |
| Linux | pure-Rust SMB 直アクセス（cifs マウントしない） | `src/smb_fs.rs` (`#[cfg(not(windows))]`、`smb2` crate) |
| `--local-path` | ローカルディレクトリを直接走査（両 OS フォールバック） | `src/source.rs` `LocalFs` |

- `smb2` crate は no C deps / no FFI で musl static cross-compile が崩れない（選定理由は Issue #1）。
  認証 (NTLM / dialect) が実機で合わない場合は `smb` crate (sspi ベース) へ切替える方針。
- 実機疎通 probe: `cargo run --example smb_probe -- --smb-host ... --smb-share ... --smb-path ...`
  （`SMB_USER` / `SMB_PASS` env、SMB と同一 LAN 内で実行）。
- `failed_files.txt` の識別子は Linux SMB では共有ルートからの相対パス、ローカルでは絶対パス。

### 主な設定パラメータ（CLI / 環境変数）

| パラメータ | デフォルト値 | 環境変数 |
|---|---|---|
| `--smb-host` | `172.18.21.102` | - |
| `--smb-share` | `共有` | - |
| `--smb-path` | `新車検証` | - |
| `--smb-user` | - | `SMB_USER` |
| `--smb-pass` | - | `SMB_PASS` |
| `--smb-domain` | `` | `SMB_DOMAIN` |
| `--device-id` | - | `SMB_WATCH_DEVICE_ID` |
| `--device-secret` | - | `SMB_WATCH_DEVICE_SECRET` |
| `--auth-url` | `https://auth.ippoan.org` | `SMB_WATCH_AUTH_URL` |
| `--upload-url` | `https://carins.ippoan.org` | `SMB_WATCH_UPLOAD_URL` |
| `--drive-letter` | `Z:` | - |
| `--dry-run` | `false` | - |

### 認証 (Phase 2 / 案B、device-token)

Google device flow は撤去済み（refresh_token 失効で無人運用が詰まる事故の根治）。
代わりに **auth-worker 発行の device-token** を使う:

1. **pairing (初回のみ)**: `device_id` + `device_secret` を auth-worker から発行する。2 経路:
   - **headless (推奨、box 上で完結)**: `smb-watch pair [--label <name>] [--env-out /etc/smb-watch/smb-watch.env]`
     を box で実行 → 承認 URL + 確認コードが表示される → operator がスマホ等で URL を開き
     auth-worker にログイン (= tenant 確定) して承認 → box が poll で credential を受領し、
     `--env-out` 指定時はその env ファイルに `SMB_WATCH_DEVICE_ID` / `_SECRET` を 600 で upsert
     (未指定なら stdout に表示して手貼り)。auth-worker 側は `/device/pair/start` ·
     `/device/pair/approve` · `/device/pair/token` (ippoan/auth-worker#298)。
   - **operator browser 経由**: operator が auth-worker `/device/pair` で発行して手で配布。
   いずれも `device_secret` は再取得不可なので発行直後に保管する。
2. **runtime (無人)**: smb-watch が `--auth-url`/device credential で auth-worker
   `POST /device/token` を叩き、短命 device JWT を取得（Google 不要）。
3. その JWT を Bearer で carins `POST {--upload-url}/api/device-upload` に multipart upload。
   carins が auth-worker introspect で検証 → 検証済 tenant_id を X-Tenant-ID として
   rust-alc-api に注入する（box は tenant を詐称できない）。

device credential が未設定（`--device-id`/`--device-secret` 両方）だと upload は loud fail する。
`--dry-run` は SMB 走査のみで認証・upload をスキップ（接続確認用）。失効・端末退役時は
auth-worker `/device/revoke` で即無効化できる。

`smb-watch pair` は SMB を一切触らず pairing だけ行って終了する（subcommand 無しの通常 run と排他）。
`device_secret` を log には出さず env ファイル or stdout にだけ出す。

---

## 開発環境セットアップ

```powershell
# Rust stable toolchain
rustup target add x86_64-pc-windows-msvc

# リリースツール
cargo install cargo-release
cargo install cargo-wix --version "0.3.9"

# WiX v3.11（MSI ビルドに必要）
# https://github.com/wixtoolset/wix3/releases からインストール
# インストール後 candle.exe が PATH に入ることを確認
```

---

## ローカルビルド

```powershell
# デバッグビルド
cargo build

# リリースビルド
cargo build --release --target x86_64-pc-windows-msvc

# MSI ビルド（WiX v3.11 が必要）
cargo wix --target x86_64-pc-windows-msvc
# 出力: target\wix\smb-watch-<version>-x86_64.msi
```

---

## リリース手順

### ドライランで確認（推奨）

```powershell
cargo release patch       # 0.1.0 → 0.1.1
cargo release minor       # 0.1.0 → 0.2.0
cargo release major       # 0.1.0 → 1.0.0
cargo release 0.2.0       # バージョン直接指定
```

### 実際にリリース

```powershell
cargo release patch --execute
```

これ一発で以下が全自動：
1. `Cargo.toml` の `version` を更新
2. `git commit` (`chore: Release <version>`)
3. `git tag v<version>`
4. `git push` + `git push --tags`
5. → GitHub Actions 起動 → MSI ビルド → GitHub Release 公開

---

## CI/CD（GitHub Actions）

| ファイル | トリガー | 役割 |
|---|---|---|
| `.github/workflows/ci.yml` | PR / push to `main` | `test` (build + test) / `deploy` (push のみ、musl build → SSH 自動デプロイ) |
| `.github/workflows/release.yml` | `v*.*.*` タグ push | Windows MSI + Linux musl binary を GitHub Release に添付 |

`release.yml` のステップ (`build-and-release`, Windows):
1. `cargo build --release --target x86_64-pc-windows-msvc --locked`
2. `cargo install cargo-wix --version "0.3.9"` → WiX v3.11 を PATH 追加 → `cargo wix`
3. GitHub Release を作成し MSI をアップロード

`release.yml` の `build-linux`: musl static binary を build して同じ Release に
`smb-watch-<tag>-x86_64-unknown-linux-musl` として添付。

---

## Linux 自動デプロイ（CI / SSH、Issue #1 Phase 4/5）

**docker は使わない。** `rust-ichibanboshi` と同じ「musl static binary を SSH で配置 →
systemd で自動反映」パターン。GitHub Actions runner は LAN 内に居ないため、
**Cloudflare Tunnel SSH**（`cloudflared access ssh` を `ProxyCommand`）+ **CF Access
service token** で LAN 内ホストへ到達する。

- 自動: `main` への merge (push) で `ci.yml` の `deploy` job が musl build →
  `scripts/deploy-remote.sh` で `/tmp` 経由 mv（atomic）→ `chmod +x`。
- 検証: smb-watch は常駐サーバではない（`/health` 無し）ので、deploy job は
  **`smb-watch --version` で焼き込み `BUILD_SHA` が deploy commit と一致するか**を
  loud にチェックし、`last_run.txt` 末尾の `status` を Step Summary に出す
  （失敗 status は `::warning::`）。
- 手動 fallback: `DEPLOY_SSH_HOST=<host> ./deploy.sh`（Tailscale 直 or CF Tunnel SSH を
  env で上書き）。実 deploy ロジックは `scripts/deploy-remote.sh` を CI と共有。

### バージョン焼き込み（`build.rs`）

`BUILD_SHA`（`GITHUB_SHA` or `git rev-parse --short HEAD`）/ `BUILD_TIME` を焼き込み、
`smb-watch --version` で出力する（deploy 検証用）。Phase 2 で device-token に移行したため
binary に焼き込む secret は無い（device credential は host の `/etc/smb-watch/smb-watch.env`）。

### 必要な GitHub secrets / variables（rust-ichibanboshi に準拠）

| 名前 | 種別 | 用途 |
|---|---|---|
| `CF_ACCESS_CLIENT_ID` / `CF_ACCESS_CLIENT_SECRET` | secret | CF Access service token（SSH 経路認証） |
| `DEPLOY_SSH_KEY` | secret | CI 専用 SSH 秘密鍵（host の `authorized_keys` に公開鍵登録） |
| `DEPLOY_SSH_HOST` | variable | CF Tunnel SSH ingress hostname（例: `ssh-smb-watch.mtamaramu.com`） |

`vars.DEPLOY_SSH_HOST` 未設定なら `deploy` job は `::error` で loud fail する。

### systemd 構成（ホスト側 one-time、`deploy/` のテンプレを配置）

ワンショット（常駐しない）なので `service`(oneshot) + `timer`(毎時) + `path`(binary 監視) の 3 点:

| unit | 役割 |
|---|---|
| `deploy/smb-watch.service` | oneshot。`EnvironmentFile=/etc/smb-watch/smb-watch.env`、`WorkingDirectory=/var/lib/smb-watch`（状態ファイルの置き場）、`ExecStart=/opt/smb-watch/smb-watch` |
| `deploy/smb-watch.timer` | `OnCalendar=Mon-Fri *-*-* 00..08:00:00`（**UTC 記述 = JST 9〜17 時**）+ `Persistent=true`（平日 毎正時、1 日 9 回）。ohishi-data は host TZ=UTC 運用（時刻依存 cron `backup`/`update_ichi` があるため host TZ は変えない）。host を JST にする場合は `09..17` に直す（systemd 245 は `OnCalendar` の TZ 接尾辞 v252+ が使えない） |
| `deploy/smb-watch-watcher.path` | `PathModified=/opt/smb-watch/smb-watch` → deploy 後に即 1 回 run（次の timer を待たない） |

one-time セットアップ:
1. LAN 内 Linux ホストに `cloudflared` で SSH ingress（`ssh-smb-watch.mtamaramu.com → ssh://localhost:22`）追加
2. CF Access app + Service Auth ポリシー（CI 専用 token のみ許可）
3. deploy ユーザーの `~/.ssh/authorized_keys` に CI 公開鍵登録
4. `/opt/smb-watch/`（binary）+ `/var/lib/smb-watch/`（状態）+ `/etc/smb-watch/smb-watch.env`（SMB 資格情報、`deploy/smb-watch.env.example` を元に 600）を作成。
   **`/opt/smb-watch` は deploy ユーザー（ubuntu）所有にする** — CI deploy は ubuntu で SSH し sudo を使わないため、root 所有だと `mv` が `Permission denied` で fail する（rust-ichibanboshi の `/opt/ichibanboshi` と同じ）:
   ```sh
   sudo mkdir -p /opt/smb-watch /var/lib/smb-watch /etc/smb-watch
   sudo chown ubuntu:ubuntu /opt/smb-watch          # ← deploy 用に必須
   sudo install -m600 deploy/smb-watch.env.example /etc/smb-watch/smb-watch.env  # 値を実値に編集
   ```
5. `deploy/*.{service,timer,path}` を `/etc/systemd/system/` に配置 → `systemctl enable --now smb-watch.timer smb-watch-watcher.path`

> SMB 資格情報（`SMB_USER` / `SMB_PASS` 等）は host の `/etc/smb-watch/smb-watch.env` に
> だけ置き、GitHub Actions / workflow YAML には載せない（資格情報は host boundary に閉じる）。

### ohishi-data 運用メモ（実機の確定事項・次回の参考）

本番ホスト `ohishi-data`（ubuntu）で実際に確認・確定した事項。同種の作業をする時の前提。

- **host TZ = UTC**（`systemctl list-timers` が全部 UTC 表示で確認）。**変更しない**。
  時刻依存の業務 cron（`/etc/cron.d/` の `backup` / `update_ichi`）があり、`timedatectl
  set-timezone` するとそれらが 9h ずれるため。JST スケジュールは **timer 側を UTC で記述**して表現する
  （JST 9〜17 時 = UTC 0〜8 時、曜日も Mon-Fri で一致 → `OnCalendar=Mon-Fri *-*-* 00..08:00:00`）。
- **systemd 245**（Ubuntu 20.04）。`OnCalendar` の TZ 接尾辞（`... Asia/Tokyo`）は **v252+ 必須**で使えない。
  なので「host TZ で解釈される」前提を崩さず、UTC 記述で JST を表現する（上記）。
- **systemd unit file は CI deploy では更新されない**（deploy job は binary だけを `/opt/smb-watch/`
  に置く）。`*.timer` / `*.service` / `*.path` を変えたら **ホスト上で手動反映**が要る。
  clone せず public repo の raw から 1 ファイルだけ取るのが楽:
  ```sh
  sudo curl -fsSL https://raw.githubusercontent.com/ohishi-exp/smb-watch/main/deploy/smb-watch.timer \
    -o /etc/systemd/system/smb-watch.timer
  sudo systemctl daemon-reload && sudo systemctl restart smb-watch.timer
  systemctl list-timers smb-watch.timer --no-pager   # NEXT が UTC 00〜08 時台(=JST 9〜17時)の平日か
  ```
- **手動 1 回実行**は `sudo systemctl start smb-watch.service`（oneshot なので start が完走まで待って返る）。
  直後に `sudo tail -n3 /var/lib/smb-watch/last_run.txt` で `... found uploaded failed ok` を確認。
- **watermark = `/var/lib/smb-watch/last_run.txt`**（service の `WorkingDirectory`）。最終行の start ts を
  `since` に使う（`dry-run`/`ok`/`seed` の status で区別せず、最終行を読む）。**dry-run でも watermark は
  進む**ので注意（dry-run 後に通常 run しても、dry-run 時点以前のファイルは拾われない）。初回 backfill は
  `--since <古い時刻>` で明示。`seed` 行を手で入れて起点を固定する運用も可。
- **このコンテナ（CCoW）からは ohishi-data へ SSH 不可**（egress TCP/443 のみ、cloudflared の UDP 7844 /
  Tailscale 不達）。host 上の操作（pairing 実行・systemd 反映・手動 run）は **operator が実機で叩く**。

### device pairing（headless、実機手順）

box（ohishi-data）で完結する pairing（auth-worker `/device/pair/*`、ippoan/auth-worker#298）:

```sh
sudo /opt/smb-watch/smb-watch pair --label ohishi-data --env-out /etc/smb-watch/smb-watch.env
# → 承認 URL + 確認コードが表示される
#   operator がブラウザで URL を開き auth.ippoan.org にログイン(= tenant 確定) → 確認コード一致を確認 → 承認
#   box が poll で credential を受領し env ファイルへ自動追記 (mode 600、device_secret は画面/log に出ない)
```

- 承認画面でログインするアカウントの **tenant が、carins にアップロードしたいデータの tenant と一致**する必要がある
  （approve したセッションの tenant_id が credential に焼かれ、carins が X-Tenant-ID として rust-alc-api に注入する）。
- 失効・端末退役は auth-worker `/device/revoke`。再 pairing は同じ `pair` コマンドをもう一度。
- 動作実績（2026-06-25）: pairing 成功 → 通常 run で 2 件 upload 成功（device JWT → carins `/api/device-upload`
  → introspect 検証 → rust-alc-api `/api/files`）まで実機疎通確認済み。

---

## インストーラー（WiX MSI）

ファイル: `wix/main.wxs`

| 項目 | 値 |
|---|---|
| インストール先 | `C:\Program Files\smb-watch\smb-watch.exe` |
| スコープ | perMachine（全ユーザー） |
| UpgradeCode | `D802E510-9F08-408B-BFFD-B0B491E7F908` |

**UpgradeCode は変更禁止。** 変更するとバージョンアップ時に別製品として扱われる。

バージョンは `Cargo.toml` の `version` から自動で MSI に同期される（`$(var.Version)` 経由）。
