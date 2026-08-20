//! 08-20-turn-usage-event-quota-view WP2 — 5h 滚动窗口配额聚合查询层。
//!
//! 零新表:全部聚合 `turn_trace`(`created_at` 滑窗 + `run_id` 主/worker
//! 拆分 + `provider_id` 归因分组)。`token_usage_json` 经 `json_extract`
//! 取数,NULL 行(未带 usage 的 trace 行)各字段 COALESCE 为 0 —— SUM
//! 天然安全。窗口时长是**整数小时**(config `quota_window_hours`,缺省
//! 5),经绑参 `-N hours` modifier 传入,不拼字符串。
//!
//! 归因:`provider_id` NULL(catalog miss 写入 / 回填近似 join 不到的
//! 遗留行)单独成组,Rust 侧映射为 `"unknown"` 桶(display_name 空)。

use serde::Serialize;
use sqlx::{Row, SqlitePool};

// ---------------------------------------------------------------------------
// Wire types
// ---------------------------------------------------------------------------

/// 窗口内四维 token 累计(input 为 Anthropic 口径,含 cache 两维)。
#[derive(Debug, Clone, Copy, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageTotals {
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_input_tokens: i64,
    pub cache_creation_input_tokens: i64,
}

impl UsageTotals {
    fn add_row(&mut self, row: &sqlx::sqlite::SqliteRow, prefix: &str) {
        let col = |suffix: &str| -> &str {
            match (prefix, suffix) {
                ("main", "input") => "main_input",
                ("main", "output") => "main_output",
                ("main", "cache_read") => "main_cache_read",
                ("main", "cache_creation") => "main_cache_creation",
                ("worker", "input") => "worker_input",
                ("worker", "output") => "worker_output",
                ("worker", "cache_read") => "worker_cache_read",
                ("worker", "cache_creation") => "worker_cache_creation",
                _ => unreachable!("fixed (prefix, suffix) pairs"),
            }
        };
        self.input_tokens += row.try_get::<i64, _>(col("input")).unwrap_or(0);
        self.output_tokens += row.try_get::<i64, _>(col("output")).unwrap_or(0);
        self.cache_read_input_tokens += row.try_get::<i64, _>(col("cache_read")).unwrap_or(0);
        self.cache_creation_input_tokens +=
            row.try_get::<i64, _>(col("cache_creation")).unwrap_or(0);
    }
}

/// per-provider 窗口切片(小时粒度)。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HourlyBucket {
    /// `YYYY-MM-DDTHH:00:00`(本地库时区 = UTC `datetime('now')`)。
    pub hour: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
}

/// 单个 provider 的窗口聚合。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderUsageWindow {
    /// NULL 归因行序列化为 `"unknown"`(前端分组键稳定非空)。
    pub provider_id: String,
    /// join providers 表的 display_name;join 不到(unknown 桶)为 None。
    pub display_name: Option<String>,
    pub totals: UsageTotals,
    pub main_totals: UsageTotals,
    pub worker_totals: UsageTotals,
    pub hourly: Vec<HourlyBucket>,
}

/// 窗口内烧量 top session(跳转入口用)。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionUsageEntry {
    pub session_id: String,
    pub title: Option<String>,
    /// 跳转入口(`openSessionInProject(projectId, sessionId)`)需要。
    pub project_id: Option<String>,
    /// 窗口内主 loop input(worker 拆行不计,见 totals 语义)。
    pub window_main_input: i64,
    pub window_worker_input: i64,
    /// session 全周期累计 —— **turn_trace 全量聚合**(主+worker),
    /// 非 `sessions.input_tokens_total`:该 A4 累计列自 2026-06-26
    /// snapshot 重构后已无写点(孤儿列,`update_last_turn_usage` 只写
    /// last_* 快照)。代价:pre-E2(2026-07-14 前)session 无 trace 行
    /// → 累计为 0,可接受(配额视角只关心近期)。
    pub lifetime_input: i64,
    pub lifetime_output: i64,
}

/// `usage_window` IPC 返回体。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageWindowReport {
    pub window_hours: i64,
    /// config `quota_limit_tokens`;None = 只显示消耗不画上限(AC5)。
    pub limit_tokens: Option<i64>,
    pub providers: Vec<ProviderUsageWindow>,
    pub top_sessions: Vec<SessionUsageEntry>,
}

// ---------------------------------------------------------------------------
// Query layer
// ---------------------------------------------------------------------------

/// 滑窗 SQL 片段:`created_at >= datetime('now', ?)`。`window_hours` 由
/// caller 校验为正整数,modifier 字符串在 Rust 侧拼整数(无注入面)。
fn window_modifier(window_hours: i64) -> String {
    format!("-{} hours", window_hours.max(1))
}

const PROVIDER_PIVOT_SQL: &str = r#"
SELECT
    t.provider_id                                AS provider_id,
    p.display_name                               AS display_name,
    SUM(CASE WHEN t.run_id = ''  THEN COALESCE(json_extract(t.token_usage_json, '$.input_tokens'), 0)             ELSE 0 END) AS main_input,
    SUM(CASE WHEN t.run_id = ''  THEN COALESCE(json_extract(t.token_usage_json, '$.output_tokens'), 0)            ELSE 0 END) AS main_output,
    SUM(CASE WHEN t.run_id = ''  THEN COALESCE(json_extract(t.token_usage_json, '$.cache_read_input_tokens'), 0)     ELSE 0 END) AS main_cache_read,
    SUM(CASE WHEN t.run_id = ''  THEN COALESCE(json_extract(t.token_usage_json, '$.cache_creation_input_tokens'), 0) ELSE 0 END) AS main_cache_creation,
    SUM(CASE WHEN t.run_id != '' THEN COALESCE(json_extract(t.token_usage_json, '$.input_tokens'), 0)             ELSE 0 END) AS worker_input,
    SUM(CASE WHEN t.run_id != '' THEN COALESCE(json_extract(t.token_usage_json, '$.output_tokens'), 0)            ELSE 0 END) AS worker_output,
    SUM(CASE WHEN t.run_id != '' THEN COALESCE(json_extract(t.token_usage_json, '$.cache_read_input_tokens'), 0)     ELSE 0 END) AS worker_cache_read,
    SUM(CASE WHEN t.run_id != '' THEN COALESCE(json_extract(t.token_usage_json, '$.cache_creation_input_tokens'), 0) ELSE 0 END) AS worker_cache_creation
FROM turn_trace t
LEFT JOIN providers p ON p.id = t.provider_id
WHERE t.created_at >= datetime('now', ?)
  AND (? IS NULL OR t.provider_id = ?)
GROUP BY t.provider_id
"#;

const HOURLY_SQL: &str = r#"
SELECT
    t.provider_id                                AS provider_id,
    strftime('%Y-%m-%dT%H:00:00', t.created_at)  AS hour,
    SUM(COALESCE(json_extract(t.token_usage_json, '$.input_tokens'), 0))  AS input_tokens,
    SUM(COALESCE(json_extract(t.token_usage_json, '$.output_tokens'), 0)) AS output_tokens
FROM turn_trace t
WHERE t.created_at >= datetime('now', ?)
  AND (? IS NULL OR t.provider_id = ?)
GROUP BY t.provider_id, hour
ORDER BY hour
"#;

const TOP_SESSIONS_SQL: &str = r#"
SELECT
    t.session_id AS session_id,
    s.title      AS title,
    s.project_id AS project_id,
    SUM(CASE WHEN t.run_id = ''  THEN COALESCE(json_extract(t.token_usage_json, '$.input_tokens'), 0) ELSE 0 END) AS window_main_input,
    SUM(CASE WHEN t.run_id != '' THEN COALESCE(json_extract(t.token_usage_json, '$.input_tokens'), 0) ELSE 0 END) AS window_worker_input
FROM turn_trace t
LEFT JOIN sessions s ON s.id = t.session_id
WHERE t.created_at >= datetime('now', ?)
GROUP BY t.session_id
ORDER BY (window_main_input + window_worker_input) DESC
LIMIT ?
"#;

/// session 全周期累计(turn_trace 全量,无窗口;`input_tokens_total`
/// 孤儿列的替代口径,见 `SessionUsageEntry::lifetime_input`)。
const LIFETIME_SQL: &str = r#"
SELECT
    t.session_id AS session_id,
    SUM(COALESCE(json_extract(t.token_usage_json, '$.input_tokens'), 0))  AS lifetime_input,
    SUM(COALESCE(json_extract(t.token_usage_json, '$.output_tokens'), 0)) AS lifetime_output
FROM turn_trace t
GROUP BY t.session_id
"#;

/// 聚合滚动窗口用量。`provider_filter = None` = 全部 provider(缺省视图)。
/// `limit_tokens` 只透传进报告(语义决策在调用侧 config),本层不读
/// config —— 同一 fn 供 IPC 与测试共用,测试无需摆 config 行。
pub async fn usage_window(
    pool: &SqlitePool,
    provider_filter: Option<&str>,
    window_hours: i64,
    limit_tokens: Option<i64>,
    top_session_limit: u32,
) -> Result<UsageWindowReport, sqlx::Error> {
    let modifier = window_modifier(window_hours);

    // 1. per-provider 主/worker 拆分累计。
    let provider_rows = sqlx::query(PROVIDER_PIVOT_SQL)
        .bind(&modifier)
        .bind(provider_filter)
        .bind(provider_filter)
        .fetch_all(pool)
        .await?;
    let mut providers: Vec<ProviderUsageWindow> = provider_rows
        .iter()
        .map(|row| {
            let mut main = UsageTotals::default();
            main.add_row(row, "main");
            let mut worker = UsageTotals::default();
            worker.add_row(row, "worker");
            let mut totals = main;
            totals.input_tokens += worker.input_tokens;
            totals.output_tokens += worker.output_tokens;
            totals.cache_read_input_tokens += worker.cache_read_input_tokens;
            totals.cache_creation_input_tokens += worker.cache_creation_input_tokens;
            ProviderUsageWindow {
                provider_id: row
                    .try_get::<Option<String>, _>("provider_id")
                    .ok()
                    .flatten()
                    .unwrap_or_else(|| "unknown".to_string()),
                display_name: row
                    .try_get::<Option<String>, _>("display_name")
                    .ok()
                    .flatten(),
                totals,
                main_totals: main,
                worker_totals: worker,
                hourly: Vec::new(),
            }
        })
        .collect();

    // 2. 小时分布(provider × hour,合进对应 provider 条目)。
    let hourly_rows = sqlx::query(HOURLY_SQL)
        .bind(&modifier)
        .bind(provider_filter)
        .bind(provider_filter)
        .fetch_all(pool)
        .await?;
    for row in &hourly_rows {
        let pid = row
            .try_get::<Option<String>, _>("provider_id")
            .ok()
            .flatten()
            .unwrap_or_else(|| "unknown".to_string());
        let bucket = HourlyBucket {
            hour: row.try_get::<String, _>("hour").unwrap_or_default(),
            input_tokens: row.try_get::<i64, _>("input_tokens").unwrap_or(0),
            output_tokens: row.try_get::<i64, _>("output_tokens").unwrap_or(0),
        };
        if let Some(entry) = providers.iter_mut().find(|p| p.provider_id == pid) {
            entry.hourly.push(bucket);
        }
    }

    // 3. top sessions(窗口排序)+ 全周期累计(无窗口,分别聚合)。
    let top_rows = sqlx::query(TOP_SESSIONS_SQL)
        .bind(&modifier)
        .bind(i64::from(top_session_limit.min(50)))
        .fetch_all(pool)
        .await?;
    let lifetime_rows = sqlx::query(LIFETIME_SQL).fetch_all(pool).await?;
    let lifetime: std::collections::HashMap<String, (i64, i64)> = lifetime_rows
        .iter()
        .map(|row| {
            (
                row.try_get::<String, _>("session_id").unwrap_or_default(),
                (
                    row.try_get::<i64, _>("lifetime_input").unwrap_or(0),
                    row.try_get::<i64, _>("lifetime_output").unwrap_or(0),
                ),
            )
        })
        .collect();
    let top_sessions = top_rows
        .iter()
        .map(|row| {
            let sid = row.try_get::<String, _>("session_id").unwrap_or_default();
            let (lifetime_input, lifetime_output) = lifetime.get(&sid).copied().unwrap_or((0, 0));
            SessionUsageEntry {
                session_id: sid,
                title: row.try_get::<Option<String>, _>("title").ok().flatten(),
                project_id: row
                    .try_get::<Option<String>, _>("project_id")
                    .ok()
                    .flatten(),
                window_main_input: row.try_get::<i64, _>("window_main_input").unwrap_or(0),
                window_worker_input: row.try_get::<i64, _>("window_worker_input").unwrap_or(0),
                lifetime_input,
                lifetime_output,
            }
        })
        .collect();

    Ok(UsageWindowReport {
        window_hours,
        limit_tokens,
        providers,
        top_sessions,
    })
}
