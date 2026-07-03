# A5 错误处理完善 — Design

> **2026-07-02 review 修订**:本版基于对 `app/src-tauri/src/` 真实代码的逐 variant 核对,修正了初版 §5 映射表的虚构 variant、§4 不存在的 Timeout override、以及"11 个 thiserror"的计数错误。所有 variant 名以代码为准。

## 1. Architecture Overview

```
┌──────────────────────────────────────────────────────────┐
│                Frontend (Vue 3 + Pinia)                  │
│                                                          │
│   invoke() ─→ .catch(e => useErrorBus().handle(e))       │
│                    │                                     │
│                    ↓                                     │
│              parseAppCommandError(e)                     │
│                    │                                     │
│                    ↓                                     │
│           useErrorBus (composable)                       │
│                    │                                     │
│                    ↓ category 路由                       │
│   ┌─────────┬─────────┬─────────┬─────────┬─────────┐   │
│   │  Auth   │RateLimit│Invalid  │ Server  │Network  │   │
│   │→Settings│→  toast │→inline  │→toast   │→toast   │   │
│   │ 引导    │ 重试提示 │ 错误展示 │+重试按钮 │ 网络提示│   │
│   └─────────┴─────────┴─────────┴─────────┴─────────┘   │
│                                                          │
│   window.onerror / unhandledrejection ─→ errorBus        │
└────────────────────┬───────────────────────────────────────┘
                       │ Tauri IPC (typed)
                       ↓
┌──────────────────────────────────────────────────────────┐
│              Backend (Rust + Tauri 2)                    │
│                                                          │
│   commands/*.rs                                          │
│   async fn cmd() -> Result<T, AppCommandError>          │
│                    │                                     │
│                    ↓                                     │
│   AppCommandError {                                      │
│     category: ErrorCategory,                             │
│     kind: String,                                        │
│     message: String,                                     │
│     retryable: bool,                                     │
│     request_id: Option<String>,                          │
│   }                                                      │
│                    │                                     │
│                    ↓ From<E> 转换                        │
│   impl From<LlmError>        for AppCommandError         │
│   impl From<GitError>        for AppCommandError         │
│   impl From<anyhow::Error>   for AppCommandError  ← 边界 │
│   ... (10 个领域类型 + anyhow)                           │
│                                                          │
│   trait AppError {                                       │
│     fn category(&self) -> ErrorCategory;                 │
│     fn user_message(&self) -> String;                    │
│     fn retryable(&self) -> bool { default }              │
│   }                                                      │
│   impl AppError for LlmError   { ... }                   │
│   impl AppError for GitError   { ... }                   │
│   ... (10 个 impl)                                       │
└──────────────────────────────────────────────────────────┘
```

边界:
- 后端 `error.rs`(新文件,模块根 `app/src-tauri/src/error.rs`)集中定义 `AppError` trait + `ErrorCategory` enum + `AppCommandError` struct + 10 个领域 `From<E>` + 1 个 `From<anyhow::Error>`;不分散到各错误类型文件,避免后续添加新错误类型时漏改
- 前端 `useErrorBus` 放在 `app/src/composables/`,与现有 `useKeyboard` / `chatInputCodeMirror` 同级
- spec 文档 `.trellis/spec/backend/error-handling.md` 是规范 source of truth;RULE-A-* 索引表集中收纳

## 2. Module Layout

### 新增

| 文件 | 职责 |
|---|---|
| `app/src-tauri/src/error.rs` | `AppError` trait + `ErrorCategory` enum + `AppCommandError` struct + 10 个领域 `From<E>` impl + `From<anyhow::Error>` impl |
| `app/src/composables/useErrorBus.ts` | composable + `parseAppCommandError` 工具 |
| `app/src/composables/useErrorBus.test.ts` | vitest:容错 + 路由 |

### 修改

| 文件 | 改动 |
|---|---|
| `app/src-tauri/src/llm/error.rs` | `LlmError` `impl AppError`(已具备 category + user_message,补 retryable 默认;现有 `LlmErrorCategory` enum 与 `ErrorCategory` 做映射) |
| `app/src-tauri/src/git/error.rs` | `GitError` `impl AppError` |
| `app/src-tauri/src/db/memories.rs` | `MemoryInsertError`, `StatusTransitionError` `impl AppError` |
| `app/src-tauri/src/agent/question_store.rs` | `QuestionStoreError` `impl AppError`(已有手写 `Display` + 空 `impl Error`,可直接 impl) |
| `app/src-tauri/src/agent/provider.rs` | `PreFlightError` **先补 `impl fmt::Display` + `impl std::error::Error`(当前两者皆无)`,再** `impl AppError`;现有 `auth_message()`/`invalid_request_message()` 收敛进 `user_message()` |
| `app/src-tauri/src/background_shell/mod.rs` | `BackgroundShellError` `impl AppError`(注意 `#[allow(dead_code)]` reserved variants 仍需在 match 中穷尽) |
| `app/src-tauri/src/agent/auto_reflect.rs` | `ReflectError` `impl AppError` |
| `app/src-tauri/src/tools/web_fetch.rs` | `WebFetchError` `impl AppError`(`HttpStatus(u16)` 在 category() 内按 4xx/5xx 分流) |
| `app/src-tauri/src/llm/provider/mod.rs` | `ProviderBuildError` `impl AppError` |
| `app/src-tauri/src/lib.rs` | `mod error;` + 注册 `error::AppCommandError` 类型导出 |
| `app/src-tauri/src/commands/*.rs` | 全部 `Result<T, String>` → `Result<T, AppCommandError>`(13 个文件 ~65 处;`commands/mod.rs` 仅 re-export 无 fn,不动) |
| `app/src/stores/chat.ts`, `streamController.ts`, `permissions.ts`, `subagentRuns.ts` | 关键 `.catch` 接入 `useErrorBus` |
| `app/src/components/chat/MessageItemEdit.vue` | `errorMessage` 改走 `useErrorBus` 路由 |
| `.trellis/spec/backend/error-handling.md` | 填 Overview / Error Types / Handling Patterns / API Error Responses 五章;修正 spec drift;加 RULES 索引表 |
| `docs/IMPLEMENTATION.md` §4 | 新增 A5 ADR 条目 |

## 3. Wire Shape: AppCommandError

```rust
// app/src-tauri/src/error.rs
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppCommandError {
    /// 5 类 category 之一,前端按其路由
    pub category: ErrorCategory,
    /// 错误类型短名,如 "LlmError::Auth" / "GitError::NotARepo" — 用于诊断与日志关联
    pub kind: String,
    /// 中文友好消息,直接展示给用户
    pub message: String,
    /// 是否可重试(由 category 派生默认,个别错误可 override)
    pub retryable: bool,
    /// 请求 ID,与后端 tracing log 关联(轻量 command 可为 None)
    pub request_id: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub enum ErrorCategory {
    Auth,           // API key 缺失/失效 / 401/403 / 解密失败
    RateLimit,      // 429
    InvalidRequest, // 4xx(非 401/403/429)/ 数据错 / 状态错 / 配置错 / SSRF / NotFound
    Server,         // 5xx / DB 错 / 内部错误 / libgit 错 / io 错
    Network,        // 连接超时 / DNS 错 / TLS 错 / 中断
}
```

JSON 示例(IPC payload):

```json
{
  "category": "RateLimit",
  "kind": "LlmError::RateLimit",
  "message": "请求过于频繁,请稍后再试",
  "retryable": true,
  "requestId": "mz8s3hqwx6rmqjswgte"
}
```

### From 实现清单(error.rs 集中)

- 10 个领域类型:`From<LlmError>` `From<GitError>` `From<MemoryInsertError>` `From<StatusTransitionError>` `From<QuestionStoreError>` `From<PreFlightError>` `From<BackgroundShellError>` `From<ReflectError>` `From<WebFetchError>` `From<ProviderBuildError>` —— 每个 `From` 内部调 `AppError::category()` / `user_message()` / `retryable()` 填字段。
- **`From<anyhow::Error>`(边界兜底)**:`commands/` 大量 `?` 把 `anyhow::Error` 往上冒(项目边界错误惯例),必须提供此转换,否则 PR-A5-3 编译失败。实现:先尝试 `e.downcast_ref::<X>()` 链匹配已知领域类型(命中则复用其 AppError),未命中则归 `{ category: Server, kind: "Anyhow", message: e.to_string(), retryable: true }`。
- 注:`From<E: AppError>` 泛型 blanket impl 不可行(`AppError` 的 impl 分散在各类型文件,coherence 冲突),故 10 个手写。

## 4. AppError Trait

```rust
pub trait AppError: std::error::Error {
    fn category(&self) -> ErrorCategory;
    fn user_message(&self) -> String;
    fn retryable(&self) -> bool {
        // 默认派生规则:Server / Network / RateLimit 可重试
        matches!(self.category(), ErrorCategory::Server | ErrorCategory::Network | ErrorCategory::RateLimit)
    }
}
```

设计意图:
- `retryable` 默认按 category 派生,所有 variant 均无 override 即可正确(**本期无任何 override 案例**;review 修订删除了初版基于不存在的 `BackgroundShellError::Timeout` 的 override 设计)。如未来出现"category 应重试但该具体 variant 不应"的真实 case,再在对应 impl 里覆写 `retryable()`。
- 超类型约束 `std::error::Error`:`QuestionStoreError` 已有空 `impl Error {}` + 手写 `Display`,直接满足;**`PreFlightError` 当前无 `Display` 也无 `Error` impl,PR-A5-2 必须先补齐这两个 impl** 才能满足约束。
- LlmError 现有 `category()` / `user_message()` 直接迁移至 trait,签名兼容(返回类型从 `LlmErrorCategory` 改为 `ErrorCategory`,在 impl 内做 1:1 映射)。
- 三个类型现有对外接口形态不同:LlmError 已有 `category()+user_message()`、PreFlightError 是两个分方法 `auth_message()`/`invalid_request_message()`、QuestionStoreError 是手写 Display 文案。PR-A5-2 逐个收敛,非"一刀切迁移"。

## 5. 错误类型 → Category 映射清单(基于真实代码,10 个对外类型)

> 排除:`tools/ask_user_question.rs:175` 的 `ValidationError` 是 `pub(crate)` tool 内部错误(5 个 variant 全是 input 校验),不冒泡到 IPC 边界,**本期不纳入 impl AppError**(假设:内部错误由调用方 tool 自行转成 `AppCommandError` 或 LlmError 路径消化)。

| 类型 :: variant | 真实代码 variant | → category | retryable | 备注 |
|---|---|---|---|---|
| `LlmError::Auth` | `Auth(String)` | Auth | false | 已有 user_message |
| `LlmError::RateLimit` | `RateLimit(String)` | RateLimit | true | |
| `LlmError::InvalidRequest` | `InvalidRequest(String)` | InvalidRequest | false | spec drift 修正点(原 spec 称 Protocol) |
| `LlmError::Server` | `Server { status, message }` | Server | true | |
| `LlmError::Network` | `Network(String)` | Network | true | |
| `GitError::NotARepo` | `NotARepo { path }` | InvalidRequest | false | 项目非 git 仓库 |
| `GitError::Io` | `Io { path, source }` | Server | true | 系统调用,可重试 |
| `GitError::Git2` | `Git2(#[from] git2::Error)` | Server | true | libgit2 内部错 |
| `GitError::Dirty` | `Dirty { path, paths }` | InvalidRequest | false | 用户需先提交/丢弃 |
| `MemoryInsertError::*`(9 个校验) | TitleEmpty/ContentEmpty/TitleTooLong/ContentTooLong/SensitiveContent/SensitivePath/TemporaryPath/ProjectScopeMissingProjectId/UserScopeHasProjectId | InvalidRequest | false | input/数据/安全校验 |
| `MemoryInsertError::Db` | `Db(#[from] sqlx::Error)` | Server | true | DB 错 |
| `StatusTransitionError::NotFound` | `NotFound(String)` | InvalidRequest | false | |
| `StatusTransitionError::Illegal` | `Illegal { from, to }` | InvalidRequest | false | 状态机非法转移 |
| `StatusTransitionError::Db` | `Db(#[from] sqlx::Error)` | Server | true | ⚠️ 初版表漏归,真实有 Db,归 Server |
| `QuestionStoreError::AlreadyPending` | `AlreadyPending` | InvalidRequest | false | 手写 enum |
| `QuestionStoreError::NotFound` | `NotFound` | InvalidRequest | false | |
| `PreFlightError::NoModel` | `NoModel` | InvalidRequest | false | 引导去 Settings 选 default model |
| `PreFlightError::ProviderMissing` | `ProviderMissing` | InvalidRequest | false | provider 已删 |
| `PreFlightError::EmptyApiKey` | `EmptyApiKey { provider_display_name }` | **Auth** | false | 引导填 key,与 Auth 路由("→Settings 检查 API key")一致 |
| `PreFlightError::DecryptFailed` | `DecryptFailed { provider_display_name }` | **Auth** | false | RULE-D-001,引导重新粘贴 key |
| `PreFlightError::BuildFailed` | `BuildFailed(ProviderBuildError)` | InvalidRequest | false | 透传 ProviderBuildError |
| `BackgroundShellError::NotFound` | `NotFound { .. }` | InvalidRequest | false | |
| `BackgroundShellError::WrongSession` | `WrongSession { .. }` | InvalidRequest | false | session 隔离违例 |
| `BackgroundShellError::Spawn` | `Spawn(#[source] io::Error)` | Server | true | 系统调用 |
| `BackgroundShellError::InvalidCwd` | `InvalidCwd { path, reason }` | InvalidRequest | false | 配置错 |
| `BackgroundShellError::Poisoned` | `Poisoned(String)` | Server | true | registry 锁污染 |
| `ReflectError::Llm` | `Llm(String)` | Server | true | 反思 LLM 失败 |
| `ReflectError::NoText` | `NoText` | InvalidRequest | false | LLM 返回空 |
| `ReflectError::Json` | `Json(#[from] serde_json::Error)` | InvalidRequest | false | 解析错 |
| `ReflectError::MissingField` | `MissingField(&'static str)` | InvalidRequest | false | |
| `ReflectError::Insert` | `Insert(String)` | InvalidRequest | false | P1 reject(默认校验类) |
| `WebFetchError::InvalidUrl` | `InvalidUrl(String)` | InvalidRequest | false | |
| `WebFetchError::BlockedAddress` | `BlockedAddress(IpAddr)` | InvalidRequest | false | SSRF 拦 |
| `WebFetchError::BlockedRedirect` | `BlockedRedirect { .. }` | InvalidRequest | false | SSRF 拦 |
| `WebFetchError::BodyTooLarge` | `BodyTooLarge` | InvalidRequest | false | 5 MiB cap |
| `WebFetchError::HttpStatus` | `HttpStatus(u16)` | **按 code 分流** | 见下 | category() 内 `if code < 500 { InvalidRequest, false } else { Server, true }` |
| `WebFetchError::Timeout` | `Timeout(u64)` | Network | true | |
| `WebFetchError::Tls` | `Tls(String)` | Network | true | TLS 握手失败 |
| `WebFetchError::Network` | `Network(String)` | Network | true | |
| `ProviderBuildError::NotImplemented` | `NotImplemented(&'static str)` | InvalidRequest | false | |
| `ProviderBuildError::UnknownProtocol` | `UnknownProtocol(String)` | InvalidRequest | false | |

**总计:10 个对外错误类型 → ~41 个 variant → 全部归并到 5 类 category。**

两个需要在 `category()` 内部做条件分流的 variant(非一对一):
- `WebFetchError::HttpStatus(u16)`:4xx → InvalidRequest(retryable=false),5xx → Server(retryable=true)。
- `PreFlightError`:variant 级分流(EmptyApiKey/DecryptFailed → Auth,其余 → InvalidRequest)。

## 6. 前端 useErrorBus

```typescript
// app/src/composables/useErrorBus.ts
import { ref, readonly } from 'vue';

export type ErrorCategory = 'Auth' | 'RateLimit' | 'InvalidRequest' | 'Server' | 'Network';

const VALID_CATEGORIES: ReadonlySet<ErrorCategory> = new Set([
  'Auth', 'RateLimit', 'InvalidRequest', 'Server', 'Network',
]);

export interface AppCommandError {
  category: ErrorCategory;
  kind: string;
  message: string;
  retryable: boolean;
  requestId?: string;
}

const MAX_ERRORS = 50; // 上限 FIFO(假设:长会话防无限增长;单条 dismiss 留给 toast UI follow-up)
const errors = ref<AppCommandError[]>([]);

export function useErrorBus() {
  const push = (err: AppCommandError) => {
    errors.value.push(err);
    if (errors.value.length > MAX_ERRORS) {
      errors.value.splice(0, errors.value.length - MAX_ERRORS); // 丢最旧
    }
    routeByCategory(err);
  };

  const handle = (e: unknown) => {
    const err = parseAppCommandError(e);
    if (err) push(err);
  };

  const clear = () => { errors.value = []; };

  return { errors: readonly(errors), push, handle, clear };
}

function isAppCommandError(x: unknown): x is AppCommandError {
  if (typeof x !== 'object' || x === null) return false;
  const o = x as Record<string, unknown>;
  return typeof o.category === 'string' && VALID_CATEGORIES.has(o.category as ErrorCategory) // 值域校验,防误判
    && typeof o.kind === 'string'
    && typeof o.message === 'string'
    && typeof o.retryable === 'boolean';
}

export function parseAppCommandError(e: unknown): AppCommandError | null {
  if (typeof e === 'object' && e !== null && isAppCommandError(e)) {
    return e as AppCommandError;
  }
  if (typeof e === 'string') {
    try {
      const parsed = JSON.parse(e);
      if (isAppCommandError(parsed)) return parsed;
    } catch {
      // fall through to string fallback
    }
    // 容错:老链路 String rejection / Tauri 拒绝原生消息
    return { category: 'Server', kind: 'Unknown', message: e, retryable: false };
  }
  return null;
}

function routeByCategory(err: AppCommandError) {
  switch (err.category) {
    case 'Auth':           showAuthToast(err); break;
    case 'RateLimit':      showRateLimitToast(err); break;
    case 'InvalidRequest': showInlineError(err); break; // 由调用点 watch errors 决定
    case 'Server':         showServerToast(err); break;
    case 'Network':        showNetworkToast(err); break;
  }
}
```

设计意图:
- 全局单例 `errors` ref,跨组件共享(类似 `useEventBus` 模式)
- **`MAX_ERRORS = 50` FIFO 上限**(假设默认;review 修订补的 lifecycle):长会话 Server/Network 风暴不会让数组无限增长。单条 dismiss / TTL 过期策略推到 toast UI follow-up。
- `isAppCommandError` 加 `category` 值域校验(review 修订):防止恰好含 4 字段的普通 IPC 返回/JSON 被误判。
- `parseAppCommandError` 兼容 3 种输入:对象、JSON 字符串、原始字符串 — 容错老链路残留与 Tauri 原生 rejection
- 路由分发在 composable 内 stub,具体 toast UI 后续接 reka-ui(本期不绑定具体 toast 组件,只暴露 errors 列表)

## 7. Compatibility & Migration

### IPC 契约迁移(一次性全切,A5-3 / A5-5 同次发布)

- **一次性全切,无 `String` 兼容期**:所有 `Result<T, String>` → `Result<T, AppCommandError>`;前端 `invoke()` 调用同步改类型。
- **PR-A5-3(后端签名)与 PR-A5-5(前端 catch)必须在同一次发布内合并**,消除"后端返回对象 / 前端仍按 String 解析"的中间不一致窗口。开发可分 PR,但发布原子。
- ~~老链路临时 `From<AppCommandError> for String` 兼容层~~ —— **删除**(review 修订:初版 implement.md 此句与"无兼容期"自相矛盾)。回滚走 `git revert`,不留兼容层。
- 老链路容错仅在前端:`parseAppCommandError` 接受原生 String rejection,降级为 `Server/Unknown`,保证开发期迁移漏点不崩。

### Spec 修正

- `error-handling.md` 中 `LlmError::Protocol` → `LlmError::InvalidRequest`。残留位置:`line 49`(规格正文 `- Protocol — 4xx...`)与 `line 54`(`The Protocol kind covers...`)是**当前规格正文,非历史叙述**,需改写;`line 205` 已是 InvalidRequest(无需动)。AC2 grep 仅允许"历史对比段落"保留 Protocol。
- 新增 `RULES` 索引表,集中 RULE-A-001/002/003/004/006/007/010/012/013/014/015/016/017。

### Frontend 迁移分批(开发批,非发布批)

- 第一批:chat / cancel / sessions / projects / merge_worker / discard_worker 等高频 command 的 `.catch`
- 第二批:streamController / permissions / subagentRuns 等 store 内部 catch
- 第三批(可选):legacy 散点(可后续 follow-up,errorBus 容错保证不崩)

## 8. Operational & Rollback

- 整个 A5 单一 git revert unit,5 章 spec + 1 个新增 Rust 模块 + N 个命令改签名 + 1 个 composable + 5+ 个 store catch 改造
- 锚点测试:10 个错误类型的 category/user_message 单测 + grep-style "Result<T, String>" 残留检测 + parseAppCommandError 容错单测
- 若 hotfix:优先保留 `error-handling.md` 文档(无代码耦合),代码按 PR 切片回滚;**不留 String 兼容层**(与 §7 一致)
- Spec drift 防护:在 `error-handling.md §Common Mistakes` 新增 "spec drift" 条目 + R5.2 grep 校验落 CI

### request_id 语义(假设默认,可推翻)

- **高频 command(chat / cancel / sessions / projects / merge_worker / discard_worker):透传前端 invoke 时传入的 `requestId`**(复用 chat 路径既有 cancel 配对 id),保证 tracing 关联。
- 轻量 command(config 读 / 健康检查 / 无 requestId 入参):`request_id: None`。
- **后端不新生成 uuid**(假设:新生成会与前端的 requestId 脱钩,tracing 对不上;若某 command 需后端独立追踪,另开 follow-up 讨论)。

## 9. Trade-offs

| 取舍 | 决策 | 理由 |
|---|---|---|
| AppCommandError 是否带 stacktrace | **不带** | IPC 体积 + 用户消息无 stack;stacktrace 留 tracing 日志,request_id 关联 |
| retryable 默认派生 vs 每个 override | **默认派生,本期零 override** | 简化 10 个 impl;review 确认无真实 variant 需 override(初版 Timeout 是虚构 variant) |
| 10 个 From<E> 手写 vs blanket impl | **手写** | AppError impl 分散各类型文件,blanket impl 触发 coherence 冲突 |
| 边界 anyhow::Error 是否提供 From | **提供** | commands 大量 `?anyhow`,无此转换 PR-A5-3 编译失败 |
| 前端 composable vs Pinia store | **composable** | errors 列表小,无需响应式持久化;composable 单例 ref 够用 |
| 前端 toast UI 本期是否接 reka-ui | **不接** | 暴露 errors 列表即可;toast UI 在 follow-up 任务做 |
| 一次性全切 vs 双协议期 | **全切,A5-3/A5-5 同次发布** | 用户决策;errorBus 容错降低回归风险;不留 String 兼容层 |
| 错误分级粒度 5 类 vs 8 类 | **5 类** | 用户决策;LlmError 5 类复用;后续可扩 |
| `ValidationError`(pub(crate))是否纳入 | **不纳入** | tool 内部错误不冒泡 IPC 边界;调用方自行消化 |
| errors lifecycle | **上限 50 FIFO** | 防长会话无限增长;单条 dismiss/TTL 留 follow-up |
