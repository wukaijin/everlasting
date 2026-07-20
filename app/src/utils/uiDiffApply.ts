// uiDiffApply.ts — Tauri invoke wrapper for the `apply_ui_diff` IPC
// (B9+ D4, 2026-07-13).
//
// Sibling to `toolQuestion.ts` / `toolModeChange.ts`. The IPC is
// user-triggered (NOT an LLM tool); the DiffPrimitive / ButtonPrimitive
// components call `applyUiDiff` when the user clicks the「应用」
// button. The backend does the project boundary check + writes files +
// audits the success — see `commands/ui.rs::apply_ui_diff` for the
// Rust-side contract.
//
// Why a thin wrapper:
//   1. Single source of truth for the command name
//      (`APPLY_UI_DIFF_CMD`). If the Rust command gets renamed, one
//      edit here vs. every call site.
//   2. Field-name discipline — `sessionId` (camelCase JS) ↔
//      `session_id` (snake_case Rust) is handled by Tauri's arg
//      binder; we mirror that explicitly here so the consuming
//      component types stay clean.
//   3. Wire-shape typing for the return value (`ApplyUiDiffResult`)
//      lives in one place; the frontend components import this type
//      rather than redeclaring it.

import { transport } from "../transport";

/** The Tauri command name on the Rust side. Matches
 *  `commands::ui::apply_ui_diff`'s `#[tauri::command]` attribute. */
export const APPLY_UI_DIFF_CMD = "apply_ui_diff";

/** Failure kinds the backend may return. Mirrors
 *  `commands::ui::ApplyUiDiffResult.kind` — `kind ∈ {"boundary",
 *  "parse", "conflict", "io", "empty"}`. The frontend maps these to
 *  inline error messages (see `DiffPrimitive.vue`'s error UI). */
export type ApplyUiDiffFailureKind =
  | "boundary"
  | "parse"
  | "conflict"
  | "io"
  | "empty";

/** One entry in the success `files` array. `path` is the
 * post-canonicalize absolute path the boundary check accepted
 * (the file the backend actually wrote). */
export interface ApplyUiDiffFile {
  path: string;
  added: number;
  removed: number;
}

/** Tagged-union wire shape returned by the backend. Frontend callers
 *  should narrow on `ok` first, then read either `files` (success) or
 *  `kind` + `error` (failure). */
export type ApplyUiDiffResult =
  | { ok: true; files: ApplyUiDiffFile[] }
  | { ok: false; kind: ApplyUiDiffFailureKind; error: string };

/** Apply a unified-diff blob to disk under the session's write
 *  target (worktree, or current_cwd fallback).
 *
 *  `sessionId` is required — the backend uses it to resolve the
 *  session's write root. The frontend `<DiffPrimitive>` /
 *  `<ButtonPrimitive>` components read this from `useChatStore`.
 *
 *  `diffText` is the raw `primitives[].diff_text` value (standard
 *  unified diff format only — the DiffPrimitive apply button is
 *  already disabled for headerless raw-fallback fragments).
 *
 *  On backend error the function REJECTS with an
 *  `ApplyUiDiffError` carrying the `kind` + `error` from the
 *  result. Successful calls resolve with the `files` list.
 *
 *  # Caller UX pattern
 *
 *  ```ts
 *  try {
 *    const res = await applyUiDiff(sid, diffText);
 *    showToast(`已应用 ${res.files.length} 个文件`, "success");
 *  } catch (e) {
 *    if (e instanceof ApplyUiDiffError) {
 *      // Map kind → inline Chinese error text. See
 *      // `DiffPrimitive.vue::applyErrorText`.
 *      cardError.value = e.kind;
 *    }
 *  }
 *  ``` */
export async function applyUiDiff(
  sessionId: string,
  diffText: string,
): Promise<ApplyUiDiffFile[]> {
  const result = await transport.invoke<ApplyUiDiffResult>(APPLY_UI_DIFF_CMD, {
    sessionId,
    diffText,
  });
  if (result.ok) {
    return result.files;
  }
  throw new ApplyUiDiffError(result.kind, result.error);
}

/** Error subclass carrying the backend's `kind` discriminator so
 *  the frontend can map to inline error UI without re-parsing the
 *  message. */
export class ApplyUiDiffError extends Error {
  public readonly kind: ApplyUiDiffFailureKind;
  constructor(kind: ApplyUiDiffFailureKind, message: string) {
    super(message);
    this.name = "ApplyUiDiffError";
    this.kind = kind;
  }
}

/** Localized inline error messages keyed by `kind`. The frontend
 *  renders these directly under the Apply button (or in the card
 *  body when the result is the apply path).
 *
 *  The text mirrors the design §7 "kind 中文文案" table and is the
 *  single source of truth — `DiffPrimitive.vue` imports this and
 *  displays it. */
export const APPLY_UI_DIFF_ERROR_TEXT: Record<ApplyUiDiffFailureKind, string> = {
  boundary: "路径越界 — diff 中包含项目外的文件路径",
  parse: "diff 格式无法应用 — 需要带 ---/+++ 路径头的标准 unified diff",
  conflict: "文件已变 — diff 的上下文行与当前文件不匹配,请重新生成 diff",
  io: "文件读写失败 — 请检查文件权限和磁盘空间",
  empty: "diff 内容为空",
};