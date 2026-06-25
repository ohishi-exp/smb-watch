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
| `--upload-url` | `https://nuxt-pwa-carins.mtamaramu.com` | `UPLOAD_URL` |
| `--auth-user` | - | `SMB_WATCH_AUTH_USER` |
| `--auth-pass` | - | `SMB_WATCH_AUTH_PASS` |
| `--auth-url` | - | `SMB_WATCH_AUTH_URL` |
| `--organization-id` | - | `ORGANIZATION_ID` |
| `--drive-letter` | `Z:` | - |
| `--dry-run` | `false` | - |

アップロード先エンドポイント: `POST /api/recieve` (multipart/form-data)

`--auth-user`, `--auth-pass`, `--auth-url` を全て指定すると、Worker (`smb-upload-worker`) 経由の JWT 認証付きアップロードに切り替わる。3 つとも指定するか、全て省略するかのどちらか。

### 組織選択（Google OAuth モード）

Google OAuth 認証時、ユーザーが複数の組織に所属している場合は対話的に選択を求める。
選択結果は `organization_config.json` に保存され、次回以降は自動で使用される。

**組織ID解決の優先順位:**
1. `--organization-id` CLI / `ORGANIZATION_ID` env var
2. `organization_config.json`（端末保存）
3. サーバーから組織一覧取得 → 複数なら対話的選択
4. JWT内のデフォルト組織

組織設定をリセットするには `organization_config.json` を削除する。

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
`smb-watch --version` で出力する（deploy 検証用）。`DEFAULT_GOOGLE_CLIENT_ID/SECRET` は
未設定でも空文字 fallback するので secret 無しでも build / test は通る。

### 必要な GitHub secrets / variables（rust-ichibanboshi に準拠）

| 名前 | 種別 | 用途 |
|---|---|---|
| `CF_ACCESS_CLIENT_ID` / `CF_ACCESS_CLIENT_SECRET` | secret | CF Access service token（SSH 経路認証） |
| `DEPLOY_SSH_KEY` | secret | CI 専用 SSH 秘密鍵（host の `authorized_keys` に公開鍵登録） |
| `DEPLOY_SSH_HOST` | variable | CF Tunnel SSH ingress hostname（例: `ssh-smb-watch.mtamaramu.com`） |
| `DEFAULT_GOOGLE_CLIENT_ID` / `DEFAULT_GOOGLE_CLIENT_SECRET` | secret | binary に焼き込む OAuth 既定値（Phase 2 で auth-worker 方式に移行予定） |

`vars.DEPLOY_SSH_HOST` 未設定なら `deploy` job は `::error` で loud fail する。

### systemd 構成（ホスト側 one-time、`deploy/` のテンプレを配置）

ワンショット（常駐しない）なので `service`(oneshot) + `timer`(毎時) + `path`(binary 監視) の 3 点:

| unit | 役割 |
|---|---|
| `deploy/smb-watch.service` | oneshot。`EnvironmentFile=/etc/smb-watch/smb-watch.env`、`WorkingDirectory=/var/lib/smb-watch`（状態ファイルの置き場）、`ExecStart=/opt/smb-watch/smb-watch` |
| `deploy/smb-watch.timer` | `OnCalendar=hourly` + `Persistent=true`（旧 Windows タスクスケジューラ毎時相当） |
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
