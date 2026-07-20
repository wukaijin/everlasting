# A5 错误处理完善

> **状态**:planning → in_progress(待 `task.py start`)。Scope 已确认 B + 1 PR,Requirements / AC 已落,research 5 份已固化。

## Goal

打磨错误处理体系,补齐 A5 主体落地(2026-07-02 commit `bab84bd` + R3.4 `eabd422`)后仍散落的 polish 缺口,提升个人工具日常使用的体感。

## Background — 现状事实(Evidence Pass 1,2026-07-17)

### 1. 已 OK(主体已落,不在 A5 范围)

- **A5 全栈错误契约主体**(07-02):`AppCommandError` wire shape + `useErrorBus` + 10 个 AppError impl + `error.rs:0-754`。
- **A5+ retry + Full Jitter + retry_after**(07-05):`llm/retry.rs` + Retry chip 前端。
- **LlmError 5 类全有 user_message 中文**(`llm/error.rs:62-70`) + **10 个 AppError 都 user_message**(`error.rs:140-373`)。
- **C4 审计日志查询 UI** + **C2+ LoopIntervention audit 落表** + **D3 EditMessage/ResendMessage audit**。
- **error wire 链路**:后端 emit `ChatEvent::Error { message, category }` → 前端 `streamController.ts:1147-1219` → `MessageItemFooter.vue:128-137` 红字 inline。
- **RUST_LOG 走 EnvFilter,默认 `info`**(`main.rs:42-43`)— 默认级别 OK。
- **DEBT.md open items:0** — 全部 closed。
- **SPEC-DRIFT.md 待审:0** — 全部已审 resolved(DRIFT-001 二次取消留 V3)。

### 2. 完全缺失 / 明显缺口(A5 高优候选)

| 缺口 | 位置 | 影响 |
|---|---|---|
| **`useErrorBus.routeByCategory` 5 个 stub 全 `console.warn`,不上 toast** | `app/src/utils/useErrorBus.ts:137-180` | 全局未捕错误 + invoke rejection 无视觉反馈,只 devtools 看到。TODO `useErrorBus.ts:158` 标"接 reka-ui toast"从未接 |
| **`AppCommandError.retryable` 前端从不消费,无"重试"按钮** | `MessageItem.vue` / `MessageItemFooter.vue` / `MessageItemEdit.vue` | A5 retry 失败后用户只能手打 prompt 重新发 |
| **LlmError::Auth 文案 hardcoded `"ANTHROPIC_API_KEY"`** | `llm/error.rs:143` | OpenAI / GLM provider 看到错误文案不对(provider name 文案化) |
| **`audit.rs:4` docstring "18 variants" 与实际 25 drift** | `audit.rs:4` | 二阶 SPEC-DRIFT(DRIFT-003 已修 spec 但未修 source docstring) |
| **`SubagentDrawer` banner + error card 全英文** | `SubagentDrawer.vue:425-441` + `SubagentDrawerErrorCard.vue` | worker 错误用户体感冷(英文) |
| **subagent `status=incomplete` chip 缺失** | `SubagentDrawer.vue:792-807` | 只有 cancelled 有专用 chip, incomplete 仅靠 banner 文字 |
| **Background shell 错误不上 toast** | `in_memory.rs:209-240` | spawn fail 只通过 agent loop notification message 给 LLM, 用户不打开 chat 流看不到 |
| **`workflow/def.rs::load_workflow` 4 级降级全失败时用户无提示** | `def.rs:626-715` + `PluginSelect.vue` | 全部 `tracing::warn!` 吞掉, 用户看到 0 个 plugin, 不知有错 |
| **window 兜底不全**:普通 Error 实例 `parseAppCommandError` 返 null → 静默丢弃 | `app/src/main.ts:16-26` + `useErrorBus.ts:128-130` | 前端非 IPC runtime 错误几乎全部静默 |
| **`extractErrorMessage` 兜底"(未知错误)"** | `useErrorBus.ts:132` | 无可操作提示 |

### 3. 存在但需 polish(中优候选)

- **后端 100+ 处 `anyhow::anyhow!("<英文>: {}")` 在 `commands/` 走 fallback**(`error.rs:437-480`):用户在前端看到英文错误。`anyhow::Context` trait 完全未用(`.context()` = 0 处)。
- **subagent `dispatch.rs` 至少 17 处 `tracing::warn!` 吞错**(`dispatch.rs:215-216, 493, 500, 569, 696, 789, 811, 1192-1193, 1202, 1274, 1303, 1316, 1518, 1535, 1560, 1663, 1678`):worker 内部错误路径用户无视觉反馈。
- **RULE-A-011 tracing-only,user-facing message 不携带 reqwest 细节**(`chat_loop.rs:1732-1750` + `LlmError::Network.user_message()` 泛化):用户看不到"per-chunk read timeout"等具体信号。
- **`audit.rs:4` docstring "18 variants"**: 已记入 §2。
- **`console.error` 38 处 / `console.warn` 9 处** 前端:仅 devtools,漏 toast 路由失败全走这里。

### 4. 不在 A5 范围(明确定位)

- **DRIFT-001 二次取消语义**:V3 评估,不在 A5。
- **⑬ ⑮ 路由/Channel 输出 audit**:ARCHITECTURE §2.5.8 标注"未实施,daemon 化未到 V2 第一档",不在 A5。
- **workflow 4 级降级** 本身(architecture OK):只补"全失败时用户提示",不动降级逻辑。
- **`Anthropic`/`OpenAI` Provider 文案化** 是 LlmError::Auth 的修复子项,不在 A5 范围。

### 5. 关键 file:line 索引(实施时回查)

- 后端错误入口:`app/src-tauri/src/lib.rs` `init_tracing` / `main.rs:40-43` `EnvFilter::new("info")`
- LlmError 中文消息:`app/src-tauri/src/llm/error.rs:62-70, 86-93, 128-152`
- AppCommandError + fallback:`app/src-tauri/src/error.rs:0-754`(`from_anyhow` fallback line 437-480)
- AuditKind enum:`app/src-tauri/src/agent/permissions/audit.rs:33-172`(25 个 variant)
- 后端 commands/ anyhow 落点:`commands/{worktree,subagents,command_palette,files,panel,memory,projects,sessions,providers,permissions,subagent_runs,config,ui}.rs`(共 100+ 处英文 anyhow)
- background_shell 错误:`app/src-tauri/src/background_shell/{mod.rs:204-226, in_memory.rs:209-240}`
- workflow 错误:`app/src-tauri/src/agent/workflow/{task.rs:244-278, state.rs:67-85, def.rs:512-545, 626-715}`
- subagent 错误:`app/src-tauri/src/agent/subagent/{sink.rs:421-700, dispatch.rs:215-216, ...}`
- 前端 toast 基础:`app/src/stores/projects.ts:40, 77-91` + `app/src/components/layout/AppShell.vue:72-80, 129-141`
- 前端 error UI:`app/src/components/chat/MessageItemFooter.vue:128-137` + `ToolCallCard.vue:64-87` + `SubagentDrawer.vue:425-441, 466-502, 792-807`
- 前端 useErrorBus:`app/src/utils/useErrorBus.ts:127-180` + `app/src/main.ts:16-26`

## Requirements

> Scope = B(最小闭环)+ 1 PR(2026-07-17 user 确认)。基于 5 份 research 收齐的事实,scope B 收敛在 2 个交付:

### R1: `useErrorBus.routeByCategory` 全 5 stub 接 reka-ui Toast(全局兜底路由)

- **动机**:`useErrorBus.ts:127-180` 5 个 stub 全 `console.warn`,TODO `useErrorBus.ts:158` 标"接 reka-ui toast"从未接;全局未捕错误(`window.onerror` / `unhandledrejection`)用户无视觉反馈。
- **决策**:
  1. **reka-ui 2.9.9 Toast primitive 完整可用**(`ToastRoot/ToastProvider/ToastViewport/ToastPortal/ToastAction/ToastClose/ToastTitle/ToastDescription`),无版本问题(`research/01-toast-mechanism.md`)。
  2. **自研 `useToast()` composable + queue 化**(`research/01-toast-mechanism.md` 方案 A,改动量 ~150-250 行)。理由:零运行时依赖增量,沿用项目 composable 风格(`useErrorBus` / `useKeyboard` 模式)。
  3. **接入范围:仅全局兜底**(`research/05-useErrorBus-stub-callsites.md`)。`routeByCategory` 实际只服务 `main.ts:17-27` 1 个调用点;主链路 100% 不走 useErrorBus(ChatEvent::Error 直写 `last.error` + 36 处 IPC 各自 try/catch),scope B 不动。
  4. **路由策略**(按 category):
     | Category | 路由 | 文案前缀 |
     |---|---|---|
     | Auth | toast(中等) | "鉴权失败" |
     | RateLimit | toast(中等) | "请求过于频繁" |
     | Server | toast(中等) | "服务端错误" |
     | Network | toast(中等) | "网络问题" |
     | InvalidRequest | 保留 `console.warn`(本地错误,不打扰用户) | — |
- **范围(留 5 stub 接 toast 的同时,不动)**:
  - 不动主链路 36 处 IPC 错误展示
  - 不改 `useErrorBus.ts:132` `extractErrorMessage` 兜底"(未知错误)"(留 §3 polish)
  - 不动 `useErrorBus.routeByError` / `routeByString` 现有调用源

### R2: `AppCommandError.retryable` 前端消费 — MessageItemFooter 加"↻ 重试"按钮

- **动机**:`research/02-retryable-wire-flow.md` 确认 wire 完整(后端 `AppCommandError.retryable` 已有,前端 interface 完整,`Category → retryable` 默认派生规则与后端 `AppError::retryable()` 一致);但消费端 0 处读。A5 retry 失败后用户只能手打 prompt 重新发。
- **决策**:
  1. **不动后端 wire** — `research/02` 确认 `ChatEvent::Error` 不带 retryable 是正确的(避免消息体膨胀),前端用 `Category` 派生(`error.ts` 已定义映射)。
  2. **重试按钮仅在 `MessageItemFooter` 加**(`research/03-message-error-ui.md`)。其余 3 个错误渲染点不动:
     - `MessageItemEdit`:edit 局部操作,语义不匹配重试
     - `ToolCallCard`:tool 决策错误,重试应走 tool 级,不在 chat message 层
     - `SubagentDrawerErrorCard`:worker 子任务错误,有独立 retry 流程(L3b),不在 scope B
  3. **重试实现**(`research/04-chat-resend-flow.md`):**复制 `chatStore.resendMessage` 结构(chat.ts:1257-1341),参数化为 `retryChat(sessionId, messageSeq)`**。关键差异:
     - **不 push 新 placeholder**
     - **mutate 已有 errored assistant**
     - 用 `max+1` seq 重建流
     - **不带 `resendSeq`**(用户级操作,不是消息编辑)
  4. **无 AbortController 引入**:Tauri `cancel_chat` 是独立 IPC,沿用项目既有模式。
- **重试按钮 UX**:
  - 仅当 `category` 派生 `retryable=true`(Auth/InvalidRequest 隐藏;RateLimit/Server/Network 显示)
  - loading 态:点击 → 按钮变 "重试中..." disabled → session 流进入时复位
  - 错误 toast 仍由 R1 的 `useToast` 兜底,按钮点完未必要弹新 toast(避免重复)

### R3(隐含,需在 implement.md 展开):测试 + 文档同步

- 新增 `useToast` composable 单测(priority + queue + dedupe 同 category 同 message)
- 新增 `MessageItemFooter` retry button 单测(retryable 显隐 / 点击触发 / loading 态)
- 文档同步:
  - `.trellis/spec/backend/error-handling.md` 加 `RULE-A-018` — 全局错误 toast 路由(对应 commit 引用)
  - `.trellis/spec/frontend/state-management.md` 加 `useToast` composable 模式条目
  - 不动 SPEC-DRIFT(scope B 不解决 audit.rs:4 docstring 等二阶 drift,留 V3)

## Acceptance Criteria

> 与 R1/R2/R3 一一对应,可机器/手测。

| AC | 验证方式 | 对应 R |
|---|---|---|
| **AC1** | `window.onerror` 抛 `Error('test auth fail')` + 自定义 category → 屏幕右上 reka-ui Toast 弹出 "鉴权失败" | R1 |
| **AC2** | `unhandledrejection` 抛 `Promise.reject({category: 'Network', message: 'timeout'})` → Toast 弹出"网络问题: timeout" | R1 |
| **AC3** | 路由策略表 5 类 × 2 入口(throw / reject)= 10 case 单元测试全过 | R1 |
| **AC4** | `MessageItemFooter` 当 category 派生 retryable=true(RateLimit/Server/Network)显示"↻ 重试"按钮;Auth/InvalidRequest 不显示 | R2 |
| **AC5** | 点击"↻ 重试" → 已有 errored assistant 卡片状态切到 "重试中..." → 复用 `retryChat(sessionId, messageSeq)` 触发新流 → 按钮复位 | R2 |
| **AC6** | `useToast` queue + dedupe 单测 + `MessageItemFooter` retry 交互 vitest 单测 全过 | R3 |
| **AC7** | `cd app/src-tauri && cargo check`(PKG_CONFIG_PATH 配置)0 err / `cd app && pnpm build` 0 err / `pnpm vitest run` 全过(0 fail)+ 无新增 warning | R3 |
| **AC8** | `.trellis/spec/backend/error-handling.md` 加 RULE-A-018 / `.trellis/spec/frontend/state-management.md` 加 useToast 条目 — 不引入事实错误,与代码 1:1 对齐 | R3 |

## Out of Scope

- DRIFT-001 二次取消(V3)
- ⑬ ⑮ audit 落表(daemon 化未到 V2)
- workflow 4 级降级逻辑(只补用户提示)
- Provider 文案化(不在 A5,可作后续独立 task)

## Open Questions — Resolved (2026-07-17)

| Q | 决策 | 依据 |
|---|---|---|
| A5 范围怎么定? | **B. 最小闭环**(useErrorBus + retryable + 重试按钮) | user 确认 2026-07-17,见 `journal-1.md` Session 25 + 本 task journal |
| PR 怎么拆? | **1 个 PR** | user 确认 2026-07-17 |
| `useToast` 用什么形态? | **自研 `useToast()` composable + queue 化,基于 reka-ui 2.9.9 Toast primitive** | `research/01-toast-mechanism.md` 方案 A;零依赖增量 + 沿用 composable 模式 |
| 重试按钮放哪? | **`MessageItemFooter` 单点**;其余 3 个错误渲染点不动 | `research/03-message-error-ui.md` 语义分析 |
| 重试实现:复用 vs 新建? | **复制 `chatStore.resendMessage` 结构并参数化为 `retryChat(sessionId, messageSeq)`** | `research/04-chat-resend-flow.md` 关键差异:不 push placeholder + mutate errored assistant + 不带 resendSeq |
| `routeByCategory` 接多广? | **仅全局兜底**(`main.ts:17-27` 唯一调用点);主链路 36 处不动 | `research/05-useErrorBus-stub-callsites.md` 调用频次分析 |

## Notes

- 调研证据来源:
  - **Evidence Pass 1**(2026-07-17):prd.md §Background,Explore sub-agent 8 维度扫描,token 用量 394k
  - **Evidence Pass 2**(2026-07-17):`research/01-05.md` 5 份,trellis-research sub-agent 深读,覆盖 toast / retryable / 错误 UI / resend / stub callsite
- 与 A6 README 独立,本 task 不写 README 相关
- 已固化为 SPEC-DRIFT 候选(若确认 A5 中需要修):`audit.rs:4` docstring 二阶 drift、SubagentDrawer banner 中文化(均留 V3,不在 scope B)
- B 类范围评估:Scope B 改动量估算 ~330-460 行(含测试 + 文档),1 PR 容量合理