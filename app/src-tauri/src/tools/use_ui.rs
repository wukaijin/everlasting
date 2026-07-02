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
//! # Schema
//!
//! `primitives: [{ type: "diff" | "code_block", title?, ... }]`.
//! Child A validates only `type` (non-empty array + known type);
//! type-specific fields (`diff_text` / `code` / `language`) are added
//! by Child B/C and pass through here unchecked
//! (`additionalProperties: true`).

use crate::llm::types::ToolDef;
use crate::tools::ToolContext;

/// The known primitive type allowlist. Child B/C populate the real
/// renderers; unknown types are rejected at `execute` so a
/// hallucinated type surfaces as an actionable error instead of a
/// silent frontend no-op.
///
/// Kept in sync with the `enum` in `definition()`'s `input_schema`
/// (the `definition_schema_type_enum_*` test guards the sync).
const KNOWN_TYPES: &[&str] = &["diff", "code_block"];

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
               for applying changes (use `edit_file` to write). Fields: `diff_text` (unified\n\
               diff string, required).\n\
             - `code_block` — a syntax-highlighted code snippet the user can copy. Fields: `code`\n\
               (string, required), `language` (optional, e.g. 'rust'/'python'; omit for auto-detect).\n\n\
             Do NOT use `use_ui` for:\n\
             - Asking the user to choose → use `ask_user_question` (single/multi select).\n\
             - Modifying files → use `edit_file` / `write_file`.\n\n\
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
                                "enum": ["diff", "code_block"],
                                "description": "The primitive kind; the frontend dispatches its renderer by this value."
                            },
                            "title": {
                                "type": "string",
                                "description": "Optional card title."
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
        // `button` is intentionally NOT in the MVP allowlist (D3:
        // independent button primitive is post-MVP).
        let v = serde_json::json!({ "primitives": [{ "type": "button" }] });
        let (out, is_err) = execute(&v, &dummy_ctx(), None).await;
        assert!(is_err);
        assert!(out.contains("button"), "{}", out);
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
