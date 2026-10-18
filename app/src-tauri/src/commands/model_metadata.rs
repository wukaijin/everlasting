//! 模型探测 + 在线模型能力匹配(2026-10-19, task
//! `10-19-merge-provider-model-settings`)。
//!
//! 两条 IPC(双形态:`_inner` 单源 + daemon route,见
//! `daemon/routes/providers.rs`;Q0 单源约定):
//!
//! 1. [`probe_provider_models_inner`] — **模型探测**:按 provider 的协议
//!    调它的模型列表端点(anthropic `GET /v1/models`、openai /
//!    openai_responses `GET /models`),返回可选模型清单(含线上元数据
//!    富化,尽力而为)。前端「探测模型」面板据此一键导入,替代手打
//!    model name。
//! 2. [`lookup_model_metadata_inner`] — **按名字自动匹配能力**:查
//!    `https://models.dev/api.json`(公开目录,232 供应商 / 9000+ 模型)
//!    取上下文窗口、最大输出、vision、thinking 与思考挡位。纯在线查询
//!    (设计决策:不内置静态表,避免发行版陈旧);daemon 侧落盘缓存
//!    `{app_data_dir}/cache/models-dev.json`(TTL 24h),网络失败时降级
//!    用过期缓存(响应 `stale: true`)。
//!
//! 匹配算法是纯函数([`ModelsCatalog::matches`] / [`score_entry`] /
//! [`normalize_model_name`]),离线单测锁定排序语义;网络面只有薄壳。
//! 正常失败(模型列表端点 4xx / 目录拉取失败)一律返回
//! `{success:false, error}` 的 JSON,不向 IPC 边界抛 `Err` —— 与
//! `test_model` 的错误面约定一致。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use serde::Serialize;
use serde_json::Value;

use crate::db;
use crate::error::AppCommandError;
use crate::state::AppState;

/// 在线模型目录(公开 JSON,token 作为供应商 key、模型作为条目)。
pub const MODELS_DEV_URL: &str = "https://models.dev/api.json";

/// 落盘缓存 TTL:目录更新频率以天计,24h 足够且避免每次点击都拉 5.8MB。
pub const CACHE_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// 单次匹配返回的最大候选数(前端下拉选择用)。
const MAX_MATCHES: usize = 8;

/// 探测列表的返回上限(OpenRouter 等聚合网关可返回数百个)。
const MAX_PROBED_MODELS: usize = 300;

// ---------------------------------------------------------------------------
// Wire types
// ---------------------------------------------------------------------------

/// 一条模型能力描述(models.dev 的归一化投影)。
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelSpec {
    /// models.dev 供应商 key(如 `anthropic` / `zai`)。
    pub provider_key: String,
    /// 供应商显示名(如 `Anthropic` / `Z.AI`)。
    pub provider_name: String,
    /// 目录内的模型 id(如 `claude-sonnet-4-5`)。
    pub model_id: String,
    /// 目录内的可读名(如 `Claude Sonnet 4.5 (latest)`)。
    pub name: String,
    /// 上下文窗口;目录缺字段时为 None。
    pub context_window: Option<u64>,
    /// 最大输出 token;目录缺字段时为 None。
    pub max_output_tokens: Option<u64>,
    /// 输入模态含 image(或旧 schema 的 attachment=true)。
    pub supports_vision: bool,
    /// 目录标注 reasoning=true。
    pub supports_thinking: bool,
    /// reasoning_options 中 type=effort 的取值集合(无则空)。
    pub thinking_levels: Vec<String>,
    /// 发布日(目录原样,ISO 日期字符串)。
    pub release_date: Option<String>,
}

/// 探测到的一个供应商侧模型。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbedModel {
    /// 供应商返回的 model id(接入后就是 `models.model_name`)。
    pub model_name: String,
    /// 供应商返回的显示名(通常等于 id)。
    pub display_name: String,
    /// 在线目录匹配到的能力;未匹配到为 None(前端按默认值导入)。
    pub spec: Option<ModelSpec>,
}

// ---------------------------------------------------------------------------
// 目录索引(纯逻辑)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Entry {
    provider_key: String,
    provider_name: String,
    priority: u8,
    norm_id: String,
    norm_name: String,
    spec: ModelSpec,
}

/// models.dev 目录的内存索引。构建一次,查询 O(n);探测批量查询先走
/// 精确表(供应商模型 id 通常与目录 id 完全一致),命中即免全表扫描。
#[derive(Debug, Default)]
pub struct ModelsCatalog {
    entries: Vec<Entry>,
    /// 归一化 id / name → entry 下标(精确命中快路径)。
    exact: HashMap<String, Vec<usize>>,
}

impl ModelsCatalog {
    /// 从 `api.json` 的根对象构建索引。结构不符时返回空目录(调用方
    /// 得到「未匹配」而不是崩溃)。
    pub fn from_json(root: &Value) -> Self {
        let mut catalog = ModelsCatalog::default();
        let Some(providers) = root.as_object() else {
            return catalog;
        };
        for (provider_key, provider_val) in providers {
            let provider_name = provider_val
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or(provider_key)
                .to_string();
            let priority = provider_priority(provider_key);
            let Some(models) = provider_val.get("models").and_then(Value::as_object) else {
                continue;
            };
            for (map_key, model_val) in models {
                let model_id = model_val
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or(map_key)
                    .to_string();
                let name = model_val
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or(&model_id)
                    .to_string();
                let context_window = model_val.pointer("/limit/context").and_then(Value::as_u64);
                let max_output_tokens = model_val.pointer("/limit/output").and_then(Value::as_u64);
                let supports_vision = match model_val
                    .pointer("/modalities/input")
                    .and_then(Value::as_array)
                {
                    Some(inputs) => inputs.iter().any(|m| m.as_str() == Some("image")),
                    // 旧 schema 只有 attachment 布尔(含图片/文件附件)。
                    None => model_val
                        .get("attachment")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                };
                let supports_thinking = model_val
                    .get("reasoning")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let mut thinking_levels: Vec<String> = Vec::new();
                if let Some(options) = model_val.get("reasoning_options").and_then(Value::as_array)
                {
                    for option in options {
                        if option.get("type").and_then(Value::as_str) != Some("effort") {
                            continue;
                        }
                        if let Some(values) = option.get("values").and_then(Value::as_array) {
                            thinking_levels = values
                                .iter()
                                .filter_map(|v| v.as_str().map(str::to_string))
                                .collect();
                            break;
                        }
                    }
                }
                let release_date = model_val
                    .get("release_date")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                let spec = ModelSpec {
                    provider_key: provider_key.clone(),
                    provider_name: provider_name.clone(),
                    model_id: model_id.clone(),
                    name: name.clone(),
                    context_window,
                    max_output_tokens,
                    supports_vision,
                    supports_thinking,
                    thinking_levels,
                    release_date,
                };
                let idx = catalog.entries.len();
                let norm_id = normalize_model_name(&model_id);
                let norm_name = normalize_model_name(&name);
                if !norm_id.is_empty() {
                    catalog.exact.entry(norm_id.clone()).or_default().push(idx);
                }
                if !norm_name.is_empty() && norm_name != norm_id {
                    catalog
                        .exact
                        .entry(norm_name.clone())
                        .or_default()
                        .push(idx);
                }
                catalog.entries.push(Entry {
                    provider_key: provider_key.clone(),
                    provider_name: provider_name.clone(),
                    priority,
                    norm_id,
                    norm_name,
                    spec,
                });
            }
        }
        catalog
    }

    /// 目录条目数(诊断 / 测试用;生产路径不读)。
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 空目录判定(诊断 / 测试用;生产路径不读)。
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 单条最佳匹配(探测批量富化用):先精确表,再全表打分。
    pub fn best(&self, query: &str, provider_hint: Option<&str>) -> Option<ModelSpec> {
        let target = normalize_model_name(query);
        if target.is_empty() {
            return None;
        }
        if let Some(indices) = self.exact.get(&target) {
            return indices
                .iter()
                .map(|&i| &self.entries[i])
                .min_by(|a, b| {
                    entry_bonus(b, provider_hint)
                        .cmp(&entry_bonus(a, provider_hint))
                        .then_with(|| a.priority.cmp(&b.priority))
                        .then_with(|| a.provider_key.cmp(&b.provider_key))
                })
                .map(|e| e.spec.clone());
        }
        self.matches(query, provider_hint, 1).into_iter().next()
    }

    /// 候选匹配(自动匹配下拉):精确命中优先,否则全文打分取前
    /// `limit` 个;同一模型 id 跨供应商去重 —— 同分时一方厂商(见
    /// [`provider_priority`])胜出。去重必须按 priority 而非「先到先得」:
    /// models.dev 的 JSON 对象遍历顺序任意,`anthropic/claude-sonnet-4-5`
    /// (聚合网关条目)与 `claude-sonnet-4-5`(官方条目)归一化后同 key
    /// 同分,否则最终 context window 会随机取到网关口径。
    pub fn matches(
        &self,
        query: &str,
        provider_hint: Option<&str>,
        limit: usize,
    ) -> Vec<ModelSpec> {
        let target = normalize_model_name(query);
        if target.is_empty() || limit == 0 {
            return Vec::new();
        }
        // key → (score, priority, entry)。
        let mut best_by_id: HashMap<String, (u32, u8, &Entry)> = HashMap::new();
        for entry in &self.entries {
            let base = score_entry(&entry.norm_id, &entry.norm_name, &target);
            if base == 0 {
                continue;
            }
            let score = base + entry_bonus(entry, provider_hint);
            let key = if entry.norm_id.is_empty() {
                entry.norm_name.clone()
            } else {
                entry.norm_id.clone()
            };
            match best_by_id.get(&key) {
                Some((prev_score, prev_priority, _))
                    if (*prev_score, *prev_priority) <= (score, entry.priority) => {}
                _ => {
                    best_by_id.insert(key, (score, entry.priority, entry));
                }
            }
        }
        let mut ranked: Vec<(u32, u8, &Entry)> = best_by_id.into_values().collect();
        ranked.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then_with(|| a.1.cmp(&b.1))
                .then_with(|| a.2.provider_name.cmp(&b.2.provider_name))
                .then_with(|| a.2.spec.model_id.cmp(&b.2.spec.model_id))
        });
        ranked
            .into_iter()
            .take(limit)
            .map(|(_, _, e)| e.spec.clone())
            .collect()
    }
}

/// 归一化模型名:小写、去供应商前缀(`anthropic/xxx` → `xxx`)、去括号
/// 注解(`(latest)`)、`_` `:` `.` `@` 空格统一成 `-`、折叠重复分隔符、
/// 去首尾 `-` 与 `-latest` 后缀。两侧(目录 / 用户输入)共用同一函数。
///
/// `@` 也归一(`claude-sonnet-4-5@20250929` = `claude-sonnet-4-5-20250929`)
/// —— Vertex 系目录条目用 `@` 拼日期,不归一会在候选里和同一模型的 `-`
/// 拼法重复出现。
pub fn normalize_model_name(raw: &str) -> String {
    let lower = raw.trim().to_lowercase();
    let last = lower.rsplit('/').next().unwrap_or(&lower);
    // 去括号注解: "claude sonnet 4.5 (latest)" → "claude sonnet 4.5"
    let mut cleaned = String::with_capacity(last.len());
    let mut depth = 0usize;
    for ch in last.chars() {
        match ch {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            _ if depth == 0 => cleaned.push(ch),
            _ => {}
        }
    }
    let replaced: String = cleaned
        .chars()
        .map(|c| match c {
            '_' | ':' | '.' | '@' | ' ' => '-',
            other => other,
        })
        .collect();
    let trimmed = replaced.trim_matches('-');
    let mut collapsed = String::with_capacity(trimmed.len());
    for ch in trimmed.chars() {
        if ch == '-' && collapsed.ends_with('-') {
            continue;
        }
        collapsed.push(ch);
    }
    let without_latest = collapsed.strip_suffix("-latest").unwrap_or(&collapsed);
    without_latest.trim_matches('-').to_string()
}

/// 目录条目的优先级(0 最高):一方厂商优先于聚合网关,同分时排序用。
fn provider_priority(key: &str) -> u8 {
    const ORDER: &[&str] = &[
        "anthropic",
        "openai",
        "google",
        "google-vertex",
        "deepseek",
        "zai",
        "zhipuai",
        "moonshotai",
        "moonshotai-cn",
        "xai",
        "mistral",
        "alibaba",
        "alibaba-cn",
        "minimax",
        "minimax-cn",
        "meta",
        "cohere",
        "groq",
        "opencode",
    ];
    match ORDER.iter().position(|k| *k == key) {
        Some(i) => i as u8,
        None => ORDER.len() as u8,
    }
}

/// provider 线索加成:同一接入供应商的条目优先(例如用户配的是 z.ai
/// 网关,匹配目录里 zai 的 GLM 条目)。
fn entry_bonus(entry: &Entry, provider_hint: Option<&str>) -> u32 {
    match provider_hint {
        Some(hint) if entry.provider_key == hint => 6,
        _ => 0,
    }
}

/// 打分(0 = 不匹配)。规则依次:
/// - 目录 id 归一化后与输入完全相同 → 100
/// - 目录可读名归一化后相同 → 96
/// - 输入 = 目录 id + 日期/版本类后缀(`-20250929` / `-latest`…) → 86
///   (用户粘贴了带日期后缀的线上 id)
/// - 目录 id = 输入 + 任意后缀(用户只打了前缀)→ 72
/// - 输入(≥5 字符)出现在目录 id 中 → 55
pub fn score_entry(entry_norm_id: &str, entry_norm_name: &str, target: &str) -> u32 {
    if entry_norm_id.is_empty() && entry_norm_name.is_empty() {
        return 0;
    }
    if entry_norm_id == target {
        return 100;
    }
    if !entry_norm_name.is_empty() && entry_norm_name == target {
        return 96;
    }
    if let Some(rest) = target
        .strip_prefix(entry_norm_id)
        .and_then(|r| r.strip_prefix('-'))
    {
        if is_variant_suffix(rest) {
            return 86;
        }
    }
    if target.chars().count() >= 4 {
        if let Some(_rest) = entry_norm_id
            .strip_prefix(target)
            .and_then(|r| r.strip_prefix('-'))
        {
            return 72;
        }
    }
    if target.chars().count() >= 5 && entry_norm_id.contains(target) {
        return 55;
    }
    0
}

/// 输入尾部多出来的段是否像「日期 / 快照 / 变体」后缀。只有这类后缀
/// 才允许前缀匹配,避免 `gpt-4o-mini`(精确命中缺席时)误配 `gpt-4o`。
fn is_variant_suffix(rest: &str) -> bool {
    if rest.is_empty() {
        return false;
    }
    if rest.chars().all(|c| c.is_ascii_digit() || c == '-') {
        return true;
    }
    matches!(
        rest,
        "latest" | "preview" | "thinking" | "instruct" | "reasoning" | "high" | "chat"
    )
}

/// 从 provider 的 base_url / 协议推断 models.dev 供应商 key。仅作匹配
/// 排序线索(不参与过滤),识别不出就返回协议默认值或 None。
pub fn guess_provider_key(base_url: &str, protocol: &str) -> Option<String> {
    let lower = base_url.to_lowercase();
    const HOSTS: &[(&str, &str)] = &[
        ("api.anthropic.com", "anthropic"),
        ("api.openai.com", "openai"),
        ("api.z.ai", "zai"),
        ("open.bigmodel.cn", "zhipuai"),
        ("api.deepseek.com", "deepseek"),
        ("openrouter.ai", "openrouter"),
        ("api.moonshot.cn", "moonshotai-cn"),
        ("api.moonshot.ai", "moonshotai"),
        ("api.siliconflow.cn", "siliconflow-cn"),
        ("api.siliconflow.com", "siliconflow"),
        ("dashscope.aliyuncs.com", "alibaba-cn"),
        ("dashscope-intl.aliyuncs.com", "alibaba"),
        ("generativelanguage.googleapis.com", "google"),
        ("api.x.ai", "xai"),
        ("api.mistral.ai", "mistral"),
        ("api.groq.com", "groq"),
        ("api.together.xyz", "togetherai"),
        ("api.fireworks.ai", "fireworks-ai"),
        ("ark.cn-beijing.volces.com", "volcengine"),
        ("api.minimax.chat", "minimax"),
    ];
    for (needle, key) in HOSTS {
        if lower.contains(needle) {
            return Some((*key).to_string());
        }
    }
    match protocol {
        "anthropic" => Some("anthropic".to_string()),
        "openai" | "openai_responses" => Some("openai".to_string()),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// 目录加载(落盘缓存 + 网络)
// ---------------------------------------------------------------------------

struct LoadedCatalog {
    catalog: ModelsCatalog,
    fetched_at: String,
    cached: bool,
    stale: bool,
}

fn cache_path(state: &Arc<AppState>) -> std::path::PathBuf {
    state.app_data_dir.join("cache").join("models-dev.json")
}

fn rfc3339(ts: SystemTime) -> String {
    chrono::DateTime::<chrono::Utc>::from(ts).to_rfc3339()
}

/// 读缓存。`require_fresh = true` 时超过 TTL 视为缺失。
async fn read_cache(path: &std::path::Path, require_fresh: bool) -> Option<(Value, String)> {
    let meta = tokio::fs::metadata(path).await.ok()?;
    let modified = meta.modified().ok()?;
    if require_fresh {
        let age = SystemTime::now()
            .duration_since(modified)
            .unwrap_or_default();
        if age > CACHE_TTL {
            return None;
        }
    }
    let bytes = tokio::fs::read(path).await.ok()?;
    let json: Value = serde_json::from_slice(&bytes).ok()?;
    Some((json, rfc3339(modified)))
}

async fn write_cache(path: &std::path::Path, json: &Value) {
    let Some(parent) = path.parent() else { return };
    if tokio::fs::create_dir_all(parent).await.is_err() {
        return;
    }
    let Ok(bytes) = serde_json::to_vec(json) else {
        return;
    };
    // 临时文件 + rename:避免读到半截 JSON(下次读失败会重拉,但少浪费)。
    let tmp = path.with_extension("json.tmp");
    if tokio::fs::write(&tmp, &bytes).await.is_ok() {
        let _ = tokio::fs::rename(&tmp, path).await;
    }
}

async fn fetch_models_dev() -> Result<Value, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| format!("failed to build HTTP client: {e}"))?;
    let resp = client
        .get(MODELS_DEV_URL)
        .header("accept", "application/json")
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(format!("HTTP {status}"));
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("failed to read response body: {e}"))?;
    serde_json::from_slice(&bytes).map_err(|e| format!("invalid JSON response: {e}"))
}

/// 加载目录:新鲜缓存 → 网络 → 过期缓存降级。错误串直接进 wire 的
/// `error` / `metadataWarning`(用户可见)。
async fn load_catalog(state: &Arc<AppState>) -> Result<LoadedCatalog, String> {
    let path = cache_path(state);
    if let Some((json, fetched_at)) = read_cache(&path, true).await {
        return Ok(LoadedCatalog {
            catalog: ModelsCatalog::from_json(&json),
            fetched_at,
            cached: true,
            stale: false,
        });
    }
    match fetch_models_dev().await {
        Ok(json) => {
            write_cache(&path, &json).await;
            Ok(LoadedCatalog {
                catalog: ModelsCatalog::from_json(&json),
                fetched_at: rfc3339(SystemTime::now()),
                cached: false,
                stale: false,
            })
        }
        Err(err) => {
            if let Some((json, fetched_at)) = read_cache(&path, false).await {
                tracing::warn!(
                    error = %err,
                    "models.dev fetch failed; falling back to stale cache"
                );
                Ok(LoadedCatalog {
                    catalog: ModelsCatalog::from_json(&json),
                    fetched_at,
                    cached: true,
                    stale: true,
                })
            } else {
                Err(err)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// IPC inner:lookup_model_metadata
// ---------------------------------------------------------------------------

/// 「按模型名自动匹配能力」:返回候选列表(首个为最佳匹配)。前端表单
/// 的「自动匹配」按钮消费;探测面板的富化复用同一目录索引。
pub async fn lookup_model_metadata_inner(
    state: &Arc<AppState>,
    model_name: String,
    provider_id: Option<String>,
) -> Result<Value, AppCommandError> {
    let query = model_name.trim().to_string();
    if query.is_empty() {
        return Ok(serde_json::json!({
            "success": false,
            "matches": [],
            "error": "model name is empty",
        }));
    }
    let provider_hint = resolve_provider_hint(state, provider_id.as_deref()).await;
    let loaded = match load_catalog(state).await {
        Ok(l) => l,
        Err(e) => {
            return Ok(serde_json::json!({
                "success": false,
                "matches": [],
                "error": e,
            }));
        }
    };
    let matches = loaded
        .catalog
        .matches(&query, provider_hint.as_deref(), MAX_MATCHES);
    Ok(serde_json::json!({
        "success": true,
        "source": "models.dev",
        "fetchedAt": loaded.fetched_at,
        "cached": loaded.cached,
        "stale": loaded.stale,
        "matches": matches,
        "error": null,
    }))
}

/// 探测的匹配线索:provider 的 base_url 主机名优先,其次协议默认。
async fn resolve_provider_hint(state: &Arc<AppState>, provider_id: Option<&str>) -> Option<String> {
    let id = provider_id?;
    let provider = db::get_provider(&state.db, id).await.ok().flatten()?;
    guess_provider_key(&provider.base_url, &provider.protocol)
}

// ---------------------------------------------------------------------------
// IPC inner:probe_provider_models
// ---------------------------------------------------------------------------

fn failure(error: String) -> Value {
    serde_json::json!({
        "success": false,
        "latencyMs": 0,
        "models": [],
        "truncated": false,
        "error": error,
        "metadataWarning": null,
    })
}

/// 真实拉一次供应商的模型列表端点(探测)。成功时逐条尝试线上元数据
/// 富化(目录不可用时 `metadataWarning` 带原因,列表照常返回)。
pub async fn probe_provider_models_inner(
    state: &Arc<AppState>,
    provider_id: String,
) -> Result<Value, AppCommandError> {
    let provider = match db::get_provider(&state.db, &provider_id).await {
        Ok(Some(p)) => p,
        Ok(None) => return Ok(failure(format!("provider '{provider_id}' not found"))),
        Err(e) => return Ok(failure(format!("failed to load provider: {e}"))),
    };

    let base = provider.base_url.trim_end_matches('/');
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|e| anyhow::anyhow!("failed to build HTTP client: {e}"))?;

    let request = match provider.protocol.as_str() {
        // Anthropic Messages API 的模型列表端点。base_url 不含 /v1
        // (默认 https://api.anthropic.com);limit 拉到 1000 避免默认 20 条截断。
        "anthropic" => client
            .get(format!("{base}/v1/models?limit=1000"))
            .header("x-api-key", &provider.api_key)
            .header("anthropic-version", "2023-06-01"),
        // Chat Completions / Responses 同走 OpenAI 兼容的 GET /models
        // (base_url 已含 /v1,见 test_model 的 URL 约定)。
        "openai" | "openai_responses" => client
            .get(format!("{base}/models"))
            .header("authorization", format!("Bearer {}", provider.api_key)),
        other => {
            return Ok(failure(format!("unsupported protocol: {other}")));
        }
    };

    let start = Instant::now();
    let resp = match request.header("accept", "application/json").send().await {
        Ok(r) => r,
        Err(e) => {
            let latency_ms = start.elapsed().as_millis() as u64;
            return Ok(serde_json::json!({
                "success": false,
                "latencyMs": latency_ms,
                "models": [],
                "truncated": false,
                "error": format!("request failed: {e}"),
                "metadataWarning": null,
            }));
        }
    };

    let status = resp.status();
    let body_text = resp.text().await.unwrap_or_default();
    let latency_ms = start.elapsed().as_millis() as u64;
    if !status.is_success() {
        let snippet: String = body_text.chars().take(200).collect();
        return Ok(serde_json::json!({
            "success": false,
            "latencyMs": latency_ms,
            "models": [],
            "truncated": false,
            "error": format!("HTTP {status}: {snippet}"),
            "metadataWarning": null,
        }));
    }

    let parsed: Value = match serde_json::from_str(&body_text) {
        Ok(v) => v,
        Err(e) => {
            return Ok(serde_json::json!({
                "success": false,
                "latencyMs": latency_ms,
                "models": [],
                "truncated": false,
                "error": format!("invalid JSON response: {e}"),
                "metadataWarning": null,
            }));
        }
    };

    let items: Vec<&Value> = match &parsed {
        // 标准形状 { data: [...] };部分兼容网关直接回数组。
        Value::Array(arr) => arr.iter().collect(),
        Value::Object(obj) => obj
            .get("data")
            .and_then(Value::as_array)
            .map(|arr| arr.iter().collect())
            .unwrap_or_default(),
        _ => Vec::new(),
    };

    let mut seen = std::collections::HashSet::new();
    let mut names: Vec<(String, String)> = Vec::new();
    for item in items {
        let Some(id) = item
            .get("id")
            .and_then(Value::as_str)
            .or_else(|| item.get("name").and_then(Value::as_str))
        else {
            continue;
        };
        if id.is_empty() || !seen.insert(id.to_string()) {
            continue;
        }
        let display = item
            .get("display_name")
            .and_then(Value::as_str)
            .or_else(|| item.get("name").and_then(Value::as_str))
            .unwrap_or(id)
            .to_string();
        names.push((id.to_string(), display));
    }
    names.sort_by(|a, b| a.0.cmp(&b.0));
    let truncated = names.len() > MAX_PROBED_MODELS;
    names.truncate(MAX_PROBED_MODELS);

    // 富化是尽力而为:目录不可用时列表照常返回,只带 warning。
    let hint = guess_provider_key(&provider.base_url, &provider.protocol);
    let (catalog, metadata_warning, fetched_at, cached, stale) = match load_catalog(state).await {
        Ok(loaded) => (
            Some(loaded.catalog),
            None,
            Some(loaded.fetched_at),
            Some(loaded.cached),
            Some(loaded.stale),
        ),
        Err(e) => (None, Some(e), None, None, None),
    };

    let models: Vec<ProbedModel> = names
        .into_iter()
        .map(|(model_name, display_name)| {
            let spec = catalog
                .as_ref()
                .and_then(|c| c.best(&model_name, hint.as_deref()));
            ProbedModel {
                model_name,
                display_name,
                spec,
            }
        })
        .collect();

    Ok(serde_json::json!({
        "success": true,
        "latencyMs": latency_ms,
        "models": models,
        "truncated": truncated,
        "error": null,
        "metadataSource": "models.dev",
        "metadataFetchedAt": fetched_at,
        "metadataCached": cached,
        "metadataStale": stale,
        "metadataWarning": metadata_warning,
    }))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Value {
        serde_json::json!({
            "anthropic": {
                "id": "anthropic",
                "name": "Anthropic",
                "models": {
                    "claude-sonnet-4-5": {
                        "id": "claude-sonnet-4-5",
                        "name": "Claude Sonnet 4.5 (latest)",
                        "reasoning": true,
                        "reasoning_options": [
                            {"type": "budget_tokens", "min": 1024}
                        ],
                        "modalities": {"input": ["text", "image", "pdf"], "output": ["text"]},
                        "limit": {"context": 200000, "output": 64000},
                        "release_date": "2025-09-29"
                    }
                }
            },
            "openai": {
                "id": "openai",
                "name": "OpenAI",
                "models": {
                    "gpt-4o": {
                        "id": "gpt-4o",
                        "name": "GPT-4o",
                        "reasoning": false,
                        "modalities": {"input": ["text", "image"], "output": ["text"]},
                        "limit": {"context": 128000, "output": 16384}
                    },
                    "gpt-4o-mini": {
                        "id": "gpt-4o-mini",
                        "name": "GPT-4o mini",
                        "reasoning": false,
                        "modalities": {"input": ["text"], "output": ["text"]},
                        "limit": {"context": 128000, "output": 16384}
                    }
                }
            },
            "zai": {
                "id": "zai",
                "name": "Z.AI",
                "models": {
                    "glm-4.7": {
                        "id": "glm-4.7",
                        "name": "GLM-4.7",
                        "reasoning": true,
                        "reasoning_options": [
                            {"type": "effort", "values": ["low", "medium", "high"]}
                        ],
                        "modalities": {"input": ["text"], "output": ["text"]},
                        "limit": {"context": 200000, "output": 131072}
                    }
                }
            }
        })
    }

    fn catalog() -> ModelsCatalog {
        ModelsCatalog::from_json(&fixture())
    }

    #[test]
    fn normalize_strips_prefix_parens_and_separators() {
        assert_eq!(
            normalize_model_name("Anthropic/claude-sonnet-4-5"),
            "claude-sonnet-4-5"
        );
        assert_eq!(
            normalize_model_name("Claude Sonnet 4.5 (latest)"),
            "claude-sonnet-4-5"
        );
        assert_eq!(normalize_model_name("GLM_4.7:free"), "glm-4-7-free");
        // Vertex 的 @ 日期拼法归一后与 - 拼法同键(候选去重)。
        assert_eq!(
            normalize_model_name("claude-sonnet-4-5@20250929"),
            "claude-sonnet-4-5-20250929"
        );
        assert_eq!(normalize_model_name("  gpt-4o  "), "gpt-4o");
    }

    #[test]
    fn exact_match_wins_over_longer_variants() {
        let c = catalog();
        let specs = c.matches("gpt-4o", None, 8);
        assert_eq!(specs.len(), 2, "{specs:?}");
        assert_eq!(specs[0].model_id, "gpt-4o");
        assert_eq!(specs[0].context_window, Some(128_000));
        assert_eq!(specs[0].max_output_tokens, Some(16_384));
        assert!(specs[0].supports_vision);
        assert!(!specs[0].supports_thinking);
        assert_eq!(specs[1].model_id, "gpt-4o-mini");
    }

    #[test]
    fn dated_model_name_matches_base_catalog_id() {
        let c = catalog();
        let best = c.best("claude-sonnet-4-5-20250929", None).unwrap();
        assert_eq!(best.model_id, "claude-sonnet-4-5");
        assert_eq!(best.provider_name, "Anthropic");
        // budget_tokens 型候选不是 effort 词表 → 挡位留空。
        assert!(best.thinking_levels.is_empty());
        assert!(best.supports_thinking);
    }

    #[test]
    fn provider_hint_breaks_ties_across_gateways() {
        // 同一模型挂在两个供应商时,hint 命中的条目优先。
        let root = serde_json::json!({
            "openrouter": {
                "id": "openrouter",
                "name": "OpenRouter",
                "models": {
                    "anthropic/claude-sonnet-4-5": {
                        "id": "anthropic/claude-sonnet-4-5",
                        "name": "Claude Sonnet 4.5",
                        "limit": {"context": 200000, "output": 64000}
                    }
                }
            },
            "anthropic": {
                "id": "anthropic",
                "name": "Anthropic",
                "models": {
                    "claude-sonnet-4-5": {
                        "id": "claude-sonnet-4-5",
                        "name": "Claude Sonnet 4.5",
                        "limit": {"context": 200000, "output": 64000}
                    }
                }
            }
        });
        let c = ModelsCatalog::from_json(&root);
        let hinted = c.best("claude-sonnet-4-5", Some("openrouter")).unwrap();
        assert_eq!(hinted.provider_key, "openrouter");
        let unhinted = c.best("claude-sonnet-4-5", None).unwrap();
        // 无 hint 时一方厂商(anthropic)按 priority 胜出。
        assert_eq!(unhinted.provider_key, "anthropic");
    }

    #[test]
    fn thinking_levels_and_vision_projection() {
        let c = catalog();
        let glm = c.best("glm-4.7", Some("zai")).unwrap();
        assert_eq!(glm.thinking_levels, vec!["low", "medium", "high"]);
        assert!(glm.supports_thinking);
        assert!(!glm.supports_vision);
        assert_eq!(glm.max_output_tokens, Some(131_072));
        let sonnet = c.best("claude-sonnet-4-5", None).unwrap();
        assert!(sonnet.supports_vision);
        assert_eq!(sonnet.release_date.as_deref(), Some("2025-09-29"));
    }

    #[test]
    fn gateway_variants_do_not_beat_official_entry_on_priority() {
        // 真实线上形状:聚合网关的 `anthropic/claude-sonnet-4-5` 与官方
        // `claude-sonnet-4-5` 归一化后同 key 同分。JSON 对象遍历顺序
        // 不可控,去重必须按 provider 优先级裁决(官方口径的
        // context=200k 而不是网关的 1M)。
        let root = serde_json::json!({
            "aihubmix": {
                "id": "aihubmix",
                "name": "AIHubMix",
                "models": {
                    "anthropic/claude-sonnet-4-5": {
                        "id": "anthropic/claude-sonnet-4-5",
                        "name": "Claude Sonnet 4.5",
                        "limit": {"context": 1000000, "output": 64000}
                    }
                }
            },
            "anthropic": {
                "id": "anthropic",
                "name": "Anthropic",
                "models": {
                    "claude-sonnet-4-5": {
                        "id": "claude-sonnet-4-5",
                        "name": "Claude Sonnet 4.5 (latest)",
                        "limit": {"context": 200000, "output": 64000}
                    }
                }
            }
        });
        let c = ModelsCatalog::from_json(&root);
        let matches = c.matches("claude-sonnet-4-5", None, 8);
        assert_eq!(
            matches.len(),
            1,
            "same normalized id must dedupe: {matches:?}"
        );
        assert_eq!(matches[0].provider_key, "anthropic");
        assert_eq!(matches[0].context_window, Some(200_000));
        // best() 走精确表,同样必须按优先级裁决。
        assert_eq!(
            c.best("anthropic/claude-sonnet-4-5", None)
                .unwrap()
                .provider_key,
            "anthropic"
        );
    }

    #[test]
    fn partial_and_unknown_names() {
        let c = catalog();
        // 用户只打了前缀(目录 id 更长)→ 72 档命中。
        assert_eq!(c.best("gpt-4o-m", None).unwrap().model_id, "gpt-4o-mini");
        // 完全无关 → 无匹配。
        assert!(c.best("definitely-not-a-model", None).is_none());
        assert!(c.matches("", None, 8).is_empty());
    }

    #[test]
    fn variant_suffix_guard_blocks_family_collision() {
        // "gpt-4o-mini" 精确存在时不得回落到 "gpt-4o";这里直接验证
        // 打分器:非日期型后缀不给 86。
        assert_eq!(score_entry("gpt-4o", "", "gpt-4o-mini"), 0);
        assert_eq!(score_entry("gpt-4o", "", "gpt-4o-2024-08-06"), 86);
    }

    #[test]
    fn guess_provider_key_uses_host_then_protocol() {
        assert_eq!(
            guess_provider_key("https://api.anthropic.com", "openai").as_deref(),
            Some("anthropic")
        );
        assert_eq!(
            guess_provider_key("https://open.bigmodel.cn/api/paas/v4", "openai").as_deref(),
            Some("zhipuai")
        );
        assert_eq!(
            guess_provider_key("https://my-proxy.example.com/v1", "openai").as_deref(),
            Some("openai")
        );
        assert_eq!(
            guess_provider_key("https://my-proxy.example.com/v1", "anthropic").as_deref(),
            Some("anthropic")
        );
    }

    #[test]
    fn empty_catalog_is_safe() {
        let c = ModelsCatalog::from_json(&serde_json::json!({}));
        assert!(c.is_empty());
        assert!(c.best("anything", None).is_none());
        assert!(c.matches("anything", None, 8).is_empty());
        // 结构不符(数组根)也不 panic。
        let c = ModelsCatalog::from_json(&serde_json::json!([]));
        assert!(c.is_empty());
    }

    #[test]
    fn cache_roundtrip_helper_builds_catalog() {
        let json = fixture();
        let c = ModelsCatalog::from_json(&json);
        assert_eq!(c.len(), 4);
    }

    /// 真实目录冒烟(默认 ignore —— 网络不确定,CI 不收)。手动跑:
    /// `cargo test -p everlasting --lib model_metadata::tests::live -- --ignored`
    /// 锁定的是「线上 api.json 的真实形状仍能被解析、常见模型名仍能
    /// 精确命中」这一条外部契约(models.dev 结构与本地 fixture 漂移时,
    /// 这一条先红)。
    #[tokio::test]
    #[ignore = "network-dependent: real models.dev fetch"]
    async fn live_catalog_parses_and_matches_common_models() {
        let json = fetch_models_dev().await.expect("models.dev reachable");
        let c = ModelsCatalog::from_json(&json);
        assert!(c.len() > 1000, "unexpectedly small catalog: {}", c.len());
        for (name, hint) in [
            ("claude-sonnet-4-5", "anthropic"),
            ("gpt-4o", "openai"),
            ("glm-4.7", "zai"),
            ("deepseek-chat", "deepseek"),
        ] {
            let best = c
                .best(name, Some(hint))
                .unwrap_or_else(|| panic!("no match for {name}"));
            assert!(
                best.context_window.unwrap_or(0) > 0,
                "{name} matched without a context window: {best:?}"
            );
        }
    }
}
