//! N1 `llm_diagnostics` tool — read-only snapshot of the LLM
//! provider/model configuration for diagnosis (task
//! `09-15-n1-onboarding-skills` R3.1).
//!
//! The `doctor` skill's data source: the model calls
//! `llm_diagnostics({})` to see what providers / models exist, which
//! one is the default, and whether anything is disabled or missing a
//! key — BEFORE burning a billable probe on `test_llm_connection`.
//!
//! # Redaction (hard boundary)
//!
//! The output view is **hand-built field by field**. `db::ProviderRow`
//! is NEVER serde-serialized here — it carries the decrypted
//! `api_key` (RULE-D-001 keeps it out of the IPC wire via
//! `#[serde(skip)]`, but a careless `json!(row)` in a tool would leak
//! the plaintext into the LLM context, which is then sent to the LLM
//! provider). Neither the plaintext key nor the `api_key_enc`
//! ciphertext is referenced; the boolean `has_key` (same naming as
//! `ProviderRow::has_key`) is the only key-related signal. A unit
//! test pins that the serialized output contains neither the
//! substring `api_key` nor the seeded key material.
//!
//! # Permission
//!
//! **Silent Allow** (Tier 5 via `ToolKind::Other` default) — same
//! model as `search_history`: a read-only DB query with no side
//! effects. `risk_for_tool` returns `Risk::Low` (`_` default); Plan
//! mode keeps it; serial dispatch (not in the L2 parallel whitelist —
//! zero declaration needed); NOT a C7D stub candidate (zero-param
//! schema, same reasoning as search_history's 3-param exclusion);
//! group chat excludes it via the `group_chat_tool_defs` whitelist
//! (no change needed there).
//!
//! # Output shape
//!
//! Compact one-row-per-entity text (LLM-facing, not the wire DTOs):
//! full `id`s are printed because `test_llm_connection` consumes the
//! exact `models.id` — a truncated id would break the doctor flow's
//! diagnostics → probe round-trip.

use crate::db;
use crate::llm::types::ToolDef;
use crate::tools::ToolContext;

/// The `llm_diagnostics` tool definition registered in
/// `builtin_tools()` (appended last — order feeds the provider prefix
/// cache; appending never shifts the existing prefix).
pub fn definition() -> ToolDef {
    ToolDef {
        name: "llm_diagnostics".to_string(),
        description: Some(
            "Read-only snapshot of the LLM provider/model configuration for diagnosis; \
             never contains API keys. Lists every provider (protocol, base_url, disabled, \
             has_key), every model (model_name, context_window, disabled) and the current \
             default_model_id. Use before test_llm_connection to see what is configured, \
             or when the user asks what providers/models are set up."
                .to_string(),
        ),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {}
        }),
    }
}

/// Execute: read providers + models + default-model config from the
/// caller's pool and render the redacted snapshot. DB failures return
/// `(message, is_error=true)` (standard ⑫ error-feedback path); an
/// empty configuration is a *valid* diagnostic result, not an error.
pub async fn execute(
    _input: &serde_json::Value,
    ctx: &ToolContext,
    _session_id: Option<&str>,
) -> (String, bool) {
    let providers = match db::list_providers(&ctx.db).await {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(error = %e, "llm_diagnostics: list_providers failed");
            return (format!("llm_diagnostics failed: {}", e), true);
        }
    };
    let models = match db::list_models(&ctx.db).await {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!(error = %e, "llm_diagnostics: list_models failed");
            return (format!("llm_diagnostics failed: {}", e), true);
        }
    };
    let default_model_id = match db::get_config_value(&ctx.db, "default_model_id").await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, "llm_diagnostics: read default_model_id failed");
            return (format!("llm_diagnostics failed: {}", e), true);
        }
    };
    (
        render(&providers, &models, default_model_id.as_deref()),
        false,
    )
}

/// Hand-built redacted view. Each field is copied explicitly — adding
/// a secret-bearing column to `ProviderRow` cannot leak into this
/// output the way a blanket serde serialize could.
fn render(
    providers: &[db::ProviderRow],
    models: &[db::ModelWithProvider],
    default_model_id: Option<&str>,
) -> String {
    let mut out = String::from("LLM configuration snapshot:\n");
    if providers.is_empty() {
        out.push_str("providers: (none configured)\n");
    } else {
        out.push_str("providers:\n");
        for p in providers {
            out.push_str(&format!(
                "- {} [id={}] protocol={} base_url={} disabled={} has_key={}\n",
                p.display_name, p.id, p.protocol, p.base_url, p.disabled, p.has_key
            ));
        }
    }
    if models.is_empty() {
        out.push_str("models: (none configured)\n");
    } else {
        out.push_str("models:\n");
        for m in models {
            out.push_str(&format!(
                "- {} [id={}] provider_id={} model_name={} context_window={} \
                 supports_images={} supports_thinking={} disabled={} provider_disabled={}\n",
                m.model.display_name,
                m.model.id,
                m.model.provider_id,
                m.model.model_name,
                m.model.context_window,
                m.model.supports_images,
                m.model.supports_thinking,
                m.model.disabled,
                m.provider_disabled,
            ));
        }
    }
    match default_model_id {
        Some(id) => out.push_str(&format!("default_model_id: {id}\n")),
        None => out.push_str("default_model_id: (unset)\n"),
    }
    let disabled = providers.iter().filter(|p| p.disabled).count()
        + models
            .iter()
            .filter(|m| m.model.disabled || m.provider_disabled)
            .count();
    let missing_key = providers.iter().filter(|p| !p.has_key).count();
    out.push_str(&format!(
        "Summary: {} provider(s), {} model(s), {disabled} disabled, {missing_key} missing key, \
         default {}",
        providers.len(),
        models.len(),
        if default_model_id.is_some() {
            "set"
        } else {
            "unset"
        }
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::db::models::create_model;
    use crate::db::providers::create_provider;

    /// Fresh in-memory pool + migrations per test — shared fixture
    /// `db::test_support::test_pool` (RULE-TESTPOOL-001, no hand-written
    /// connect+PRAGMA+migrations triple).
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

    /// Distinctive key material: if any code path serialized the raw
    /// provider row, this plaintext (or its ciphertext) would appear
    /// in the output and the redaction test below would go red.
    const SEEDED_KEY: &str = "sk-PLAINTEXT-LEAK-CANARY-0123456789";

    /// `run_migrations` seeds a default catalog (2 providers / 4
    /// models / `default_model_id` set) — wipe it so each test starts
    /// from a genuinely empty configuration (providers delete cascades
    /// to models).
    async fn clear_catalog(pool: &sqlx::SqlitePool) {
        sqlx::query("DELETE FROM providers")
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM app_config WHERE key = 'default_model_id'")
            .execute(pool)
            .await
            .unwrap();
    }

    async fn seed_provider_and_model(ctx: &ToolContext) -> (String, String) {
        let p = create_provider(
            &ctx.db,
            "openai",
            "Prov-A",
            "https://api.example.com/v1",
            SEEDED_KEY,
        )
        .await
        .unwrap();
        let m = create_model(
            &ctx.db,
            &p.id,
            "test-model-1",
            "Test Model One",
            None,
            None,
            false,
            true,
            128_000,
        )
        .await
        .unwrap();
        (p.id, m.id)
    }

    #[test]
    fn definition_has_name_and_no_required_params() {
        let def = definition();
        assert_eq!(def.name, "llm_diagnostics");
        let required = def
            .input_schema
            .get("required")
            .and_then(|v| v.as_array())
            .map(|a| a.len())
            .unwrap_or(0);
        assert_eq!(required, 0, "zero-param schema");
        assert!(
            def.description
                .as_deref()
                .unwrap_or_default()
                .contains("never contains API keys"),
            "description must state the redaction promise"
        );
    }

    #[tokio::test]
    async fn execute_empty_config_is_valid_snapshot() {
        let ctx = make_ctx().await;
        clear_catalog(&ctx.db).await;
        let (out, is_err) = execute(&serde_json::json!({}), &ctx, None).await;
        assert!(!is_err, "empty config is data, not a tool error: {out}");
        assert!(out.contains("providers: (none configured)"), "{out}");
        assert!(out.contains("models: (none configured)"), "{out}");
        assert!(out.contains("default_model_id: (unset)"), "{out}");
    }

    #[tokio::test]
    async fn execute_renders_three_sections() {
        let ctx = make_ctx().await;
        clear_catalog(&ctx.db).await;
        let (_pid, mid) = seed_provider_and_model(&ctx).await;
        crate::db::set_config_value(&ctx.db, "default_model_id", &mid)
            .await
            .unwrap();

        let (out, is_err) = execute(&serde_json::json!({}), &ctx, None).await;
        assert!(!is_err);
        // 三段齐全(prd R3.1):providers / models / default_model_id。
        assert!(out.contains("providers:"), "{out}");
        assert!(out.contains("Prov-A"), "{out}");
        assert!(out.contains("https://api.example.com/v1"), "{out}");
        assert!(out.contains("models:"), "{out}");
        assert!(out.contains("Test Model One"), "{out}");
        assert!(out.contains("model_name=test-model-1"), "{out}");
        assert!(out.contains(&format!("default_model_id: {mid}")), "{out}");
        assert!(out.contains("Summary: 1 provider(s), 1 model(s)"), "{out}");
    }

    /// AC2 脱敏断言:序列化全文既不含 `api_key` 子串(字段名层面,
    /// `api_key_enc` 亦被覆盖),也不含明文 key 与密文材料。
    #[tokio::test]
    async fn execute_output_never_contains_key_material() {
        let ctx = make_ctx().await;
        clear_catalog(&ctx.db).await;
        let (pid, mid) = seed_provider_and_model(&ctx).await;
        crate::db::set_config_value(&ctx.db, "default_model_id", &mid)
            .await
            .unwrap();

        // 直接从 DB 取密文,断言密文材料同样不出现在输出里。
        let enc: String = sqlx::query_scalar("SELECT api_key_enc FROM providers WHERE id = ?")
            .bind(&pid)
            .fetch_one(&ctx.db)
            .await
            .unwrap();
        assert!(!enc.is_empty(), "seed stored a ciphertext");

        let (out, is_err) = execute(&serde_json::json!({}), &ctx, None).await;
        assert!(!is_err);
        assert!(
            !out.contains("api_key"),
            "no api_key substring at all: {out}"
        );
        assert!(!out.contains(SEEDED_KEY), "plaintext key leaked: {out}");
        assert!(!out.contains(&enc), "ciphertext leaked: {out}");

        // 默认模型存在但 disabled 的 provider 也照实渲染(诊断信息)。
        crate::db::set_provider_disabled(&ctx.db, &pid, true)
            .await
            .unwrap();
        let (out2, _) = execute(&serde_json::json!({}), &ctx, None).await;
        assert!(out2.contains("disabled=true"), "{out2}");
    }

    #[tokio::test]
    async fn execute_counts_missing_key_and_unset_default_in_summary() {
        let ctx = make_ctx().await;
        clear_catalog(&ctx.db).await;
        let (_pid, _mid) = seed_provider_and_model(&ctx).await;
        // seed 的 provider has_key=true。再造一个无 key 行
        // (api_key_enc = '' → has_key=false)凑 missing-key 计数
        // (create_provider 对空串也会产出非空密文,所以直接落库)。
        sqlx::query(
            r#"
 INSERT INTO providers (id, protocol, display_name, base_url, api_key, api_key_enc,
                        key_migrated_at, created_at, updated_at)
 VALUES ('p-nokey', 'anthropic', 'Prov-B', 'https://b.example.com', '', '', NULL, 't', 't')
 "#,
        )
        .execute(&ctx.db)
        .await
        .unwrap();
        let (out, is_err) = execute(&serde_json::json!({}), &ctx, None).await;
        assert!(!is_err);
        assert!(out.contains("1 missing key"), "{out}");
        assert!(out.contains("has_key=false"), "{out}");
        assert!(out.contains("default unset"), "{out}");
    }
}
