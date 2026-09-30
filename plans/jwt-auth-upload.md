# smb-watch アップロード認証基盤 構築計画

## Context

smb-watch から `<carins-host>/api/recieve` にアップロードすると Cloudflare Access (Zero Trust) でブロックされる。
rust-logi に JWT 認証を追加し、専用 Worker 経由でファイルをアップロードする経路を構築する。

## アーキテクチャ

```
                        smb-upload-worker
                     (Cloudflare Worker, 新規リポ)
                     ┌──────────────────────┐
                     │  POST /auth/login    │
smb-watch ──────────►│  POST /upload        │
                     └──────┬───────────────┘
                            │ Service Binding
                            ▼
                       cf-grpc-proxy
                            │ gRPC-Web / Connect
                            ▼
                        rust-logi (Cloud Run)
                     ┌──────────────────────┐
                     │  AuthService/Login   │ → JWT 発行 + DB 保存
                     │  FilesService/Create │ → JWT 検証 + GCS 保存
                     └──────────────────────┘
```

rust-logi 側の変更は [rust-logi-auth.md](rust-logi-auth.md) を参照。

---

## smb-upload-worker (新規リポ)

- [ ] リポジトリ作成 (`smb-upload-worker`)
- [ ] `wrangler.toml` 作成
  - name: `smb-upload-worker`
  - Service Binding: `GRPC_PROXY_SERVICE` → `cf-grpc-proxy`
- [ ] `POST /auth/login` 実装
  - Body: `{ "username", "password" }`
  - → cf-grpc-proxy → rust-logi `AuthService/Login`
  - → `{ "token", "expiresAt" }`
- [ ] `POST /upload` 実装
  - Headers: `Authorization: Bearer <JWT>`
  - Body: multipart/form-data (`data` = ファイル)
  - Worker で base64 変換 → cf-grpc-proxy → rust-logi `FilesService/CreateFile`
  - → `{ "uuid", "message" }`（smb-watch UploadResponse 互換）
- [ ] `wrangler deploy`

---

## smb-watch の変更

### CLI オプション追加 (`src/cli.rs`)

- [x] `--auth-user` 追加 (`env: SMB_WATCH_AUTH_USER`)
- [x] `--auth-pass` 追加 (`env: SMB_WATCH_AUTH_PASS`)
- [x] `--auth-url` 追加 (`env: SMB_WATCH_AUTH_URL`)

### 認証モジュール新規作成 (`src/auth.rs`)

- [x] `login(client, auth_url, user, pass) -> Result<String>` — JWT 取得
- [x] `mod auth` を `main.rs` に追加

### アップローダー変更 (`src/uploader.rs`)

- [x] `upload_file()` に `token: Option<&str>` パラメータ追加
- [x] リクエストに `Authorization: Bearer <JWT>` ヘッダー付与

### メイン処理変更 (`src/main.rs`)

- [x] 起動時に `auth::login()` で JWT 取得
- [x] JWT を `upload_file()` に渡す

---

## 検証

- [ ] `curl` で Worker `POST /auth/login` → JWT 取得
- [ ] `curl` で Worker `POST /upload` → JWT 付きファイルアップロード
- [ ] smb-watch で `--since 2026-02-09T05:03:00Z` の 2 ファイルでテスト
- [ ] サーバー側 (logi DB / GCS) でファイル到達を確認
