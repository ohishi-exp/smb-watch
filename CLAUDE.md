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

## 主なコマンド

```powershell
cargo build --release --target x86_64-pc-windows-msvc   # ビルド (Windows)
cargo release patch --execute                            # リリース (tag push → GitHub Release)
```

Linux 側は `ci.yml` の `deploy` job が `main` merge 時に musl build → SSH で自動デプロイする
（常駐しない oneshot、systemd timer で定期実行）。

## 必ず守ること

- **`wix/main.wxs` の `UpgradeCode` (`D802E510-9F08-408B-BFFD-B0B491E7F908`) は変更禁止。**
  変更するとバージョンアップ時に別製品として扱われる。
- **SMB 資格情報 (`SMB_USER`/`SMB_PASS` 等) は host の `/etc/smb-watch/smb-watch.env` にだけ
  置き、GitHub Actions / workflow YAML には載せない**（host boundary に閉じる）。
- **<運用ホスト> の host TZ (UTC) は変更しない**（時刻依存の業務 cron `backup`/`update_ichi` が
  9h ずれるため）。JST スケジュールは systemd timer 側を UTC 記述で表現する。
- device credential (`--device-id`/`--device-secret`) が未設定だと upload は loud fail する
  （想定挙動、隠さず落とす）。`--dry-run` は SMB 走査のみで認証・upload をスキップする。
- `smb-watch pair` は SMB を一切触らず pairing のみ行う（subcommand 無しの通常 run と排他）。
  `device_secret` を log には出さない。

## 詳細

SMB アクセス方式 (OS 別)・設定パラメータ・device-token 認証・開発環境セットアップ・
ローカルビルド・リリース手順・CI/CD・Linux 自動デプロイ (systemd 構成・CF Tunnel SSH・
<運用ホスト> 実機運用メモ・device pairing 実機手順)・WiX MSI インストーラの詳細は
`smb-watch-map` skill を参照。
