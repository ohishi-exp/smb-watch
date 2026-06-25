#!/bin/bash
#
# 手動 deploy fallback。musl binary を build して remote host へ送る。
# 実 deploy ロジックは scripts/deploy-remote.sh に集約し、CI (CF Tunnel SSH 経路) と
# 共有している。緊急時 / 手元からの即時反映用。
#
# 使い方:
#   # Tailscale 直 (proxy / service token なし)
#   DEPLOY_SSH_HOST=<host>.ts.net ./deploy.sh
#
#   # CF Tunnel SSH を手元から使う場合 (cloudflared + service token が必要)
#   DEPLOY_SSH_HOST=ssh-smb-watch.mtamaramu.com \
#   DEPLOY_SSH_PROXY_COMMAND="cloudflared access ssh --hostname %h" \
#   CF_ACCESS_CLIENT_ID=... CF_ACCESS_CLIENT_SECRET=... ./deploy.sh
set -euo pipefail

cd "$(dirname "$0")"

echo "=== Building musl release binary ==="
cargo build --release --target x86_64-unknown-linux-musl

export DEPLOY_SSH_HOST="${DEPLOY_SSH_HOST:?DEPLOY_SSH_HOST is required (e.g. <host>.ts.net or ssh-smb-watch.mtamaramu.com)}"
export DEPLOY_SSH_USER="${DEPLOY_SSH_USER:-ubuntu}"

exec bash scripts/deploy-remote.sh
