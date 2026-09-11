//! GCE-P1(2026-09-12, task `09-12-gc-preset-settings`)— 用户群聊
//! 预设 CRUD IPC 四件:list / create / update / delete。
//!
//! 双形态三层先例(scheduled_tasks 同款):`_inner` 业务(Q0 单源)+
//! `#[tauri::command]` 包装 + daemon route(`daemon/routes/
//! group_chat_presets.rs`)。**内置四档预设(review / fe_review / arch /
//! retro)不经过本模块** —— 它们是 `scripts/group-chat-presets.json`
//! 的单一事实源(M1 CLI / MCP / 前端三处消费),只读;本模块只管
//! 用户预设,校验层负责与内置 key 不撞名。
//!
//! 校验(design §3,单一事实源在本文件 `validate_preset_input`):
//! 名称 trim 非空 ≤40 字符、大小写不敏感唯一(与用户行 + 内置 key
//! 双查)、描述 ≤200、主持人与全部参与者模型存在(允许 disabled)、
//! 参与者 2..=3 人且名字非空 ≤20 字符预设内唯一、persona 五 kind
//! 白名单。错误一律 `InvalidRequest`(→ HTTP 400),message 中文可读。
//!
//! wire:行形状 camelCase(`GcPresetRow`);命令参数扁平标量(IPC 形状
//! 铁律)。

use std::sync::Arc;

use serde::Serialize;
use tauri::State;

use crate::db::group_chat_presets as gc;
use crate::db::group_chat_presets::{GcPresetParticipant, GcPresetRow};
use crate::error::{AppCommandError, ErrorCategory};
use crate::state::AppState;

// ---------------------------------------------------------------------------
// 硬编码白名单(与 scripts/group-chat-presets.json 的同步义务)
// ---------------------------------------------------------------------------

/// 内置预设 key(`scripts/group-chat-presets.json` 的 `presets` 四键)。
/// **同步义务**:改 JSON 的 key 集合必须同步这里 —— 撞名校验用,
/// 漏改会出现「用户预设与内置档同名」的歧义选择区。
pub const BUILTIN_PRESET_KEYS: [&str; 4] = ["review", "fe_review", "arch", "retro"];

/// persona kind 白名单(`scripts/group-chat-presets.json` 的
/// `persona_md` 由 `composePersonaMd(kind)` 从这五种 kind 生成)。
/// **同步义务**:前端 persona 下拉(五档)与 JSON 的 kind 域同源,
/// 改任何一侧必须三处同步。自定义 persona 文本是 P3(非本期)。
pub const PERSONA_KINDS: [&str; 5] = ["arch", "product", "backend", "frontend", "outsider"];

/// 名称长度上限(design §3;按字符数非 UTF-8 字节,沿
/// `set_tunnel_display_name` 的 chars().count() 惯例)。
const NAME_MAX_CHARS: usize = 40;
/// 描述长度上限。
const DESCRIPTION_MAX_CHARS: usize = 200;
/// 参与者名字长度上限。
const PARTICIPANT_NAME_MAX_CHARS: usize = 20;
/// 参与者人数边界(沿 GroupChatConfigModal MVP 边界):2..=3。
const PARTICIPANTS_MIN: usize = 2;
const PARTICIPANTS_MAX: usize = 3;

fn invalid(msg: impl Into<String>) -> AppCommandError {
    AppCommandError::new(ErrorCategory::InvalidRequest, msg)
}

/// `delete_group_chat_preset` 的响应(`{ok: true}`;不存在也 ok,
/// 幂等删除沿 clear 先例)。
#[derive(Debug, Serialize)]
pub struct DeletedGroupChatPreset {
    pub ok: bool,
}

// ---------------------------------------------------------------------------
// 校验(design §3 单一事实源;create / update 共用)
// ---------------------------------------------------------------------------

/// 校验 + 规范化一个预设输入。返回 trim 后的规范名(调用方以它落库)。
/// `exclude_id`:update 时排除自身行后查重(None = create)。
/// 模型存在性经 `db::get_model`(**允许 disabled** —— 禁用是使用处
/// 问题,保存不拦;使用处已有禁用反诊 UX)。
async fn validate_preset_input(
    db: &sqlx::SqlitePool,
    name: &str,
    description: &str,
    moderator_model_id: &str,
    participants: &[GcPresetParticipant],
    exclude_id: Option<&str>,
) -> Result<String, AppCommandError> {
    // 1. 名称:trim 非空、≤40 字符。
    let name = name.trim();
    if name.is_empty() {
        return Err(invalid("预设名称不能为空"));
    }
    if name.chars().count() > NAME_MAX_CHARS {
        return Err(invalid(format!("预设名称过长(最多 {NAME_MAX_CHARS} 字符)")));
    }
    // 2. 与内置 key 大小写不敏感不撞。
    if BUILTIN_PRESET_KEYS
        .iter()
        .any(|k| k.eq_ignore_ascii_case(name))
    {
        return Err(invalid(format!(
            "预设名称「{name}」与内置预设冲突,请换一个名称"
        )));
    }
    // 3. 与其它用户行大小写不敏感不重名(update 排除自身)。
    let name_lc = name.to_lowercase();
    let existing_names: Vec<(String,)> = match exclude_id {
        Some(id) => sqlx::query_as("SELECT name FROM group_chat_presets WHERE id <> ?")
            .bind(id)
            .fetch_all(db)
            .await
            .map_err(|e| anyhow::anyhow!("validate_preset_input: list names failed: {}", e))?,
        None => sqlx::query_as("SELECT name FROM group_chat_presets")
            .fetch_all(db)
            .await
            .map_err(|e| anyhow::anyhow!("validate_preset_input: list names failed: {}", e))?,
    };
    if existing_names
        .iter()
        .any(|(n,)| n.to_lowercase() == name_lc)
    {
        return Err(invalid(format!("预设名称「{name}」已存在,请换一个名称")));
    }

    // 4. 描述 ≤200 字符(可空串)。
    if description.chars().count() > DESCRIPTION_MAX_CHARS {
        return Err(invalid(format!(
            "预设描述过长(最多 {DESCRIPTION_MAX_CHARS} 字符)"
        )));
    }

    // 5. 参与者 2..=3 人;每条名字 trim 非空、≤20 字符、预设内唯一;
    //    persona ∈ 五 kind 白名单。
    if participants.len() < PARTICIPANTS_MIN || participants.len() > PARTICIPANTS_MAX {
        return Err(invalid(format!(
            "参与者数量必须是 {PARTICIPANTS_MIN}~{PARTICIPANTS_MAX} 人,当前 {} 人",
            participants.len()
        )));
    }
    let mut seen_names = std::collections::HashSet::new();
    for p in participants {
        let pname = p.name.trim();
        if pname.is_empty() {
            return Err(invalid("参与者名字不能为空"));
        }
        if pname.chars().count() > PARTICIPANT_NAME_MAX_CHARS {
            return Err(invalid(format!(
                "参与者名字过长(最多 {PARTICIPANT_NAME_MAX_CHARS} 字符):{pname}"
            )));
        }
        if !seen_names.insert(pname.to_string()) {
            return Err(invalid(format!("参与者重名:「{pname}」")));
        }
        if !PERSONA_KINDS.contains(&p.persona.as_str()) {
            return Err(invalid(format!(
                "参与者「{pname}」的 persona 非法(仅支持 {})",
                PERSONA_KINDS.join(" / ")
            )));
        }
    }

    // 6. 主持人与全部参与者的模型必须存在(允许 disabled)。集中一条
    //    错误列出全部缺失项,镜像 commands/scheduled_tasks.rs 的口径。
    let mut missing: Vec<&str> = Vec::new();
    if crate::db::get_model(db, moderator_model_id.trim())
        .await
        .map_err(|e| anyhow::anyhow!("validate_preset_input: load model failed: {}", e))?
        .is_none()
    {
        missing.push("moderator");
    }
    for p in participants {
        if crate::db::get_model(db, p.model_id.trim())
            .await
            .map_err(|e| anyhow::anyhow!("validate_preset_input: load model failed: {}", e))?
            .is_none()
        {
            missing.push(p.name.trim());
        }
    }
    if !missing.is_empty() {
        return Err(invalid(format!(
            "预设引用的模型不存在(缺:{}),请刷新模型列表后重选",
            missing.join(" / ")
        )));
    }

    Ok(name.to_string())
}

// ---------------------------------------------------------------------------
// list
// ---------------------------------------------------------------------------

/// `list_group_chat_presets()` — 全量用户预设,`ORDER BY name` 稳定序。
pub async fn list_group_chat_presets_inner(
    state: &Arc<AppState>,
) -> Result<Vec<GcPresetRow>, AppCommandError> {
    let rows = gc::list_group_chat_presets(&state.db)
        .await
        .map_err(|e| anyhow::anyhow!("list_group_chat_presets failed: {}", e))?;
    Ok(rows)
}

#[tauri::command]
pub async fn list_group_chat_presets(
    state: State<'_, Arc<AppState>>,
) -> Result<Vec<GcPresetRow>, AppCommandError> {
    list_group_chat_presets_inner(state.inner()).await
}

// ---------------------------------------------------------------------------
// create
// ---------------------------------------------------------------------------

/// `create_group_chat_preset(name, description, moderatorModelId,
/// participants)` — 校验(design §3)通过后落库,返回新行(id 服务端
/// 生成)。名称唯一性大小写不敏感校验在 [`validate_preset_input`],
/// DB 的 `UNIQUE` 列约束只作精确匹配兜底。
pub async fn create_group_chat_preset_inner(
    state: &Arc<AppState>,
    name: String,
    description: String,
    moderator_model_id: String,
    participants: Vec<GcPresetParticipant>,
) -> Result<GcPresetRow, AppCommandError> {
    let name = validate_preset_input(
        &state.db,
        &name,
        &description,
        &moderator_model_id,
        &participants,
        None,
    )
    .await?;
    let row = gc::create_group_chat_preset(
        &state.db,
        &name,
        &description,
        moderator_model_id.trim(),
        normalize_participants(participants),
    )
    .await
    .map_err(|e| anyhow::anyhow!("create_group_chat_preset: insert failed: {}", e))?;
    Ok(row)
}

#[tauri::command]
pub async fn create_group_chat_preset(
    state: State<'_, Arc<AppState>>,
    name: String,
    description: String,
    moderator_model_id: String,
    participants: Vec<GcPresetParticipant>,
) -> Result<GcPresetRow, AppCommandError> {
    create_group_chat_preset_inner(
        state.inner(),
        name,
        description,
        moderator_model_id,
        participants,
    )
    .await
}

// ---------------------------------------------------------------------------
// update
// ---------------------------------------------------------------------------

/// `update_group_chat_preset(id, ...同 create)` — 全量 patch(表单
/// 整体提交语义,无部分更新)。行不存在 → `InvalidRequest`(防编辑
/// 竞态静默);查重排除自身(改名保留原名合法)。
pub async fn update_group_chat_preset_inner(
    state: &Arc<AppState>,
    id: String,
    name: String,
    description: String,
    moderator_model_id: String,
    participants: Vec<GcPresetParticipant>,
) -> Result<GcPresetRow, AppCommandError> {
    let name = validate_preset_input(
        &state.db,
        &name,
        &description,
        &moderator_model_id,
        &participants,
        Some(&id),
    )
    .await?;
    let row = gc::update_group_chat_preset(
        &state.db,
        &id,
        &name,
        &description,
        moderator_model_id.trim(),
        normalize_participants(participants),
    )
    .await
    .map_err(|e| anyhow::anyhow!("update_group_chat_preset failed: {}", e))?
    .ok_or_else(|| invalid(format!("群聊预设 {id} 不存在")))?;
    Ok(row)
}

#[tauri::command]
pub async fn update_group_chat_preset(
    state: State<'_, Arc<AppState>>,
    id: String,
    name: String,
    description: String,
    moderator_model_id: String,
    participants: Vec<GcPresetParticipant>,
) -> Result<GcPresetRow, AppCommandError> {
    update_group_chat_preset_inner(
        state.inner(),
        id,
        name,
        description,
        moderator_model_id,
        participants,
    )
    .await
}

// ---------------------------------------------------------------------------
// delete
// ---------------------------------------------------------------------------

/// `delete_group_chat_preset(id)` — 硬删,无守卫(design §5 快照语义:
/// 已建定时任务的 config 是创建时展开的 UUID 阵容快照,自包含、
/// 不悬空,删预设不回溯影响)。不存在也 ok(幂等删除沿 clear 先例)。
pub async fn delete_group_chat_preset_inner(
    state: &Arc<AppState>,
    id: String,
) -> Result<DeletedGroupChatPreset, AppCommandError> {
    gc::delete_group_chat_preset(&state.db, &id)
        .await
        .map_err(|e| anyhow::anyhow!("delete_group_chat_preset failed: {}", e))?;
    Ok(DeletedGroupChatPreset { ok: true })
}

#[tauri::command]
pub async fn delete_group_chat_preset(
    state: State<'_, Arc<AppState>>,
    id: String,
) -> Result<DeletedGroupChatPreset, AppCommandError> {
    delete_group_chat_preset_inner(state.inner(), id).await
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

/// 参与者名字统一 trim 后落库(校验以 trim 形态判定,存储与判定
/// 同形,防「看着重名却查不到」的 whitespace 假象)。
fn normalize_participants(participants: Vec<GcPresetParticipant>) -> Vec<GcPresetParticipant> {
    participants
        .into_iter()
        .map(|p| GcPresetParticipant {
            name: p.name.trim().to_string(),
            model_id: p.model_id.trim().to_string(),
            persona: p.persona,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// 校验矩阵测试(design §6:逐条覆盖校验臂 + 成功路径 + update 语义)
// ---------------------------------------------------------------------------

#[cfg(test)]
#[path = "tests_group_chat_presets.rs"]
mod tests_group_chat_presets;
