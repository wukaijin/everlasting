# E2 backend trace pipeline — 执行计划(child-1)

> checklist 详见 `../07-14-e2-harness-trace-viewer/implement.md` §2(B1-B9)。本文件为子 task 视角的执行入口 + 验证 + 风险。

## 执行 checklist(有序,详见 parent implement §2)

- [ ] B1 v7 migration:`turn_trace` 表 + `session_audit_events.turn_seq` 列。
- [ ] B2 db 层:TurnTraceRow + 4 upsert + list_turn_traces + clear_session_trace。
- [ ] B3 ChatEvent 3 变体 + wire。
- [ ] B4 `agent/trace.rs` trace_pipeline helper(3 record_* + token upsert)。
- [ ] B5 写点接入(chat_loop.rs:1261/2181/1800 + inject.rs:343)。**行号漂移,grep 复核**。
- [ ] B6 record_audit_event 扩 turn_seq + 21 类调用点传 seq。**grep 防漏**。
- [ ] B7 list_turn_traces + clear_session_trace IPC + 注册。
- [ ] B8 测试:upsert 累积 / turn_seq 填充 / emit+落盘一致 / worker gate / v7 升级零错。
- [ ] B9 验证:cargo check/test --lib/fmt(WSL PKG_CONFIG_PATH)。

## 验证命令

```bash
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo check
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib
cd app/src-tauri && cargo fmt
```

## 风险 / 回滚点(详见 parent implement §5)

| 文件 | 风险 | 回滚 |
|---|---|---|
| `db/migrations.rs` | v7 加表/列 | DROP TABLE + DROP COLUMN |
| `db/permissions.rs` + record_* 调用点 | turn_seq 签名扩散漏传 | git revert(机械) |
| `agent/chat_loop.rs` | 4 写点接入主 loop | 各写点独立逐个 revert |
| `llm/types.rs` | ChatEvent 加变体 | 删变体 |
| `agent/inject.rs` | breadcrumb 写点取 seq | 传参可回退 |

## start 前 follow-up

- [ ] sub-agent dispatch:implement.jsonl / check.jsonl 已 curate 真实入口(见两文件)。
- [ ] 实施时 trellis-implement prompt 以 `Active task: .trellis/tasks/07-14-e2-backend-trace-pipeline` 开头。
- [ ] 行号漂移:每个写点 grep 复核(research §7)。
- [ ] 完成后 trellis-check 验 AC1-AC4/AC8 + 零回归,再交 child-2。
