# R1b 取证包:4 次 tools=0 请求归因(数据在另一台机,命令即拿即用)

> 背景:原始证据(daemon.log + 含 session `d6728b3a` 的 everlasting.db)不在本机。
> 本文件是把归因流程固化成可机械执行的命令,数据回来(拷贝或在原机跑)即出结论。
> 判别依据见 `call-site-inventory.md` §2/§3。

目标事件(父任务 research 记录,本地时间 +0800):

- 4 次 `tools_count=0` 请求:08:50:58 / 09:00:35 / 09:00:35.509→.550 紧邻 tools=13 /
  09:12:58 / 09:24:56
- 关联 miss:seq 285(09:13:03,cache_read=0)、seq 401(09:28:20,部分回退 154,112)

## 步骤 1:日志行提取(原机或拷回的 daemon.log)

```bash
# 清洗 ANSI 后提取全部请求行(tools=0 与紧邻上下文)
sed 's/\x1b\[[0-9;]*m//g' ~/.local/state/dev.everlasting.app/daemon.log \
  | grep "→ LLM request" | grep "2026-08-31" > /tmp/req-0831.log

# 4 次目标 + 前后各 3 行上下文(判紧邻配对/间隔)
grep -n -B3 -A3 "tools_count=0" /tmp/req-0831.log
```

**判别 1(has_system)**:

- `tools_count=0 has_system=false` → 压缩摘要(auto #2 / 手动 #3)
- `tools_count=0 has_system=true` → auto_reflect(#4)

## 步骤 2:DB 交叉(sqlite3 -readonly,Windows/WSL 路径
`~/.local/share/dev.everlasting.app/everlasting.db`;macOS 见 docs/DEBUG_DB.md §1)

```sql
-- (a) 目标 session 的轮次轨迹:cache_read 口径(token_usage_json)+ 压缩事件
SELECT seq, created_at, token_usage_json, compaction_json
FROM turn_trace WHERE session_id LIKE 'd6728b3a%'
  AND created_at BETWEEN '2026-08-30T22:00' AND '2026-08-31T04:00'  -- UTC 窗口
ORDER BY seq;
-- 注:created_at 是 UTC datetime('now');本地 08:50–09:41 = UTC 00:50–01:41

-- (b) 该 session 的压缩摘要行(messages.metadata.kind)
SELECT seq, created_at, substr(metadata,1,120) FROM messages
WHERE session_id LIKE 'd6728b3a%' AND metadata LIKE '%compaction_summary%'
ORDER BY seq;

-- (c) pitfall 写入(auto_reflect 指纹)
SELECT memory_id, kind, created_at, title FROM autonomous_memories
WHERE kind='pitfall' AND created_at LIKE '2026-08-31%'
ORDER BY created_at;

-- (d) tools=13 跟随者身份:worker dispatch 时点
SELECT id, session_id, status, created_at, finished_at FROM subagent_runs
WHERE created_at BETWEEN '2026-08-30T22:00' AND '2026-08-31T04:00'
ORDER BY created_at;
```

## 步骤 3:判别树(出结论)

对每次 tools=0 请求 t:

1. `has_system=true`?→ auto_reflect,结(小输入,驱逐嫌疑低,仅记录)。
2. `has_system=false` + **该时刻附近 (b)/(a) 有本 session 压缩事件** →
   **本 session 自己的压缩**。随后主 loop 的 miss(如 seq 285/401)判
   **机械性 miss(折叠改前缀,by design)**,非驱逐 —— 若 4 次全部落此分支,
   驱逐假说对 seq 285 **排除**,结案走 R3 排除臂(仍以 R2-C 臂实验佐证)。
3. `has_system=false` + 本 session 无压缩事件 → 别 session 的压缩(或手动)。
   若紧邻的 miss 轮(如 seq 285)前缀未变(turn_trace 相邻两轮 tools_token/
   system_token 稳定、无 compaction_json 事件、无 breadcrumb/instruction 变更)
   却 cache_read=0 → **驱逐假说成立**,进入 R2-C 复现 + R3 缓解臂。
4. tools=13 跟随者:(d) 匹配到 subagent_runs → worker 本体;否则查其他 session
   的 turn_trace(tools_token 一致性)。

## 附:需要从原机带回的最小材料

- `daemon.log`(08-31 全天段即可)
- `everlasting.db` 中 4 张表的导出(若整库不便拷):
  `turn_trace`(该 session 全部行)、`messages`(该 session metadata 含
  compaction_summary 的行 + 前后各 20 行)、`autonomous_memories`(08-31 pitfall 行)、
  `subagent_runs`(08-31 窗口行)
