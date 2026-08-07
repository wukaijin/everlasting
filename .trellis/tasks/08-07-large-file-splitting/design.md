# Design — 大文件拆分(档位 1+2)与文档同步

> PRD: `prd.md`。本设计覆盖源码拆分(8 文件)、测试搬迁、文档更新、文档拆分(hub+parts)、链接修复。

## 0. 拆分总原则

- **纯搬迁**:内容原样复制 → 删源 → 新 module 接线;禁止顺手改逻辑(R5)。
- **验证节奏**:每个文件拆分后跑对应模块测试;Phase 4 全量终验。
- **回滚**:每个拆分独立 commit,`git revert <commit>` 即回滚。
- **链接更新范围**:只改活跃文档(`docs/`、`.trellis/spec/`、`AGENTS.md`、`.trellis/workflow.md`、活跃任务 artifacts);`.trellis/tasks/archive/` 是历史记录,**不改**。
- **调用点不变**:尽量用 re-export 保持既有 `use crate::...::wire::X` 等调用点零改动。

## 1. 源码拆分(8 个文件)

### 1.1 `wire.rs`(2,081 行)→ `wire/` 目录模块 — 第一优先,边界最干净

- 现状:`llm/provider/wire.rs`:类型 L71-238 + Chat→wire 转换 L239-686 + wire→Chat 转换 L757-970 + 内嵌测试 L970-2081(1,112 行)。
- 目标:
  - `wire/types.rs` — `WireCapabilities`/`WireRequest`/`WireMessage`/`WireBlock`/`WireTool` 等类型,全部 `pub(crate)`
  - `wire/to_wire.rs` — Chat→wire 方向(`chat_request_to_wire` 等),只依赖 `super::types`(命名说明:`in`/`out` 是 Rust 关键字,`mod in;` 非法,故用 `to_wire`/`from_wire`)
  - `wire/from_wire.rs` — wire→Chat 方向,只依赖 `super::types`
  - `wire/mod.rs` — `mod types/to_wire/from_wire;` + re-export,保持 `wire::` 路径调用点不变(测试专用项带 `#[allow(unused_imports)]`,对应原 `#[allow(dead_code)]` 语义)
- 测试:迁往 `llm/provider/tests_wire.rs`(provider/mod.rs 声明,参照 agent/mod.rs 的 tests_* 惯例);被测转换 fn 升 `pub(crate)`(R1.2 允许)。
- 验证:`cargo test --lib`(过滤 wire/provider)。

### 1.2 `group_chat_loop.rs`(1,933 行)→ + `group_chat_prompts.rs`

- 现状:纯函数簇 L129-423(`HaltReason`/`moderator_system_prompt`/`role_history`/`extract_tool_use_ids`/`row_carries_any_tool_result`/`group_chat_tool_defs`/`participant_system_prompt`)+ `reload_messages` L423 + `run_group_chat_loop` L482-899 + 内嵌测试 L916-1933(1,018 行)。
- 目标:
  - `group_chat_prompts.rs` — 上述纯函数(零 IO);`HaltReason`(循环退出枚举)与 `reload_messages`(DB IO)留在主文件
  - 测试去向(**落地修正,2026-08-08**):全部内嵌测试(纯函数单测 + identity_contract)→ 新建 `agent/tests_group_chat_prompts.rs`(agent/mod.rs 声明)——内嵌进新文件会使 292 行生产 + 1028 行测试 = 1320 行,超 1200 目标;纯函数已 `pub(crate)`(主文件本就需要),可见性无障碍。**注意:现有 `agent/tests_group_chat.rs`(919 行)是 08-04 集成测试(mock 完整多轮流程),不并入**。
- 验证:`cargo test --lib`(过滤 group_chat)。
- 注意:`agent-loop-architecture.md` "Group-chat transcript view" Pattern 引用 `group_chat_loop.rs` 路径/行号 → 归入 R2 sweep。

### 1.3 `loader.rs`(2,290 行)→ + `frontmatter.rs` + `cache.rs`

- `frontmatter.rs` — `Frontmatter` 结构 + `parse_frontmatter`/`apply_kv`/`parse_isolation`/`parse_tools_array`(纯函数零依赖)
- `cache.rs` — `SubagentCache` + 目录扫描/`locate_agent_file`
- 其余(`merge_with_inheritance`/`read_through`/`apply_model_line` 等)留在 `loader.rs`,实现时按依赖就近判断
- 测试:frontmatter 测试(L1191-1339)随迁;scan/cache 测试(L1359-1860)随迁;其余 → `subagent/tests_loader.rs`
- 验证:`cargo test --lib`(过滤 loader/subagent)。

### 1.4 `openai.rs`(2,219 行)→ + `streaming.rs`

- `streaming.rs` — `ToolCallBuf`/`accumulate_tool_call_delta`/`build_tool_call_event`/`parse_openai_usage`(L850-998,自包含单元)
- 测试:流式相关随迁,其余 → `llm/provider/tests_openai.rs`
- 验证:`cargo test --lib`(过滤 openai)。

### 1.5 `dispatch.rs`(3,040 行)→ + `resolve.rs` + `worktree.rs`

- `resolve.rs` — `resolve_model_by_name_or_id`/`resolve_final_model`/`resolve_worker_provider`/`resolve_project_main_path`(L1733-1835 中已 `pub(crate)` 的 4 个)+ `resolve_project_id`(L1814,**当前私有**)
- `worktree.rs` — `create_worker_worktree`(L1868-1986,**当前私有**)
- **可见性补正(评审 P1-2)**:`resolve_project_id`(L1814)与 `create_worker_worktree`(L1868)迁出后需升 `pub(crate)` —— 父模块 `dispatch.rs` 调用不受影响(同 crate),但测试迁到 `subagent/tests_dispatch.rs` 后需要该可见性(R1.2 允许)。
- 测试:resolve 相关随迁;其余 → `agent/subagent/tests_dispatch.rs`(subagent/mod.rs 声明)
- 验证:`cargo test --lib`(过滤 dispatch/subagent)。

### 1.6 测试搬迁统一规则

- 纯函数簇测试 → 随迁到新文件内 `#[cfg(test)] mod tests`(零可见性改动,最小 churn)
- 留在原文件的生产代码测试 → 同级 `tests_*.rs`(父 mod.rs 声明,`use super::*`);被测 fn 升 `pub(crate)` 或 `pub(crate) use` re-export(R1.2)
- 禁止:为测试改公开 API、顺手改逻辑

### 1.7 `streamController.ts`(2,683 行)→ + `streamEvents.ts`

- `streamEvents.ts` — 事件处理块 L1042-2183(`handleChatEvent` 633 行/`handleToolCall`/`handleToolResult`/`handleToolQuestion`/`finalizeRequest`/`reloadAfterFinalize`)+ 模块级 `buildPendingNotification`/`groupChatNotice`(L825-866)
- **硬约束(R1.3)**:`streamController.ts` return 块暴露给测试的项(`handleChatEvent`/`handleToolCall`/`finalizeRequest`/`putMessages`/`pinnedSessions`/`loadedFromDb`)必须保留 —— 从 `streamEvents.ts` import 后 re-export
- 测试:`streamController.test.ts` / `streamController.review.test.ts` 适配 import;若测试直接 import 内部符号,改为从 store 文件 import(re-export 保证路径不变)
- **注意(评审 P3-1)**:`.trellis/spec/frontend/state-management.md` 当前是 51 行空模板("To be filled"),**没有** Stream Controller Pattern 内容 —— 不是约束来源。Phase 2.7 拆分时把"return 块导出保留"约束就地写入该文件 Stream Controller 小节(填补真实 spec,符合 R3 文档同步精神);拆分参照以本 design §1.7 + 现有代码结构为准。
- 验证:`pnpm test`(streamController 过滤)。

### 1.8 `MessageItem.vue`(2,054 行)→ + `messageCards/*.ts` + `messageTimeline.ts`

- `messageCards/askCard.ts` — `parseAnswerEnvelope`/`resolveAskCardState`/`resolveAskCardProps`/`askCardPropsFor`(L234-481)
- `messageCards/modeChangeCard.ts` — 同构(L481-661)
- `messageCards/taskStateTransitionCard.ts` — 同构(L661-823)
- `messageTimeline.ts` — `renderTimeline`/`useTimeline`/`speakerLabel`/`speakerAccent`/`showSpeakerChip`(L144-228;从 vue import ref/computed,无组件依赖)
- 其余(流式状态 L823、内联编辑 L843-1091、markdown 管线 L1091-1164)留在 .vue
- 验证:`pnpm test` + `vue-tsc`(或项目现有 typecheck 命令)。

## 2. 文档拆分(6 个文件,统一 hub+parts 模式)

**hub+parts 原则**:原文件路径保留为小 hub(<500 行,总览+链接),内容按主题迁入同名目录 parts 文件(`<name>/<topic>.md`)。原路径引用不失效,锚点失效的按 R4 修。Rust 文件+同名目录可共存,markdown 同理。

**活跃引用文件数**(口径:精确文件名 `grep -rl`,排除 `tasks/archive/` 与本任务自身;命令 `grep -rl "<file>" docs .trellis/spec .trellis/workflow.md AGENTS.md .trellis/tasks | grep -v tasks/archive`):tool-contract.md 18 / memory.md 9 / agent-loop-architecture.md 10 / multi-provider-contract.md 8 / IMPLEMENTATION.md 33 / WORKFLOW-INTEGRATION.md 2。

| 文件(行数) | hub 内容 | parts 划分(实现时按实际章节定稿) |
|---|---|---|
| `.trellis/spec/backend/tool-contract.md`(3,671) | Scenario 索引 | 按 `## Scenario:` 分组:ReadGuard+Bash spillover / web_fetch / merge_worker / discard_worker / 其余 |
| `.trellis/spec/backend/memory.md`(1,498) | 总览+章节索引 | 按现有章节主题拆分 |
| `.trellis/spec/backend/agent-loop-architecture.md`(1,462) | 总览+Pattern 索引 | 按 Pattern/Signature 分组 |
| `.trellis/spec/backend/multi-provider-contract.md`(1,401) | 总览+章节索引 | 按 provider 契约章节拆分 |
| `docs/IMPLEMENTATION.md`(1,423) | §1-§3 索引 + §4 决策日志 | §4 决策日志 → `IMPLEMENTATION/decisions.md`(或按体量再分);注意 ROADMAP 的 "IMPLEMENTATION §4" 链接锚点需同步(R4);§4 内 `chat_loop.rs:657` 等行号引用随迁并保持锚点(评审 P3-2) |
| `docs/WORKFLOW-INTEGRATION.md`(1,318) | 总览+章节索引 | 按章节拆分 |

- **`tool-contract.md` 拆分前先去重(评审 P2-3)**:`## Scenario: request_mode_change tool (B6+ A)` 存在**两份**(L2599 带反引号 / L3063 不带,共 ~460 行,内容重叠——一份偏 audit 语义、一份偏 Signature),拆分 part 前先合并去重,否则拆出的 part 自带冗余。
- journals 不拆(已有 2000 行轮换规则,out of scope)。
- 拆分后每个 part <1200 行,hub <500 行(AC4)。

## 3. 文档更新与链接修复(R2/R4)

- 每次代码拆分落地后 sweep:`grep -rn "<旧路径/被搬符号>" docs/ .trellis/spec/ AGENTS.md .trellis/workflow.md .trellis/tasks/`(排除 archive),更新路径/行号/代码片段。
- 文档拆分后 sweep 锚点:`#xxx` 链接失效 → 指向新文件新锚点。
- 已知待修:ROADMAP/IMPLEMENTATION 里 "spec 见 agent-loop-architecture.md" 类引用在 hub 拆分后锚点可能变;26eb54b 刚同步的群聊章节含 `group_chat_loop.rs` 路径,1.2 拆分后需更新。

## 4. 兼容性与风险

- **零运行时变化**:全部为编译期搬迁;module 路径经 re-export 保持。
- **风险最高**:`streamController.ts`(测试依赖 return 块导出,两个测试文件要适配)、`dispatch.rs` 测试可见性(升 pub(crate) 面较大)。
- **不涉及**:`chat_loop.rs`(RULE-A-006 单一入口约束不碰)、`chat.ts`(薄门面)、生成文件。
- **顺序**:最干净 → 最复杂(wire → group_chat → loader → openai → dispatch → MessageItem → streamController),每个拆分可独立验收/回滚。
