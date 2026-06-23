// Audit payload parsing helpers — typed frontend view of the
// `payload_json` column on `session_audit_events` rows.
//
// The backend stores a raw JSON string per row; the schema differs
// per `kind`. This module is the single source of truth for the
// 11-kind → TS-shape mapping. The C4 AuditLogModal renders each
// row by dispatching on `kind` and reading the typed fields, so
// any future kind added on the Rust side needs to land here too.
//
// Failure policy (PRD AC "payload 为 null / malformed 时不崩"):
//   - `raw == null`                → returns `{ kind, raw: null }`
//   - `JSON.parse(raw)` throws     → returns the `{ raw }` fallback
//   - parsed object missing fields → individual getters fall back
//     to a safe default (empty string, null, false)
//
// The helper NEVER throws — the modal's per-kind renderer relies
// on this. Mismatched schema (e.g. an old DB row from a previous
// release) renders as "raw payload" with no extra chrome.

// ---------------------------------------------------------------------------
// AuditEventRow — wire shape, mirrors `db::AuditEventRow` on the
// Rust side (`#[serde(rename_all = "camelCase")]` per the IPC
// convention, see `.trellis/spec/backend/database-guidelines.md`).
// ---------------------------------------------------------------------------

export interface AuditEventRow {
  id: number;
  sessionId: string;
  /** "YYYY-MM-DD HH:MM:SS" — SQLite `datetime('now')` second
   *  precision. The backend sorts `ts DESC`; for tie-breaking
   *  when two rows share the same second we expose `id DESC`
   *  at the store layer. */
  ts: string;
  /** One of the 13 `AuditKind::as_str()` outputs (see
   *  `agent/permissions/mod.rs`,拆分自 mod.rs,2026-06-23 拆为 8 模块,AuditKind + record_* 落 `agent/permissions/audit.rs`). The two new D3 PR1/PR3
   *  kinds — `edit_message` and `resend_message` — are
   *  user-initiated direct IPCs (not ⑨ 关 decisions), so the
   *  parser falls back to `kind: "raw"` for them; the UI
   *  surfaces them via the dropdown filter + the
   *  `labelForKind` map. */
  kind: string;
  /** Raw JSON payload (or `null`). Parse via `parseAuditPayload`. */
  payloadJson: string | null;
}

// ---------------------------------------------------------------------------
// Typed payload shapes — one per `kind` family. The names track the
// Rust payload schema (see `record_audit` / `record_tool_executed_audit`
// / `set_session_mode`'s inline `serde_json::json!({...})`).
// ---------------------------------------------------------------------------

/** Payload for `tool_denied` / `tool_denied_yolo` /
 *  `tool_permission_ask` / `permission_granted` / `permission_timeout`
 *  / `tool_allowed` / `request_cancelled`. All these go through the
 *  `record_audit` helper so they share the same `{ tool_name,
 *  tool_input, reason?, mode, critical }` shape. `reason` is
 *  populated by the Tier 2 / Tier 3 deny path; other writes leave
 *  it `null` and the renderer hides the row. */
export interface ToolAuditPayload {
  tool_name?: string;
  tool_input?: Record<string, unknown> | null;
  reason?: string | null;
  mode?: string | null;
  critical?: boolean;
}

/** Payload for `tool_executed` (PR1 of C4, 2026-06-14). Carries
 *  wall-clock duration + the tool's exit code (the latter only
 *  for `shell`; other tools emit `null`). */
export interface ToolExecutedPayload {
  tool_name?: string;
  tool_input?: Record<string, unknown> | null;
  duration_ms?: number;
  /** `null` = tool has no exit code (read_file / write_file /
   *  edit_file / grep / glob / list_dir / web_fetch). `0` =
   *  success. Non-zero = the tool failed. `-1` = the child was
   *  killed (timeout or cancel). */
  exit_code?: number | null;
}

/** Payload for `mode_changed` / `yolo_entered` / `yolo_exited`.
 *  Written directly by `commands::permissions::set_session_mode`
 *  (NOT through `record_audit`), so the shape is the inline
 *  `{ prev_mode, new_mode }` JSON. */
export interface ModeAuditPayload {
  prev_mode?: string;
  new_mode?: string;
}

/** Union of all parsed payload shapes + a `raw` fallback for
 *  malformed / unknown kinds. The renderer switches on the
 *  parsed shape's kind via the `kind` field on the row, not on
 *  this union's discriminator — the union exists for type-narrowing
 *  inside the per-kind component. */
export type ParsedPayload =
  | { kind: "tool"; payload: ToolAuditPayload }
  | { kind: "tool_executed"; payload: ToolExecutedPayload }
  | { kind: "mode"; payload: ModeAuditPayload }
  | { kind: "raw"; raw: unknown }
  | { kind: "empty" };

// ---------------------------------------------------------------------------
// Grouping helpers — the renderer groups kinds into 4 families
// so it can re-use one Vue template per family. The "tool" group
// covers the 7 kinds that go through `record_audit` (all the
// permission-decision kinds + the cancel path).
// ---------------------------------------------------------------------------

const TOOL_KINDS = new Set<string>([
  "tool_denied",
  "tool_denied_yolo",
  "tool_allowed",
  "tool_permission_ask",
  "permission_granted",
  "permission_timeout",
  "request_cancelled",
]);

const MODE_KINDS = new Set<string>([
  "mode_changed",
  "yolo_entered",
  "yolo_exited",
]);

const TOOL_EXECUTED_KIND = "tool_executed";

/** Parse the raw `payloadJson` for a row into a typed shape.
 *  Never throws — malformed / null / unknown shapes degrade
 *  gracefully to the `{ kind: "raw", raw }` fallback. */
export function parseAuditPayload(
  kind: string,
  raw: string | null,
): ParsedPayload {
  if (raw === null || raw === "") {
    return { kind: "empty" };
  }
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return { kind: "raw", raw };
  }
  if (parsed === null || typeof parsed !== "object" || Array.isArray(parsed)) {
    return { kind: "raw", raw: parsed };
  }
  if (kind === TOOL_EXECUTED_KIND) {
    return { kind: "tool_executed", payload: parsed as ToolExecutedPayload };
  }
  if (MODE_KINDS.has(kind)) {
    return { kind: "mode", payload: parsed as ModeAuditPayload };
  }
  if (TOOL_KINDS.has(kind)) {
    return { kind: "tool", payload: parsed as ToolAuditPayload };
  }
  // Unknown kind (e.g. a future release added a new variant and
  // this frontend predates it). Don't crash — show the raw blob.
  return { kind: "raw", raw: parsed };
}

// ---------------------------------------------------------------------------
// Display helpers — Chinese labels + icon-name mapping for the
// AuditLogModal list. The 11-kind → label map mirrors the Rust
// `AuditKind::as_str()` outputs.
// ---------------------------------------------------------------------------

/** Static map for the kind dropdown filter. `value === null` is
 *  the "全部" option. The list is in the order they typically
 *  appear in a session: allowed/executed first (high-volume),
 *  denied/ask/timeout/cancel mid (the "agent 做了哪些权限决策"),
 *  mode-changed last (rare). */
export const AUDIT_KIND_OPTIONS: ReadonlyArray<{ value: string | null; label: string }> = [
  { value: null, label: "全部" },
  { value: "tool_executed", label: "工具执行" },
  { value: "tool_allowed", label: "放行" },
  { value: "tool_denied", label: "拒绝" },
  { value: "tool_denied_yolo", label: "Yolo 静默拒绝" },
  { value: "tool_permission_ask", label: "弹窗询问" },
  { value: "permission_granted", label: "始终允许" },
  { value: "permission_timeout", label: "询问超时" },
  { value: "request_cancelled", label: "用户取消" },
  { value: "mode_changed", label: "Mode 切换" },
  { value: "yolo_entered", label: "进入 Yolo" },
  { value: "yolo_exited", label: "退出 Yolo" },
  // D3 PR1 (2026-06-17): user-initiated message edit (in-place
  // content update + cascade delete of the assistant tail).
  // Surfaces in the audit log so the user can review the
  // edit history of a session without scrolling back through
  // the message list. Wire string is locked by
  // `AuditKind::EditMessage.as_str()` in the Rust side.
  { value: "edit_message", label: "编辑消息" },
  // D3 PR3 (2026-06-17): user-initiated message resend (re-fire
  // an existing user prompt; no content mutation). Mirrors
  // `edit_message` in the audit trail so the user can
  // distinguish "you edited this prompt" from "you re-ran this
  // prompt" when reviewing a session's history. Wire string is
  // locked by `AuditKind::ResendMessage.as_str()`.
  { value: "resend_message", label: "重新发送" },
];

/** Visual family for the list row's leading icon. The renderer
 *  uses this to pick icon name + color token. */
export type AuditIconFamily =
  | "denied"
  | "denied-yolo"
  | "allowed"
  | "granted"
  | "ask"
  | "timeout"
  | "cancelled"
  | "executed"
  | "mode"
  | "message-edit"
  | "message-resend"
  | "unknown";

/** Map a wire `kind` to the icon family the modal renders.
 *  The two D3 PR1/PR3 user-action kinds (`edit_message` /
 *  `resend_message`) get their own families so the icon reads
 *  "user-initiated edit/resend" instead of falling into the
 *  generic "unknown / gray" bucket. Colors reuse the existing
 *  tool color tokens (no new tokens added per design-tokens.md). */
export function iconFamilyForKind(kind: string): AuditIconFamily {
  switch (kind) {
    case "tool_denied":
      return "denied";
    case "tool_denied_yolo":
      return "denied-yolo";
    case "tool_allowed":
      return "allowed";
    case "permission_granted":
      return "granted";
    case "tool_permission_ask":
      return "ask";
    case "permission_timeout":
      return "timeout";
    case "request_cancelled":
      return "cancelled";
    case "tool_executed":
      return "executed";
    case "mode_changed":
    case "yolo_entered":
    case "yolo_exited":
      return "mode";
    // D3 PR1 (2026-06-17): user-initiated message edit.
    case "edit_message":
      return "message-edit";
    // D3 PR3 (2026-06-17): user-initiated message resend.
    case "resend_message":
      return "message-resend";
    default:
      return "unknown";
  }
}

/** Chinese label for a single `kind` (used by the list row's
 *  kind chip). Mirrors `AUDIT_KIND_OPTIONS` minus the leading
 *  "全部" pseudo-option. */
export function labelForKind(kind: string): string {
  const opt = AUDIT_KIND_OPTIONS.find((o) => o.value === kind);
  return opt ? opt.label : kind;
}

// ---------------------------------------------------------------------------
// Small formatting helpers — used by the AuditLogItem renderer.
// Centralized so a refactor to i18n later has one chokepoint.
// ---------------------------------------------------------------------------

/** Format the SQLite `ts` ("YYYY-MM-DD HH:MM:SS") as `HH:MM:SS`.
 *  Defensive: on a malformed `ts` (e.g. NULL-ish empty string),
 *  returns the input verbatim. */
export function formatTimeOfDay(ts: string): string {
  // SQLite `datetime('now')` is "YYYY-MM-DD HH:MM:SS" (24h, UTC).
  // We only display the time portion — the date is implicit (it's
  // always "today" or "this session").
  const idx = ts.indexOf(" ");
  if (idx < 0) return ts;
  const time = ts.slice(idx + 1);
  return time.length === 8 ? time : ts;
}

/** Format `duration_ms` as a short human string.
 *  - < 1000ms   → "123ms"
 *  - < 60_000ms → "1.2s"
 *  - >= 60_000  → "1m 23s"
 *  Defensive: missing / negative / non-finite → "". */
export function formatDuration(ms: number | undefined): string {
  if (typeof ms !== "number" || !Number.isFinite(ms) || ms < 0) {
    return "";
  }
  if (ms < 1000) return `${Math.round(ms)}ms`;
  if (ms < 60_000) {
    const s = ms / 1000;
    return `${s.toFixed(s < 10 ? 1 : 0)}s`;
  }
  const totalSec = Math.round(ms / 1000);
  const m = Math.floor(totalSec / 60);
  const s = totalSec % 60;
  return `${m}m ${s}s`;
}

/** Shorten a `tool_input` object to a single-line preview. We
 *  pick the most informative field per tool name; for unknown
 *  tools we fall back to `JSON.stringify` (truncated). The
 *  goal is a 1-line chip, not a full preview — the modal's
 *  reason / extra row carries the longer context. */
export function summarizeToolInput(
  toolName: string | undefined,
  input: Record<string, unknown> | null | undefined,
): string {
  if (!input || typeof input !== "object") return "";
  const cmd = typeof input.command === "string" ? input.command : "";
  const path = typeof input.path === "string" ? input.path : "";
  const url = typeof input.url === "string" ? input.url : "";
  const pattern = typeof input.pattern === "string" ? input.pattern : "";

  if (toolName === "shell" && cmd) return cmd;
  if (toolName === "web_fetch" && url) return url;
  if ((toolName === "grep" || toolName === "glob") && pattern) {
    return path ? `${pattern} @ ${path}` : pattern;
  }
  if (path) return path;
  if (cmd) return cmd;
  if (url) return url;
  if (pattern) return pattern;

  // Last-resort: JSON.stringify truncated to 80 chars.
  try {
    const s = JSON.stringify(input);
    return s.length > 80 ? `${s.slice(0, 77)}...` : s;
  } catch {
    return "";
  }
}
