# DB 取证 — 会话 5df29977(2026-08-18 09:29 终止)

> 采集时间 2026-08-18。所有查询均只读(`sqlite3 -readonly ~/.local/share/dev.everlasting.app/everlasting.db`)。

## 会话本体

```
id         5df29977-2f4b-478e-ab22-01171fcd4aa2
title      看一下当前任务
mode       chat(→ DB 列显示 'chat',3 档化后 wire 名 edit/plan/yolo,该会话从未成功切 mode)
workflow_enabled 1, plugin_name dev(dev workflow 会话)
created    2026-08-18T08:46:25Z, updated 2026-08-18T09:29:10Z
messages   0..141(142 条),turn_trace seq 1..141(缺 140)
```

## 审计事件统计(该 session)

```
tool_allowed              182
tool_executed             158
tool_permission_ask        21  (全部 shell)
permission_granted         16
permission_timeout          5  (08:50, 08:56, 08:58, 09:00, 09:05)
loop_intervention           4  (09:21:28 asked / 09:21:40 continued / 09:28:11 asked / 09:28:18 continued)
task_state_transition_requested 2
task_state_transition_allowed   2
```

- loop_intervention 4 条全是 **soft** + hit_count=3;**没有 hard 干预**。
- `mode_changed` / `yolo_entered` 0 条 → 会话全程没成功切过 mode(问题 4 佐证)。

## 21 次 shell 权限 ask 命令(摘)

```
08:46:55  cd … && git log --oneline -50
08:47:19  cd … && ls -la docs/*.md | head -40 && echo … && wc -l …
08:47:21  cd … && git log --oneline --stat -10 -- docs/*.md …
08:48:00  cd … && ls .trellis/workspace/…  && echo —ARCHIVE— && ls .everlasting/tasks/…
08:54:46  cd … && grep -rn '#[tauri::command]' app/src-tauri/src/commands/ …
08:56:46  cd … && grep -rn 'builtin_tools|tools::builtin' …
08:58:46  cd … && ls app/src-tauri/src/daemon/routes/ …
09:00:58  cd … && grep -rln '#[tauri::command]' …
09:01:20  cd … && grep -c …
09:01:22  cd … && ls app/src-tauri/src/daemon/routes/(timeout 30000)
09:01:43  cd … && grep -rn 'fn definition' …
09:02:11  cd … && ls app/src-tauri/src/tools/ | head -40
09:02:12  cd … && cat tools/mod.rs | grep -A 50 'fn builtin_tools'
09:02:13  cd … && wc -l Cargo.toml && cat Cargo.toml | head -25
09:03:43  cd … && ls app/src-tauri/src/bin/ && grep -c '\bdefinition()\b' …
09:22:09  grep -rn '97 个|24 个 builtin|0.80 * 触发|C3 = …' docs/*.md
09:22:20  grep -rn 'decisions\.md\b' docs/*.md | grep -v …
09:23:17  echo '=== … ===' + grep -rn 'decisions\.md\b' README.md docs/*.md …
09:23:54  echo '=== … ===' + grep …(重复上一条变体)
09:25:01  cd … + echo + git status -s + git log --oneline -10
09:25:29  cd … + git commit -m 'docs(sync): …' -m …
```

- 5 次 timeout 集中在 08:50–09:05(用户在翻源码/忙别的事),间隔 ~2min。
- 权限表该 session 只有 3 条 prefix:`cd` / `grep` / `echo`,**无 git**;前一 session `fec57568` 07:38 有 `shell prefix git`(所以它不需要逐条问 git)。

## turn_trace 循环检测轨迹

| turn seq | 时刻 | loop_hint(verdict/hit) | 对应 messages 事件 |
|---|---|---|---|
| 60  | 08:52:42 | soft, hit 1 | (早前) |
| 106 | 09:20:59 | soft, hit 1 | seq106「loop suspected」(edit_file 3 连空参数) |
| 108 | 09:21:15 | hard, hit 2 | seq108「loop detected: edit_file identical arguments 3 times」 |
| 110 | 09:21:40 | —(干预 asked+continued) | seq110「loop intervention」 |
| 112 | 09:21:44 | soft, hit 1 | seq112 再次 suspected |
| 136 | 09:27:35 | hard, hit 1 | seq136「loop detected: write_file identical arguments 3 times」 |
| 138 | 09:27:52 | soft, hit 2 | seq138「loop suspected (Jaccard 1.00)」 |
| —   | 09:28:11 | (asked)  | seq140「loop intervention」= hit 3 触发 |
| —   | 09:28:18 | (continued)| audit continued |

### 关键还原(问题 5)

- `write_file` 签名 = `write_file:{path}`(path 缺失 → `write_file:` 空串)。
- seq 129/131/133/135 连续 4 次 `write_file:{}` → hard(hit 1, seq 136)。
- seq 137 成功写 progress.md(签名 `write_file:{real path}`)后,窗口尾部 run=1,但窗口内 4 个空 WFE 两两 Jaccard 1.0 → 6 对 → SoftLoop(hit 2, seq 138)。
- seq 139 turn 调用 `request_task_state_transition`(与 write_file 完全不同),窗口内仍残留 3 个空 WFE → 3 对 1.0 → SoftLoop(hit 3)→ **干预触发**(09:29:02)。用户视角「最近没有相似操作」正确——相似操作是 09:27:35 的 3 个空 write_file,已隔 5+ 轮。

## edit_file memory 注脚(问题 1)

- seq 116/120 等多条 tool_result 以 `⚠️ Memory: 此前在本项目执行类似操作时踩过坑 —` 开头,携带:
  - [Re-read file before retrying edit after on-disk change] hit_count=60
  - [Always supply required path in edit_file calls] hit_count=15
- 两条均 `status=active, kind=pitfall, tool_name=edit_file`。
- 文案源头:`app/src-tauri/src/agent/permissions/check/pitfall.rs:119`(P3 footnote)与 `:397`(P5 Footnote 分支)。
- 每条成功 edit_file 都带注脚 → pre-tool recall 对每次 edit_file 都命中;hit_count 60 次 = 该 session(以及历史)累计 bump。

## 问题 4 佐证

- 全库 audit 仅 2 条 `mode_changed`(2026-06-16 df0b9570/82cb3489 切 plan);**没有任何 yolo_entered / yolo_exited**。
- workflow_enabled=1 的 10 个会话 mode 均为 edit/plan,无一 yolo。
- 前端链路已排查:`ModeSelect.vue`(普通 popover + YoloConfirmModal child,z-index 1200 fixed backdrop)→ `chatStore.requestSetMode` → `pendingYoloConfirm`。后端 `set_session_mode_internal` 仅 root 守卫 + DB 写,无 workflow 限制。→ 问题在**前端层叠/渲染**,需实机复现确认(checklist 浮动卡 z-index 50、loop 浮动卡、ReviewMatrix 等 workflow 专属覆盖层是否遮挡 1200 的 modal)。

## 复现/验证用查询

```sql
-- 该 session 的 loop 干预
SELECT ts, kind, payload_json FROM session_audit_events
WHERE session_id='5df29977-2f4b-478e-ab22-01171fcd4aa2' AND kind='loop_intervention';
-- 该 session 权限 ask
SELECT ts, kind, substr(payload_json,1,120) FROM session_audit_events
WHERE session_id='5df29977-2f4b-478e-ab22-01171fcd4aa2' AND kind IN ('tool_permission_ask','permission_timeout');
-- edit_file pitfall 记忆
SELECT memory_id, title, command_pattern, status, hit_count FROM autonomous_memories
WHERE tool_name='edit_file';
-- 全库 mode 变更
SELECT session_id, ts, kind, payload_json FROM session_audit_events WHERE kind LIKE 'mode_%';
```
