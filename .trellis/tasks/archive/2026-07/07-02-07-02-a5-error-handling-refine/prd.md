# A5 错误处理完善

> **2026-07-02 review 修订**:基于真实代码核对,修正了"11 个 thiserror"计数(实际 10 个对外类型,含 2 个手写 enum)、§代码侧 variant 名、spec drift 残留位置、以及 Out of Scope 与 implement.md 的兼容层矛盾。

## Goal

把 `.trellis/spec/backend/error-handling.md` 从"半成品模板 + spec drift"补成全栈错误契约的活文档;并按规范在 Rust + 前端落地:Tauri command 错误返回一次性升级为结构化 wire shape、10 个对外错误类型统一 `user_message()` / `category()` 模式、前端建错误总线按 category 路由。

价值:错误可观测(开发可定位)、可恢复(用户可重试)、用户可读(中文友好)、不静默(继承 RULE-A-003/007/012 不变量)。

## Background

### 当前状态(evidence 2026-07-02)

**规范侧**:`.trellis/spec/backend/error-handling.md`(237 行)— 仅 LlmError 5 类(Protocol 名与代码 InvalidRequest 不一致)+ Anthropic thinking 400 + RULE-A-007 终端不变量 + RULE-A-012 streaming/tracing + 2 条 Common Mistakes 已写;Overview / Error Types / Handling Patterns / API Error Responses 四章主框架共 **3 处** `To be filled by the team`(line 19/27/35)。

**代码侧**:**10 个对外错误类型**(权威盘点 `^pub enum .*Error`),仅 LlmError 有 `category()` / `user_message()`,其余 9 个无统一对外接口。组成:
- 8 个 thiserror:`LlmError`(`llm/error.rs`)、`GitError`(`git/error.rs`)、`MemoryInsertError` + `StatusTransitionError`(`db/memories.rs`)、`BackgroundShellError`(`background_shell/mod.rs`)、`ReflectError`(`agent/auto_reflect.rs`)、`WebFetchError`(`tools/web_fetch.rs`)、`ProviderBuildError`(`llm/provider/mod.rs`)。
- 2 个手写:`QuestionStoreError`(`agent/question_store.rs`,手写 Display + 空 `impl Error`)、`PreFlightError`(`agent/provider.rs`,**无 Display、无 Error impl**,仅有 `auth_message()`/`invalid_request_message()` 分方法)。
- 排除:`ValidationError`(`tools/ask_user_question.rs:175`,`pub(crate)` tool 内部错误,不冒泡 IPC 边界,本期不纳入)。

**Tauri command**:`commands/` 14 个文件中 13 个含 `Result<T, String>`(共 ~65 处;`commands/mod.rs` 仅 re-export 无 fn),无结构化错误返回。`commands/` 之外的 `chat` 走 ChatEvent stream 不在此列。

**前端错误处理**:散点 — `.catch(console.error)` 吞错(MessageItemEdit.vue 直接渲染后端原始字符串、streamController.ts 多处 .catch(log))、局部 toast(WorkerMergeControls)、Pinia store 内无统一错误入口。

**RULE-A-* 错误相关规则**(分散于 spec / DESIGN / ARCHITECTURE):RULE-A-001/002/006(闭环)/ A-003 / A-004 / A-007 / A-010(closed)/ A-012 / A-013 / A-014/015/016 / A-017,未在 error-handling.md 索引。

### spec drift

`error-handling.md` 称 LlmError 5 类包含 `Protocol`,代码 `llm/error.rs` 实际命名为 `InvalidRequest`。残留位置:**line 49 / line 54 是当前规格正文**(非历史叙述),需改写;line 205 已是 InvalidRequest。本期必修。

## Requirements

### R1 — error-handling.md 规范化

- **R1.1** 五章主框架(Overview / Error Types / Handling Patterns / API Error Responses / Common Mistakes)从 `To be filled by the team` 替换为活文档。
- **R1.2** LlmError Protocol → InvalidRequest 命名修正(line 49 / 54 改写),全文无残留(历史对比段落除外)。
- **R1.3** `API Error Responses` 章节定义 `AppCommandError` wire shape(`{ category, kind, message, retryable, request_id }`)与 5 类 category 全集。
- **R1.4** 新增 `RULES` 索引表,集中列出 RULE-A-001/002/003/004/006/007/010/012/013/014/015/016/017 + 简介 + 落地文件:line。

### R2 — Rust 错误类型契约统一

- **R2.1** 新增 trait `AppError`(`app/src-tauri/src/error.rs`):`fn category(&self) -> ErrorCategory`、`fn user_message(&self) -> String`、`fn retryable(&self) -> bool`(后者按 category 派生默认)。trait 超类型约束 `: std::error::Error` —— **`PreFlightError` 当前不满足,PR-A5-2 必须先补 `impl fmt::Display` + `impl std::error::Error`**。
- **R2.2** 10 个对外错误类型全部 `impl AppError`(精确 variant→category 映射见 design.md §5,以真实代码为准):
  - LlmError(直接迁移现有 category/user_message,LlmErrorCategory ↔ ErrorCategory 1:1 映射;补 retryable 默认)
  - GitError(NotARepo/Dirty→InvalidRequest,Io/Git2→Server)、MemoryInsertError(9 校验→InvalidRequest,Db→Server)、StatusTransitionError(NotFound/Illegal→InvalidRequest,Db→Server)、QuestionStoreError(AlreadyPending/NotFound→InvalidRequest)、PreFlightError(NoModel/ProviderMissing/BuildFailed→InvalidRequest,EmptyApiKey/DecryptFailed→Auth)、BackgroundShellError(NotFound/WrongSession/InvalidCwd→InvalidRequest,Spawn/Poisoned→Server)、ReflectError(Llm→Server,其余→InvalidRequest)、WebFetchError(HttpStatus 按 4xx/5xx 分流,BlockedAddress/BlockedRedirect/BodyTooLarge/InvalidUrl→InvalidRequest,Timeout/Tls/Network→Network)、ProviderBuildError(全部→InvalidRequest)。
- **R2.3** 新增 `AppCommandError { category, kind, message, retryable, request_id }` 结构(`app/src-tauri/src/error.rs`),`#[derive(Serialize)]` + `From<E> for AppCommandError` 覆盖 10 个领域类型 **+ `From<anyhow::Error>` 边界兜底**(commands 大量 `?anyhow`,无此转换 PR-A5-3 编译失败)。
- **R2.4** 10 个错误类型的 `user_message()` 返回中文友好消息,LlmError 现有文案不动;PreFlightError 现有 `auth_message()`/`invalid_request_message()` 收敛为单一 `user_message()`;其余按 category 模板填充。

### R3 — Tauri command 错误返回一次性升级

- **R3.1** `commands/` 全部 `Result<T, String>` 替换为 `Result<T, AppCommandError>`(13 个文件 ~65 处)。
- **R3.2** 每个 command 内部 `Err(anyhow!("..."))` / `Err(e.to_string())` 路径显式 `.map_err(AppCommandError::from)` 或 `?` 配合 `From` 转换(含 `From<anyhow::Error>`)。
- **R3.3** `grep -rE "Result<.*, *String>" app/src-tauri/src/commands/` 无结果(允许 `Result<T, ()>` / `Result<T, serde_json::Value>` 等非错误返回)。
- **R3.4** request_id 由高频 command(chat / cancel / sessions / projects / merge_worker / discard_worker)**透传前端 invoke 传入的 `requestId`**;轻量 command(config读 / 健康检查)可为 None。后端不新生成 uuid(假设默认,详见 design §8)。

### R4 — 前端错误总线

- **R4.1** 新增 `app/src/composables/useErrorBus.ts`(composable,推荐)— 统一错误收口:`invoke()` 错误 → `parseAppCommandError(e)` → `errorBus.push(err)`;前端 API 调用层 `.catch(e => useErrorBus().handle(e))`。
- **R4.2** `parseAppCommandError(e: unknown): AppCommandError | null` 工具 — 容错 Tauri `String` rejection(老链路残留 / JSON parse 失败 → 降级为 `{ category: 'Server', kind: 'Unknown', message: String(e), retryable: false }`);`isAppCommandError` 含 `category` 值域校验(防误判)。
- **R4.3** 路由表(`category` → 用户行为):
  - `Auth` → toast + 引导去 Settings(检查 ANTHROPIC_API_KEY)
  - `RateLimit` → toast("请求过于频繁,请稍后再试")
  - `InvalidRequest` → 内联错误(按调用点 MessageItemEdit / 表单字段下)
  - `Server` → 顶部红色 toast(retryable=true 时附"重试"按钮)
  - `Network` → 顶部红色 toast(检查网络)
- **R4.4** 全局未捕错误器:`window.addEventListener('error')` / `unhandledrejection` 统一入错误总线。
- **R4.5** TypeScript 类型:`AppCommandError { category: ErrorCategory; kind: string; message: string; retryable: boolean; requestId?: string }`(`ErrorCategory = 'Auth' | 'RateLimit' | 'InvalidRequest' | 'Server' | 'Network'`)。
- **R4.6** errors 数组上限 FIFO(`MAX_ERRORS = 50`),防长会话无限增长;单条 dismiss / TTL 留 toast UI follow-up。

### R5 — 测试与验证

- **R5.1** Rust 单元测试:10 个错误类型的 `category()` + `user_message()` 输出快照(~41 个 variant),含 `WebFetchError::HttpStatus` 4xx/5xx 分流、`PreFlightError::EmptyApiKey`→Auth 边界;`AppCommandError::from(E)` 转换 + `From<anyhow::Error>` 兜底正确。
- **R5.2** Rust 单元测试(grep-style,落 `tests_error_contract.rs` 或 build.rs):`grep -rE "Result<.*, *String>" app/src-tauri/src/commands/` 无结果。
- **R5.3** 前端 vitest:`parseAppCommandError` 容错(Tauri String / JSON parse fail / 正常结构 / 含 4 字段但 category 非法值不误判)、`useErrorBus` 按 category 路由 mock、FIFO 上限。
- **R5.4** `cargo check`、`cargo test --lib`、`cd app && pnpm build`(`vue-tsc --noEmit` + `vite build`)、`pnpm vitest run` 全绿。
- **R5.5** 端到端冒烟(可选):Tauri dev 跑通 chat 路径、故意构造错误触发 5 类 category 各一次,目视 toast 行为。

## Acceptance Criteria

- [x] AC1: `.trellis/spec/backend/error-handling.md` 五章(Overview / Error Types / Handling Patterns / API Error Responses / Common Mistakes)无 `To be filled by the team` 字样。
- [x] AC2: `grep -n "Protocol" .trellis/spec/backend/error-handling.md` 仅在历史叙述或对比段落出现;line 49 / 54 的当前规格正文 Protocol 必须改写;代码 `llm/error.rs` 无 `LlmError::Protocol` 变体。
- [x] AC3: `error-handling.md §API Error Responses` 定义 `AppCommandError` 完整 schema + 5 类 category 全集与含义。
- [x] AC4: `error-handling.md` 顶部新增 `RULES` 索引表,覆盖 RULE-A-001/002/003/004/006/007/010/012/013/014/015/016/017。
- [x] AC5: 10 个对外错误类型均有 `impl AppError`,提供 `category()` + `user_message()` + `retryable()`;Rust 单元测试覆盖 10/10 类型及 ~41 variant(含 HttpStatus 分流、PreFlightError Auth 边界)。
- [x] AC6: `grep -rE "Result<.*, *String>" app/src-tauri/src/commands/` 无结果(允许 `Result<T, ()>` / `Result<T, serde_json::Value>` 等非错误返回)。
- [x] AC7: `AppCommandError { category, kind, message, retryable, request_id }` 在 `app/src-tauri/src/error.rs` 集中定义;`From<E>` 覆盖 10 个领域类型 + `From<anyhow::Error>`。
- [x] AC8: `app/src/composables/useErrorBus.ts` 存在;`parseAppCommandError` 工具存在且容错 3 种输入形态 + category 值域校验;Tauri String 拒绝 → JSON parse 失败 → 正常结构。
- [x] AC9: 前端按 category 路由表在 useErrorBus 内实现,5 类 category 各自有 mock 单测覆盖。
- [x] AC10: 全局未捕错误器(`window.onerror` / `unhandledrejection`)接入 useErrorBus。
- [x] AC11: `cargo check` 0 warning、`cargo test --lib` 全绿、`pnpm build` 全绿、`pnpm vitest run` 全绿。

## Out of Scope

- 多语言 i18n(目前 `user_message` 中文单语,后续如需多语种,可抽 resource 表,本期不做)
- 自动重试策略(`retryable` 字段暴露给前端决策,本期前端不实现自动重试)
- Telemetry / Metrics 上报(线上监控方案后续 follow-up)
- `PermissionDenied` / `Cancelled` / `NotFound` 等独立 category(本期归并到 LlmError 5 类,后续可按需扩展)
- 现有 `.catch(console.error)` 调用点全量替换为 useErrorBus(本期仅替换关键高频点;剩余视为 follow-up,errorBus 容错保证遗留点不崩)
- IPC `String` 老链路兼容期(**本期一次性全切,无兼容期,不留 `From<AppCommandError> for String` 兼容层**;PR-A5-3 后端签名与 PR-A5-5 前端 catch 必须**同一次发布**合并,消除中间不一致窗口。若上线出现 regression 走 `git revert` hotfix)
- `ValidationError`(`tools/ask_user_question.rs`,`pub(crate)` tool 内部错误)纳入 impl AppError(本期不纳入,调用方自行消化)
- 单条错误 dismiss / TTL 过期 UI(留给 toast UI follow-up,本期仅做 FIFO 上限)

## Risks & Mitigations

| Risk | Mitigation |
|---|---|
| 一次性全切 IPC 契约 → 既有 Tauri signature 全改(~65 处),可能漏 command | R5.2 grep-style 测试兜底 + design.md §5 列 10 类型 ~41 variant 真实映射清单 |
| spec 与代码不同步复发(Protocol→InvalidRequest 刚改完) | R5.1 + R5.2 测试 + error-handling.md `Common Mistakes` 加 "spec drift" 条目 |
| `user_message` 中文一致性差 | R2.4 按 category 模板填充 + 单元测试断言包含特定关键词 |
| 前端 .catch 散点未全替换 → 错误静默 | R4.2 容错 + 全局未捕错误器兜底(legacy 漏点仍可见) |
| `PreFlightError` 缺 Display/Error impl → trait 约束不满足 | PR-A5-2 显式补 `impl Display` + `impl Error`(design §2/§4 已标注隐藏工作量) |
| commands `?anyhow` 无 From → PR-A5-3 编译失败 | R2.3 `From<anyhow::Error>` 边界兜底(design §3) |
| PR-A5-3/A5-5 中间窗口前后端不一致 | Out of Scope 强制两者同次发布(design §7) |

## Open Questions

无(已 brainstorm 收敛 + review 修订;request_id / errors lifecycle / ValidationError 取舍 / PR 发布窗口 均按 design §8/§6 假设默认落地,可推翻)。
