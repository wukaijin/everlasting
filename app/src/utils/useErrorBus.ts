// useErrorBus — 全局错误总线(A5 错误处理完善,2026-07-02)。
//
// 统一收口 invoke() 的 IPC 错误:parseAppCommandError 把 Tauri rejection
// (可能是 AppCommandError 对象 / JSON 字符串 / 老链路原始 String)容错
// 解析成 AppCommandError,push 进全局 errors 数组,按 category 路由分发。
//
// 为什么用模块级单例 ref(而非 Pinia store):errors 列表小、无需持久化、
// 跨组件共享用模块单例足够(与 useKeyboard 的 window-listener 单例模式
// 同构)。errors 上限 50 FIFO,防长会话 Server/Network 风暴无限增长。
//
// 路由分发(routeByCategory)本期是 stub(showXToast 只 console.warn),
// 具体 toast UI(reka-ui)在 follow-up 任务接 —— 组件可 watch errors 自行
// 渲染,总线只保证错误被收纳 + 分类。
//
// 与后端契约对齐:AppCommandError 的字段名(camelCase)与 Rust
// `app/src-tauri/src/error.rs` 的 `#[serde(rename_all = "camelCase")]` 一致。
// category 用 PascalCase(与 Rust ErrorCategory variant 名一致)。

import { ref, readonly } from "vue";

export type ErrorCategory =
  | "Auth"
  | "RateLimit"
  | "InvalidRequest"
  | "Server"
  | "Network";

const VALID_CATEGORIES: ReadonlySet<ErrorCategory> = new Set([
  "Auth",
  "RateLimit",
  "InvalidRequest",
  "Server",
  "Network",
]);

export interface AppCommandError {
  category: ErrorCategory;
  kind: string;
  message: string;
  retryable: boolean;
  requestId?: string;
}

// 上限 FIFO:防长会话 Server/Network 风暴让 errors 数组无限增长。
// 单条 dismiss / TTL 过期策略推到 toast UI follow-up。
const MAX_ERRORS = 50;
const errors = ref<AppCommandError[]>([]);

export function useErrorBus() {
  const push = (err: AppCommandError) => {
    errors.value.push(err);
    if (errors.value.length > MAX_ERRORS) {
      // 丢最旧,保留最近 MAX_ERRORS 条。
      errors.value.splice(0, errors.value.length - MAX_ERRORS);
    }
    routeByCategory(err);
  };

  /** 入口:把 `invoke().catch(e => useErrorBus().handle(e))` 的未知错误
   *  容错解析后 push。无法识别的错误(返回 null)静默丢弃。 */
  const handle = (e: unknown) => {
    const err = parseAppCommandError(e);
    if (err) push(err);
  };

  const clear = () => {
    errors.value.splice(0, errors.value.length);
  };

  return {
    errors: readonly(errors),
    push,
    handle,
    clear,
  };
}

function isAppCommandError(x: unknown): x is AppCommandError {
  if (typeof x !== "object" || x === null) return false;
  const o = x as Record<string, unknown>;
  return (
    typeof o.category === "string" &&
    // category 值域校验:防恰好含 4 字段的普通对象/JSON 被误判。
    VALID_CATEGORIES.has(o.category as ErrorCategory) &&
    typeof o.kind === "string" &&
    typeof o.message === "string" &&
    typeof o.retryable === "boolean"
  );
}

/**
 * 容错解析 IPC 错误为 AppCommandError。兼容 3 种输入:
 * 1. AppCommandError 对象 —— Tauri 把序列化后的 rejection 传给 `.catch`。
 * 2. JSON 字符串 —— 老链路 / 手动 wrap 的 JSON。
 * 3. 原始 string —— Tauri 原生 String rejection / 老链路残留 → 降级 Server/Unknown。
 * 返回 `null` 表示无法识别(调用方静默,不崩)。
 */
export function parseAppCommandError(e: unknown): AppCommandError | null {
  if (typeof e === "object" && e !== null && isAppCommandError(e)) {
    return e as AppCommandError;
  }
  if (typeof e === "string") {
    try {
      const parsed: unknown = JSON.parse(e);
      if (isAppCommandError(parsed)) return parsed as AppCommandError;
    } catch {
      // 非 JSON,fall through 到 string fallback。
    }
    return {
      category: "Server",
      kind: "Unknown",
      message: e,
      retryable: false,
    };
  }
  return null;
}

/** 从未知错误提取中文消息。前端错误显示的统一入口,兼容:
 * 1. `AppCommandError` 对象(后端 IPC 错误)→ `message`
 * 2. `Error` 实例(本地 JS 错误)→ `e.message`
 * 3. 原始 string → 原样
 * 4. 其他 → "(未知错误)"
 *
 * A5(2026-07-02):后端 command 改返 `AppCommandError` 对象后,直接 `String(e)`
 * 会显示 `[object Object]`;本 helper 让前端所有错误显示点统一兼容结构化错误 +
 * 裸 Error + 裸字符串,替换散落的 `String(e)` / `e instanceof Error ? e.message : String(e)`。 */
export function extractErrorMessage(e: unknown): string {
  const parsed = parseAppCommandError(e);
  if (parsed) return parsed.message;
  if (e instanceof Error) return e.message;
  if (typeof e === "string") return e;
  return "(未知错误)";
}

// 5 类路由分发。本期 stub(showXToast 只 console.warn);toast UI(reka-ui)
// 在 follow-up 任务接,届时组件 watch errors 决定渲染。
function routeByCategory(err: AppCommandError): void {
  switch (err.category) {
    case "Auth":
      showAuthToast(err);
      break;
    case "RateLimit":
      showRateLimitToast(err);
      break;
    case "InvalidRequest":
      showInlineError(err);
      break;
    case "Server":
      showServerToast(err);
      break;
    case "Network":
      showNetworkToast(err);
      break;
  }
}

// TODO(A5 follow-up):接 reka-ui toast。当前 stub 只 console.warn,保证错误
// 不静默(开发可见)+ errors 数组收纳(组件可 watch 渲染)。
function showAuthToast(err: AppCommandError): void {
  // Auth 路由:引导去 Settings 检查 ANTHROPIC_API_KEY。
  console.warn(`[errorBus:Auth] ${err.message}`, err);
}

function showRateLimitToast(err: AppCommandError): void {
  console.warn(`[errorBus:RateLimit] ${err.message}`, err);
}

function showInlineError(err: AppCommandError): void {
  // InvalidRequest 由调用点 watch errors 决定内联渲染(表单字段下),不全局 toast。
  console.warn(`[errorBus:InvalidRequest] ${err.message}`, err);
}

function showServerToast(err: AppCommandError): void {
  // retryable=true 时附"重试"按钮(follow-up)。
  console.warn(`[errorBus:Server] ${err.message}`, err);
}

function showNetworkToast(err: AppCommandError): void {
  console.warn(`[errorBus:Network] ${err.message}`, err);
}
