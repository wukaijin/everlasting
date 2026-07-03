# A5 错误处理完善 — Implement

> **2026-07-02 review 修订**:PR-A5-2 补 PreFlightError Display+Error impl 与 `From<anyhow::Error>` 隐藏工作量;PR-A5-3 删除与"无兼容期"矛盾的临时兼容层、明确 request_id 透传、注明与 PR-A5-5 同次发布;§4 行号按真实代码校准;variant 映射以 design.md §5 为准。

## 1. PR 切片(按依赖顺序,共 5 PR)

### PR-A5-1: 规范文档与契约定义(spec drift 修正)

**改动**:
- `.trellis/spec/backend/error-handling.md`:填 Overview / Error Types / Handling Patterns / API Error Responses 五章;修正 LlmError Protocol → InvalidRequest(**line 49 / 54 当前规格正文改写**,line 205 已是 InvalidRequest 不动);新增 RULES 索引表(RULE-A-001/002/003/004/006/007/010/012/013/014/015/016/017)
- `docs/IMPLEMENTATION.md §4`:新增 A5 ADR 条目

**验证**:
- `grep -n "To be filled by the team" .trellis/spec/backend/error-handling.md` 无结果
- `grep -n "Protocol" .trellis/spec/backend/error-handling.md` 仅历史叙述保留

**风险**:无,纯文档
**回滚**:`git revert` 即可

### PR-A5-2: Rust AppError 契约层(10 个 impl + 隐藏工作量)

**改动**:
- 新增 `app/src-tauri/src/error.rs`:`AppError` trait + `ErrorCategory` enum + `AppCommandError` struct + 10 个领域 `From<E>` impl + `From<anyhow::Error>` 边界兜底
- `app/src-tauri/src/lib.rs`:`mod error;`
- **`app/src-tauri/src/agent/provider.rs`:先补 `impl fmt::Display for PreFlightError` + `impl std::error::Error for PreFlightError`(当前两者皆无),再 `impl AppError`**;现有 `auth_message()`/`invalid_request_message()` 收敛进 `user_message()`
- 其余 9 个错误类型文件 `impl AppError`(LlmError / GitError / MemoryInsertError / StatusTransitionError / QuestionStoreError / BackgroundShellError / ReflectError / WebFetchError / ProviderBuildError),variant→category 映射严格按 design.md §5(真实代码 variant 名)
- 单元测试:`error.rs` 内覆盖 10 个 impl 的 category / user_message 输出(含 `WebFetchError::HttpStatus` 4xx/5xx 分流、`PreFlightError::EmptyApiKey/DecryptFailed`→Auth 边界)

**验证**:
- `cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib error::` 全绿
- 10 个 impl 各有至少 1 个 category() + 1 个 user_message() 断言测试;HttpStatus 分流有 2 个(4xx + 5xx)

**风险文件**:`llm/error.rs`(核心 hot path);`commands/` 还未迁移,但 trait + struct 是 additive,不影响现有逻辑
**回滚**:`git revert` 即可(trait + struct 未被 commands 使用时零影响)

### PR-A5-3: Tauri command 错误返回一次性切换

**改动**:
- `app/src-tauri/src/commands/*.rs`(13 个文件 ~65 处):全部 `Result<T, String>` → `Result<T, AppCommandError>`,内部 `?` 配合 `From` 转换(含 `From<anyhow::Error>`)
- 高频 command(chat / cancel / create_session / delete_session / update_provider / merge_worker / discard_worker):**透传前端 invoke 传入的 `requestId`** 填 `request_id` 字段(后端不新生成 uuid)

**验证**:
- `grep -rE "Result<.*, *String>" app/src-tauri/src/commands/` 无结果
- `cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo check` 0 warning
- `cd app/src-tauri && PKG_CONFIG_PATH="..." cargo test --lib` 全绿(原 957 passed 不退化)

**风险**:中等 — 命令签名变化,前端调用点需同步;**必须与 PR-A5-5 同一次发布**(见 §5),消除中间窗口
**回滚**:`git revert`;**不留 `From<AppCommandError> for String` 临时兼容层**(与 PRD Out of Scope "无兼容期" 一致)

### PR-A5-4: 前端 useErrorBus composable

**改动**:
- 新增 `app/src/composables/useErrorBus.ts`:composable + `parseAppCommandError` 工具(含 category 值域校验)+ 5 类 category 路由 stub + `MAX_ERRORS = 50` FIFO 上限
- 新增 `app/src/composables/useErrorBus.test.ts`:vitest 覆盖 parseAppCommandError 3 种输入 + 非法 category 不误判 + 5 类路由 mock + FIFO 上限
- `app/src/types/error.d.ts`(新增):`AppCommandError` + `ErrorCategory` TypeScript 类型

**验证**:
- `cd app && pnpm vitest run useErrorBus` 全绿
- TypeScript 类型导出在 `app/src/types/index.ts` 聚合

**风险**:低,纯新增
**回滚**:`git revert`

### PR-A5-5: 前端 invoke 调用迁移 + 全局错误器

**改动**:
- `app/src/stores/chat.ts` / `streamController.ts` / `permissions.ts` / `subagentRuns.ts`:关键 `.catch(e => ...)` 接入 `useErrorBus().handle(e)`(高频点优先,legacy 散点可后续 follow-up)
- `app/src/components/chat/MessageItemEdit.vue`:`errorMessage` 改走 useErrorBus 路由
- `app/src/main.ts` 或合适位置:挂 `window.addEventListener('error', ...)` + `unhandledrejection` 接入 useErrorBus

**验证**:
- `cd app && pnpm vitest run` 全绿(原有用例不退化)
- `cd app && pnpm build`(`vue-tsc --noEmit` + `vite build`)全绿
- 手动冒烟(可选):Tauri dev 跑通,故意构造 5 类 category 各一次,目视错误总线响应

**风险**:中等 — 涉及前端多处 catch 替换;**必须与 PR-A5-3 同次发布**
**回滚**:`git revert`;前端可单独回滚不影响后端

## 2. 验证命令汇总

```bash
# Rust 全套
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo check
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test

# 前端全套
cd app && pnpm vitest run
cd app && pnpm build      # vue-tsc --noEmit + vite build

# 端到端(可选)
cd app && pnpm tauri dev  # 冒烟 5 类 category 各一次
```

## 3. 风险点 & 回滚策略

| 风险 | 触发条件 | 缓解 | 回滚 |
|---|---|---|---|
| PR-A5-3 命令签名破坏前端 | 前端 invoke 类型不匹配 | **PR-A5-3 与 PR-A5-5 同次发布**;errorBus 容错降级 | revert PR-A5-3 + PR-A5-5(不留 String 兼容层) |
| `From<anyhow::Error>` 漏写 | commands `?anyhow` 编译失败 | PR-A5-2 显式实现(已在 design §3 列入) | N/A(编译期) |
| 10 个 From<E> 漏写某个 variant | 编译时 match 不穷尽 | 编译失败即捕获 | N/A(编译期) |
| `PreFlightError` 缺 Display/Error impl | trait 约束不满足,编译失败 | PR-A5-2 先补两个 impl(design §2/§4) | N/A(编译期) |
| spec drift 复发 | Protocol 命名回到 spec | PR-A5-1 grep 测试 + R5.2 grep 校验落 CI | revert spec 改动 |
| 前端 useErrorBus 单例 ref 在 SSR 失效 | 当前 Tauri 桌面应用,无 SSR | 仅桌面运行,无影响 | N/A |
| `parseAppCommandError` 误判 native string | 老链路 String rejection 被识别为 Server | 容错降级 Server/Unknown,不会崩 | 调整判定逻辑 |
| request_id 字段未填 | 轻量 command 漏 request_id | design 允许 `request_id: Option<String>`,None 表示无关联 | N/A |
| errors 数组长会话无限增长 | Server/Network 风暴 | `MAX_ERRORS = 50` FIFO(design §6) | N/A |

## 4. 关键文件索引(实施时以 `grep` 实时定位为准,行号会漂移)

后端(改动)— 真实行号(2026-07-02 核对):
- `app/src-tauri/src/llm/error.rs:17` — LlmError enum;`:34-46` category()+user_message() 迁移至 trait
- `app/src-tauri/src/git/error.rs:8` — GitError(NotARepo/Io/Git2/Dirty)
- `app/src-tauri/src/db/memories.rs:310` — MemoryInsertError;`:1491` — StatusTransitionError(⚠️ 非 1490)
- `app/src-tauri/src/agent/question_store.rs:318` — QuestionStoreError(手写,`:332` Display,`:341` 空 Error impl)
- `app/src-tauri/src/agent/provider.rs:36` — PreFlightError(无 Display/Error impl,`:58` auth_message/invalid_request_message)
- `app/src-tauri/src/background_shell/mod.rs:207` — BackgroundShellError(NotFound/WrongSession/Spawn/InvalidCwd/Poisoned)
- `app/src-tauri/src/agent/auto_reflect.rs:322` — ReflectError(Llm/NoText/Json/MissingField/Insert)
- `app/src-tauri/src/tools/web_fetch.rs:146` — WebFetchError(HttpStatus 按 code 分流)
- `app/src-tauri/src/llm/provider/mod.rs:197` — ProviderBuildError
- `app/src-tauri/src/commands/*.rs` — 13 个 command 文件

后端(新增):
- `app/src-tauri/src/error.rs` — 新文件,集中定义

前端(改动)— 行号未重新核对,实施时 grep 定位:
- `app/src/stores/chat.ts` — invoke edit_user_message 调用点(grep `edit_user_message`)
- `app/src/stores/streamController.ts` — .catch 散点(grep `\.catch`)
- `app/src/stores/permissions.ts` — respond catch
- `app/src/stores/subagentRuns.ts` — error toast
- `app/src/components/chat/MessageItemEdit.vue` — errorMessage 渲染
- `app/src/main.ts` — 挂全局错误器

前端(新增):
- `app/src/composables/useErrorBus.ts`
- `app/src/composables/useErrorBus.test.ts`
- `app/src/types/error.d.ts`

规范:
- `.trellis/spec/backend/error-handling.md` — 主文档
- `docs/IMPLEMENTATION.md §4` — 新增 A5 ADR

## 5. PR 合并顺序与发布窗口

开发顺序:PR-A5-1 → PR-A5-2 → PR-A5-3 → PR-A5-4 → PR-A5-5

- PR-A5-1(spec)可单独先行(无代码依赖)
- PR-A5-2(trait + From)需在 PR-A5-3 之前(PR-A5-3 依赖 `From<E>` + `From<anyhow::Error>`)
- PR-A5-4(composable)可与 PR-A5-3 并行(无依赖)
- **PR-A5-3 与 PR-A5-5 必须同一次发布**(`--no-ff` 合并到一个发布单元),消除"后端返回 AppCommandError 对象 / 前端仍按 String 解析"的中间不一致窗口

## 6. 完成后 follow-up 候选(本期不做)

- `PermissionDenied` / `Cancelled` / `NotFound` 独立 category
- i18n 多语言 user_message
- 自动重试策略(前端 useErrorBus 集成)
- Telemetry / Metrics 上报(request_id → Sentry/OTel)
- legacy `.catch(console.error)` 全量替换
- toast UI 组件接 reka-ui + 单条 dismiss / TTL
- `ValidationError`(pub(crate))是否纳入 impl AppError

## 7. 完成检查清单(任务 start 前 review)

- [x] `prd.md` 通过 PRD convergence pass(已做 + review 修订)
- [x] `design.md` 完整(已做 + review 修订,variant 表基于真实代码)
- [x] `implement.md` 完整(本文档 + review 修订)
- [x] `implement.jsonl` + `check.jsonl` 含真实 spec/research 条目(review 修订)
- [ ] 用户 review 通过
- [ ] `task.py start` 后进入 Phase 2 Execute
