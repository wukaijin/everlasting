// uiCard.types.ts — public types for the B9 generative UI `use_ui`
// tool (Child A of 07-02-b9-generative-ui, 2026-07-02).
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
 *  type-specific (defined by Child B/C: `diff` → diff_text,
 *  `code_block` → code/language). Child A renders a mock that only
 *  reads `type` + dumps the JSON. snake_case wire (mirrors Rust, no
 *  rename_all). */
export interface UiPrimitive {
  type: string;
  /** Optional card title. */
  title?: string;
  /** Type-specific fields pass through unchecked at this layer. */
  [key: string]: unknown;
}
