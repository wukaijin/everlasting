# Implement — tools[] 上下文 token 治理 (C7)

> 配套 `prd.md` + `design.md`。执行顺序、验证命令、风险点。R2 已降级 Phase 2(prd decision 5),MVP 只做 R1 + R3。

## 执行顺序

**R1(度量基础设施)→ R3(静态裁剪)。** R2(Anthropic cache 断点)Phase 2,见 prd decision 5 / design §R2。
- R1 先:C 的收益量化依赖 R1 的 tools_token 数据。

---

## R1. tools[] token 度量

1. **migration 加列** — `db/migrations/schema.rs` turn_trace 段(`:967` `add_session_audit_events_column_if_missing(... "turn_seq" ...)` 旁):
   - 参照该 helper 写 `add_turn_trace_column_if_missing(pool, "tools_token", "INTEGER").await?;`(幂等)。
2. **trace CRUD** — `db/trace.rs`:
   - 扩 `upsert_turn_trace_token`(trace.rs:55)签名加 `tools_token: Option<u32>`,SQL `INSERT ... tools_token ... ON CONFLICT DO UPDATE SET tools_token = excluded.tools_token`。
   - `list_turn_traces`(trace.rs:255)返回行 struct 加 `tools_token: Option<i64>`。
3. **估算 + 落盘** — `agent/chat_loop/drive.rs:550`(`turn_tool_defs` freeze 后):
   - `let tools_json = serde_json::to_string(&turn_tool_defs).unwrap_or_default();`
   - `let tools_token = crate::memory::tokens::count_tokens(&tools_json).await;`(best-effort)
   - 在现有 `upsert_turn_trace_token` 调用点(drive.rs:801)传 `Some(tools_token)`。
   - worker turn(`skip_persist`):估算与落盘同门短路(评审 P2-2,ms 级,不修也可)。
4. **前端** — `<TracePanel>` TurnCard 加 tools_token 维度(复用 E2 store/IPC)。

**验证**:跑一个单对话 turn,TracePanel tools_token ≈ ~7-8k(对齐 design 静态估算)。

---

## R2. Anthropic tools cache 断点 — Phase 2(跳过)

实测降级(prd decision 5):2026-08-14 实测 session 50b91178(MiniMax-M3 / wukaijin relay)显示 relay 吃 `cache_control` 不 400 但 `cache_creation=0` → **relay 不缓存,R2 零收益**;原生 Claude 路径无 provider 未验证。

完整设计(第 0 步实测 gate、body-patch、断点预算、automatic caching 隐患)保留在 `design.md` §R2,等配原生 Anthropic provider 后重启。MVP 不实施。

---

## R3. 静态分组裁剪

1. **新过滤环节** — `tools/mod.rs`(或 `permissions/mode.rs` 旁):
   - `pub fn filter_tools_for_session_type(tools: Vec<ToolDef>, is_group_chat: bool) -> Vec<ToolDef>`:非群聊砍 `nominate_speaker` + `end_discussion`(落实 `tools/mod.rs:224` 注释 Phase 4)。
2. **接入** — `agent/chat_loop/drive.rs:504` 过滤链:
   - 在 `filter_tools_for_workflow(...)` 旁接 `filter_tools_for_session_type(..., is_group_chat)`。session_type/is_group_chat 从现有上下文取(复用 `group_chat.rs:80` 判定)。
3. **复核** `filter_tools_for_workflow`(`tools/mod.rs:244`)已覆盖 workflow 专属,不重复。

**验证**:非群聊 session TracePanel 看 tools 数 −2、tools_token 降 ~465;群聊 session 不变(`group_chat_tool_defs` 白名单优先)。

---

## 全局验证

```bash
# 后端单元测试(PKG_CONFIG_PATH 见 AGENTS.md,勿加 --test-threads=1)
cd app/src-tauri && \
  PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" \
  cargo test --lib

# 重点回归模块(单条 cargo test 调用内 filter,避免逐模块重跑的 11s 税)
cargo test --lib "permissions::tests_mode::filter_tools_for_mode"   # mode 过滤不回归
cargo test --lib "group_chat"                                        # 群聊白名单不变
cargo test --lib "trace::"                                           # turn_trace CRUD

# 前端
cd app && pnpm test && pnpm vue-tsc  # TracePanel 改动

# 手测
# 1. 单对话跑一轮 → TracePanel 看 tools_token ≈ ~7-8k
# 2. 群聊 vs 单对话 → 单对话 tools 数 −2
```

## 风险点 / 回滚

- **R1**:`count_tokens` async + `!Send` encoder,确认不阻塞主 loop(失败 `tracing::warn` 跳过,非 fatal — 与现有 trace 写入的 best-effort 一致,drive.rs:801 已是 `if let Err(e) ... warn`)。
- **R3**:确认 `nominate_speaker`/`end_discussion` 在非群聊被 chat_loop 拦截 no-op(`tools/mod.rs:220` 注释已说明);裁掉工具注册不影响该拦截逻辑(拦截按 tool_name,不依赖 tools[] 注册)。

## 不做(Phase 2 / OOS)
- D Stub 注册:等 R1 数据 tools 占比 >15% 窗口(prd Phase 2 触发条件)。
- memory 指令块治理:`docs/BACKLOG.md` §3.1。
