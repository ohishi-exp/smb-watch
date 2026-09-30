# smb-watch

SMB 共有の新しいファイルを Cloudflare Worker が Workers VPC + Tunnel 経由で読み、
auth-worker の Service Binding で carins に取り込み、LINE WORKS に通知する。
構成・設定・検査・本番への切り替えは [workers/smb-ingest/README.md](workers/smb-ingest/README.md) を参照。

## box 版 (native) の退役

以前は社内の box で native binary (この repo のルートの Rust crate) が systemd timer から動き、
SMB を走査してファイルを HTTP でアップロードしていた。取り込みが Worker (`workers/smb-ingest`) に
移り本番で稼働したため、native のコード・box への自動 deploy・Windows MSI のリリースを repo から削除した (#17 #15 #14)。
過去の `v*.*.*` タグと GitHub Releases は履歴として残す。

退役手順 (repo の外で行う本番操作):

- [ ] box の `smb-watch.timer` / `smb-watch-watcher.path` を `systemctl disable --now`
- [ ] smb-watch の device credential を auth-worker `/device/revoke` で失効
- [ ] auth-worker の KV `device-notify-targets` の box 用キー (`device-uploader`) は、同じ role の有効な device が他にあれば残す (キーを消すとそれらの通知も止まる)。Worker 版の通知は別キー (`smb-ingest`) を使う
- [ ] box 上の env ファイル・バイナリ・systemd unit ファイル・状態ファイルの削除
- [ ] box への自動 deploy 経路の撤去
  - host の authorized_keys にある deploy 用の鍵
  - Cloudflare Access の service token
  - Tunnel の SSH 用 public hostname
  - repo の secrets `DEPLOY_SSH_KEY` / `CF_ACCESS_CLIENT_ID` / `CF_ACCESS_CLIENT_SECRET` / `DEFAULT_GOOGLE_CLIENT_ID` / `DEFAULT_GOOGLE_CLIENT_SECRET` と vars `DEPLOY_SSH_HOST`
- [ ] 過去の `v*.*.*` タグと GitHub Releases は残す (履歴)
