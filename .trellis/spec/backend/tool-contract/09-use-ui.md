## Scenario: `use_ui` — B9 generative UI carrier tool (2026-07-02)

**Carrier**: single tool `use_ui({ primitives: [...] })` (parent D1). NOT a
`ui_render` content block — reuses the mature tool pipeline (⑨ permission /
audit / `persist_turn` / frontend `tool_name` dispatch). Zero Provider wire
change. The `ui_render` / `use_ui` / `ui:render` references in
`ARCHITECTURE.md §⑭` were paper-only (zero impl) before this.

**Execution model (D2)**: NON-blocking. `execute` returns immediately with
`("已渲染 N 个 primitive", false)` — does NOT wait for user interaction
(unlike `ask_user_question`'s blocking oneshot). Dispatched via the normal
`execute_tool_inner` match arm `"use_ui"` (NOT `ask_user_question`'s
`chat_loop.rs` interception pattern).

**Permission**: Tier 5 silent Allow (display-only, no side effects). Goes
through `risk_for_tool`'s `_ => Risk::Low` arm (same as `remember`);
`filter_tools_for_mode` keeps it in Plan mode (writes nothing — not the
filesystem, not the DB).

**Schema** (`additionalProperties: true` — Child A validates only `type`;
type-specific fields are added by Child B/C and pass through unchecked):
`primitives: [{ type: "diff" | "code_block", title?, ... }]`. `minItems: 1`,
`maxItems: 8` (anti-flood). Unknown/missing `type` or empty array →
`is_error: true` + actionable 中文 message naming the bad index.

**`diff_text` format (2026-07-02, RULE-FrontDiff-001)**: the `diff`
primitive accepts **two formats** for `diff_text` and the frontend
`DiffPrimitive` + `DiffView` both render correctly (frontend invariant
documented in `frontend/chat/generative-ui.md` §"DiffPrimitive raw fallback contract"):

| Format | Example | Render path |
|---|---|---|
| **PREFERRED** standard unified-diff | `--- a/foo\n+++ b/foo\n@@ -1,3 +1,3 @@\n-x\n+y` | `jsdiff parsePatch` hunks → colored `+`/`-` lines with line numbers + collapse |
| **ACCEPTED** LLM-style `+`/`-` fragment without headers | ` foo\n-x\n+y\n bar` | raw fallback — per-line tint by prefix + real `+N/-M` counts |

Either form is valid; the description teaches the model the natural
LLM-style writeup is also accepted so it doesn't pad `diff_text` with
invented `---`/`+++` headers (which would also work but is unnecessary
friction). Test `diff_description_advertises_both_accepted_formats`
locks both markers in the LLM-facing description string.

**Persistence**: tool_result persists via the existing `persist_turn` (no
new DB table / column / migration).

**selector primitive**: NOT carried by `use_ui` — `ask_user_question` IS
the selector primitive (D2, reuse, zero code change). Frontend dispatch:
`tool_name === ask_user_question` → `<AskUserQuestionCard>`;
`tool_name === use_ui` → `<UiCard>` (registry by `primitive.type`).

**Post-MVP (out of scope)**: independent `button` primitive + action
allowlist (D3 — the security-critical one); diff apply action (D4);
session `allow_generative_ui` switch (D5); free-form HTML UI.

**Tests required**: `cargo test --lib use_ui` (definition schema sync +
execute happy / reject-empty / reject-unknown-type / reject-too-many /
reports-bad-index). Frontend `UiCard.test.ts` (registry dispatch +
unknown-type fallback + empty/missing guard). Full PRD:
`.trellis/tasks/07-02-b9-generative-ui/`.
