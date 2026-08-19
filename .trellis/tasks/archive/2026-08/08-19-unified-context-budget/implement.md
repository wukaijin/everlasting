# Implement — 统一 token 预算表 + 关卡⑤硬卡

> 4 PR 顺序(后端 WP1 → 前端 WP1 → 后端 WP2 → 前端 WP2 + 烟测收口)。验证命令见文末;风险文件见 §3。

## PR1 — WP1 后端:新列 + 统一估算 + 口径修正 + 时序重排

- [ ] `db/trace.rs`:migration 幂等加 `at_files_token` / `system_token` / `context_window` 三列(`add_turn_trace_column_if_missing` 模式);`upsert_turn_trace_token` 扩参;`TurnTraceRow` struct + 查询侧同步。
- [ ] `agent/at_file.rs`:`inject_at_tokens` 扩展——遍历全部 user message 时对注入正文 `count_tokens`,返回同请求临时 spans `Vec<AtFileSpan { msg_idx, start, end, path, tokens }>`(经 loop state 传给 budget_gate,**不落 DB**——@文件每 request 重展开,DB spans 必 stale,评审 F5 裁定)。
- [ ] `agent/chat_loop/init.rs`:system prompt 本体(发送部件)+ skill listing 合成消息(messages 内归因)`count_tokens` → loop state。
- [ ] `agent/budget.rs`(新):`estimate_request_tokens(system, tools_json, messages)`(**三部件加法:messages 已含 memory/skill/@文件/图片,不单独加计任何切片**——评审 F1)+ `BUDGET_LINE_RATIO = 0.95` 常量。
- [ ] `agent/chat_loop/drive.rs`:**时序重排**——tools 过滤链 + stubify + 元工具 append + tools_token 估算(`:752-852`)整体挪到压缩块(`:215`)之前;压缩触发 / 摘要 postcheck / 机械 compact 三处口径改 `estimate_request_tokens`(机械路径无 gate 一并切换)。
- [ ] `ChatEvent::Done` 写点扩三列(含 `context_window` 快照)。
- [ ] 测试:AC1(总量 = 三部件之和;**归因切片互不重叠且之和 ≤ 总量**)、AC2(tools+system 挤窗触发压缩,旧口径不触发)、AC3 前置用例(**注入后 message 被 APPEND 合成消息的轮次 span 定位仍正确** + span 失配 fail-open——评审 F5)、时序重排回归(stub 粘性 / tools_token 语义 / messages 部件自身超线场景行为不变)。
- [ ] `scripts/turn-smoke.sh` 报告列扩展。

## PR2 — WP1 前端:预算行 + per-model 窗口

- [ ] `app/src/types/turnTrace.ts`:三新字段。
- [ ] `app/src/components/trace/TurnCard.vue`:`contextUtilPct` 弃用 `CONTEXT_WINDOW_REF = 200_000` 硬编码,改行内 `contextWindow`(NULL 回退 200_000);加预算行(**实发**总量 vs 窗口)+ 五切片占比条(残差 = 实发总量 − 五切片,钳 0;不混用 `context_input`,D9)。
- [ ] 测试:AC4 前端半边(渲染/回退/占比)。

## PR3 — WP2 后端:budget_gate + 裁剪引擎

- [ ] `agent/budget.rs`:`enforce_budget` + 裁剪三臂(@文件 span 占位替换 / 旧图 B1 占位降级 / memory 节目录态视图)+ 臂尽 fail-fast(`stop_reason="context_over_budget"`,错误含 breakdown);每臂记 `{kind, count, tokens_freed}`。
- [ ] `db/config.rs` 消费:`context_budget_enabled`(fail-open on);gate `&& !worker && !群聊`。
- [ ] `agent/chat_loop/drive.rs`:send 前接入 gate(图片 resolve 后);done 分支落 audit;**trace 各切片列改记实发值(预裁 − freed,评审 F2/D9)**。
- [ ] `AuditKind::ContextBudgetTrim`(enum 变体,无 migration);payload 见 design §3。
- [ ] 测试:AC3(优先级顺序/非破坏性/当前 turn 不裁/span 失配 fail-open)、AC5(fail-fast breakdown)、AC6(gate 豁免与开关)、AC4 后端半边(实发口径落值)。

## PR4 — WP2 前端 + live 收口

- [ ] `ChatEvent::BudgetTrim`(非持久化,`Retrying` 先例)+ streamController 路由 + ChatPanel 瞬时 chip;TracePanel 徽标。
- [ ] AuditLogModal 加 kind 条目(icon/配色)。
- [ ] live 烟测(AC7):turn-smoke 构造大 @文件注入 session,验证预算行 + 裁剪链路 + audit;重编 daemon 后实跑。
- [ ] spec 回写:`token-usage-tracking`(新列口径 + 统一估算)、`agent-loop-architecture`(关卡⑤ budget gate Pattern)、`database-guidelines`(turn_trace 新列)。

## 验证命令

```bash
# 后端(WSL 需 PKG_CONFIG_PATH,见 AGENTS.md)
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib
cargo clippy --all-targets -- -D warnings && cargo fmt --check

# 前端
cd app && pnpm test && pnpm build   # build 含 vue-tsc

# live 单轮烟测(daemon 需重编重启)
scripts/turn-smoke.sh
```

## 风险文件 / 回滚点

| 文件 | 风险 | 回滚 |
|------|------|------|
| `agent/chat_loop/drive.rs` | 时序重排动主干(压缩块与 tools 链换序) | PR1 单 commit revert;口径可临时降 TRIGGER_RATIO |
| `agent/chat_loop/init.rs` | 计数点插入 + metadata 扩展 | 纯加法 |
| `agent/at_file.rs` | spans 标记(需与注入区间精确对齐) | 纯加法,WP2 消费 |
| `db/trace.rs` | 幂等加列 | 老 DB 零动作 |
| WP2 总闸 | 行为变化 | `context_budget_enabled=false` 即回 WP1 行为 |

## start 前检查

- [x] prd/design/implement 三件齐备 + start 前评审 6 项发现独立核实处置完毕(见 prd.md Review Resolution)。
- [x] `implement.jsonl` / `check.jsonl` 已 curate 真实条目(ZCode 属 sub-agent-dispatch 平台,`workflow.md:186`,jsonl 门适用——评审 F3)。
- [ ] 基线:start 时 main 全量测试实测通过(已知 2 处既有 flaky,见 journal 104;AC8 基线以实测为准)。
