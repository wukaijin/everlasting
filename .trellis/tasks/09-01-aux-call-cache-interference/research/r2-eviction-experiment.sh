#!/usr/bin/env bash
# r2-eviction-experiment.sh — R2 对照实验(v2):跨 session 大 tools=0 摘要旁路
# 是否驱逐主 loop 的上游前缀缓存条目(任务 09-01-aux-call-cache-interference)。
#
# v1 教训:① create_session 的 model 参数是 legacy 标签,实际模型每轮从
# sessions.model_id(优先)→ app_config.default_model_id 解析 —— v2 改用
# update_session_model_id 按 session 指定 deepseek(OpenAI 兼容路径,原事故同款);
# ② 内联 python 的 f-string 转义在 Python<3.12 语法错 —— v2 全部改为先读 env 再
# 拼接;③ 加 T1 后的路径守卫(daemon.log 必须出现 openai transport + deepseek,
# 否则中止,防无效臂污染结论)。
#
# 臂设计(见 research/call-site-inventory.md §3):
#   A(对照)  session S:T1 大消息(~30k tok)→ T2 小消息;T2 cache_read 应 ≈ T1 尾部
#            input(稳态命中基线;不命中则该 provider 缓存不可测,实验终止)。
#   C(决定)  session S2 堆 4 轮大消息(~120k+ 上下文)→ POST compact_session
#            (触发大 tools=0 has_system=false 摘要旁路,同 provider 同 model)→
#            S 发 T3 小消息(前缀未动):cache_read 塌 → 驱逐成立;≈ T2 水位 → 排除。
#            T4 观察缓存是否恢复(重 prefill 后应回到命中)。
#
# 用法: ./r2-eviction-experiment.sh [--keep]
set -uo pipefail

PORT=7456
KEEP=0
[ "${1:-}" = "--keep" ] && KEEP=1
BASE="http://127.0.0.1:${PORT}"
DB_PATH="${XDG_DATA_HOME:-$HOME/.local/share}/dev.everlasting.app/everlasting.db"
DAEMON_LOG=~/.local/state/dev.everlasting.app/daemon.log
DEEPSEEK_MODEL_ID="fec6ff5c-2ceb-4fde-acd5-d8ec1c9ee24d"   # deepseek-v4-flash / Carlos-OpenAI(openai 协议)
TIMEOUT=300
PROJECT_PATH="/usr/local/code/github/everlasting"
# 规模参数(v2 默认:S 条目 ~31k / S2 压缩 ~161k;v3 加压:各 ~150k+)
BIG_CHARS="${BIG_CHARS:-40000}"       # 每轮大消息字符数(~0.74 tok/char)
S2_TURNS="${S2_TURNS:-4}"             # S2 堆上下文轮数

command -v sqlite3 >/dev/null || { echo "ERR: sqlite3 not found" >&2; exit 2; }
curl -sf -m 3 "$BASE/health" >/dev/null || { echo "ERR: daemon not reachable at $BASE" >&2; exit 2; }

S_SID=""; S2_SID=""; SSE_PID=""
cleanup() {
  [ -n "$SSE_PID" ] && kill "$SSE_PID" 2>/dev/null || true
  if [ "$KEEP" != "1" ]; then
    for s in "$S_SID" "$S2_SID"; do
      [ -n "$s" ] && curl -s -X POST "$BASE/api/v1/sessions/delete_session" \
        -H 'Content-Type: application/json' -d "{\"session_id\":\"$s\"}" >/dev/null 2>&1 || true
    done
  fi
}
trap cleanup EXIT

PROJ_ID="$(curl -sf -X POST "$BASE/api/v1/projects/list_projects" -H 'Content-Type: application/json' -d '{}' \
  | WANT="$PROJECT_PATH" python3 -c '
import json,sys,os,pathlib
want=pathlib.PurePath(os.environ["WANT"])
for p in json.load(sys.stdin):
    if pathlib.PurePath(p["path"])==want: print(p["id"]); break
')"
[ -n "$PROJ_ID" ] || { echo "ERR: project not found: $PROJECT_PATH" >&2; exit 2; }

create_session() {
  curl -sf -X POST "$BASE/api/v1/sessions/create_session" -H 'Content-Type: application/json' \
    -d "{\"project_id\":\"$PROJ_ID\",\"initial_cwd\":\"$PROJECT_PATH\"}" \
    | python3 -c 'import json,sys; print(json.load(sys.stdin)["id"])'
}
use_deepseek() { # $1=sid:per-session 模型覆盖(chat.rs 解析优先 sessions.model_id)
  curl -sf -X POST "$BASE/api/v1/providers/update_session_model_id" \
    -H 'Content-Type: application/json' \
    -d "{\"session_id\":\"$1\",\"model_id\":\"$DEEPSEEK_MODEL_ID\"}" >/dev/null
}
max_seq() {
  sqlite3 -readonly "$DB_PATH" \
    "SELECT COALESCE(MAX(seq),-1) FROM turn_trace WHERE session_id='$1';" 2>/dev/null || echo -1
}

send_and_wait() { # $1=sid $2=msg-file
  local SID="$1" MSGFILE="$2" BEFORE_SEQ ELAPSED=0 REQ_ID DONE_LINE
  BEFORE_SEQ="$(max_seq "$SID")"
  REQ_ID="r2-$(date +%s%N)-$BEFORE_SEQ"
  REQ_ID="$REQ_ID" SID="$SID" MSGFILE="$MSGFILE" python3 -c '
import json,os
msg=open(os.environ["MSGFILE"],encoding="utf-8").read()
print(json.dumps({"request_id":os.environ["REQ_ID"],"session_id":os.environ["SID"],
                  "messages":[{"role":"user","content":msg}]}))' \
    | curl -sf -X POST "$BASE/api/v1/agent/chat" -H 'Content-Type: application/json' -d @- >/dev/null
  echo "  sent $REQ_ID @$(date +%H:%M:%S), waiting kind=done..."
  while [ "$ELAPSED" -lt "$TIMEOUT" ]; do
    DONE_LINE="$(grep -a "\"request_id\":\"$REQ_ID\"" "$SSE_LOG" 2>/dev/null | grep '"kind":"done"' | tail -n 1 || true)"
    [ -n "$DONE_LINE" ] && { echo "  done  $REQ_ID @$(date +%H:%M:%S)"; return 0; }
    if grep -aq "\"request_id\":\"$REQ_ID\"" "$SSE_LOG" 2>/dev/null && \
       grep -a "\"request_id\":\"$REQ_ID\"" "$SSE_LOG" | grep -q '"kind":"error"'; then
      echo "ERR: $REQ_ID ended kind=error" >&2; return 1
    fi
    sleep 5; ELAPSED=$((ELAPSED+5))
  done
  echo "ERR: no terminal event for $REQ_ID after ${TIMEOUT}s" >&2; return 1
}

trace_row() { # $1=sid(可选 $2=起始 seq,只打新增)
  SID="$1" DB_PATH="$DB_PATH" python3 -c '
import json,os,sqlite3
sid=os.environ["SID"]; dbp=os.environ["DB_PATH"]
con=sqlite3.connect("file:"+dbp+"?mode=ro",uri=True)
for seq,j in con.execute(
  "SELECT seq,token_usage_json FROM turn_trace WHERE session_id=? ORDER BY seq",(sid,)):
    u=json.loads(j or "{}")
    print("  seq=%3d input=%7d cache_read=%7d ctx_in=%7d" % (
      seq,u.get("input_tokens",0),u.get("cache_read_input_tokens",0),
      u.get("context_input_tokens",0)))'
}

filler() { # $1=target_chars $2=seed $3=outfile(变长内容,禁重复文本夹具)
  python3 -c "
import random
random.seed($2)
words='缓存 前缀 令牌 上下文 摘要 压缩 会话 模型 请求 路由 条目 水位 保留区 折叠 观测 干扰 驱逐 旁路 补全 流式'.split()
out=[]; i=0
while sum(len(x) for x in out) < $1:
    i+=1
    out.append('段落%d:'%i+''.join(random.choice(words) for _ in range(24))+'。')
open('$3','w',encoding='utf-8').write('\n'.join(out)+'\n(以上为背景资料)请只回一句:收到。')"
}

SSE_LOG="$(mktemp /tmp/r2-sse.XXXXXX)"
curl -sN "$BASE/api/v1/stream" >"$SSE_LOG" 2>/dev/null &
SSE_PID=$!
sleep 1

M_SMALL="$(mktemp /tmp/r2-msg.XXXXXX)"; echo "继续,仍然只回一句,不要调用任何工具。" >"$M_SMALL"

echo "══ Arm A:session S 稳态命中基线(deepseek / openai 路径)══"
S_SID="$(create_session)"; use_deepseek "$S_SID"; echo "S=$S_SID"
M1="$(mktemp)"; filler "$BIG_CHARS" 11 "$M1"
echo " T1 (big ~30k tok):"; GUARD_TS="$(date -u +%Y-%m-%dT%H:%M:%S)"; send_and_wait "$S_SID" "$M1" || exit 1
# 路径守卫:本轮必须已走 openai transport + deepseek,否则整臂无效
# (v3 教训:tail -N 窗口在大请求下被日志密度冲掉,改时间戳窗口 + 只看请求行)
if ! sed 's/\x1b\[[0-9;]*m//g' "$DAEMON_LOG" | awk -v t="$GUARD_TS" '$1>=t' \
     | grep "→ LLM request" | grep -q "provider::openai.*deepseek-v4-flash"; then
  echo "ERR: 守卫失败 —— daemon.log 无 openai/deepseek 请求行,模型覆盖未生效,中止(防无效臂)" >&2; exit 1
fi
echo " (路径守卫过:openai transport + deepseek ✓)"
echo " T2 (small):"; send_and_wait "$S_SID" "$M_SMALL" || exit 1
echo " trace S:"; trace_row "$S_SID"

echo "══ Arm C:session S2 堆上下文 → compact(tools=0 大旁路)══"
S2_SID="$(create_session)"; use_deepseek "$S2_SID"; echo "S2=$S2_SID"
i=0
while [ "$i" -lt "$S2_TURNS" ]; do
  i=$((i+1))
  MB="$(mktemp)"; filler "$BIG_CHARS" $((20+i)) "$MB"
  echo " S2.T$i (big):"; send_and_wait "$S2_SID" "$MB" || exit 1
done
echo " trace S2:"; trace_row "$S2_SID"
echo " compact S2 @$(date +%H:%M:%S)..."
COMPACT_RESP="$(curl -sf -m 300 -X POST "$BASE/api/v1/sessions/compact_session" \
  -H 'Content-Type: application/json' -d "{\"session_id\":\"$S2_SID\"}")" || {
    echo "ERR: compact_session failed"; exit 1; }
echo "$COMPACT_RESP" | python3 -c '
import json,sys
r=json.load(sys.stdin)
print("  compact ok: before=%s after=%s cutoff=%s" % (r["tokens_before"],r["tokens_after"],r["cutoff_seq"]))'

echo "══ 判读:S 前缀未动,T3 观测驱逐 ══"
echo " T3 (small):"; send_and_wait "$S_SID" "$M_SMALL" || exit 1
echo " T4 (small,恢复观测):"; send_and_wait "$S_SID" "$M_SMALL" || exit 1
echo " trace S(全量):"; trace_row "$S_SID"

echo "══ daemon.log 佐证(compact 前后的请求行)══"
sed 's/\x1b\[[0-9;]*m//g' "$DAEMON_LOG" | grep "→ LLM request" | tail -12
echo "(判读标准:T3 cache_read ≈ T2 ctx_in → 排除驱逐;T3 cache_read 塌至 ~0/骤降 >50% → 确认驱逐;T4 应回升)"
