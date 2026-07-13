// uiCard.types.ts — public types for the B9 generative UI `use_ui`
// tool (Child A of 07-02-b9-generative-ui, 2026-07-02; B9+ D3 added
// `button` on 2026-07-13).
//
// `use_ui` is the non-blocking display tool that carries generative-
// UI primitives. The frontend reads `call.input.primitives` directly
// (no separate IPC event — the data lives in the tool_use input; see
// parent prd D2). This module mirrors the Rust `tools::use_ui` schema
// (snake_case verbatim, per the project's snake_case wire convention
// — BACKLOG §5.2).

/** Tool name (mirrors Rust `use_ui::definition().name`).
 *  MessageItem keys on this constant to route `use_ui` tool_use
 *  blocks to `<UiCard>` (sibling to `<ToolCallCard>`), the same
 *  dispatch pattern `ASK_USER_QUESTION_TOOL_NAME` uses for
 *  `<AskUserQuestionCard>`. */
export const USE_UI_TOOL_NAME = "use_ui";

/** One primitive in a `use_ui` payload. `type` is the discriminator
 *  the frontend registry dispatches on; the remaining fields are
 *  type-specific (defined by Child B/C/D3: `diff` → diff_text,
 *  `code_block` → code/language, `button` → action/label/payload).
 *  snake_case wire (mirrors Rust, no `rename_all`). */
export interface UiPrimitive {
  type: string;
  /** Optional card title. */
  title?: string;
  /** Type-specific fields pass through unchecked at this layer. */
  [key: string]: unknown;
}

/** B9+ D3 (2026-07-13): the action enum for `type: "button"`
 *  primitives. Mirrors Rust `tools::use_ui::KNOWN_BUTTON_ACTIONS`.
 *  The renderer dispatches by this field at click-time; each
 *  action has a different side-effect path:
 *
 *  - `apply_diff` → invokes `apply_ui_diff` IPC with `payload.diff_text`.
 *    Same user-triggered IPC as the `<DiffPrimitive>` Apply button.
 *  - `copy` → writes `payload.text` to clipboard (pure frontend).
 *  - `dismiss` → hides the card locally (pure frontend, no backend).
 *
 *  Adding a new action requires a coordinated update on the Rust
 *  side (the `KNOWN_BUTTON_ACTIONS` const + `execute` validator)
 *  — see the test `definition_schema_type_enum_matches_known_types`
 *  for the lock-step guard.
 */
export type UiButtonAction = "apply_diff" | "copy" | "dismiss";

/** Payload shape for `type: "button"`. Field presence is
 *  action-dependent (validated on the Rust side):
 *  - `apply_diff` → `{ diff_text: string }` (non-empty standard unified diff)
 *  - `copy` → `{ text: string }`
 *  - `dismiss` → no payload required
 */
export interface UiButtonPayload {
  diff_text?: string;
  text?: string;
  [key: string]: unknown;
}

/** A typed view of a `button` primitive. Cast from `UiPrimitive`
 *  after narrowing `type === "button"`. The frontend
 *  `<ButtonPrimitive>` reads `action` / `label` / `payload` from
 *  this shape. */
export interface UiButtonPrimitive extends UiPrimitive {
  type: "button";
  action: UiButtonAction;
  label?: string;
  payload?: UiButtonPayload;
}