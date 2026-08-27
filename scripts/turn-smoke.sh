#!/usr/bin/env bash
# scripts/turn-smoke.sh — daemon 单轮烟测(经 HTTP API 跑一轮真实 LLM turn,
# 等 SSE 请求终态后报 turn_trace 落库的 per-turn token)。
#
# 源起 C7(08-14)AC1 live 烟测的手工流程沉淀:建临时 session → 发一句
# 问候 → 等 turn_trace 行出现 → 报 tools_token / context_input / 占比 →
# 删 session。以后任何"改了 agent loop / trace / tools 链路,想实跑一轮
# 看真实落库"的场景都可以用它,不必手翻 DB。
#
# memory-block-governance WP1(08-15):报告加 memory_token / mem_pct 列;
# 新增 --turns N(同 session 连发 N 条消息)——AC4 的"第二轮起 cache_read
# 率"对比依赖双轮(--turns 2)。
#
# RULE-SMOKE-001(08-27-rule-smoke-perm-cleanup):send_and_wait 的等待条件
# 从"turn_trace 行出现"改为"SSE 流上出现本请求终态(kind=done)"——多轮
# 工具 turn 的内层 LLM 调用 Done 时行已落库但请求仍在跑,旧轮询提前命中 →
# EXIT trap delete_session 把进行中 chat 腰斩(daemon 取消)。脚本启动即挂
# 常驻 /api/v1/stream 订阅(须先挂再发,新连接不回放历史;EXIT trap 清理),
# --assert-turn-usage 复用同一份日志。
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
#   ./scripts/turn-smoke.sh --turns 2           # 同 session 连发 2 轮(cache 对比)
#   ./scripts/turn-smoke.sh --compact           # 手动 /compact live 冒烟(08-18-
#                                               # manual-compact-command):小轮 →
#                                               # 大消息轮(~20k token)→ idle 后
#                                               # POST compact_session → 断言摘要行
#                                               # 契约(trigger=manual/focus/seq)→
#                                               # 再跑一轮验证水位续跑
#   ./scripts/turn-smoke.sh --handoff           # /handoff live 冒烟(08-18-
#                                               # handoff-mechanism):一轮小消息 →
#                                               # POST handoff_session → 断言子
#                                               # session 首行契约(kind=handoff_
#                                               # summary/prefix/trigger)+ 双向
#                                               # metadata + parent 行数不变 → 子
#                                               # session 续跑一轮 → 清理两会话。
#                                               # 全量覆盖无保留区,无需大消息。
#   ./scripts/turn-smoke.sh --assert-turn-usage # 08-20-turn-usage-event-quota-view
#                                               # WP1 AC8:并发订阅 /api/v1/stream,
#                                               # 捕获 kind=turn_usage 事件,断言
#                                               # 字段齐全且与 DB 落值同值(顺带
#                                               # 验 AC1 的一致性半边);WP2 顺带
#                                               # 断言 provider_id 列落值。
#   ./scripts/turn-smoke.sh --keep              # 保留烟测 session(默认跑完即删)
#   ./scripts/turn-smoke.sh --timeout 300       # 等请求终态(kind=done)的超时秒数(默认 180)
#
# DB 路径见 AGENTS.md「DB / 烟测速查」;只读查询,daemon 可继续跑。
set -euo pipefail

PORT=7456
PROJECT_PATH="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MESSAGE="早上好,请只回一句问候,不要调用任何工具"
FOLLOWUP_MESSAGE="继续,仍然只回一句,不要调用任何工具"
TURNS=1
TIMEOUT=180
KEEP=0
COMPACT=0
HANDOFF=0
ASSERT_TURN_USAGE=0
while [ $# -gt 0 ]; do
  case "$1" in
    --port) PORT="$2"; shift 2 ;;
    --project-path) PROJECT_PATH="$2"; shift 2 ;;
    --message) MESSAGE="$2"; shift 2 ;;
    --turns) TURNS="$2"; shift 2 ;;
    --timeout) TIMEOUT="$2"; shift 2 ;;
    --keep) KEEP=1; shift ;;
    --assert-turn-usage) ASSERT_TURN_USAGE=1; shift ;;
    --compact) COMPACT=1; shift ;;
    --handoff) HANDOFF=1; shift ;;
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
PARENT_SID=""
CHILD_SID=""
SSE_LOG=""
SSE_PID=""
cleanup() {
  # 常驻 SSE 订阅先收线(set -euo pipefail 下 kill 失败不得打断清理链)。
  if [ -n "$SSE_PID" ]; then
    kill "$SSE_PID" 2>/dev/null || true
  fi
  if [ "$KEEP" != "1" ]; then
    # --handoff 会把 SID 切到子 session;三个变量覆盖 parent/child 双引用
    # (--handoff 后 SID==CHILD_SID,重复 delete 是幂等 no-op)。
    for s in "$SID" "$PARENT_SID" "$CHILD_SID"; do
      [ -n "$s" ] && curl -s -X POST "$BASE/api/v1/sessions/delete_session" \
        -H 'Content-Type: application/json' -d "{\"session_id\":\"$s\"}" >/dev/null 2>&1 || true
    done
  fi
  [ -n "$SSE_LOG" ] && rm -f "$SSE_LOG" || true
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

# ── 2. 建临时 session + 发 N 轮(每轮:发消息 → 轮询新 turn_trace 行) ──
SID="$(curl -sf -X POST "$BASE/api/v1/sessions/create_session" \
  -H 'Content-Type: application/json' \
  -d "{\"project_id\":\"$PROJ_ID\",\"initial_cwd\":\"$PROJECT_PATH\"}" \
  | python3 -c 'import json,sys; print(json.load(sys.stdin)["id"])')"
echo "session: $SID"

max_seq() {
  sqlite3 -readonly "$DB_PATH" \
    "SELECT COALESCE(MAX(seq),-1) FROM turn_trace WHERE session_id='$SID';" 2>/dev/null || echo -1
}

send_and_wait() {
  # $1 = 消息文本;发一条 user 消息,轮询直到 SSE 流上出现本请求终态
  # (kind=done)。RULE-SMOKE-001(08-27):旧"seq 更大的 turn_trace 行"条件
  # 在多轮工具 turn 上提前命中(第一段内层 Done 落库时请求仍在跑),会让
  # EXIT trap 的 delete_session 取消进行中 chat;循环层终端 Done 每请求
  # 只 emit 一次且必然在请求真正结束。
  # --compact / --handoff 模式发**全量 wire**(DB 行 text 列 + 新消息,
  # 镜像真实前端 rehydrate + reloadAfterFinalize 行为)—— 水位折叠的
  # wire↔DB 对齐前提依赖全量 wire;单条 wire 会让对齐防御 fail-open,
  # 验不到水位(handoff 子会话无水位,但全量 wire 同样镜像真实前端)。
  # 其他模式保持单条 wire 的历史语义(--turns 2 的 cache 对比口径)。
  local MSG="$1" BEFORE_SEQ ELAPSED=0
  BEFORE_SEQ="$(max_seq)"
  local REQ_ID="turn-smoke-$(date +%s)-$BEFORE_SEQ"
  local BODY
  BODY="$(MESSAGE="$MSG" REQ_ID="$REQ_ID" SID="$SID" DB_PATH="$DB_PATH" FULLWIRE="$((COMPACT + HANDOFF))" python3 -c '
import json,os,sqlite3
msgs=[]
if os.environ.get("FULLWIRE")=="1":
    con=sqlite3.connect("file:"+os.environ["DB_PATH"]+"?mode=ro",uri=True)
    for role,text in con.execute(
        "SELECT role,text FROM messages WHERE session_id=? ORDER BY seq",
        (os.environ["SID"],)):
        msgs.append({"role":role,"content":text})
msgs.append({"role":"user","content":os.environ["MESSAGE"]})
print(json.dumps({"request_id":os.environ["REQ_ID"],"session_id":os.environ["SID"],
                  "messages":msgs}))
')"
  # body 走 stdin(--compact 的大消息 ~90KB,走 -d argv 会撞单参数上限)。
  printf '%s' "$BODY" | curl -sf -X POST "$BASE/api/v1/agent/chat" \
    -H 'Content-Type: application/json' -d @- >/dev/null
  echo "turn sent (request_id=$REQ_ID), waiting for terminal event (kind=done)..."
  local DONE_LINE STOP_REASON ERR_LINE
  while [ "$ELAPSED" -lt "$TIMEOUT" ]; do
    # compact JSON 无空格,request_id 与 kind 同行,两个 grep 串联即可。
    # 管道一律 `|| true` 兜底:set -o pipefail 下 grep -q 命中即退会让上游
    # grep 撞 SIGPIPE(141),整管道判失败 → 错误终态被漏读;变量捕获 +
    # tail(读全量不早退)无此问题。
    DONE_LINE="$(grep -a "\"request_id\":\"$REQ_ID\"" "$SSE_LOG" 2>/dev/null \
      | grep "\"kind\":\"done\"" | tail -n 1 || true)"
    if [ -n "$DONE_LINE" ]; then
      STOP_REASON="$(printf '%s' "$DONE_LINE" | grep -o '"stop_reason":"[^"]*"' \
        | cut -d'"' -f4 || true)"
      echo "turn done (request_id=$REQ_ID, stop_reason=${STOP_REASON:-null})"
      return 0
    fi
    ERR_LINE="$(grep -a "\"request_id\":\"$REQ_ID\"" "$SSE_LOG" 2>/dev/null \
      | grep "\"kind\":\"error\"" | tail -n 1 || true)"
    if [ -n "$ERR_LINE" ]; then
      echo "ERR: chat request $REQ_ID ended with kind=error (check daemon logs: ./scripts/daemon.sh logs)" >&2
      return 1
    fi
    sleep 5; ELAPSED=$((ELAPSED+5))
  done
  echo "ERR: no terminal chat event (kind=done) for request $REQ_ID after ${TIMEOUT}s (check daemon logs: ./scripts/daemon.sh logs)" >&2
  return 1
}

TURN_MSG="$MESSAGE"
T=1
# 常驻 SSE 订阅(RULE-SMOKE-001,08-27):send_and_wait 等本请求终态
# (kind=done)、--assert-turn-usage 断言都读这一份日志;须在发第一轮之前
# 挂好(新连接不回放历史,迟挂会漏)。不用 --max-time —— compact /
# handoff 模式多轮合并,时长不可预知,靠 EXIT trap kill 收线。
SSE_LOG="$(mktemp /tmp/turn-smoke-sse.XXXXXX)"
curl -sN "$BASE/api/v1/stream" >"$SSE_LOG" 2>/dev/null &
SSE_PID=$!
while [ "$T" -le "$TURNS" ]; do
  [ "$T" -gt 1 ] && TURN_MSG="$FOLLOWUP_MESSAGE"
  send_and_wait "$TURN_MSG"
  T=$((T+1))
done
# 订阅保持存活到脚本退出(--compact / --handoff 后续轮仍要等终态),
# 3.5 断言读共享日志前再小睡兜 curl 写盘 flush。

# ── 2.5 手动 /compact live 冒烟(08-18-manual-compact-command) ────────
# 布局:小轮(上面)+ 大消息轮(下面)。保留区预算 clamp 下限 15k token,
# 大消息单组(~20k token)进保留区,待压区 = 更早的小轮对 —— 管道全链路
# (HTTP → gate → provider → 摘要 → 落库)可跑通;token 骤降的强断言在
# mock 端到端,这里验证真实 LLM 下的落库契约与水位续跑。
if [ "$COMPACT" = "1" ]; then
  BIG_MSG="$(python3 -c 'print("这是背景资料段落,用于撑过保留区预算。 " * 1600 + "\n(以上为背景资料)请只回一句:收到。")')"
  send_and_wait "$BIG_MSG"
  echo "invoking compact_session (focus=冒烟定向)..."
  COMPACT_RESP="$(SID="$SID" python3 -c '
import json,os
print(json.dumps({"session_id":os.environ["SID"],"focus":"冒烟定向"}))
' | curl -sf -m 180 -X POST "$BASE/api/v1/sessions/compact_session" \
      -H 'Content-Type: application/json' -d @-)" || {
    echo "ERR: compact_session call failed (check daemon logs)" >&2; exit 1;
  }
  echo "$COMPACT_RESP" | python3 -c '
import json,sys
r=json.load(sys.stdin)
print("compact ok: before={} after={} cutoff_seq={} model={}".format(
    r["tokens_before"], r["tokens_after"], r["cutoff_seq"], r["model"]))
'
  # 落库契约:kind / trigger=manual / focus / seq=MAX+1(摘要行是最后插入行)。
  SUMMARY_CHECK="$(SID="$SID" DB_PATH="$DB_PATH" python3 - << 'PYEOF'
import json, os, sqlite3
sid = os.environ["SID"]
con = sqlite3.connect(f"file:{os.environ['DB_PATH']}?mode=ro", uri=True)
rows = con.execute(
    "SELECT seq, text, metadata FROM messages WHERE session_id=? "
    "ORDER BY seq DESC LIMIT 1", (sid,)).fetchall()
assert rows, "no messages row after compaction"
seq, text, meta_json = rows[0]
meta = json.loads(meta_json or "{}")
assert meta.get("kind") == "compaction_summary", f"kind={meta.get('kind')}"
assert meta.get("trigger") == "manual", f"trigger={meta.get('trigger')}"
assert meta.get("focus") == "冒烟定向", f"focus={meta.get('focus')}"
print(f"summary row ok: seq={seq} trigger=manual focus=冒烟定向 "
      f"cutoff={meta.get('cutoff_seq')} body[:40]={text[:40]!r}")
PYEOF
)" || { echo "ERR: summary row contract check failed" >&2; exit 1; }
  echo "$SUMMARY_CHECK"
  # 水位续跑:再发一轮,turn_trace 出现新行 = 下一请求正常吃到新水位。
  send_and_wait "$FOLLOWUP_MESSAGE"
  echo "post-compact turn ok (watermark pickup verified by new turn_trace row)"
fi

# ── 2.6 /handoff live 冒烟(08-18-handoff-mechanism)─────────────────────
# 全量覆盖无保留区,一轮小消息即可(无需大消息撑预算)。管道:HTTP →
# gate → provider → 全量摘要(真实 LLM)→ 子 session 继承 + 首行落库 →
# 双向 metadata;然后子 session 续跑一轮验证接力 context 可用。
if [ "$HANDOFF" = "1" ]; then
  PARENT_MSGS_BEFORE="$(sqlite3 -readonly "$DB_PATH" \
    "SELECT COUNT(*) FROM messages WHERE session_id='$SID';")"
  echo "invoking handoff_session (focus=冒烟定向)..."
  HANDOFF_RESP="$(SID="$SID" python3 -c '
import json,os
print(json.dumps({"session_id":os.environ["SID"],"focus":"冒烟定向"}))
' | curl -sf -m 180 -X POST "$BASE/api/v1/sessions/handoff_session" \
      -H 'Content-Type: application/json' -d @-)" || {
    echo "ERR: handoff_session call failed (check daemon logs)" >&2; exit 1;
  }
  CHILD_SID="$(echo "$HANDOFF_RESP" | python3 -c '
import json,sys
r=json.load(sys.stdin)
print(r["new_session_id"])
')"
  echo "$HANDOFF_RESP" | python3 -c '
import json,sys
r=json.load(sys.stdin)
print("handoff ok: child_title={} before={} after={} cutoff_seq={} model={}".format(
    r["new_session_title"], r["tokens_before"], r["tokens_after"],
    r["cutoff_seq"], r["model"]))
  '
  # 落库契约:子首行(seq=1/kind=handoff_summary/trigger=handoff/focus/
  # prefix 自包含)+ 双向 metadata + parent 行数不变 + parent 标题继承。
  HANDOFF_CHECK="$(SID="$SID" CHILD="$CHILD_SID" DB_PATH="$DB_PATH" python3 - << 'PYEOF'
import json, os, sqlite3
sid, child = os.environ["SID"], os.environ["CHILD"]
con = sqlite3.connect(f"file:{os.environ['DB_PATH']}?mode=ro", uri=True)
rows = con.execute(
    "SELECT seq, text, metadata FROM messages WHERE session_id=? ORDER BY seq",
    (child,)).fetchall()
assert rows, "child session has no messages"
seq, text, meta_json = rows[0]
meta = json.loads(meta_json or "{}")
assert seq == 1, f"first row seq={seq} (want 1)"
assert meta.get("kind") == "handoff_summary", f"kind={meta.get('kind')}"
assert meta.get("trigger") == "handoff", f"trigger={meta.get('trigger')}"
assert meta.get("focus") == "冒烟定向", f"focus={meta.get('focus')}"
assert meta.get("parent_session_id") == sid, "parent_session_id mismatch"
assert text.startswith("This session is being continued"), \
    f"prefix not persisted: {text[:40]!r}"
assert len(rows) == 1, f"child has {len(rows)} rows (want 1 pre-continuation)"
sess = con.execute(
    "SELECT title, metadata FROM sessions WHERE id=?", (child,)).fetchone()
assert sess and sess[0].startswith("接力: "), f"child title={sess[0] if sess else None}"
child_meta = json.loads(sess[1] or "{}")
assert child_meta.get("handoff", {}).get("parent_session_id") == sid, \
    f"child session metadata: {child_meta}"
parent_meta_row = con.execute(
    "SELECT metadata FROM sessions WHERE id=?", (sid,)).fetchone()
parent_meta = json.loads((parent_meta_row or ["{}"])[0] or "{}")
children = parent_meta.get("handoff_children", [])
assert child in children, f"parent handoff_children missing child: {children}"
parent_msgs = con.execute(
    "SELECT COUNT(*) FROM messages WHERE session_id=?", (sid,)).fetchone()[0]
print(f"handoff contract ok: child seq=1 kind=handoff_summary prefix=persisted "
      f"title={sess[0]!r} children={len(children)} parent_msgs={parent_msgs}")
PYEOF
)" || { echo "ERR: handoff contract check failed" >&2; exit 1; }
  echo "$HANDOFF_CHECK"
  PARENT_MSGS_AFTER="$(sqlite3 -readonly "$DB_PATH" \
    "SELECT COUNT(*) FROM messages WHERE session_id='$SID';")"
  [ "$PARENT_MSGS_BEFORE" = "$PARENT_MSGS_AFTER" ] || {
    echo "ERR: parent message count changed: $PARENT_MSGS_BEFORE → $PARENT_MSGS_AFTER" >&2; exit 1;
  }
  echo "parent rows intact: $PARENT_MSGS_AFTER (AC4)"
  # 续跑:切到子 session 发一轮(全量 wire = 接力行 + 新消息,镜像真实
  # 前端),turn_trace 新行 = 接力 context 可正常续跑。PARENT_SID 记原会话
  # 供 cleanup 删除。
  PARENT_SID="$SID"
  SID="$CHILD_SID"
  send_and_wait "$FOLLOWUP_MESSAGE"
  echo "post-handoff continuation turn ok (relay context usable)"
fi

# ── 3. 列存在性检查(缺失 = daemon 二进制早于对应任务) ─────────────────
if ! sqlite3 -readonly "$DB_PATH" "SELECT tools_token FROM turn_trace LIMIT 1;" >/dev/null 2>&1; then
  echo "ERR: turn_trace has no tools_token column — daemon binary predates C7 R1 (rebuild)" >&2; exit 1
fi
if ! sqlite3 -readonly "$DB_PATH" "SELECT memory_token FROM turn_trace LIMIT 1;" >/dev/null 2>&1; then
  echo "ERR: turn_trace has no memory_token column — daemon binary predates memory-block-governance WP1 (rebuild)" >&2; exit 1
fi
if ! sqlite3 -readonly "$DB_PATH" "SELECT images_token FROM turn_trace LIMIT 1;" >/dev/null 2>&1; then
  echo "ERR: turn_trace has no images_token column — daemon binary predates B1 PR4 (rebuild)" >&2; exit 1
fi
# unified-context-budget WP1(08-19):at_files / system / context_window 三列。
if ! sqlite3 -readonly "$DB_PATH" "SELECT at_files_token FROM turn_trace LIMIT 1;" >/dev/null 2>&1; then
  echo "ERR: turn_trace has no at_files_token column — daemon binary predates unified-context-budget WP1 (rebuild)" >&2; exit 1
fi
# 08-20-turn-usage-event-quota-view WP2:provider 归因列。
if ! sqlite3 -readonly "$DB_PATH" "SELECT provider_id FROM turn_trace LIMIT 1;" >/dev/null 2>&1; then
  echo "ERR: turn_trace has no provider_id column — daemon binary predates turn-usage-event-quota-view WP2 (rebuild)" >&2; exit 1
fi
if [ "$(max_seq)" -lt 0 ]; then
  echo "ERR: no turn_trace rows for session (check daemon logs: ./scripts/daemon.sh logs)" >&2; exit 1
fi

# -- 3.5 --assert-turn-usage assertion (08-20 WP1 AC8) -------------------------
if [ "$ASSERT_TURN_USAGE" = "1" ]; then
  # RULE-SMOKE-001(08-27):复用常驻订阅日志;done 已见则 turn_usage 必然
  # 先到,小睡兜 curl 写盘 flush。临时文件统一由 EXIT trap 清理。
  sleep 3
  TURN_USAGE_CHECK="$(SID="$SID" DB_PATH="$DB_PATH" LOG="$SSE_LOG" python3 - << 'PYEOF2'
import json, os, sqlite3

sid, db_path, log = os.environ["SID"], os.environ["DB_PATH"], os.environ["LOG"]
events = []
with open(log, "r", errors="replace") as fh:
    for line in fh:
        if line.startswith("data:"):
            try:
                payload = json.loads(line[5:].strip())
            except json.JSONDecodeError:
                continue
            if payload.get("kind") == "turn_usage":
                events.append(payload)
assert events, "no kind=turn_usage event captured on /api/v1/stream"
for ev in events:
    assert ev.get("run_id") == "", f"run_id={ev.get('run_id')!r} (main loop sentinel)"
    u = ev.get("usage") or {}
    for field in ("input_tokens", "output_tokens", "cache_creation_input_tokens",
                  "cache_read_input_tokens", "context_input_tokens"):
        assert isinstance(u.get(field), int), f"usage.{field} missing/not int: {u}"
    assert isinstance(ev.get("seq"), int), "seq missing"
    assert isinstance(ev.get("context_window"), int), "context_window missing"
con = sqlite3.connect(f"file:{db_path}?mode=ro", uri=True)
db_rows = con.execute(
    "SELECT seq, token_usage_json FROM turn_trace "
    "WHERE session_id=? AND run_id='' AND token_usage_json IS NOT NULL", (sid,)).fetchall()
db_by_seq = {seq: json.loads(js) for seq, js in db_rows}
ev_by_seq = {ev["seq"]: ev["usage"] for ev in events}
for seq, usage in ev_by_seq.items():
    assert seq in db_by_seq, f"event seq {seq} has no DB row"
    assert db_by_seq[seq] == usage, f"seq {seq}: event usage {usage} != DB {db_by_seq[seq]}"
prov = con.execute(
    "SELECT provider_id FROM turn_trace WHERE session_id=? AND run_id='' "
    "ORDER BY seq DESC LIMIT 1", (sid,)).fetchone()
assert prov and prov[0], f"fresh main row provider_id is NULL: {prov}"
print(f"turn_usage events ok: {len(events)} captured, "
      f"{len(ev_by_seq)} seq-consistent with DB, provider_id={prov[0][:8]}...")
PYEOF2
)" || { echo "ERR: turn_usage event assertion failed (see above)" >&2; exit 1; }
  echo "$TURN_USAGE_CHECK"
fi

# ── 4. 报告 ───────────────────────────────────────────────────────────
sqlite3 -readonly -header -column "$DB_PATH" \
  "SELECT seq, tools_token, memory_token, images_token, at_files_token, system_token, context_window,
          json_extract(token_usage_json,'\$.input_tokens')      AS input_tok,
          json_extract(token_usage_json,'\$.output_tokens')     AS output_tok,
          json_extract(token_usage_json,'\$.cache_read_input_tokens') AS cache_read,
          json_extract(token_usage_json,'\$.context_input_tokens')    AS ctx_input,
          CAST(ROUND(100.0*tools_token/json_extract(token_usage_json,'\$.context_input_tokens')) AS INT) AS tools_pct,
          CAST(ROUND(100.0*memory_token/json_extract(token_usage_json,'\$.context_input_tokens')) AS INT) AS mem_pct,
          CAST(ROUND(100.0*images_token/json_extract(token_usage_json,'\$.context_input_tokens')) AS INT) AS img_pct,
          CAST(ROUND(100.0*at_files_token/json_extract(token_usage_json,'\$.context_input_tokens')) AS INT) AS at_pct
   FROM turn_trace WHERE session_id='$SID' ORDER BY seq;"
MSG_COUNT="$(sqlite3 -readonly "$DB_PATH" "SELECT COUNT(*) FROM messages WHERE session_id='$SID';")"
echo "messages persisted: $MSG_COUNT (user+assistant)"
[ "$KEEP" = "1" ] && echo "session kept: $SID" || echo "smoke session deleted"
echo "OK"
