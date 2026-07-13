//! `use_ui` tool — agent outputs generative UI cards (B9).
//!
//! The model calls `use_ui({ primitives: [...] })` to emit one or more
//! interactive UI cards rendered inline by the frontend's component
//! registry. This is the carrier for B9 generative UI (parent task
//! `07-02-b9-generative-ui`; D1 = `use_ui` single tool + primitives
//! array).
//!
//! # Execution model (D2)
//!
//! **Non-blocking**. `execute` returns immediately with a plain
//! "已渲染 N 个 primitive" tool_result — it does NOT wait for user
//! interaction (unlike `ask_user_question`, which is a blocking
//! reverse-question). The primitives are display-only; their data
//! lives in the tool_use `input`, which the frontend reads directly
//! (`call.input.primitives`) — no separate IPC event is needed.
//!
//! # Scope (Child A — infrastructure only)
//!
//! This child wires the **plumbing**: tool definition + non-blocking
//! dispatch + frontend component registry + `<UiCard>` container +
//! MessageItem dispatch. The primitives render via a **mock
//! placeholder** (`<MockPrimitive>`) that dumps the type + JSON, so
//! the pipeline can be validated end-to-end before any real renderer
//! exists. Real renderers land in Child B (code_block → hljs) and
//! Child C (diff → reuses `DiffView`).
//!
//! # Permission
//!
//! **Silent Allow** (Tier 5, does NOT route to Tier 4 ask). `use_ui`
//! is display-only with no side effects (D4: diff is read-only, no
//! apply; D3: independent button + action allowlist is post-MVP).
//! `risk_for_tool` returns `Risk::Low` (the `_` default); Plan mode
//! keeps the tool (it writes nothing — not the filesystem, not the
//! DB), mirroring `remember`.
//!
//! **B9+ D3/D4 (2026-07-13)**: the `button` primitive's `apply_diff`
//! action triggers a user-driven `apply_ui_diff` IPC at click-time.
//! `use_ui` itself STILL does no writes — it's pure display. The
//! write path is `commands::ui::apply_ui_diff`, a separate IPC that's
//! NOT in `builtin_tools()` (D-Q1: user-triggered, not LLM tool).
//!
//! # Schema
//!
//! `primitives: [{ type: "diff" | "code_block" | "button", title?, ... }]`.
//! Child A validates only `type` (non-empty array + known type);
//! type-specific fields (`diff_text` / `code` / `language` /
//! `button.action` / `button.payload`) are added by Child B/C/D3
//! and pass through here unchecked (`additionalProperties: true`).
//!
//! `diff_text` accepts two formats (see `definition().description`
//! and `frontend/chat.md` "DiffPrimitive raw fallback contract"):
//!   - PREFERRED: standard unified-diff with `---`/`+++`/`@@` headers
//!     (full colored hunk rendering, line numbers, collapse)
//!   - ACCEPTED: plain +/-/context-line fragment without headers
//!     (raw fallback — line-classified tinting + real `+N/-M` counts)
//! Either form is valid; frontend renders both. The description
//! teaches the model the natural LLM-style writeup is also accepted,
//! so it doesn't pad `diff_text` with invented `---`/`+++` headers.

use crate::llm::types::ToolDef;
use crate::tools::ToolContext;

/// The known primitive type allowlist. Child B/C populate the real
/// renderers; unknown types are rejected at `execute` so a
/// hallucinated type surfaces as an actionable error instead of a
/// silent frontend no-op.
///
/// Kept in sync with the `enum` in `definition()`'s `input_schema`
/// (the `definition_schema_type_enum_*` test guards the sync).
///
/// # B9+ D3 (2026-07-13, `07-13-b9plus-generative-ui-followup`):
/// `button` joins the allowlist. The renderer is `<ButtonPrimitive>`
/// (D3 sibling to `<DiffPrimitive>` / `<CodeBlockPrimitive>`).
/// `button.action ∈ {"apply_diff", "copy", "dismiss"}` is a
/// **predefined enum** (D-Q2a/b); the renderer dispatches the action
/// at click-time. `apply_diff` routes to the same `apply_ui_diff`
/// IPC that DiffPrimitive uses (D-Q1: user-triggered, not LLM
/// tool-driven). `copy` / `dismiss` are pure-frontend with no
/// backend touch.
const KNOWN_TYPES: &[&str] = &["diff", "code_block", "button"];

/// The action enum for `type: "button"` primitives. Mirrors the
/// frontend `<ButtonPrimitive>` action dispatch table (D3 sibling
/// to `tools::use_ui::execute`); the renderer is the source of
/// truth for click-time behavior, this constant is the backend's
/// authoritative validation set.
///
/// `apply_diff` requires the LLM to provide `payload.diff_text` in
/// the button's JSON — the same payload shape as
/// `<DiffPrimitive>`'s `diff_text`. We validate the field is
/// present (string, non-empty) here; the actual file write is the
/// `apply_ui_diff` IPC's responsibility.
const KNOWN_BUTTON_ACTIONS: &[&str] = &["apply_diff", "copy", "dismiss"];

/// Max primitives per call (anti-abuse: one turn shouldn't flood the
/// chat with cards). Mirrors the `maxItems: 8` in the schema.
const MAX_PRIMITIVES: usize = 8;

/// The `use_ui` tool definition registered in `builtin_tools()`.
pub fn definition() -> ToolDef {
    ToolDef {
        name: "use_ui".to_string(),
        description: Some(
            "Output one or more interactive UI cards (generative UI) rendered inline in the \
             chat. Use this when a visual presentation is clearer than prose.\n\n\
             Supported `primitive.type`:\n\
             - `diff` — a read-only code diff (compare two versions / two approaches). NOT \
               for applying changes (use `edit_file` to write). Fields: `diff_text` (string,\
               required). Two accepted formats:\n\
               • PREFERRED: standard unified-diff with `--- a/path` / `+++ b/path` / \
                 `@@ -oldStart,oldLines +newStart,newLines @@` headers. Renders as \
                 colored hunks with line numbers + collapse. The card also exposes an \
                 Apply button (user-triggered `apply_ui_diff` IPC; see 2026-07-13 B9+ D4).\n\
               • ACCEPTED: plain +/-/context-line fragment WITHOUT `---`/`+++` headers \
                 (the natural \"show old vs new\" writeup). Renders as raw fallback — \
                 Apply button is disabled for this form.\n\
             - `code_block` — a syntax-highlighted code snippet the user can copy. Fields: `code`\n\
               (string, required), `language` (optional, e.g. 'rust'/'python'; omit for auto-detect).\n\
             - `button` — a user-clickable action button (B9+ D3, 2026-07-13). **Human-in-the-loop\n\
               intent**: use this when the LLM wants the user to confirm / apply / dismiss a\n\
               suggested action. NOT a way for the LLM to write files directly — that's what\n\
               `edit_file` is for in edit/yolo mode. Fields:\n\
               • `action` (string, required) ∈ `{\"apply_diff\", \"copy\", \"dismiss\"}`:\n\
                 - `apply_diff` — apply the proposed diff to disk (user-triggered IPC).\n\
                   `payload.diff_text` (string, required) carries the standard unified diff.\n\
                 - `copy` — copy `payload.text` to the user's clipboard (pure frontend).\n\
                 - `dismiss` — close / hide this card (pure frontend).\n\
               • `label` (string, optional) — the button text. Defaults to a sensible per-action label.\n\
               • `payload` (object, optional) — type-specific payload (see above).\n\n\
             Do NOT use `use_ui` for:\n\
             - Asking the user to choose → use `ask_user_question` (single/multi select).\n\
             - Modifying files directly → use `edit_file` / `write_file` (those are LLM-driven,\n\
               run via the permission layer; the user doesn't have to click).\n\n\
             When to use which (LLM behavior guide):\n\
             - Edit/Yolo mode + the LLM is authorized to change the file → use `edit_file`.\n\
               Don't make the user click a button when they're already in a permissive mode.\n\
             - Plan mode + the LLM wants to propose a change → use `use_ui({type:\"diff\"})` or\n\
               `use_ui({type:\"button\",action:\"apply_diff\", payload:{diff_text:\"...\"}})`.\n\
               Plan mode blocks `edit_file`; use_ui diff/button is the **only** way to suggest\n\
               a write for the user to approve.\n\
             - Compare multiple alternatives → render multiple `diff` primitives side-by-side.\n\n\
             Pass `primitives: [{ type, title?, ...type-specific fields }]`. The frontend \
             renders each card by `type`."
                .to_string(),
        ),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "primitives": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": MAX_PRIMITIVES,
                    "items": {
                        "type": "object",
                        "properties": {
                            "type": {
                                "type": "string",
                                "enum": ["diff", "code_block", "button"],
                                "description": "The primitive kind; the frontend dispatches its renderer by this value."
                            },
                            "title": {
                                "type": "string",
                                "description": "Optional card title."
                            },
                            "action": {
                                "type": "string",
                                "enum": ["apply_diff", "copy", "dismiss"],
                                "description": "(button only) The action the user triggers by clicking. Predefined enum — see description."
                            },
                            "label": {
                                "type": "string",
                                "description": "(button only) Optional override for the button label."
                            }
                        },
                        "required": ["type"],
                        "additionalProperties": true
                    }
                }
            },
            "required": ["primitives"]
        }),
    }
}

/// Execute `use_ui`: validate the `primitives` array (present,
/// non-empty, ≤ `MAX_PRIMITIVES`, every `type` known), then return a
/// non-blocking "rendered N" ack. Performs no side effects — the
/// actual rendering is frontend-side (primitives data is carried in
/// the tool_use `input`).
pub async fn execute(
    input: &serde_json::Value,
    _ctx: &ToolContext,
    _session_id: Option<&str>,
) -> (String, bool) {
    let primitives = match input.get("primitives").and_then(|v| v.as_array()) {
        Some(arr) => arr,
        None => {
            return (
                "use_ui 需要一个 `primitives` 数组（至少 1 个，最多 8 个）".to_string(),
                true,
            );
        }
    };
    let n = primitives.len();
    if n == 0 {
        return ("use_ui 的 `primitives` 数组不能为空".to_string(), true);
    }
    if n > MAX_PRIMITIVES {
        return (
            format!(
                "use_ui 的 `primitives` 数组最多 {} 个，收到 {} 个",
                MAX_PRIMITIVES, n
            ),
            true,
        );
    }
    for (i, p) in primitives.iter().enumerate() {
        let t = match p.get("type").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => {
                return (
                    format!("use_ui 的 primitives[{}] 缺少字符串 `type` 字段", i),
                    true,
                );
            }
        };
        if !KNOWN_TYPES.contains(&t) {
            return (
                format!(
                    "use_ui 的 primitives[{}] `type`='{}' 不在支持列表 {:?} 内",
                    i, t, KNOWN_TYPES
                ),
                true,
            );
        }
        // B9+ D3 (2026-07-13): for `button` primitives, additionally
        // validate `action ∈ KNOWN_BUTTON_ACTIONS` so a hallucinated
        // action surfaces as an actionable LLM-facing error (rather
        // than a silent frontend no-op or — worse — arbitrary code
        // path on click).
        if t == "button" {
            let action = p.get("action").and_then(|v| v.as_str());
            match action {
                None => {
                    return (
                        format!(
                            "use_ui 的 primitives[{}] (`type=button`) 缺少 `action` 字段",
                            i
                        ),
                        true,
                    );
                }
                Some(a) if !KNOWN_BUTTON_ACTIONS.contains(&a) => {
                    return (
                        format!(
                            "use_ui 的 primitives[{}] (`type=button`) `action`='{}' 不在支持列表 {:?} 内",
                            i, a, KNOWN_BUTTON_ACTIONS
                        ),
                        true,
                    );
                }
                // `apply_diff` requires `payload.diff_text` to be a
                // non-empty string (the renderer dispatches it to
                // `apply_ui_diff` IPC on click). We catch the missing
                // field here so the LLM gets immediate feedback
                // rather than a click-time IPC error.
                Some("apply_diff") => {
                    let diff_text = p
                        .get("payload")
                        .and_then(|p| p.get("diff_text"))
                        .and_then(|v| v.as_str());
                    match diff_text {
                        None => {
                            return (
                                format!(
                                    "use_ui 的 primitives[{}] (`type=button`, `action=apply_diff`) 缺少 `payload.diff_text` 字符串字段",
                                    i
                                ),
                                true,
                            );
                        }
                        Some(s) if s.trim().is_empty() => {
                            return (
                                format!(
                                    "use_ui 的 primitives[{}] (`type=button`, `action=apply_diff`) 的 `payload.diff_text` 不能为空",
                                    i
                                ),
                                true,
                            );
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }
    (format!("已渲染 {} 个 primitive", n), false)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal `ToolContext` for `use_ui` tests. `use_ui` ignores ctx
    /// entirely (display-only), so the fields only need to type-check;
    /// a lazy (unconnected) pool avoids the cost of running migrations
    /// per test. Mirrors the field set of `remember::tests::make_ctx`.
    fn dummy_ctx() -> ToolContext {
        ToolContext {
            worktree_path: std::path::PathBuf::from("/repo/proj"),
            cwd: std::path::PathBuf::from("/repo/proj"),
            checklist: crate::tools::update_checklist::new_handle(),
            background_shells: crate::background_shell::default_registry(),
            db: sqlx::SqlitePool::connect_lazy("sqlite::memory:").expect("lazy pool"),
            project_id: "/repo/proj".to_string(),
            data_dir: std::path::PathBuf::from("/repo"),
            workflow_name: None,
        }
    }

    // ---- definition ----

    #[test]
    fn definition_has_correct_name() {
        assert_eq!(definition().name, "use_ui");
    }

    #[test]
    fn definition_schema_requires_primitives() {
        let def = definition();
        let required = def
            .input_schema
            .get("required")
            .and_then(|v| v.as_array())
            .expect("required array present");
        let names: Vec<&str> = required.iter().filter_map(|v| v.as_str()).collect();
        assert_eq!(names, vec!["primitives"]);
    }

    #[test]
    fn definition_schema_type_enum_matches_known_types() {
        // Guards the manual sync between the schema `enum` and the
        // `KNOWN_TYPES` const used by `execute`. If Child B/C add a
        // new type, BOTH must change together.
        let def = definition();
        let strs: Vec<&str> = def
            .input_schema
            .pointer("/properties/primitives/items/properties/type/enum")
            .and_then(|v| v.as_array())
            .expect("type enum")
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert_eq!(strs, KNOWN_TYPES);
    }

    #[test]
    fn definition_schema_enforces_maxitems() {
        let max = definition()
            .input_schema
            .pointer("/properties/primitives/maxItems")
            .and_then(|v| v.as_u64())
            .expect("maxItems present");
        assert_eq!(max as usize, MAX_PRIMITIVES);
    }

    /// Lock the LLM-facing description mentions BOTH accepted `diff_text`
    /// formats (standard unified-diff PREFERRED + LLM-style +/- fragment
    /// ACCEPTED). If a future cleanup collapses to a single format, this
    /// fails and forces the author to choose between expanding the
    /// renderer's accepted formats and shrinking the description.
    /// Mirrors the raw fallback contract in `frontend/chat.md`
    /// `RULE-FrontDiff-001` and the b00dde2 + b5073ea bug fixes.
    #[test]
    fn diff_description_advertises_both_accepted_formats() {
        let desc = definition().description.expect("description set");
        assert!(desc.contains("PREFERRED"), "missing PREFERRED marker");
        assert!(desc.contains("ACCEPTED"), "missing ACCEPTED marker");
        assert!(
            desc.contains("`diff_text`"),
            "must name the `diff_text` field"
        );
    }

    // ---- execute: happy paths ----

    #[tokio::test]
    async fn execute_happy_path_single() {
        let v = serde_json::json!({ "primitives": [{ "type": "diff" }] });
        let (out, is_err) = execute(&v, &dummy_ctx(), None).await;
        assert!(!is_err, "{}", out);
        assert!(out.contains("1"), "{}", out);
    }

    #[tokio::test]
    async fn execute_happy_path_multiple_mixed_types() {
        // type-specific fields (title / language) pass through
        // unchecked (Child A only validates `type`).
        let v = serde_json::json!({
            "primitives": [
                { "type": "diff", "title": "v1 vs v2", "diff_text": "..." },
                { "type": "code_block", "language": "rust", "code": "fn main(){}" }
            ]
        });
        let (out, is_err) = execute(&v, &dummy_ctx(), None).await;
        assert!(!is_err, "{}", out);
        assert!(out.contains("2"), "{}", out);
    }

    // ---- execute: rejections ----

    #[tokio::test]
    async fn execute_rejects_missing_primitives() {
        let v = serde_json::json!({});
        let (out, is_err) = execute(&v, &dummy_ctx(), None).await;
        assert!(is_err);
        assert!(out.contains("primitives"), "{}", out);
    }

    #[tokio::test]
    async fn execute_rejects_empty_array() {
        let v = serde_json::json!({ "primitives": [] });
        let (out, is_err) = execute(&v, &dummy_ctx(), None).await;
        assert!(is_err);
        assert!(out.contains("空"), "{}", out);
    }

    #[tokio::test]
    async fn execute_rejects_too_many() {
        let arr: Vec<serde_json::Value> = (0..(MAX_PRIMITIVES + 1))
            .map(|_| serde_json::json!({ "type": "diff" }))
            .collect();
        let v = serde_json::json!({ "primitives": arr });
        let (out, is_err) = execute(&v, &dummy_ctx(), None).await;
        assert!(is_err);
        assert!(out.contains(&MAX_PRIMITIVES.to_string()), "{}", out);
    }

    #[tokio::test]
    async fn execute_rejects_missing_type_field() {
        let v = serde_json::json!({ "primitives": [{ "title": "no type" }] });
        let (out, is_err) = execute(&v, &dummy_ctx(), None).await;
        assert!(is_err);
        assert!(out.contains("type"), "{}", out);
    }

    #[tokio::test]
    async fn execute_rejects_unknown_type() {
        // B9+ D3 (2026-07-13): `button` is now a known type (was
        // intentionally NOT in the MVP allowlist pre-D3, see
        // `git log 07-02-b9-generative-ui`); use `chart` here as
        // the still-unknown example.
        let v = serde_json::json!({ "primitives": [{ "type": "chart" }] });
        let (out, is_err) = execute(&v, &dummy_ctx(), None).await;
        assert!(is_err);
        assert!(out.contains("chart"), "{}", out);
    }

    // ---- execute: button primitive (B9+ D3, 2026-07-13) ----

    #[tokio::test]
    async fn execute_button_apply_diff_happy() {
        let v = serde_json::json!({
            "primitives": [{
                "type": "button",
                "action": "apply_diff",
                "label": "Apply proposed fix",
                "payload": { "diff_text": "--- a/x\n+++ b/x\n@@ -1 +1 @@\n-old\n+new\n" }
            }]
        });
        let (out, is_err) = execute(&v, &dummy_ctx(), None).await;
        assert!(!is_err, "{}", out);
        assert!(out.contains("1"), "{}", out);
    }

    #[tokio::test]
    async fn execute_button_copy_happy() {
        let v = serde_json::json!({
            "primitives": [{
                "type": "button",
                "action": "copy",
                "payload": { "text": "hello" }
            }]
        });
        let (out, is_err) = execute(&v, &dummy_ctx(), None).await;
        assert!(!is_err, "{}", out);
    }

    #[tokio::test]
    async fn execute_button_dismiss_happy() {
        let v = serde_json::json!({
            "primitives": [{
                "type": "button",
                "action": "dismiss"
            }]
        });
        let (out, is_err) = execute(&v, &dummy_ctx(), None).await;
        assert!(!is_err, "{}", out);
    }

    #[tokio::test]
    async fn execute_button_missing_action_rejected() {
        let v = serde_json::json!({
            "primitives": [{ "type": "button", "label": "no action" }]
        });
        let (out, is_err) = execute(&v, &dummy_ctx(), None).await;
        assert!(is_err);
        assert!(out.contains("action"), "{}", out);
    }

    #[tokio::test]
    async fn execute_button_unknown_action_rejected() {
        let v = serde_json::json!({
            "primitives": [{
                "type": "button",
                "action": "run_command",
                "payload": { "command": "rm -rf /" }
            }]
        });
        let (out, is_err) = execute(&v, &dummy_ctx(), None).await;
        assert!(is_err);
        assert!(out.contains("run_command"), "{}", out);
    }

    #[tokio::test]
    async fn execute_button_apply_diff_missing_payload_rejected() {
        let v = serde_json::json!({
            "primitives": [{
                "type": "button",
                "action": "apply_diff",
                "label": "Apply"
            }]
        });
        let (out, is_err) = execute(&v, &dummy_ctx(), None).await;
        assert!(is_err);
        assert!(out.contains("diff_text"), "{}", out);
    }

    #[tokio::test]
    async fn execute_button_apply_diff_empty_diff_rejected() {
        let v = serde_json::json!({
            "primitives": [{
                "type": "button",
                "action": "apply_diff",
                "payload": { "diff_text": "   " }
            }]
        });
        let (out, is_err) = execute(&v, &dummy_ctx(), None).await;
        assert!(is_err);
        assert!(out.contains("不能为空"), "{}", out);
    }

    #[tokio::test]
    async fn execute_reports_index_of_bad_primitive() {
        let v = serde_json::json!({
            "primitives": [
                { "type": "diff" },
                { "type": "chart" }
            ]
        });
        let (out, is_err) = execute(&v, &dummy_ctx(), None).await;
        assert!(is_err);
        assert!(out.contains("[1]"), "should name the bad index: {}", out);
    }
}
