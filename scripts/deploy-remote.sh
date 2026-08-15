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
#   --no-build     都不 build,但照常上传现有产物 + 重启
#   -h | --help    显示本帮助
#
# 进度:每步 [n/N] + 单步耗时,结尾总耗时。
# 测活:重启后轮询 localhost /health 最多 15s,起不来自动 tail /tmp/remote.log;
#       外网(nginx)验证失败自动重试 3 次。
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

# deploy 开关(部署哪些组件)与 build 开关(是否本机重新构建)分开:
# --no-build = 两个 build 都关但照常部署(用现有产物)。
DEPLOY_BIN=true; DEPLOY_FE=true
BUILD_BIN=true;  BUILD_FE=true
for a in "$@"; do
  case "$a" in
    --no-binary)   DEPLOY_BIN=false;;
    --no-frontend) DEPLOY_FE=false;;
    --no-build)    BUILD_BIN=false; BUILD_FE=false;;
    -h|--help)     sed -n '2,/^set -euo/p' "$0" | sed 's/^# \?//; /^set -euo/d'; exit 0;;
    *) echo "未知参数: $a(见 --help)"; exit 1;;
  esac
done

if ! $DEPLOY_BIN && ! $DEPLOY_FE; then
  echo "无变更(--no-binary + --no-frontend:啥都不部署)"; exit 0
fi

# ── 进度(步骤 [n/N] + 耗时)──────────────────────────────────
TOTAL=2   # 结尾固定两步:测活 + 外网验证
if $DEPLOY_BIN && $BUILD_BIN; then TOTAL=$((TOTAL+1)); fi
if $DEPLOY_FE   && $BUILD_FE;   then TOTAL=$((TOTAL+1)); fi
if $DEPLOY_BIN; then TOTAL=$((TOTAL+1)); fi
if $DEPLOY_FE;   then TOTAL=$((TOTAL+1)); fi
if $DEPLOY_BIN || $DEPLOY_FE; then TOTAL=$((TOTAL+1)); fi

START_T=$(date +%s); _T0=$START_T; STEP=0
step()      { STEP=$((STEP+1)); printf '\n▸ [%d/%d] %s\n' "$STEP" "$TOTAL" "$1"; _T0=$(date +%s); }
step_done() { printf '  ✓ 完成(%ds)\n' "$(( $(date +%s) - _T0 ))"; }
fmt_total() { local s=$(( $(date +%s) - START_T )); if [ "$s" -ge 60 ]; then printf '%dm%ds' $((s/60)) $((s%60)); else printf '%ds' "$s"; fi; }

cd "$REPO_ROOT"

# ── 1. 本机 build ─────────────────────────────────────────────
if $DEPLOY_BIN && $BUILD_BIN; then
  step "cargo build --release -p everlasting-remote"
  cargo build --release -p everlasting-remote
  step_done
fi
if $DEPLOY_FE && $BUILD_FE; then
  step "pnpm -C app build(前端 dist)"
  pnpm -C app build
  step_done
fi

# ── 2. 上传(.new 暂存,防上传中断留半成品) ─────────────────────
if $DEPLOY_BIN; then
  [[ -f target/release/everlasting-remote ]] || { echo "✗ target/release/everlasting-remote 不存在"; exit 1; }
  step "上传二进制($(du -h target/release/everlasting-remote | cut -f1))→ $HOST:$DIR/everlasting-remote.new"
  scp target/release/everlasting-remote "$HOST:$DIR/everlasting-remote.new"
  step_done
fi
if $DEPLOY_FE; then
  [[ -d app/dist ]] || { echo "✗ app/dist 不存在"; exit 1; }
  step "上传前端($(du -sh app/dist | cut -f1))→ $HOST:$DIR/dist.new"
  ssh "$HOST" "rm -rf '$DIR/dist.new'"
  scp -r app/dist "$HOST:$DIR/dist.new"
  step_done
fi

# ── 3. 切换 + (按需)重启 ───────────────────────────────────────
# 二进制更新 → 重启(进程换);前端 dist 更新 → 不重启(ServeDir 实时读,切换即生效)。
# pkill pattern 写成 '[e]verlasting-remote --port':pkill -f 按完整 cmdline 匹配,
# 本 ssh 远端 shell 的 cmdline 里含 pattern 原文,不带 [] 的话 pkill 会把它自己也
# 杀掉,后续 mv/重启全部不执行(2026-08-16 线上 502 事故根因)。
if $DEPLOY_BIN; then
  step "停旧 remote + 切换二进制 + 重启(中断 ~2s)"
  ssh "$HOST" "
    set -e
    pkill -f '[e]verlasting-remote --port' 2>/dev/null || true
    for _i in 1 2 3 4 5; do
      pgrep -f '[e]verlasting-remote --port' >/dev/null 2>&1 || break
      sleep 1
    done
    mv '$DIR/everlasting-remote.new' '$DIR/everlasting-remote'
    chmod +x '$DIR/everlasting-remote'
  "
  # 二进制更新时,如同时也更了前端,一并切 dist
  if $DEPLOY_FE; then
    ssh "$HOST" "rm -rf '$DIR/dist-old'; mv '$DIR/dist' '$DIR/dist-old'; mv '$DIR/dist.new' '$DIR/dist'; rm -rf '$DIR/dist-old'"
  fi
  ssh "$HOST" "nohup '$DIR/start.sh' > /tmp/remote.log 2>&1 < /dev/null & disown"
  step_done
elif $DEPLOY_FE; then
  step "只前端:切换 dist(不重启,ServeDir 实时生效,零中断)"
  ssh "$HOST" "rm -rf '$DIR/dist-old'; mv '$DIR/dist' '$DIR/dist-old'; mv '$DIR/dist.new' '$DIR/dist'; rm -rf '$DIR/dist-old'"
  step_done
fi

# ── 4. 测活 + 外网验证 ─────────────────────────────────────────
ALIVE_TIMEOUT=15

step "测活:轮询 localhost:7457/health(最多 ${ALIVE_TIMEOUT}s)"
_alive=""
_waited=-1
while [ "$_waited" -lt "$ALIVE_TIMEOUT" ]; do
  _waited=$((_waited+1))
  if _h=$(ssh "$HOST" 'curl -fsS -m 2 http://localhost:7457/health' 2>/dev/null); then
    _alive=1; break
  fi
  sleep 1
done
if [ -n "$_alive" ]; then
  printf '  ✓ 服务存活(等待 %ds):%s\n' "$_waited" "$_h"
  step_done
else
  printf '  ✗ %ds 内未存活,自动 tail /tmp/remote.log:\n' "$ALIVE_TIMEOUT"
  ssh "$HOST" 'tail -30 /tmp/remote.log' || true
  exit 1
fi

step "验证外网($REMOTE_URL 经 nginx,重试 3 次)"
_ext=""
for _i in 1 2 3; do
  if _e=$(curl -fsS -m 5 "$REMOTE_URL/health" 2>/dev/null); then _ext=1; break; fi
  if [ "$_i" -lt 3 ]; then sleep 1; fi
done
if [ -z "$_ext" ]; then
  echo "  ✗ 外网 health 失败(本机活着但外网不通 → 查 nginx 配置/上游端口)"
  exit 1
fi
printf '  ✓ %s\n' "$_e"
step_done

echo
echo "✅ 部署完成(总耗时 $(fmt_total))"

if $DEPLOY_FE; then
  echo
  echo "⚠ 前端更新后,手机若仍显示旧版 = service worker 缓存。让用户:"
  echo "  - Safari:设置→Safari→清除历史与网站数据;或用无痕模式打开"
  echo "  - 新版 SW(sw.js)会自动更新,下次打开(刷新两次)生效"
fi
