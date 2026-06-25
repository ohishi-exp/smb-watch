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

ファイル: `.github/workflows/release.yml`

トリガー: `v*.*.*` 形式のタグ push

ステップ:
1. `cargo build --release --target x86_64-pc-windows-msvc --locked`
2. `cargo install cargo-wix --version "0.3.9"`
3. WiX v3.11 を PATH に追加（`windows-latest` にプリインストール済み）
4. `cargo wix --target x86_64-pc-windows-msvc`
5. GitHub Release を作成し MSI をアップロード

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
