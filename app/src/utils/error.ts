// error.ts — Error category derived helpers (A5 R2 / scope B,
// 2026-07-17)。
//
// 两个 helper,R1 + R2 都消费:
//   - `categoryRetryable(category)` 与后端 `AppError::retryable()`
//     默认派生保持一致(rust `error.rs:55-66`),决定 R2 retry 按钮显隐。
//   - `categoryToastKey(category)` 把两类形态(PascalCase from
//     `useErrorBus.AppCommandError` / snake_case from
//     `message.error.category`)统一映射到 4 类 toast key。
//
// 字段名约定:`useErrorBus` 内部用 PascalCase(对接 Rust `ErrorCategory`
// variant 名),`message.error.category` 用 snake_case(对接 Rust 的
// `#[serde(rename_all = "snake_case")]` wire format)。两个 helper
// 都容忍两种输入;新增 category 时两层 case 都加。
//
// 09-21-error-bus-category-recovery:新增 `AppErrorCategory` ——
// PascalCase 五值 union,作为 category **字段类型**的单一事实源:
// `transport/http.ts` 的 `TransportError.category` 与 `useErrorBus`
// 的 `AppCommandError.category` 只从此导入,不得本地平行定义
// (评审裁决:防三处定义漂移)。下方双 case 的 `ErrorCategory`
// 保留为两个 helper 的入参容忍类型(snake_case 脏值须能类型安全地
// 传进来、但过不了值域形状门)。

/** PascalCase 五值 category union —— category 字段类型的单一事实源
 *  (09-21-error-bus-category-recovery 评审裁决)。与 Rust
 *  `error.rs::ErrorCategory` 的 `#[serde(rename_all = "PascalCase")]`
 *  variant 名 1:1(IPC `AppCommandError` wire 形态;daemon HTTP
 *  body 的 `category` 字段同此)。消费方:
 *  - `useErrorBus.AppCommandError.category`(`ErrorCategory` 别名)
 *  - `transport/http.ts.TransportError.category`
 *  两处只导入、不本地定义。新增 category 时此处 + 两个 helper 的
 *  case + Rust 侧枚举四处同步。 */
export type AppErrorCategory =
  | "Auth"
  | "RateLimit"
  | "InvalidRequest"
  | "Server"
  | "Network";

/** categoryRetryable / categoryToastKey 都接受的 category 字符串
 *  形态。两种共存的现状:PascalCase(`Auth` / `RateLimit` / ...)用于
 *  `useErrorBus` 的 `AppCommandError`,snake_case(`auth` / `rate_limit` /
 *  ...)用于 `ChatEvent::Error` / `ChatMessage.error.category`(wire
 *  format)。本文件两个 helper 都覆盖两种 case;新增 category 时记得
 *  两层都加。
 *
 *  09-21 起降级为 helper 入参容忍类型:字段类型一律用上面的
 *  `AppErrorCategory`,本 union 只保证 snake_case 输入能类型安全地
 *  传进 helper(脏值由值域形状门拦截)。 */
export type ErrorCategory =
  | "auth"
  | "Auth"
  | "rate_limit"
  | "RateLimit"
  | "invalid_request"
  | "InvalidRequest"
  | "server"
  | "Server"
  | "network"
  | "Network";

/** Map an error `category` to whether the UI should surface a
 *  retry affordance. Mirrors the backend's `AppError::retryable()`
 *  default impl (`app/src-tauri/src/error.rs:55-66`):
 *    RateLimit / Server / Network → true
 *    Auth / InvalidRequest        → false
 *
 *  Two reasons to keep this in lock-step with the backend default:
 *  1. Wire field consistency — `ChatEvent::Error` deliberately
 *     OMITS a `retryable` field (research/02 confirms). The
 *     frontend derives it. Any drift between this fn and the
 *     Rust default breaks the contract silently.
 *  2. No double-source — if a future override is added in the
 *     backend (e.g. variant-specific), this default MUST become
 *     a wire field, NOT an extended frontend derivation. The
 *     derivation logic is intentionally simple (~5 LOC) so any
 *     drift stands out in review.
 *
 *  Accepts both PascalCase (`useErrorBus`) and snake_case
 *  (`message.error.category`) inputs — see the file-level
 *  comment for why both forms exist in this codebase. */
export function categoryRetryable(category: string | undefined | null): boolean {
  if (!category) return false;
  switch (category) {
    case "rate_limit":
    case "RateLimit":
    case "server":
    case "Server":
    case "network":
    case "Network":
      return true;
    case "auth":
    case "Auth":
    case "invalid_request":
    case "InvalidRequest":
    default:
      return false;
  }
}

/** Route a category (any case) through our 4-toast palette.
 *  Returns the category ONE-OF-`Auth`/`RateLimit`/`Server`/`Network`
 *  if it maps to a toast; otherwise `null` for the no-toast case
 *  (`InvalidRequest`, unknown).
 *
 *  Used by:
 *  - `useErrorBus.routeByCategory` (PascalCase input from
 *    `AppCommandError.category`)
 *  - Future `ChatEvent::Error` wiring (snake_case input from
 *    `event.category` / `message.error.category`)
 *
 *  Both PascalCase and snake_case map to the same 4-key output.
 *  `InvalidRequest` (either case) returns `null` — no global toast,
 *  the error stays in devtools console + the in-message footer. */
export function categoryToastKey(
  category: string | undefined | null,
): "Auth" | "RateLimit" | "Server" | "Network" | null {
  switch (category) {
    case "Auth":
    case "auth":
      return "Auth";
    case "RateLimit":
    case "rate_limit":
      return "RateLimit";
    case "Server":
    case "server":
      return "Server";
    case "Network":
    case "network":
      return "Network";
    case "InvalidRequest":
    case "invalid_request":
    default:
      return null;
  }
}
