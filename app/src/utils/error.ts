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

/** categoryRetryable / categoryToastKey 都接受的 category 字符串
 *  形态。两种共存的现状:PascalCase(`Auth` / `RateLimit` / ...)用于
 *  `useErrorBus` 的 `AppCommandError`,snake_case(`auth` / `rate_limit` /
 *  ...)用于 `ChatEvent::Error` / `ChatMessage.error.category`(wire
 *  format)。本文件两个 helper 都覆盖两种 case;新增 category 时记得
 *  两层都加。 */
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
