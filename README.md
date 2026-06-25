# smb-watch

SMB 共有フォルダを監視し、変更されたファイルを HTTP でアップロードするツール。**Linux 無人運用**(systemd timer + device-token 認証)を主用途とし、Windows でも動作します。

## 概要

起動のたびに SMB 共有フォルダをスキャンし、前回実行以降に変更されたファイルを検出して carins(`/api/device-upload`)へアップロードします。アップロードに失敗したファイルは次回実行時に自動でリトライします。

- **常駐しない**ワンショット実行。Linux では systemd timer が定期起動します。
- 認証は **auth-worker 発行の device-token**(後述の pairing)。Google ログインや常駐 refresh token は不要で、無人運用に耐えます。

## アーキテクチャ / 認証

```
smb-watch (box)
  ├─ SMB 共有を直接スキャン (Linux: pure-Rust / Windows: net use マウント)
  ├─ device credential → auth-worker POST /device/token → 短命 device JWT
  └─ device JWT (Bearer) → carins POST /api/device-upload (multipart)
        └─ carins が auth-worker introspect で検証 → tenant_id を X-Tenant-ID で
           rust-alc-api に注入 (box は tenant を詐称できない)
```

`device_id` + `device_secret`(= device credential)は一度 **pairing** で取得し、`/etc/smb-watch/smb-watch.env` 等に保管します。`device_secret` は再取得不可です。

## インストール

### Linux(主用途)

- **自動**: `ippoan`/`ohishi-exp` の CI が `main` への merge で musl static binary を `ohishi-data:/opt/smb-watch/smb-watch` に自動デプロイします(運用ホスト)。
- **手動**: [GitHub Releases](https://github.com/ohishi-exp/smb-watch/releases) の `smb-watch-<tag>-x86_64-unknown-linux-musl` を配置するか、ローカルで `cargo build --release --target x86_64-unknown-linux-musl`。

systemd 構成(`service`/`timer`/`path`)の配置手順は [CLAUDE.md](./CLAUDE.md) の「systemd 構成」「ohishi-data 運用メモ」を参照。

### Windows

[GitHub Releases](https://github.com/ohishi-exp/smb-watch/releases) から MSI をダウンロードして実行。`smb-watch.exe` が `C:\Program Files\smb-watch\` にインストールされます。

## ペアリング(初回のみ)

ブラウザを持たない box 上で完結する headless pairing:

```sh
# Linux (例)
sudo /opt/smb-watch/smb-watch pair --label ohishi-data --env-out /etc/smb-watch/smb-watch.env
```

実行すると承認 URL と確認コードが表示されます:

```
==> デバイスのペアリングを開始しました
    ブラウザで次の URL を開いて承認してください:
      https://auth.ippoan.org/device/pair/approve?user_code=XXXX-XXXX
    確認コード: XXXX-XXXX
```

1. URL を **スマホ/PC のブラウザ**で開く(box である必要はない)
2. **アップロード先テナントのアカウント**で auth-worker(Google)にログイン
3. 確認コードが端末表示と一致するのを確認 → 承認
4. box が poll で credential を受領し、`--env-out` のファイルへ `SMB_WATCH_DEVICE_ID` / `SMB_WATCH_DEVICE_SECRET` を **mode 600** で追記(`--env-out` 省略時は stdout に表示)

> 承認時のアカウントの tenant が、アップロード先データの tenant になります(approve したセッションの tenant_id が credential に焼かれる)。失効・端末退役は auth-worker `/device/revoke`。再ペアリングは同じ `pair` をもう一度。

## 実行

### Linux(systemd)

通常は timer に任せます(運用ホストでは平日 9〜17 時 JST を毎正時)。手動 1 回実行:

```sh
sudo systemctl start smb-watch.service                 # oneshot。完走まで待って返る
sudo tail -n3 /var/lib/smb-watch/last_run.txt          # 結果: ... found uploaded failed ok
sudo journalctl -u smb-watch.service -n50 --no-pager   # 詳細ログ
```

### 直接実行(任意 OS、デバッグ用)

```sh
SMB_USER=ユーザー SMB_PASS=パスワード \
SMB_WATCH_DEVICE_ID=... SMB_WATCH_DEVICE_SECRET=... \
smb-watch --smb-host 172.18.21.102 --smb-share 共有 --smb-path 新車検証
```

接続確認だけなら `--dry-run`(SMB スキャンのみ、認証・アップロードをスキップ)。

## オプション

| オプション | デフォルト | 環境変数 | 説明 |
|---|---|---|---|
| `--smb-host` | `172.18.21.102` | - | SMB サーバーのホスト名/IP |
| `--smb-share` | `共有` | - | SMB 共有名 |
| `--smb-path` | `新車検証` | - | 共有内の監視対象パス |
| `--smb-user` | - | `SMB_USER` | SMB 接続ユーザー名 |
| `--smb-pass` | - | `SMB_PASS` | SMB 接続パスワード |
| `--smb-domain` | `` | `SMB_DOMAIN` | SMB ドメイン名(省略可) |
| `--device-id` | - | `SMB_WATCH_DEVICE_ID` | device credential ID(pairing で取得) |
| `--device-secret` | - | `SMB_WATCH_DEVICE_SECRET` | device credential secret(pairing で取得、再取得不可) |
| `--auth-url` | `https://auth.ippoan.org` | `SMB_WATCH_AUTH_URL` | auth-worker(device JWT 発行 `/device/token`) |
| `--upload-url` | `https://carins.ippoan.org` | `SMB_WATCH_UPLOAD_URL` | アップロード先(`/api/device-upload`) |
| `--state-file` | `last_run.txt` | - | 実行履歴ファイル(watermark) |
| `--since` | - | - | 基準時刻を上書き(RFC3339、例 `2026-06-01T00:00:00Z`)。`last_run.txt` より優先 |
| `--dry-run` | `false` | - | アップロードせず検出のみ(認証もスキップ) |
| `--drive-letter` | `Z:` | - | (Windows) SMB マウントのドライブレター |
| `--local-path` | - | - | ローカルディレクトリを監視(SMB マウントをスキップ) |
| `--log-level` | `info` | - | ログレベル(trace/debug/info/warn/error) |

サブコマンド `pair` は上記とは別系統で、`--label` / `--env-out` を取ります(SMB を一切触らず pairing だけ行って終了)。

> SMB 資格情報・device credential は環境変数 or `/etc/smb-watch/smb-watch.env`(mode 600)で渡し、コマンドラインに直書きしないことを推奨します。

## 動作の流れ

1. SMB 共有をスキャン(Linux: pure-Rust 直アクセス / Windows: `--drive-letter` に `net use` マウント)
2. `last_run.txt` 最終行の start ts を基準時刻にする(`--since` 指定時はそちら優先)
3. 監視対象を再帰スキャンし、基準時刻以降に変更されたファイルを検出
4. 前回失敗分(`failed_files.txt`)と統合
5. device credential → `/device/token` で device JWT 取得 → `POST {upload-url}/api/device-upload` へ multipart アップロード
6. 失敗分を `failed_files.txt` に保存(次回リトライ)、実行記録を `last_run.txt` に追記

> **注意**: `--dry-run` でも `last_run.txt` の watermark は前進します。dry-run 後に通常実行すると dry-run 時点以前のファイルは拾われません。初回 backfill は `--since <古い時刻>` で明示してください。

## 状態ファイル

`--state-file`(既定 `last_run.txt`、Linux service では `/var/lib/smb-watch/` に配置)とその隣の `failed_files.txt`:

| ファイル | 説明 |
|---|---|
| `last_run.txt` | 実行履歴(TSV: start, end, found, uploaded, failed, status)。最終行の start を次回基準時刻に使う |
| `failed_files.txt` | アップロード失敗ファイルの一覧。全て成功すると削除される |

## トラブルシュート

- **`--device-id`/`--device-secret` 未設定で実行 → loud fail**: pairing が未完了。`pair` で credential を発行する(または `--dry-run` で認証なしスキャン)。
- **アップロードが 401/403**: credential 失効 or tenant 不一致。再ペアリングするか、承認時のアカウント tenant を確認。
- **`found=0` で何も上がらない**: 基準時刻以降に変更が無い。`--since` で過去に遡るか、対象パスを確認。
- **詳細ログ**: `--log-level debug`、Linux は `journalctl -u smb-watch.service`。

## 要件

- Linux x64(musl static binary、追加ランタイム不要)/ Windows x64
- SMB 共有への読み取りアクセス権
- pairing 済みの device credential(アップロード時)

## ビルド

```sh
# Linux (musl static)
cargo build --release --target x86_64-unknown-linux-musl

# Windows
cargo build --release --target x86_64-pc-windows-msvc
cargo wix --target x86_64-pc-windows-msvc   # MSI (WiX v3.11 が必要)

# テスト
cargo test
```

開発・デプロイ・systemd・運用ホスト固有の事項は [CLAUDE.md](./CLAUDE.md) を参照してください。
