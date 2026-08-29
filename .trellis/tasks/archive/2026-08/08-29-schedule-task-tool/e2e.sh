#!/usr/bin/env bash
# E2E 冒烟(08-29-schedule-task-tool 步骤 7):经 HTTP daemon 跑真实 LLM
# turn,验证 LLM 异步消费 schedule_task 的 tool_result 并响应:
#   turn 1: 要求 LLM 用 schedule_task 建任务(interval 10080min + max_runs 1,
#           7 天后才到期,E2E 窗口内零 fire 风险)→ 断言
#           ① scheduled_tasks 新行 created_by='agent';
#           ② 最终 assistant 回复**原样包含任务 id**(id 服务端生成,
#             LLM 只能从 tool_result 读到 → 消费链实证)。
#   turn 2: 要求 LLM schedule_status 列出 + schedule_cancel 取消 → 断言行已删。
# 清理:临时 session 删除(EXIT trap);任务由 turn 2 的 cancel 删除(若
# turn 2 失败,cleanup 兜底按 DB 里查到的 id 删)。
#
# 前置:daemon(新二进制)在 :7456、默认 model 可用、sqlite3/python3。
# 用法:.trellis/tasks/08-29-schedule-task-tool/e2e.sh
set -euo pipefail

BASE="http://127.0.0.1:7456"
PROJECT_PATH="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
DB_PATH="${XDG_DATA_HOME:-$HOME/.local/share}/dev.everlasting.app/everlasting.db"
TIMEOUT=300

command -v sqlite3 >/dev/null || { echo "ERR: sqlite3 not found" >&2; exit 1; }
curl -sf -m 3 "$BASE/health" >/dev/null || { echo "ERR: daemon not reachable" >&2; exit 1; }

SID=""
SSE_LOG="$(mktemp /tmp/sched-e2e-sse.XXXXXX)"
SSE_PID=""
TASK_ID=""
cleanup() {
  [ -n "$SSE_PID" ] && kill "$SSE_PID" 2>/dev/null || true
  # 任务兜底清理(turn 2 cancel 失败时)。
  if [ -n "$TASK_ID" ]; then
    curl -s -X POST "$BASE/api/v1/scheduled_tasks/delete_scheduled_task" \
      -H 'Content-Type: application/json' -d "{\"id\":\"$TASK_ID\"}" >/dev/null 2>&1 || true
  fi
  [ -n "$SID" ] && curl -s -X POST "$BASE/api/v1/sessions/delete_session" \
    -H 'Content-Type: application/json' -d "{\"session_id\":\"$SID\"}" >/dev/null 2>&1 || true
  rm -f "$SSE_LOG"
}
trap cleanup EXIT

# 0. 常驻 SSE 订阅(须先于发消息;RULE-SMOKE-001 同款)。
curl -sN "$BASE/api/v1/stream" >"$SSE_LOG" 2>/dev/null &
SSE_PID=$!

# 1. 解析 project(本仓库路径)+ 建临时 session。
PROJ_ID="$(curl -sf -X POST "$BASE/api/v1/projects/list_projects" \
  -H 'Content-Type: application/json' -d '{}' | WANT="$PROJECT_PATH" python3 -c '
import json,sys,os,pathlib
want=pathlib.PurePath(os.environ["WANT"])
for p in json.load(sys.stdin):
    if pathlib.PurePath(p["path"])==want:
        print(p["id"]); break
')"
SID="$(curl -sf -X POST "$BASE/api/v1/sessions/create_session" \
  -H 'Content-Type: application/json' \
  -d "{\"project_id\":\"$PROJ_ID\",\"initial_cwd\":\"$PROJECT_PATH\"}" \
  | python3 -c 'import json,sys; print(json.load(sys.stdin)["id"])')"
echo "session: $SID (project $PROJ_ID)"

# 2. 发消息并等本请求终态(kind=done)。
send_and_wait() {
  local MSG="$1" REQ_ID="sched-e2e-$(date +%s)-$2"
  printf '%s' "$(MSG="$MSG" REQ_ID="$REQ_ID" SID="$SID" python3 -c '
import json,os
print(json.dumps({"request_id":os.environ["REQ_ID"],"session_id":os.environ["SID"],
                  "messages":[{"role":"user","content":os.environ["MSG"]}]}))')" \
  | curl -sf -X POST "$BASE/api/v1/agent/chat" -H 'Content-Type: application/json' -d @- >/dev/null
  echo "turn $2 sent ($REQ_ID), waiting terminal event..."
  local ELAPSED=0 LINE
  while [ "$ELAPSED" -lt "$TIMEOUT" ]; do
    LINE="$(grep -a "\"request_id\":\"$REQ_ID\"" "$SSE_LOG" 2>/dev/null \
      | grep -E '"kind":"(done|error)"' | tail -n 1 || true)"
    if [ -n "$LINE" ]; then
      echo "$LINE" | grep -q '"kind":"error"' && {
        echo "ERR: turn $2 ended with kind=error" >&2; return 1; }
      echo "turn $2 done"; return 0
    fi
    sleep 5; ELAPSED=$((ELAPSED+5))
  done
  echo "ERR: turn $2 no terminal event in ${TIMEOUT}s" >&2; return 1
}

last_assistant_text() {
  SID="$SID" DB_PATH="$DB_PATH" python3 -c '
import os, sqlite3
db = os.environ["DB_PATH"]
sid = os.environ["SID"]
con = sqlite3.connect("file:" + db + "?mode=ro", uri=True)
row = con.execute(
    "SELECT text FROM messages WHERE session_id=? AND role='\''assistant'\'' "
    "ORDER BY seq DESC LIMIT 1", (sid,)).fetchone()
print(row[0] if row else "")'
}

# ── turn 1:create ──────────────────────────────────────────────────────
send_and_wait "请用 schedule_task 工具创建一个定时任务:名字叫「E2E 冒烟任务」,\
提示词是「这是冒烟测试,请只回复:冒烟完成」,调度为 interval 每 10080 分钟一次,\
最多运行 1 次(max_runs=1)。如果工具要求先 load schema 就先调用 load_tool_schemas。\
创建成功后,把工具返回的任务 id(task_id)原样告诉我,并说明下次触发时间。" 1

ROW="$(sqlite3 -readonly "$DB_PATH" \
  "SELECT id || '|' || created_by || '|' || max_runs FROM scheduled_tasks \
   WHERE name='E2E 冒烟任务' ORDER BY created_at DESC LIMIT 1;")"
[ -n "$ROW" ] || { echo "ERR: no scheduled_tasks row created" >&2; exit 1; }
TASK_ID="${ROW%%|*}"
CREATED_BY="$(printf '%s' "$ROW" | cut -d'|' -f2)"
MAX_RUNS="$(printf '%s' "$ROW" | cut -d'|' -f3)"
echo "row: id=$TASK_ID created_by=$CREATED_BY max_runs=$MAX_RUNS"
[ "$CREATED_BY" = "agent" ] || { echo "ERR: created_by=$CREATED_BY != agent" >&2; exit 1; }
[ "$MAX_RUNS" = "1" ] || { echo "ERR: max_runs=$MAX_RUNS != 1" >&2; exit 1; }

REPLY="$(last_assistant_text)"
echo "assistant reply (turn 1, 前 200 字): ${REPLY:0:200}"
case "$REPLY" in
  *"$TASK_ID"*) echo "PASS(turn1): assistant 原样复述 task_id → tool_result 消费实证" ;;
  *) echo "ERR: assistant reply does not contain task_id $TASK_ID" >&2; exit 1 ;;
esac

# ── turn 2:list + cancel 闭环 ─────────────────────────────────────────
send_and_wait "请用 schedule_status 查看你在这个项目创建的定时任务,找到「E2E 冒烟任务」\
并用 schedule_cancel 把它取消掉。完成后告诉我取消结果。" 2

COUNT="$(sqlite3 -readonly "$DB_PATH" \
  "SELECT COUNT(*) FROM scheduled_tasks WHERE id='$TASK_ID';")"
[ "$COUNT" = "0" ] || { echo "ERR: task $TASK_ID still exists after cancel" >&2; exit 1; }
REPLY2="$(last_assistant_text)"
echo "assistant reply (turn 2, 前 200 字): ${REPLY2:0:200}"
case "$REPLY2" in
  *已*取消*|*取消*成功*|*cancel*) echo "PASS(turn2): 行已删 + assistant 确认取消" ;;
  *) echo "ERR: turn 2 reply lacks cancellation ack" >&2; exit 1 ;;
esac

TASK_ID=""  # 已删,cleanup 兜底不再重复
echo "E2E OK"
