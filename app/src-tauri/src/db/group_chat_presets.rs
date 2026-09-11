//! Group-chat preset CRUD (GCE-P1, task `09-12-gc-preset-settings`).
//!
//! 用户群聊预设(Settings 可管理),一行一个预设。**内置四档**
//! (review / fe_review / arch / retro)不在此表 —— 它们仍是
//! `scripts/group-chat-presets.json` 的单一事实源(M1 CLI / MCP /
//! 前端三处消费 + 跨机器可移植性依赖 JSON 原样),用户预设叠加其上;
//! 与内置 key 的撞名校验在 commands 层(常量旁注明同步义务)。
//!
//! Schema(建表段在 `db/migrations/schema.rs` 尾部,幂等):
//! ```sql
//! CREATE TABLE IF NOT EXISTS group_chat_presets (
//!   id                 TEXT NOT NULL PRIMARY KEY,  -- Uuid::new_v4()(models.rs 同款)
//!   name               TEXT NOT NULL UNIQUE,       -- 显示名;大小写不敏感唯一在 commands 层
//!   description        TEXT NOT NULL DEFAULT '',
//!   moderator_model_id TEXT NOT NULL,              -- soft FK → models.id(不建约束)
//!   participants       TEXT NOT NULL,              -- JSON 数组(camelCase,与 wire 同形)
//!   created_at         TEXT NOT NULL,
//!   updated_at         TEXT NOT NULL
//! )
//! ```
//!
//! `moderator_model_id` 与 `participants[].model_id` 是 models.id
//! (UUID)的 soft reference——无 FK 约束,模型被删不级联;保存时
//! commands 层校验存在(允许 disabled),使用处 catalog 预检 / 反诊
//! 兜底(与 `subagent_model_overrides.model_id` 同一定案)。
//! `participants` 用 JSON 列而非子表(`scheduled_tasks.group_chat_config`
//! 同款形态):查询面没有「按参与者查预设」的需求。列内 JSON 与
//! wire 同用 camelCase(单一 `GcPresetParticipant` 结构,防双形状漂移);
//! JSON↔Vec 的序列化封装在本模块内(`map_row` 读路径 + create/update
//! 写路径),调用方只碰结构化形状。

use chrono::Utc;
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

/// One participant of a preset. wire 形状(design §2):
/// `{"name": "架构", "modelId": "<uuid>", "persona": "arch"}`。
/// `model_id` 是 models.id UUID;`persona` 是五种内置 kind 之一
/// (白名单校验在 commands 层)。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GcPresetParticipant {
    pub name: String,
    pub model_id: String,
    pub persona: String,
}

/// One DB row from `group_chat_presets`(wire camelCase,沿
/// `subagent_model_overrides` 的 IPC parity 惯例)。`participants` 在
/// db 层做 JSON 列 ↔ Vec 的序列化(见 [`map_row`]),调用方拿到
/// 结构化形状。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GcPresetRow {
    pub id: String,
    pub name: String,
    pub description: String,
    pub moderator_model_id: String,
    pub participants: Vec<GcPresetParticipant>,
    /// RFC 3339。
    pub created_at: String,
    /// RFC 3339。
    pub updated_at: String,
}

/// 把一行 `group_chat_presets` 查询结果映射成 [`GcPresetRow`]
/// (`participants` JSON 列在此反序列化成 Vec)。list / get 两条读路径
/// 共用,避免列清单漂移(models.rs 的 map_model_row 同款)。
fn map_row(r: &sqlx::sqlite::SqliteRow) -> Result<GcPresetRow, sqlx::Error> {
    let participants_json: String = r.try_get("participants")?;
    Ok(GcPresetRow {
        id: r.try_get("id")?,
        name: r.try_get("name")?,
        description: r.try_get("description")?,
        moderator_model_id: r.try_get("moderator_model_id")?,
        participants: serde_json::from_str(&participants_json).map_err(|e| {
            sqlx::Error::ColumnDecode {
                index: "participants".into(),
                source: Box::new(e),
            }
        })?,
        created_at: r.try_get("created_at")?,
        updated_at: r.try_get("updated_at")?,
    })
}

/// Insert a new preset. `id` 服务端生成(`Uuid::new_v4`);名称唯一性
/// (大小写不敏感 + 不撞内置 key)由 commands 层校验,`UNIQUE` 列
/// 约束只作精确匹配兜底。校验序见 `commands/group_chat_presets.rs`。
pub async fn create_group_chat_preset(
    pool: &SqlitePool,
    name: &str,
    description: &str,
    moderator_model_id: &str,
    participants: Vec<GcPresetParticipant>,
) -> Result<GcPresetRow, sqlx::Error> {
    let now = Utc::now().to_rfc3339();
    let id = Uuid::new_v4().to_string();
    let participants_json = serde_json::to_string(&participants)
        .map_err(|e| sqlx::Error::Encode(format!("serialize participants: {}", e).into()))?;
    sqlx::query(
        r#"
 INSERT INTO group_chat_presets
 (id, name, description, moderator_model_id, participants, created_at, updated_at)
 VALUES (?, ?, ?, ?, ?, ?, ?)
 "#,
    )
    .bind(&id)
    .bind(name)
    .bind(description)
    .bind(moderator_model_id)
    .bind(&participants_json)
    .bind(&now)
    .bind(&now)
    .execute(pool)
    .await?;
    Ok(GcPresetRow {
        id,
        name: name.to_string(),
        description: description.to_string(),
        moderator_model_id: moderator_model_id.to_string(),
        participants,
        created_at: now.clone(),
        updated_at: now,
    })
}

/// List all user presets, `ORDER BY name`(稳定序:列表渲染不因重取
/// 而重排;内置档由前端按 JSON 声明序另列,与本表无关)。
pub async fn list_group_chat_presets(pool: &SqlitePool) -> Result<Vec<GcPresetRow>, sqlx::Error> {
    let rows = sqlx::query(
        r#"
 SELECT id, name, description, moderator_model_id, participants,
 created_at, updated_at
 FROM group_chat_presets
 ORDER BY name
 "#,
    )
    .fetch_all(pool)
    .await?;
    rows.iter().map(map_row).collect()
}

/// Get a single preset row by `id`. Returns `None` when the row
/// doesn't exist.
pub async fn get_group_chat_preset(
    pool: &SqlitePool,
    id: &str,
) -> Result<Option<GcPresetRow>, sqlx::Error> {
    let row = sqlx::query(
        r#"
 SELECT id, name, description, moderator_model_id, participants,
 created_at, updated_at
 FROM group_chat_presets
 WHERE id = ?
 "#,
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    match row {
        None => Ok(None),
        Some(r) => Ok(Some(map_row(&r)?)),
    }
}

/// Patch a preset by `id`. Returns `None` if the row doesn't exist
/// (commands 层转 `InvalidRequest`,防编辑竞态静默)。成功时回读整行
/// (镜像 update_model 的 re-read 模式:UPDATE 不触碰 `created_at`,
/// 回读才能带回真实值)。
pub async fn update_group_chat_preset(
    pool: &SqlitePool,
    id: &str,
    name: &str,
    description: &str,
    moderator_model_id: &str,
    participants: Vec<GcPresetParticipant>,
) -> Result<Option<GcPresetRow>, sqlx::Error> {
    let now = Utc::now().to_rfc3339();
    let participants_json = serde_json::to_string(&participants)
        .map_err(|e| sqlx::Error::Encode(format!("serialize participants: {}", e).into()))?;
    let res = sqlx::query(
        r#"
 UPDATE group_chat_presets
 SET name = ?, description = ?, moderator_model_id = ?,
 participants = ?, updated_at = ?
 WHERE id = ?
 "#,
    )
    .bind(name)
    .bind(description)
    .bind(moderator_model_id)
    .bind(&participants_json)
    .bind(&now)
    .bind(id)
    .execute(pool)
    .await?;
    if res.rows_affected() == 0 {
        return Ok(None);
    }
    get_group_chat_preset(pool, id).await
}

/// Delete a preset by `id`. Returns whether a row was actually
/// removed. **无守卫**(design §5 快照语义:已建定时任务的 config 是
/// 创建时展开的 UUID 阵容快照,自包含、不悬空,删预设不回溯影响)。
pub async fn delete_group_chat_preset(pool: &SqlitePool, id: &str) -> Result<bool, sqlx::Error> {
    let res = sqlx::query("DELETE FROM group_chat_presets WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected() > 0)
}
