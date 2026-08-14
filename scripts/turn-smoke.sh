#!/usr/bin/env bash
# scripts/turn-smoke.sh — daemon 单轮烟测(经 HTTP API 跑一轮真实 LLM turn,
# 轮询 turn_trace 落库结果并报 per-turn token)。
#
# 源起 C7(08-14)AC1 live 烟测的手工流程沉淀:建临时 session → 发一句
# 问候 → 等 turn_trace 行出现 → 报 tools_token / context_input / 占比 →
# 删 session。以后任何"改了 agent loop / trace / tools 链路,想实跑一轮
# 看真实落库"的场景都可以用它,不必手翻 DB。
#
# 前置:
#   - daemon 在跑(默认 :7456;`./scripts/daemon.sh bg` 可拉起)
#   - 默认 model 已配好 key(跑一轮真实 LLM 调用,input ~1 万多 token)
#   - sqlite3 + python3 可用
#
# 用法:
#   ./scripts/turn-smoke.sh                     # 默认:本仓库 project + 一句问候
#   ./scripts/turn-smoke.sh --port 7457         # 指定 daemon 端口
#   ./scripts/turn-smoke.sh --project-path /x   # 指定 project(不在列表则自动创建)
#   ./scripts/turn-smoke.sh --message "..."     # 自定义消息
#   ./scripts/turn-smoke.sh --keep              # 保留烟测 session(默认跑完即删)
#   ./scripts/turn-smoke.sh --timeout 300       # 等 turn_trace 的超时秒数(默认 180)
#
# DB 路径见 AGENTS.md「DB / 烟测速查」;只读查询,daemon 可继续跑。
set -euo pipefail

PORT=7456
PROJECT_PATH="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MESSAGE="早上好,请只回一句问候,不要调用任何工具"
TIMEOUT=180
KEEP=0
while [ $# -gt 0 ]; do
  case "$1" in
    --port) PORT="$2"; shift 2 ;;
    --project-path) PROJECT_PATH="$2"; shift 2 ;;
    --message) MESSAGE="$2"; shift 2 ;;
    --timeout) TIMEOUT="$2"; shift 2 ;;
    --keep) KEEP=1; shift ;;
    -h|--help) grep '^#' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown arg: $1 (see --help)" >&2; exit 2 ;;
  esac
done

BASE="http://127.0.0.1:${PORT}"
DB_PATH="${EVERLASTING_SMOKE_DB:-${XDG_DATA_HOME:-$HOME/.local/share}/dev.everlasting.app/everlasting.db}"

command -v sqlite3 >/dev/null || { echo "ERR: sqlite3 not found" >&2; exit 2; }
command -v python3 >/dev/null || { echo "ERR: python3 not found" >&2; exit 2; }
curl -sf -m 3 "$BASE/health" >/dev/null || { echo "ERR: daemon not reachable at $BASE (./scripts/daemon.sh bg)" >&2; exit 2; }
[ -f "$DB_PATH" ] || { echo "ERR: DB not found at $DB_PATH (see AGENTS.md / docs/DEBUG_DB.md)" >&2; exit 2; }

SID=""
cleanup() {
  if [ -n "$SID" ] && [ "$KEEP" != "1" ]; then
    curl -s -X POST "$BASE/api/v1/sessions/delete_session" \
      -H 'Content-Type: application/json' -d "{\"session_id\":\"$SID\"}" >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT

# ── 1. 解析 project(按路径匹配,查不到则创建) ─────────────────────────
PROJ_ID="$(curl -sf -X POST "$BASE/api/v1/projects/list_projects" \
  -H 'Content-Type: application/json' -d '{}' | WANT="$PROJECT_PATH" python3 -c '
import json,sys,os,pathlib
want=pathlib.PurePath(os.environ["WANT"])
for p in json.load(sys.stdin):
    if pathlib.PurePath(p["path"])==want:
        print(p["id"]); break
' 2>/dev/null || true)"
if [ -z "$PROJ_ID" ]; then
  echo "project not in list, creating: $PROJECT_PATH"
  PROJ_ID="$(curl -sf -X POST "$BASE/api/v1/projects/create_project" \
    -H 'Content-Type: application/json' -d "{\"path\":\"$PROJECT_PATH\"}" \
    | python3 -c 'import json,sys; print(json.load(sys.stdin)["id"])')"
fi

# ── 2. 建临时 session + 发单轮 ────────────────────────────────────────
SID="$(curl -sf -X POST "$BASE/api/v1/sessions/create_session" \
  -H 'Content-Type: application/json' \
  -d "{\"project_id\":\"$PROJ_ID\",\"initial_cwd\":\"$PROJECT_PATH\"}" \
  | python3 -c 'import json,sys; print(json.load(sys.stdin)["id"])')"
echo "session: $SID"

REQ_ID="turn-smoke-$(date +%s)"
BODY="$(MESSAGE="$MESSAGE" REQ_ID="$REQ_ID" SID="$SID" python3 -c '
import json,os
print(json.dumps({"request_id":os.environ["REQ_ID"],"session_id":os.environ["SID"],
                  "messages":[{"role":"user","content":os.environ["MESSAGE"]}]}))
')"
curl -sf -X POST "$BASE/api/v1/agent/chat" -H 'Content-Type: application/json' -d "$BODY" >/dev/null
echo "turn sent (request_id=$REQ_ID), polling turn_trace..."

# ── 3. 轮询 turn_trace(工具列缺失 = daemon 二进制早于 C7 R1) ──────────
ELAPSED=0; INTERVAL=5
while [ "$ELAPSED" -lt "$TIMEOUT" ]; do
  ROW="$(sqlite3 -readonly "$DB_PATH" \
    "SELECT seq FROM turn_trace WHERE session_id='$SID' ORDER BY seq DESC LIMIT 1;" 2>/dev/null || true)"
  [ -n "$ROW" ] && break
  sleep "$INTERVAL"; ELAPSED=$((ELAPSED+INTERVAL))
done

if ! sqlite3 -readonly "$DB_PATH" "SELECT tools_token FROM turn_trace LIMIT 1;" >/dev/null 2>&1; then
  echo "ERR: turn_trace has no tools_token column — daemon binary predates C7 R1 (rebuild)" >&2; exit 1
fi
if [ -z "${ROW:-}" ]; then
  echo "ERR: no turn_trace row after ${TIMEOUT}s (check daemon logs: ./scripts/daemon.sh logs)" >&2; exit 1
fi

# ── 4. 报告 ───────────────────────────────────────────────────────────
sqlite3 -readonly -header -column "$DB_PATH" \
  "SELECT seq, tools_token,
          json_extract(token_usage_json,'\$.input_tokens')      AS input_tok,
          json_extract(token_usage_json,'\$.output_tokens')     AS output_tok,
          json_extract(token_usage_json,'\$.cache_read_input_tokens') AS cache_read,
          json_extract(token_usage_json,'\$.context_input_tokens')    AS ctx_input,
          CAST(ROUND(100.0*tools_token/json_extract(token_usage_json,'\$.context_input_tokens')) AS INT) AS tools_pct
   FROM turn_trace WHERE session_id='$SID' ORDER BY seq;"
MSG_COUNT="$(sqlite3 -readonly "$DB_PATH" "SELECT COUNT(*) FROM messages WHERE session_id='$SID';")"
echo "messages persisted: $MSG_COUNT (user+assistant)"
[ "$KEEP" = "1" ] && echo "session kept: $SID" || echo "smoke session deleted"
echo "OK"
