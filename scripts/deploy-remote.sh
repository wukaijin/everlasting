#!/usr/bin/env bash
# scripts/deploy-remote.sh — 更新 remote 服务器上的 remote 二进制 + 前端 dist
#
# 配置(脱敏):真实 HOST/URL/DIR 放 scripts/.deploy-remote.env(gitignored),
# 脚本自动 source;仓库只有 .deploy-remote.env.example 模板。也支持 shell env
# 覆盖(REMOTE_HOST/REMOTE_URL/REMOTE_DIR)。
#
# 部署形态(默认,值在 .deploy-remote.env 里):
#   - <REMOTE_DIR>/everlasting-remote  (二进制,nohup 跑)
#   - <REMOTE_DIR>/dist                (前端 PWA 产物)
#   - <REMOTE_DIR>/start.sh            (启动脚本,含 shared_secret + DIST_DIR)
#   - /tmp/remote.log                  (nohup 日志)
#
# 用法:
#   ./scripts/deploy-remote.sh              # 全量:cargo build 二进制 + pnpm build dist + 上传 + 重启
#   ./scripts/deploy-remote.sh --no-binary  # 只更前端(不碰二进制,不重启,零中断)
#   ./scripts/deploy-remote.sh --no-frontend# 只更二进制(不碰前端,重启 ~2s)
#   ./scripts/deploy-remote.sh --no-build   # 都不 build(用现有产物,上传 + 重启)
#
# 选项:
#   --no-binary    只更前端:跳过 cargo build + 不上传二进制 + 不重启(ServeDir 实时)
#   --no-frontend  只更二进制:跳过 pnpm build + 不上传 dist + 重启(~2s 中断)
#   --no-build     都不 build(用现有产物)
#   -h | --help    显示本帮助
#
# 首次使用:
#   cp scripts/.deploy-remote.env.example scripts/.deploy-remote.env
#   # edit .deploy-remote.env 填真实 HOST/URL/DIR
#
# 中断:二进制更新 ~2s(PC tunnel 自动重连,手机请求重试);前端更新零中断。
# remote.db(token/devices/配对码)不动,无需重新配对。
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# ── 读本地配置(gitignored)─────────────────────────────────────
# .deploy-remote.env(gitignored)填真实值;.example 是模板(commit)。
ENV_FILE="$REPO_ROOT/scripts/.deploy-remote.env"
[ -f "$ENV_FILE" ] && { set -a; . "$ENV_FILE"; set +a; }

HOST="${REMOTE_HOST:-}"
REMOTE_URL="${REMOTE_URL:-}"
DIR="${REMOTE_DIR:-}"

[ -n "$HOST" ]       || { echo "✗ REMOTE_HOST 未设。cp scripts/.deploy-remote.env.example scripts/.deploy-remote.env 填值,或 export REMOTE_HOST=..."; exit 1; }
[ -n "$REMOTE_URL" ] || { echo "✗ REMOTE_URL 未设(同上)"; exit 1; }
[ -n "$DIR" ]        || { echo "✗ REMOTE_DIR 未设(同上)"; exit 1; }

BUILD_BIN=true
BUILD_FE=true
for a in "$@"; do
  case "$a" in
    --no-binary)   BUILD_BIN=false;;
    --no-frontend) BUILD_FE=false;;
    --no-build)    BUILD_BIN=false; BUILD_FE=false;;
    -h|--help)     sed -n '2,/^set -euo/p' "$0" | sed 's/^# \?//; /^set -euo/d'; exit 0;;
    *) echo "未知参数: $a(见 --help)"; exit 1;;
  esac
done

cd "$REPO_ROOT"

# ── 1. 本机 build ──────────────────────────────────────────────
if $BUILD_BIN; then
  echo "▸ cargo build --release -p everlasting-remote..."
  cargo build --release -p everlasting-remote
fi
if $BUILD_FE; then
  echo "▸ pnpm -C app build..."
  pnpm -C app build
fi

# ── 2. 上传(.new 暂存,防上传中断留半成品) ─────────────────────
if $BUILD_BIN; then
  [[ -f target/release/everlasting-remote ]] || { echo "✗ target/release/everlasting-remote 不存在"; exit 1; }
  echo "▸ scp 二进制 → $HOST:$DIR/everlasting-remote.new..."
  scp target/release/everlasting-remote "$HOST:$DIR/everlasting-remote.new"
fi
if $BUILD_FE; then
  [[ -d app/dist ]] || { echo "✗ app/dist 不存在"; exit 1; }
  echo "▸ scp dist → $HOST:$DIR/dist.new..."
  ssh "$HOST" "rm -rf '$DIR/dist.new'"
  scp -r app/dist "$HOST:$DIR/dist.new"
fi

# ── 3. 切换 + (按需)重启 ───────────────────────────────────────
# 二进制更新 → 重启(进程换);前端 dist 更新 → 不重启(ServeDir 实时读,切换即生效)。
if $BUILD_BIN; then
  echo "▸ 停旧 remote + 切换二进制 + 重启(中断 ~2s)..."
  ssh "$HOST" "
    set -e
    pkill -f 'everlasting-remote --port' 2>/dev/null || true
    sleep 1
    mv '$DIR/everlasting-remote.new' '$DIR/everlasting-remote'
    chmod +x '$DIR/everlasting-remote'
  "
  # 二进制更新时,如同时也更了前端,一并切 dist
  if $BUILD_FE; then
    ssh "$HOST" "rm -rf '$DIR/dist-old'; mv '$DIR/dist' '$DIR/dist-old'; mv '$DIR/dist.new' '$DIR/dist'; rm -rf '$DIR/dist-old'"
  fi
  ssh "$HOST" "nohup '$DIR/start.sh' > /tmp/remote.log 2>&1 < /dev/null & disown"
  sleep 3
elif $BUILD_FE; then
  echo "▸ 只前端:切换 dist(不重启,ServeDir 实时生效,零中断)..."
  ssh "$HOST" "rm -rf '$DIR/dist-old'; mv '$DIR/dist' '$DIR/dist-old'; mv '$DIR/dist.new' '$DIR/dist'; rm -rf '$DIR/dist-old'"
else
  echo "▸ 无变更(--no-build 且无 build,跳过)"
  exit 0
fi

# ── 4. 验证(health + 外网) ─────────────────────────────────────
echo "▸ 验证 health(localhost)..."
ssh "$HOST" 'curl -sS -m 3 http://localhost:7457/health 2>&1 && echo' || {
  echo "⚠ health 无响应,看日志:ssh $HOST 'tail -20 /tmp/remote.log'"
  exit 1
}
echo "▸ 验证外网($REMOTE_URL 经 nginx)..."
curl -sS -m 5 "$REMOTE_URL/health" && echo

echo "✅ 部署完成"
echo
echo "⚠ 前端更新后,手机若仍显示旧版 = service worker 缓存。让用户:"
echo "  - Safari:设置→Safari→清除历史与网站数据;或用无痕模式打开"
echo "  - 新版 SW(sw.js)会自动更新,下次打开(刷新两次)生效"
