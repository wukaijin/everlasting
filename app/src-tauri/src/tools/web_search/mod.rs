//! F4 `web_search` 工具 — snippet-only 网页搜索(ROADMAP F4,task
//! `08-25-web-search-tool`)。
//!
//! 两段式模型(同 Claude Code `WebSearch`):`web_search(query)` 只返回
//! 前 N 条 `title + url + snippet`,正文由模型对选中条目自行调
//! `web_fetch`——不触碰 web_fetch 的私有管线(`fetch_and_process`
//! 保持私有),也不复用它的 SSRF 面(固定域名固定 endpoint,无用户
//! 可控 URL)。
//!
//! # 后端抽象(design §2 定稿:enum dispatch)
//!
//! [`SearchBackend`] 是两 variant 的 enum 而非 trait + dyn:仓内无
//! async-trait 依赖、edition 2021 的原生 async fn in trait 不支持
//! dyn,而两 provider 用不上 `Provider` trait 那种手写 boxed stream
//! 的重形态。「留口」= 加 variant(智谱 / 博查未来照此扩)。
//!
//! # 选路与配置
//!
//! 每次 execute 时读 `app_config`(best-effort,fail-open):
//! `web_search.provider` = `auto|tavily|ddg`(缺省/读失败/未知值 →
//! `auto`);`auto` 按有无可解密的 tavily key 静态选路——有 key →
//! Tavily,无 key → DDG(零配置兜底)。失败不跨 provider 自动兜底:
//! 静默换道会把「Tavily 额度尽」掩盖成「DDG 结果质量差」,auto 的
//! 静态选路让失败原因单一。
//!
//! # 权限
//!
//! 零权限层改动:`classify_tool` 未列 → `ToolKind::Other` → Tier 5
//! 静默放行(同 `search_history` / `remember` 先例;查询词本来就要
//! 发给 LLM provider,二跳外泄增量小)。Tier 6 审计照记。
//!
//! # C7D stub
//!
//! 在 `STUB_CANDIDATES`(低频重型工具渐进披露);由此**不能**进
//! L2 并行白名单(不变量 `STUB_CANDIDATES ∩ PARALLEL_WHITELIST =
//! ∅`,`stub.rs` 单测护住;搜索通常单发,并行收益低)。

pub(crate) mod ddg;
pub(crate) mod tavily;

use std::time::Duration;

use serde::Serialize;

use crate::llm::types::ToolDef;
use crate::tools::ToolContext;
use ddg::DdgClient;
use tavily::TavilyClient;

// ---------------------------------------------------------------------------
// app_config keys(WP2 的 get/set helper 与 execute 时选路共用)
// ---------------------------------------------------------------------------

/// provider 选择:`auto | tavily | ddg`(缺省 auto)。
pub(crate) const KEY_PROVIDER: &str = "web_search.provider";
/// Tavily API key 的 AEAD 密文(crypto::encrypt(master_key, key,
/// aad = KEY_AAD))。明文不落盘、不出后端。
pub(crate) const KEY_TAVILY_API_KEY: &str = "web_search.tavily_api_key";
/// AEAD aad——把密文绑死在本配置槽位上(providers 表用 provider id
/// 作 aad 的同款先例,06-24-p1-api-key-encryption)。
pub(crate) const KEY_AAD: &str = "web_search";

// ---------------------------------------------------------------------------
// 常量
// ---------------------------------------------------------------------------

const DEFAULT_COUNT: u8 = 5;
const MAX_COUNT: u8 = 10;
/// query 长度上限(字符,非字节——CJK 友好)。
const MAX_QUERY_CHARS: usize = 400;
const TITLE_MAX_CHARS: usize = 120;
const SNIPPET_MAX_CHARS: usize = 300;

/// 整体预算:**所有**重试尝试共用(外层 `tokio::time::timeout` 单包,
/// 不为每次尝试另开 30s)。30s 与 web_fetch 默认超时对齐。
const SEARCH_TIMEOUT: Duration = Duration::from_secs(30);
/// 外包 wrapper 的额外余量(web_fetch 同款:body 读在边界上也能归类
/// 为 Timeout 而非 Network;测试注短值,否则超时路径没法单测)。
const TIMEOUT_GRACE: Duration = Duration::from_secs(5);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// 重试退避基距:delay = base * 2^attempt + jitter(0..base)。借
/// `RetryPolicy::wait` 公式概念(`retry_open` 绑定 Provider 流,
/// 不可复用)。
const RETRY_BASE_DELAY: Duration = Duration::from_millis(300);
/// 最大重试次数(总尝试 ≤ 3)。
const MAX_RETRIES: u32 = 2;

// ---------------------------------------------------------------------------
// 结果 / 错误 / 后端 enum(design §2 契约)
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub(crate) struct SearchHit {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

#[derive(Debug)]
pub(crate) enum SearchError {
    /// DDG 202 软封锁(Ratelimit,非 429 常规语义):不重试,文案
    /// 引导模型改用 web_fetch 直取已知 URL。
    RateLimited,
    /// 4xx/5xx 终态。401/432/433 = Tavily key/额度问题,渲染层按
    /// code 区分文案。
    HttpStatus(u16),
    /// 瞬时网络错:可重试。
    Network(String),
    Timeout,
    /// 页面/响应形状漂移(解析失败或 200 但 0 条结果)。保留原始
    /// 片段长度信息便于诊断。
    Parse(String),
}

/// 两 provider 的 enum dispatch(design §2:无 dyn、无新依赖)。
#[derive(Debug)]
pub(crate) enum SearchBackend {
    Tavily(TavilyClient),
    Ddg(DdgClient),
}

impl SearchBackend {
    pub(crate) fn name(&self) -> &'static str {
        match self {
            SearchBackend::Tavily(_) => "tavily",
            SearchBackend::Ddg(_) => "ddg",
        }
    }

    async fn search(&self, query: &str, count: u8) -> Result<Vec<SearchHit>, SearchError> {
        match self {
            SearchBackend::Tavily(c) => c.search(query, count).await,
            SearchBackend::Ddg(c) => c.search(query, count).await,
        }
    }
}

/// 可注入的执行参数(生产默认走 `Default`;测试把超时/退避缩到毫秒
/// 级,否则 30s 超时与数百 ms 退避路径没法单测)。
#[derive(Clone, Copy)]
pub(crate) struct SearchOpts {
    /// 所有尝试共用的整体预算。
    pub timeout: Duration,
    /// 外包 wrapper 余量。
    pub grace: Duration,
    /// 重试退避基距。
    pub retry_base_delay: Duration,
}

impl Default for SearchOpts {
    fn default() -> Self {
        Self {
            timeout: SEARCH_TIMEOUT,
            grace: TIMEOUT_GRACE,
            retry_base_delay: RETRY_BASE_DELAY,
        }
    }
}

// ---------------------------------------------------------------------------
// Tool definition
// ---------------------------------------------------------------------------

pub fn definition() -> ToolDef {
    ToolDef {
        name: "web_search".to_string(),
        description: Some(
            "Search the web and return the top results (title, url, snippet) for a \
             query. Use it for current documentation, API changes, or error-message \
             solutions. To read a result's full page, follow up with web_fetch on \
             its url. Returns up to `count` results (default 5, max 10)."
                .to_string(),
        ),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query (trimmed, max 400 chars)."
                },
                "count": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 10,
                    "default": 5,
                    "description": "Number of results to return (1-10, default 5)."
                }
            },
            "required": ["query"]
        }),
    }
}

// ---------------------------------------------------------------------------
// Execute
// ---------------------------------------------------------------------------

/// 生产入口:parse → 按 app_config 选路 → run_search → 渲染。
pub async fn execute(input: &serde_json::Value, ctx: &ToolContext) -> (String, bool) {
    let (query, count) = match parse_args(input) {
        Ok(args) => args,
        Err(e) => return (e, true),
    };
    let backend = match resolve_backend(ctx).await {
        Ok(b) => b,
        Err(e) => return (e, true),
    };
    execute_on(&backend, &query, count, SearchOpts::default()).await
}

/// 对给定后端执行(生产 `execute` 与测试共用同一路径):run_search +
/// 错误文案/渲染。
pub(crate) async fn execute_on(
    backend: &SearchBackend,
    query: &str,
    count: u8,
    opts: SearchOpts,
) -> (String, bool) {
    match run_search(backend, query, count, opts).await {
        Ok(hits) => (render_hits(&hits, query, backend.name()), false),
        Err(e) => (error_to_llm_string(&e, backend.name()), true),
    }
}

/// 测试出口:直接调内部选路(生产入口是 `execute`,它把选路与执行
/// 串在一起)。放在 `#[cfg(test)]` 下避免未使用的死代码告警。
#[cfg(test)]
pub(crate) async fn resolve_backend_for_test(ctx: &ToolContext) -> Result<SearchBackend, String> {
    resolve_backend(ctx).await
}

/// 输入校验收口:query trim + 非空 + ≤400 字符;count 手动取数
/// (`as_u64`),缺省/0/负数/类型错 → 默认 5(count 语义宽松,同
/// `search_history` 的 limit 先例),合法值 clamp 到 `1..=10`。
fn parse_args(input: &serde_json::Value) -> Result<(String, u8), String> {
    let query = input
        .get("query")
        .and_then(|v| v.as_str())
        .ok_or("web_search requires a `query` string")?
        .trim()
        .to_string();
    if query.is_empty() {
        return Err("`query` must not be empty or whitespace".to_string());
    }
    let chars = query.chars().count();
    if chars > MAX_QUERY_CHARS {
        return Err(format!(
            "`query` is {} chars (max {MAX_QUERY_CHARS}); use a more focused search phrase",
            chars
        ));
    }
    let count = match input.get("count").and_then(|v| v.as_u64()) {
        Some(n) if n >= 1 => u8::try_from(n).unwrap_or(MAX_COUNT).min(MAX_COUNT),
        // Absent, 0, negative, or non-integer → default. Not an error.
        _ => DEFAULT_COUNT,
    };
    Ok((query, count))
}

/// 按 app_config 选路(fail-open:读失败 → auto;key 解密失败 → 视为
/// 无 key,同 `tools_stub_enabled` 的读法先例)。auto 只按 key 有无
/// **静态**选路,失败不跨 provider 兜底。
async fn resolve_backend(ctx: &ToolContext) -> Result<SearchBackend, String> {
    let provider = crate::db::get_config_value(&ctx.db, KEY_PROVIDER)
        .await
        .ok()
        .flatten()
        .unwrap_or_default();
    let key = stored_tavily_key(&ctx.db).await;
    match (provider.as_str(), key) {
        ("tavily", Some(k)) => Ok(SearchBackend::Tavily(TavilyClient::new(
            &k,
            tavily::DEFAULT_BASE_URL,
            CONNECT_TIMEOUT,
        ))),
        ("tavily", None) => Err(
            "web_search: provider is set to `tavily` but no Tavily API key is stored. \
             Open Settings → Web 搜索 to add one (a free key works), or switch the \
             provider to `auto`/`ddg`."
                .to_string(),
        ),
        ("ddg", _) => Ok(SearchBackend::Ddg(DdgClient::new(
            ddg::DEFAULT_BASE_URL,
            CONNECT_TIMEOUT,
        ))),
        // auto / 未知值 / 读失败 → 按 key 有无静态选路。
        (_, Some(k)) => Ok(SearchBackend::Tavily(TavilyClient::new(
            &k,
            tavily::DEFAULT_BASE_URL,
            CONNECT_TIMEOUT,
        ))),
        (_, None) => Ok(SearchBackend::Ddg(DdgClient::new(
            ddg::DEFAULT_BASE_URL,
            CONNECT_TIMEOUT,
        ))),
    }
}

/// 读出并解密已存的 tavily key。密文行缺失/为空/解密失败(如
/// machine-id 变了)→ `None`——选路视为无 key,静默回落 DDG。
async fn stored_tavily_key(pool: &sqlx::SqlitePool) -> Option<String> {
    let enc = crate::db::get_config_value(pool, KEY_TAVILY_API_KEY)
        .await
        .ok()
        .flatten()?;
    if enc.is_empty() {
        return None;
    }
    let master_key = crate::crypto::derive_master_key().ok()?;
    crate::crypto::decrypt(&master_key, &enc, KEY_AAD)
        .ok()
        .filter(|k| !k.is_empty())
}

// ---------------------------------------------------------------------------
// 配置 get / set helper(WP2;commands/config.rs 与 daemon route 共用)
// ---------------------------------------------------------------------------

/// `get_web_search_config` 返回体。**明文 key 永不回前端**——只有
/// masked 形态(`tvly-****` + 后 4 位,同 providers key 展示策略;新面
/// 不扩大旧债)。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebSearchConfigPayload {
    /// `auto | tavily | ddg`(存储值;未存/空 → `auto`)。
    pub provider: String,
    /// 密文行存在且非空(解密失败也算 set——行在,只是不可预览)。
    pub tavily_key_set: bool,
    /// masked key,仅当密文可解密时返回。
    pub tavily_key_masked: Option<String>,
}

/// 读配置。DB 读失败向上抛(配置面板要真错误,不 fail-open——与
/// execute 时选路的 best-effort 读法有意相反)。
pub async fn get_config_state(
    pool: &sqlx::SqlitePool,
) -> Result<WebSearchConfigPayload, sqlx::Error> {
    let provider = crate::db::get_config_value(pool, KEY_PROVIDER)
        .await?
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "auto".to_string());
    let enc = crate::db::get_config_value(pool, KEY_TAVILY_API_KEY)
        .await?
        .unwrap_or_default();
    let tavily_key_set = !enc.is_empty();
    // masked 需要明文前 5 + 后 4 字符:解密失败 → None(行仍在,
    // tavily_key_set 照实为 true)。
    let tavily_key_masked = if tavily_key_set {
        crate::crypto::derive_master_key()
            .ok()
            .and_then(|mk| crate::crypto::decrypt(&mk, &enc, KEY_AAD).ok())
            .map(|k| mask_key(&k))
    } else {
        None
    };
    Ok(WebSearchConfigPayload {
        provider,
        tavily_key_set,
        tavily_key_masked,
    })
}

/// `tvly-****` + 后 4 位(design §5)。短 key(≤9 字符)整体打码,
/// 不泄露可拼接的片段。
fn mask_key(key: &str) -> String {
    let chars: Vec<char> = key.chars().collect();
    if chars.len() < 10 {
        return "****".to_string();
    }
    let head: String = chars[..5].iter().collect();
    let tail: String = chars[chars.len() - 4..].iter().collect();
    format!("{head}****{tail}")
}

/// 写配置。provider 三值校验(错值拒绝,**不写库**);key 三态语义
/// (review P1-2):
/// - `Some(非空)` → AEAD 重加密落盘;
/// - `Some("")` → **清除**(删 `app_config` 行——切 ddg 停用后残留
///   密文不会在切回 auto 时静默复活 Tavily);
/// - `None` → 不动(前端「留空 = 不改」)。
pub async fn set_config_state(
    pool: &sqlx::SqlitePool,
    provider: &str,
    tavily_api_key: Option<&str>,
) -> Result<(), String> {
    if !matches!(provider, "auto" | "tavily" | "ddg") {
        return Err(format!(
            "invalid provider '{provider}' (expected \"auto\", \"tavily\", or \"ddg\")"
        ));
    }
    match tavily_api_key {
        Some(k) if !k.trim().is_empty() => {
            let master_key = crate::crypto::derive_master_key()
                .map_err(|e| format!("derive master key failed: {e}"))?;
            let enc = crate::crypto::encrypt(&master_key, k.trim(), KEY_AAD)
                .map_err(|e| format!("encrypt tavily key failed: {e}"))?;
            crate::db::set_config_value(pool, KEY_TAVILY_API_KEY, &enc)
                .await
                .map_err(|e| format!("store tavily key failed: {e}"))?;
        }
        Some(_) => {
            // 空串 = 显式清除动作。
            crate::db::delete_config_value(pool, KEY_TAVILY_API_KEY)
                .await
                .map_err(|e| format!("clear tavily key failed: {e}"))?;
        }
        None => { /* 不动 */ }
    }
    crate::db::set_config_value(pool, KEY_PROVIDER, provider)
        .await
        .map_err(|e| format!("store provider failed: {e}"))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// 重试环(design §7):所有尝试共用一个外层 `tokio::time::timeout`
// (整体预算,不为每次尝试另开)。只对 `Network(_)` 与 429/5xx 重试;
// 退避 `base * 2^n + jitter(0..base)`。
// ---------------------------------------------------------------------------

pub(crate) async fn run_search(
    backend: &SearchBackend,
    query: &str,
    count: u8,
    opts: SearchOpts,
) -> Result<Vec<SearchHit>, SearchError> {
    tokio::time::timeout(opts.timeout + opts.grace, async {
        let mut attempt: u32 = 0;
        loop {
            match backend.search(query, count).await {
                Ok(hits) => return Ok(hits),
                Err(e) if attempt < MAX_RETRIES && is_retryable(&e) => {
                    let delay = backoff_delay(opts.retry_base_delay, attempt);
                    tracing::warn!(
                        provider = backend.name(),
                        attempt,
                        delay_ms = delay.as_millis() as u64,
                        error = ?e,
                        "web_search: transient failure, retrying"
                    );
                    tokio::time::sleep(delay).await;
                    attempt += 1;
                }
                Err(e) => return Err(e),
            }
        }
    })
    .await
    .unwrap_or(Err(SearchError::Timeout))
}

fn is_retryable(e: &SearchError) -> bool {
    match e {
        SearchError::Network(_) => true,
        SearchError::HttpStatus(c) => *c == 429 || (500..=599).contains(c),
        SearchError::RateLimited | SearchError::Timeout | SearchError::Parse(_) => false,
    }
}

/// `base * 2^attempt + jitter(0..base)`。jitter 用亚毫秒时钟做伪随机
/// 来源——不值得为此引 rand 依赖。
fn backoff_delay(base: Duration, attempt: u32) -> Duration {
    let exp = base.saturating_mul(1u32 << attempt.min(6));
    let jitter = Duration::from_nanos(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos() as u64 % base.as_nanos() as u64)
            .unwrap_or(0),
    );
    exp + jitter
}

// ---------------------------------------------------------------------------
// 渲染(纯文本,进 tool_result)
// ---------------------------------------------------------------------------

/// 按字符截断(UTF-8 边界安全;`chars().take()` 天然落在字符边界)。
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut t: String = s.chars().take(max).collect();
    t.push('…');
    t
}

/// 渲染结果集。attribution 注释放**尾部**——本工具结果集自截断(每条
/// title/snippet 已限长),不存在整体被截掉尾行的风险(web_fetch 头部
/// 前缀是为防整体截断,此处无需同款防御)。
fn render_hits(hits: &[SearchHit], query: &str, provider: &str) -> String {
    if hits.is_empty() {
        return format!(
            "No results for \"{}\" via {}. Try a more specific query, or if you know a \
             relevant site, web_fetch its URL directly.",
            query, provider
        );
    }
    let mut out = String::new();
    for (i, h) in hits.iter().enumerate() {
        out.push_str(&format!(
            "{}. {}\n   {}\n   {}\n",
            i + 1,
            truncate_chars(h.title.trim(), TITLE_MAX_CHARS),
            h.url.trim(),
            truncate_chars(h.snippet.trim(), SNIPPET_MAX_CHARS)
        ));
    }
    out.push_str(&format!(
        "\n<!-- searched: \"{}\" via {} at {} · {} results -->",
        query,
        provider,
        chrono::Utc::now().to_rfc3339(),
        hits.len()
    ));
    out
}

/// 把内部错误翻成模型可操作的文案(可操作 > 精确:模型读完能自己换
/// 路径)。401/432/433 是 Tavily 专属 key/额度语义,按 code 区分。
fn error_to_llm_string(e: &SearchError, provider: &str) -> String {
    match e {
        // DDG 专属终态:文案固定,不随 provider 插值(其他 provider 不会
        // 产生 202)。
        SearchError::RateLimited => concat!(
            "web_search: DuckDuckGo soft-blocked this client (HTTP 202 rate limit; ",
            "NOT retried — retrying makes the block worse). Wait a while before ",
            "searching again, or if you already know a relevant URL, read it ",
            "directly with web_fetch."
        )
        .to_string(),
        SearchError::HttpStatus(code) => match code {
            401 => format!(
                "web_search: {provider} rejected the API key (HTTP 401). Check the key \
                 in Settings → Web 搜索 — a stale or mistyped key will fail every call."
            ),
            432 => format!(
                "web_search: {provider} free quota is exhausted (HTTP 432). Add a \
                 payment method or wait for the monthly reset; meanwhile switch the \
                 provider to `auto`/`ddg` in Settings, or web_fetch known URLs directly."
            ),
            433 => format!(
                "web_search: {provider} pay-as-you-go spending limit reached (HTTP 433). \
                 Raise the limit or switch provider to `auto`/`ddg` in Settings."
            ),
            other => format!("web_search: {provider} returned HTTP {other}."),
        },
        SearchError::Network(msg) => format!(
            "web_search: network error reaching {provider}: {msg}. If a proxy is \
             required for this provider it may be down; try again later or web_fetch a \
             known URL directly."
        ),
        SearchError::Timeout => {
            "web_search: request timed out. Try a shorter query or retry later.".to_string()
        }
        SearchError::Parse(detail) => format!(
            "web_search: {provider} response shape was not recognized (possible \
             upstream page-format drift; {detail}). Try again later; if you know a \
             relevant URL, web_fetch it directly."
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- definition ----

    #[test]
    fn definition_has_correct_name_and_required_query_only() {
        let def = definition();
        assert_eq!(def.name, "web_search");
        let required: Vec<&str> = def
            .input_schema
            .get("required")
            .and_then(|v| v.as_array())
            .expect("required array")
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert_eq!(required, vec!["query"]);
        assert_eq!(
            def.input_schema
                .pointer("/properties/count/maximum")
                .and_then(|v| v.as_u64()),
            Some(MAX_COUNT as u64)
        );
    }

    // ---- parse_args(AC5:count 三态 + query 长度)----

    #[test]
    fn parse_args_defaults_count_5() {
        let (q, c) = parse_args(&serde_json::json!({"query": "rust async"})).unwrap();
        assert_eq!(q, "rust async");
        assert_eq!(c, DEFAULT_COUNT);
    }

    #[test]
    fn parse_args_count_invalid_falls_back_and_clamps() {
        // 0 / 负数 / 类型错 → 默认 5(语义宽松,不改搜索方向)
        for bad in [0u64, 999u64, 0u64] {
            let (_, c) = parse_args(&serde_json::json!({"query": "x", "count": bad})).unwrap();
            if bad == 0 {
                assert_eq!(c, DEFAULT_COUNT);
            }
        }
        let (_, c) = parse_args(&serde_json::json!({"query": "x", "count": "five"})).unwrap();
        assert_eq!(c, DEFAULT_COUNT);
        // 负数经 as_u64 → None → 默认
        let (_, c) = parse_args(&serde_json::json!({"query": "x", "count": -3})).unwrap();
        assert_eq!(c, DEFAULT_COUNT);
        // 合法值 clamp(1,10)
        let (_, c) = parse_args(&serde_json::json!({"query": "x", "count": 99})).unwrap();
        assert_eq!(c, MAX_COUNT);
        let (_, c) = parse_args(&serde_json::json!({"query": "x", "count": 3})).unwrap();
        assert_eq!(c, 3);
    }

    #[test]
    fn parse_args_rejects_empty_and_overlong_query() {
        assert!(parse_args(&serde_json::json!({"query": "  "})).is_err());
        assert!(parse_args(&serde_json::json!({})).is_err());
        let long = "x".repeat(MAX_QUERY_CHARS + 1);
        let err = parse_args(&serde_json::json!({"query": long})).unwrap_err();
        assert!(err.contains("400"), "error states the cap: {err}");
        // 恰好 400 通过;字符计数(非字节)——4 字中文按 4 计。
        let ok = "x".repeat(MAX_QUERY_CHARS);
        assert!(parse_args(&serde_json::json!({"query": ok})).is_ok());
        let cjk = "汉".repeat(MAX_QUERY_CHARS);
        assert!(parse_args(&serde_json::json!({"query": cjk})).is_ok());
    }

    // ---- 渲染截断 + attribution(AC5)----

    #[test]
    fn render_truncates_title_and_snippet() {
        let hits = vec![SearchHit {
            title: "T".repeat(300),
            url: "https://example.com/a".to_string(),
            snippet: "S".repeat(1000),
        }];
        let out = render_hits(&hits, "q", "ddg");
        assert!(out.contains(&format!("1. {}…", "T".repeat(TITLE_MAX_CHARS))));
        assert!(out.contains(&format!("   {}…", "S".repeat(SNIPPET_MAX_CHARS))));
        assert!(out.contains("https://example.com/a"));
        assert!(out.contains("<!-- searched: \"q\" via ddg at "));
        assert!(out.ends_with("· 1 results -->"));
    }

    #[test]
    fn render_empty_hits_gives_actionable_hint() {
        let out = render_hits(&[], "q", "tavily");
        assert!(out.contains("No results"), "{out}");
        assert!(out.contains("web_fetch"), "{out}");
    }

    // ---- 错误文案(AC5:202 含 web_fetch 引导)----

    #[test]
    fn rate_limited_error_mentions_web_fetch() {
        let s = error_to_llm_string(&SearchError::RateLimited, "ddg");
        assert!(s.contains("web_fetch"), "{s}");
        assert!(s.contains("202"), "{s}");
    }

    #[test]
    fn tavily_http_error_codes_get_distinct_copy() {
        assert!(error_to_llm_string(&SearchError::HttpStatus(401), "tavily").contains("API key"));
        assert!(error_to_llm_string(&SearchError::HttpStatus(432), "tavily").contains("quota"));
        assert!(error_to_llm_string(&SearchError::HttpStatus(433), "tavily").contains("limit"));
        assert!(error_to_llm_string(&SearchError::HttpStatus(500), "tavily").contains("500"));
    }

    // ---- 重试分类 ----

    #[test]
    fn retryable_classification() {
        assert!(is_retryable(&SearchError::Network("conn reset".into())));
        assert!(is_retryable(&SearchError::HttpStatus(429)));
        assert!(is_retryable(&SearchError::HttpStatus(503)));
        assert!(!is_retryable(&SearchError::HttpStatus(401)));
        assert!(!is_retryable(&SearchError::HttpStatus(400)));
        assert!(!is_retryable(&SearchError::RateLimited));
        assert!(!is_retryable(&SearchError::Timeout));
        assert!(!is_retryable(&SearchError::Parse("0 results".into())));
    }

    // ---- 配置 get / set(AC4:key 三态 + masked + 三值校验)----

    async fn cfg_pool() -> sqlx::SqlitePool {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        crate::db::migrations::run_migrations(&pool).await.unwrap();
        pool
    }

    #[tokio::test]
    async fn set_provider_validates_three_values() {
        let pool = cfg_pool().await;
        for ok in ["auto", "tavily", "ddg"] {
            set_config_state(&pool, ok, None).await.unwrap();
        }
        let err = set_config_state(&pool, "brave", None).await.unwrap_err();
        assert!(err.contains("brave"), "{err}");
        // 拒绝时不写库
        let st = get_config_state(&pool).await.unwrap();
        assert_eq!(st.provider, "ddg", "上次合法值保留");
    }

    #[tokio::test]
    async fn key_three_state_semantics() {
        let pool = cfg_pool().await;
        // 初始:无 key、provider 默认 auto
        let st = get_config_state(&pool).await.unwrap();
        assert_eq!(st.provider, "auto");
        assert!(!st.tavily_key_set);
        assert!(st.tavily_key_masked.is_none());

        // Some(非空):加密落盘 + masked 回显
        set_config_state(&pool, "auto", Some("tvly-abcdEFGH1234"))
            .await
            .unwrap();
        let st = get_config_state(&pool).await.unwrap();
        assert!(st.tavily_key_set);
        assert_eq!(st.tavily_key_masked.as_deref(), Some("tvly-****1234"));
        // DB 中无明文(sqlite3 层面验证在 route 测试 / AC4 冒烟)
        let raw = crate::db::get_config_value(&pool, KEY_TAVILY_API_KEY)
            .await
            .unwrap()
            .unwrap();
        assert!(!raw.contains("tvly-abcd"), "密文不得含明文片段: {raw}");

        // None:不动(key 仍在)
        set_config_state(&pool, "ddg", None).await.unwrap();
        let st = get_config_state(&pool).await.unwrap();
        assert_eq!(st.provider, "ddg");
        assert!(st.tavily_key_set, "None = 不动,不清 key");

        // Some(""):显式清除 → 行删除
        set_config_state(&pool, "ddg", Some("")).await.unwrap();
        let st = get_config_state(&pool).await.unwrap();
        assert!(!st.tavily_key_set);
        assert!(st.tavily_key_masked.is_none());
        assert!(
            crate::db::get_config_value(&pool, KEY_TAVILY_API_KEY)
                .await
                .unwrap()
                .is_none(),
            "清除 = 删行,非空串占位"
        );
    }

    #[test]
    fn mask_key_shapes() {
        assert_eq!(mask_key("tvly-abcdEFGH1234"), "tvly-****1234");
        assert_eq!(mask_key("short"), "****"); // ≤9 字符整体打码
    }
}
