#!/bin/bash
#
# musl static binary を remote host へ転送し、systemd PathModified watcher
# (smb-watch-watcher.path) に拾わせる共通 deploy ロジック。
# (rust-ichibanboshi の scripts/deploy-remote.sh を、常駐サーバではない
#  ワンショット tool 向けに調整したもの)
#
# smb-watch は HTTP サーバではないので /health は無い。代わりに deploy 検証として
#   1. `smb-watch --version` を remote で実行 → exit 0 かつ焼き込み BUILD_SHA を確認
#      (= 新 binary が配置され実行可能。EXPECTED_BUILD_SHA 指定時は突合して loud fail)
#   2. last_run.txt 末尾を tail → 直近 run の status を可視化 (失敗は ::warning::)
# を行う。
#
# 経路 (Tailscale 直 / Cloudflare Tunnel SSH) は env で切替える:
#   - deploy.sh (手動 fallback)  … DEPLOY_SSH_HOST=<tailscale MagicDNS>
#   - ci.yml deploy job (自動)   … DEPLOY_SSH_HOST=ssh-smb-watch.mtamaramu.com
#                                   DEPLOY_SSH_PROXY_COMMAND="cloudflared access ssh --hostname %h"
#                                   CF_ACCESS_CLIENT_ID / CF_ACCESS_CLIENT_SECRET (service token)
#
# 必須 env:
#   DEPLOY_SSH_HOST            … 接続先 SSH ホスト名
#
# 任意 env:
#   DEPLOY_SSH_USER           … SSH ユーザー (default: ubuntu)
#   DEPLOY_TARGET_DIR         … binary インストール先 (default: /opt/smb-watch)
#   DEPLOY_STATE_DIR          … 状態ファイル dir (default: /var/lib/smb-watch)
#   DEPLOY_BINARY             … 転送する binary path
#                               (default: target/x86_64-unknown-linux-musl/release/smb-watch)
#   DEPLOY_SSH_KEY_FILE       … 秘密鍵 path (未指定なら ssh-agent / 既定鍵)
#   DEPLOY_SSH_PROXY_COMMAND  … ssh -o ProxyCommand=<...> に渡す値
#   EXPECTED_BUILD_SHA        … 突合する短縮 SHA (CI は github.sha[:7])。
#                               一致しなければ loud fail (古い binary が残った検出)
#   CF_ACCESS_CLIENT_ID       … CF Access service token id  (cloudflared が読む)
#   CF_ACCESS_CLIENT_SECRET   … CF Access service token secret
#
# deploy 失敗 (build 不在 / scp / ssh / version 不一致) は即 exit != 0 で loud fail。
set -euo pipefail

SSH_USER="${DEPLOY_SSH_USER:-ubuntu}"
TARGET_HOST="${DEPLOY_SSH_HOST:?DEPLOY_SSH_HOST is required}"
TARGET="$SSH_USER@$TARGET_HOST"
TARGET_DIR="${DEPLOY_TARGET_DIR:-/opt/smb-watch}"
STATE_DIR="${DEPLOY_STATE_DIR:-/var/lib/smb-watch}"
BINARY="${DEPLOY_BINARY:-target/x86_64-unknown-linux-musl/release/smb-watch}"

if [[ ! -f "$BINARY" ]]; then
  echo "::error::deploy binary not found: $BINARY" >&2
  exit 1
fi

# CF Access service token は cloudflared が TUNNEL_SERVICE_TOKEN_* env を読む。
if [[ -n "${CF_ACCESS_CLIENT_ID:-}" ]]; then
  export TUNNEL_SERVICE_TOKEN_ID="$CF_ACCESS_CLIENT_ID"
fi
if [[ -n "${CF_ACCESS_CLIENT_SECRET:-}" ]]; then
  export TUNNEL_SERVICE_TOKEN_SECRET="$CF_ACCESS_CLIENT_SECRET"
fi

SSH_OPTS=(-o StrictHostKeyChecking=accept-new -o BatchMode=yes)
if [[ -n "${DEPLOY_SSH_KEY_FILE:-}" ]]; then
  SSH_OPTS+=(-i "$DEPLOY_SSH_KEY_FILE" -o IdentitiesOnly=yes)
fi
if [[ -n "${DEPLOY_SSH_PROXY_COMMAND:-}" ]]; then
  SSH_OPTS+=(-o "ProxyCommand=$DEPLOY_SSH_PROXY_COMMAND")
fi

echo "=== Deploying $BINARY to $TARGET ($TARGET_DIR) ==="
# 実行中 (timer 発火中) でも上書きできるよう /tmp 経由で mv (mv はアトミック)。
scp "${SSH_OPTS[@]}" "$BINARY" "$TARGET:/tmp/smb-watch.new"
ssh "${SSH_OPTS[@]}" "$TARGET" \
  "mv /tmp/smb-watch.new $TARGET_DIR/smb-watch && chmod +x $TARGET_DIR/smb-watch"

# smb-watch-watcher.path (PathModified) が binary 変更を検知して次回 / 即時 run する。
echo "=== Verifying deployed binary (--version) ==="
VERSION_OUT="$(ssh "${SSH_OPTS[@]}" "$TARGET" \
  "$TARGET_DIR/smb-watch --version" 2>&1)" || {
  echo "::error::'smb-watch --version' failed on $TARGET_HOST" >&2
  echo "$VERSION_OUT" >&2
  exit 1
}
echo "deployed: $VERSION_OUT"

if [[ -n "${EXPECTED_BUILD_SHA:-}" ]]; then
  if [[ "$VERSION_OUT" != *"$EXPECTED_BUILD_SHA"* ]]; then
    echo "::error::deployed binary SHA mismatch — expected '$EXPECTED_BUILD_SHA', got: $VERSION_OUT" >&2
    exit 1
  fi
  echo "build SHA matches expected ($EXPECTED_BUILD_SHA)"
fi

# 直近 run の状態を可視化する (silent failure 検知)。last_run.txt が無くても fatal にしない。
echo "=== Last run record (tail of last_run.txt) ==="
LAST_RUN="$(ssh "${SSH_OPTS[@]}" "$TARGET" \
  "tail -n1 $STATE_DIR/last_run.txt 2>/dev/null || true")"
if [[ -z "$LAST_RUN" ]]; then
  echo "(no last_run.txt yet — まだ 1 度も走っていない可能性)"
else
  echo "last run: $LAST_RUN"
  # 形式: start \t end \t found \t uploaded \t failed \t status
  STATUS_FIELD="$(printf '%s' "$LAST_RUN" | awk -F'\t' '{print $6}')"
  FAILED_FIELD="$(printf '%s' "$LAST_RUN" | awk -F'\t' '{print $5}')"
  if [[ "$STATUS_FIELD" != "ok" && "$STATUS_FIELD" != "dry-run" ]] || [[ "${FAILED_FIELD:-0}" =~ ^[0-9]+$ && "${FAILED_FIELD:-0}" -gt 0 ]]; then
    echo "::warning::直近 run が status='$STATUS_FIELD' failed='$FAILED_FIELD' — 認証 / アップロード失敗の可能性"
  fi
fi

# CI の Step Summary に deploy 情報を出す。
if [[ -n "${GITHUB_STEP_SUMMARY:-}" ]]; then
  {
    echo "### ✅ Deploy 成功 — ${TARGET_HOST}:${TARGET_DIR}"
    echo ""
    echo "- version: \`${VERSION_OUT}\`"
    echo "- last run: \`${LAST_RUN:-<none>}\`"
  } >> "$GITHUB_STEP_SUMMARY"
fi

echo "=== Done! deployed to $TARGET_HOST:$TARGET_DIR ==="
