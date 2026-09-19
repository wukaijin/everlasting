//! N1 `test_llm_connection` tool — per-model connectivity probe on the
//! agent surface (task `09-15-n1-onboarding-skills` R3.2).
//!
//! Thin wrapper over [`crate::commands::providers::test_model_inner`]
//! (the exact code path behind the `test_model` IPC / daemon route —
//! signature converged to `(db, model_id)` so the tool can reuse it).
//! Semantics are frozen by spec `backend/test-model-contract.md`:
//! per-model minimal round-trip (`anthropic` → `/v1/messages`,
//! `openai` → `/chat/completions`, `openai_responses` → `/responses`,
//! `body.model` = the catalog's `model_name`), 15s timeout,
//! `{success, latencyMs, error}` result, four failure paths never
//! Rust-`Err`. This wrapper only translates that JSON into
//! LLM-compact text and appends a per-category fix hint for the
//! `doctor` skill to act on. Protocol-specific hints for
//! `openai_responses` (404 / effort vocabulary) are embedded in the
//! `error` string by `test_model_inner` itself, so they flow through
//! here unchanged.
//!
//! # Egress
//!
//! Same argument shape as `web_search` (spec tool-contract/15): the
//! endpoint is the user's own configured provider `base_url` — the
//! identical egress face a normal chat turn already uses. Fixed
//! minimal payload (`max_tokens: 1`), no user-controllable URL beyond
//! that configured base_url, so no SSRF surface beyond chat itself.
//! The probe is a REAL billable request (one token) — the
//! description says to use it sparingly.
//!
//! # Permission
//!
//! Silent Allow (Tier 5 via `ToolKind::Other` default, same as
//! `llm_diagnostics` / `search_history`); serial dispatch; not a C7D
//! stub candidate (1 optional param); group chat excludes it via the
//! `group_chat_tool_defs` whitelist. It performs no local write — the
//! only effect happens on the provider side (a 1-token bill).
//!
//! # Error arming
//!
//! Config-level failures (`model_id` unknown, no default model,
//! orphaned model) return `is_error: true` — they mean the CALLER
//! misreferenced the catalog and should consult `llm_diagnostics`.
//! Connectivity failures (HTTP 4xx/5xx, request failed) are the
//! tool's *data*, so `is_error: false` — the doctor skill reads them
//! diagnostically. No automated HTTP tests (spec §6: manual smoke is
//! the contract); unit tests cover the HTTP-free arms (missing rows,
//! unsupported protocol — the client is built but no request is sent).

use crate::llm::types::ToolDef;
use crate::tools::ToolContext;

/// The `test_llm_connection` tool definition registered in
/// `builtin_tools()` (appended last — order feeds the provider prefix
/// cache; appending never shifts the existing prefix).
pub fn definition() -> ToolDef {
    ToolDef {
        name: "test_llm_connection".to_string(),
        description: Some(
            "Run a real per-model connectivity probe (a minimal 1-token chat request) \
             against one configured model and report success/latency/error with a fix \
             hint. Use to verify a provider/model actually works — e.g. after config \
             changes or when diagnosing chat failures. Costs one billed token per call; \
             do not loop retries. model_id defaults to the app's default model."
                .to_string(),
        ),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "model_id": {
                    "type": "string",
                    "description": "Catalog model id to test (see llm_diagnostics). \
                                    Omitted = the default model."
                }
            }
        }),
    }
}

/// Execute: resolve the target model (explicit `model_id` → default
/// model), call the shared `test_model_inner`, translate the result
/// to compact text with a classified fix hint.
pub async fn execute(
    input: &serde_json::Value,
    ctx: &ToolContext,
    _session_id: Option<&str>,
) -> (String, bool) {
    let explicit = input
        .get("model_id")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let model_id = match explicit {
        Some(id) => id.to_string(),
        None => match crate::db::get_config_value(&ctx.db, "default_model_id").await {
            Ok(Some(id)) if !id.is_empty() => id,
            Ok(_) => {
                return (
                    "test_llm_connection: no model to test — no model_id given and no \
                     default model is configured. Set one in Settings → Models, or pass \
                     an explicit model_id (see llm_diagnostics)."
                        .to_string(),
                    true,
                )
            }
            Err(e) => {
                tracing::warn!(error = %e, "test_llm_connection: read default_model_id failed");
                return (format!("test_llm_connection failed: {}", e), true);
            }
        },
    };

    match crate::commands::providers::test_model_inner(&ctx.db, model_id.clone()).await {
        Err(e) => (
            // Contract: the only Rust-Err path is "failed to build HTTP
            // client" (unrecoverable local TLS init) — surface as tool error.
            format!("test_llm_connection failed: {:?}", e),
            true,
        ),
        Ok(result) => {
            let success = result
                .get("success")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let latency = result.get("latencyMs").and_then(|v| v.as_u64());
            let error = result
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            if success {
                (
                    format!(
                        "test_llm_connection OK: model '{model_id}' responded in {} ms.",
                        latency.unwrap_or(0)
                    ),
                    false,
                )
            } else {
                // Config-level failures mean the caller misreferenced the
                // catalog → is_error; connectivity failures are data.
                let config_level = error.contains("not found")
                    || error.contains("is missing")
                    || error.contains("failed to load");
                (
                    format!(
                        "test_llm_connection FAILED for model '{model_id}': {error}\n{}",
                        fix_hint(error)
                    ),
                    config_level,
                )
            }
        }
    }
}

/// Map a `test_model_inner` error string to a short fix hint. The
/// categories mirror the app's `LlmError` classification (auth /
/// rate_limit / network / server / invalid_request) plus the
/// catalog-level arms. Order matters: specific HTTP codes before the
/// generic `HTTP 5` prefix match.
fn fix_hint(error: &str) -> &'static str {
    if error.contains("not found") {
        "hint: unknown model id — call llm_diagnostics to list valid model ids."
    } else if error.contains("is missing") {
        "hint: the model's parent provider row is gone — re-create the model under an \
         existing provider (see llm-setup)."
    } else if error.contains("failed to load") {
        "hint: local catalog read failed — retry once; a persistent failure is an app bug."
    } else if error.starts_with("unsupported protocol") {
        "hint: provider protocol must be `anthropic`, `openai`, or `openai_responses` — \
         fix it in Settings → Providers (see llm-setup)."
    } else if error.contains("HTTP 401") || error.contains("HTTP 403") {
        "hint (auth): API key invalid or missing — update the key in Settings → Providers \
         (never paste keys into this chat)."
    } else if error.contains("HTTP 429") || error.contains("HTTP 529") {
        "hint (rate_limit): the provider is rate-limiting or overloaded — wait and retry, \
         or switch to another model."
    } else if error.contains("HTTP 400") {
        "hint (invalid_request): the model_name is likely a typo or the request is \
         unsupported — verify the exact model id against the provider docs (llm-setup)."
    } else if error.contains("HTTP 404") {
        "hint (network): endpoint path not found — check the provider base_url spelling \
         and its /v1 rule (see llm-setup)."
    } else if error.starts_with("request failed") {
        "hint (network): cannot reach the endpoint — check base_url, local proxy/firewall, \
         DNS; for local Ollama make sure the server is running."
    } else if error.starts_with("HTTP 5") {
        "hint (server): provider-side outage — retry later or check the provider's status \
         page."
    } else {
        "hint: inspect the error text above; the llm-setup skill covers common base_url / \
         model-name pitfalls."
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::db::models::create_model;
    use crate::db::providers::create_provider;

    /// Fresh in-memory pool + migrations per test — shared fixture
    /// `db::test_support::test_pool` (RULE-TESTPOOL-001). NOTE:
    /// migrations seed a default catalog AND `default_model_id` —
    /// tests that need a controlled state wipe it first (mirrors
    /// `tools/llm_diagnostics.rs::clear_catalog`). All tests here stay
    /// on the HTTP-free arms (missing rows / unsupported protocol),
    /// per spec test-model-contract.md §6 (no automated wire tests).
    async fn make_ctx() -> ToolContext {
        ToolContext {
            tool_use_id: None,
            escalation: Default::default(),
            worktree_path: std::path::PathBuf::from("/repo/proj"),
            cwd: std::path::PathBuf::from("/repo/proj"),
            checklist: crate::tools::update_checklist::new_handle(),
            background_shells: crate::background_shell::default_registry(),
            db: crate::db::test_support::test_pool().await,
            project_id: String::new(),
            data_dir: std::path::PathBuf::from("/repo"),
            workflow_name: None,
            mode: crate::db::Mode::Edit,
        }
    }

    #[test]
    fn definition_model_id_is_optional() {
        let def = definition();
        assert_eq!(def.name, "test_llm_connection");
        let required = def
            .input_schema
            .get("required")
            .and_then(|v| v.as_array())
            .map(|a| a.len())
            .unwrap_or(0);
        assert_eq!(required, 0, "model_id is optional");
        let desc = def.description.as_deref().unwrap_or_default();
        assert!(desc.contains("1-token"), "description discloses the cost");
        assert!(
            desc.contains("defaults to the app's default model"),
            "description states the default-model fallback"
        );
    }

    #[tokio::test]
    async fn no_default_model_is_an_error() {
        let ctx = make_ctx().await;
        sqlx::query("DELETE FROM app_config WHERE key = 'default_model_id'")
            .execute(&ctx.db)
            .await
            .unwrap();
        let (out, is_err) = execute(&serde_json::json!({}), &ctx, None).await;
        assert!(is_err);
        assert!(out.contains("no default model"), "{out}");
        assert!(out.contains("llm_diagnostics"), "{out}");
    }

    #[tokio::test]
    async fn empty_model_id_string_falls_through_to_default_resolution() {
        // `""` / whitespace is treated as absent (lenient — the value
        // doesn't change WHAT is tested, so don't fail loud).
        let ctx = make_ctx().await;
        sqlx::query("DELETE FROM app_config WHERE key = 'default_model_id'")
            .execute(&ctx.db)
            .await
            .unwrap();
        let (out, is_err) = execute(&serde_json::json!({"model_id": "  "}), &ctx, None).await;
        assert!(is_err);
        assert!(out.contains("no default model"), "{out}");
    }

    #[tokio::test]
    async fn default_pointing_at_missing_row_is_an_error_without_http() {
        let ctx = make_ctx().await;
        crate::db::set_config_value(&ctx.db, "default_model_id", "no-such-model")
            .await
            .unwrap();
        let (out, is_err) = execute(&serde_json::json!({}), &ctx, None).await;
        assert!(is_err, "stale default is a caller-level mistake: {out}");
        assert!(out.contains("not found"), "{out}");
        assert!(out.contains("llm_diagnostics"), "{out}");
    }

    #[tokio::test]
    async fn explicit_unknown_model_id_is_an_error_without_http() {
        let ctx = make_ctx().await;
        let (out, is_err) = execute(&serde_json::json!({"model_id": "ghost-id"}), &ctx, None).await;
        assert!(is_err);
        assert!(out.contains("ghost-id"), "{out}");
        assert!(out.contains("not found"), "{out}");
    }

    #[tokio::test]
    async fn unsupported_protocol_arm_is_http_free_and_diagnostic() {
        // The client is built but the protocol match rejects before any
        // request — deterministic, offline coverage of the translate
        // path (connectivity-class result → is_error=false + hint).
        let ctx = make_ctx().await;
        sqlx::query("DELETE FROM providers")
            .execute(&ctx.db)
            .await
            .unwrap();
        sqlx::query("DELETE FROM app_config WHERE key = 'default_model_id'")
            .execute(&ctx.db)
            .await
            .unwrap();
        let p = create_provider(
            &ctx.db,
            "grpc",
            "Weird",
            "https://weird.example.com",
            "sk-t",
        )
        .await
        .unwrap();
        let m = create_model(
            &ctx.db,
            &p.id,
            "weird-1",
            "Weird One",
            None,
            None,
            false,
            false,
            8,
        )
        .await
        .unwrap();
        let (out, is_err) = execute(&serde_json::json!({"model_id": m.id}), &ctx, None).await;
        assert!(!is_err, "connectivity-class result is data: {out}");
        assert!(out.contains("FAILED"), "{out}");
        assert!(out.contains("unsupported protocol: grpc"), "{out}");
        assert!(
            out.contains("`anthropic`, `openai`, or `openai_responses`"),
            "{out}"
        );
    }

    #[test]
    fn fix_hint_covers_all_categories() {
        // 分类提示文案与 doctor skill 的五类错误映射同源。
        assert!(fix_hint("model 'x' not found").contains("llm_diagnostics"));
        assert!(fix_hint("provider for model 'x' is missing").contains("llm-setup"));
        assert!(fix_hint("failed to load model: boom").contains("app bug"));
        assert!(fix_hint("unsupported protocol: grpc").contains("anthropic"));
        assert!(fix_hint("HTTP 401: {\"error\":...}").contains("auth"));
        assert!(fix_hint("HTTP 403: forbidden").contains("auth"));
        assert!(fix_hint("HTTP 429: slow down").contains("rate_limit"));
        assert!(fix_hint("HTTP 529: overloaded").contains("rate_limit"));
        assert!(fix_hint("HTTP 400: model_not_found").contains("invalid_request"));
        assert!(fix_hint("HTTP 404: no path").contains("/v1"));
        assert!(fix_hint("request failed: connection refused").contains("network"));
        assert!(fix_hint("HTTP 502: bad gateway").contains("server"));
        // 顺序锚:HTTP 5xx 的前缀匹配不得吃掉更具体的 4xx/529。
        assert_eq!(fix_hint("HTTP 529: overloaded"), fix_hint("HTTP 429: x"));
    }
}
