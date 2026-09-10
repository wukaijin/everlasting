//! 4 槽位植入开关(2026-09-10 hard switch PR2,任务
//! `09-10-memory-everlasting-md-hard-switch`;评审 session `55838776`
//! P0 #2/#4)。
//!
//! ## 为什么过滤在 cache 读出之后、块组装之前,而不是
//! `load_for_session` 出口或 cache 槽内
//!
//! - **不在 cache 槽内**:cache 存原始 `Loaded` 层。开关值不走
//!   mtime fence——若 Disabled 进缓存,翻开关不动文件 mtime,禁用
//!   态会在 cache 里滞留到下次文件编辑。
//! - **不在 `load_for_session` 出口单点**:`execute_load_memory_
//!   sections`(digest 元工具)从 mtime-fence cache 现取层、不经
//!   `load_for_session`,出口过滤会被工具通道穿透(模型直呼
//!   `load_memory_sections(["all"])` 仍能拉到已禁用层全文)。
//! - **正确位置 = 四个 db 持有方调用点**(评审裁定"逻辑一处、
//!   读取按生效语义分布"):
//!   1. `chat_loop/init.rs` 经 `load_for_session_frozen`(flags 在
//!      freeze 快照形成**之前**施加——"下一会话生效"由 freeze 机制
//!      强制,不是文档约定);
//!   2. `subagent/prompt.rs` dispatch 时读(下一 dispatch 生效);
//!   3. digest executor 调用时读(穿透通道关闭);
//!   4. `commands::memory::read_memory_layers`(Preview"已禁用"徽标
//!      的数据通道)。
//!
//! ## 存储语义
//!
//! `app_config` KV,4 key 常量单源在本文件(照
//! `permissions::ask::ASK_NO_TIMEOUT_KEY` 形态;`SETTABLE_APP_FLAGS`
//! 白名单与 Settings UI 引用这组常量,杜绝双边字面量)。fail-open:
//! 仅字面 `"false"` 关,读法与 `memory_digest_enabled` 等先例一致。

use sqlx::SqlitePool;

use super::types::{LayerStatus, MemoryKind, MemoryLayer, MemorySource};

/// app_config key:User 层 EVERLASTING.md 注入开关。
pub const KEY_USER_EVERLASTING: &str = "memory_user_everlasting_enabled";
/// app_config key:User 层 AGENTS.md 注入开关。
pub const KEY_USER_AGENTS: &str = "memory_user_agents_enabled";
/// app_config key:Project 层 EVERLASTING.md 注入开关。
pub const KEY_PROJECT_EVERLASTING: &str = "memory_project_everlasting_enabled";
/// app_config key:Project 层 AGENTS.md 注入开关。
pub const KEY_PROJECT_AGENTS: &str = "memory_project_agents_enabled";

/// 4 槽位注入开关快照。`Copy`,一次 DB 读出后按值传递。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemorySlotFlags {
    pub user_everlasting: bool,
    pub user_agents: bool,
    pub project_everlasting: bool,
    pub project_agents: bool,
}

impl Default for MemorySlotFlags {
    fn default() -> Self {
        Self::all_on()
    }
}

impl MemorySlotFlags {
    /// 全开(缺省)。不写任何 key 时与开关机制落地前行为逐字节
    /// 一致(单测锁)。
    pub const fn all_on() -> Self {
        Self {
            user_everlasting: true,
            user_agents: true,
            project_everlasting: true,
            project_agents: true,
        }
    }

    /// 从 `app_config` 读 4 key,fail-open(仅字面 `"false"` 关;
    /// key 缺失 / DB 错误 → 开)。读法与 `memory_digest_enabled`
    /// 先例一致——开关自身坏掉不能瘫痪记忆注入。
    pub async fn read(db: &SqlitePool) -> Self {
        let read_one = |key: &'static str| async move {
            match crate::db::config::get_config_value(db, key).await {
                Ok(Some(v)) => v != "false",
                _ => true,
            }
        };
        let (user_everlasting, user_agents, project_everlasting, project_agents) = tokio::join!(
            read_one(KEY_USER_EVERLASTING),
            read_one(KEY_USER_AGENTS),
            read_one(KEY_PROJECT_EVERLASTING),
            read_one(KEY_PROJECT_AGENTS),
        );
        Self {
            user_everlasting,
            user_agents,
            project_everlasting,
            project_agents,
        }
    }

    /// 该 `(kind, source)` 槽位是否启用。
    fn enabled(self, kind: MemoryKind, source: MemorySource) -> bool {
        match (kind, source) {
            (MemoryKind::User, MemorySource::Everlasting) => self.user_everlasting,
            (MemoryKind::User, MemorySource::Agents) => self.user_agents,
            (MemoryKind::Project, MemorySource::Everlasting) => self.project_everlasting,
            (MemoryKind::Project, MemorySource::Agents) => self.project_agents,
            // V2 2 期 forward-compat 槽位:不受 4 槽位开关管辖
            //(与 `resolve_path` 返回 None 的口径一致,永不加载)。
            (MemoryKind::Session | MemoryKind::Runtime, _) => true,
        }
    }
}

/// 单点过滤:命中关闭槽位 → 该层降级为 `LayerStatus::Disabled`
/// (content/tokens 清零,path 保留供前端定位),Vec 长度与 canonical
/// 索引不变——`load_for_session` 恒返 4 元 Vec 是 agent loop 的结构
/// 契约(loader.rs:197),banner / 注入块 / `memory_token` 都按
/// `status == Loaded` 过滤,Disabled 天然被跳过(评审 P0 #3)。
pub fn apply_slot_flags(layers: Vec<MemoryLayer>, flags: &MemorySlotFlags) -> Vec<MemoryLayer> {
    if *flags == MemorySlotFlags::all_on() {
        // 快路径:全开时逐字节返回原 Vec(零 clone 扰动)。
        return layers;
    }
    layers
        .into_iter()
        .map(|mut l| {
            if !flags.enabled(l.kind, l.source)
                && matches!(l.status, LayerStatus::Loaded | LayerStatus::Missing)
            {
                l.status = LayerStatus::Disabled;
                l.content = String::new();
                l.tokens = 0;
            }
            l
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn layer(kind: MemoryKind, source: MemorySource, status: LayerStatus) -> MemoryLayer {
        MemoryLayer {
            kind,
            source,
            path: std::path::PathBuf::from("/fake"),
            content: if matches!(status, LayerStatus::Loaded) {
                "body".to_string()
            } else {
                String::new()
            },
            tokens: if matches!(status, LayerStatus::Loaded) {
                42
            } else {
                0
            },
            status,
        }
    }

    fn four_loaded() -> Vec<MemoryLayer> {
        vec![
            layer(
                MemoryKind::User,
                MemorySource::Everlasting,
                LayerStatus::Loaded,
            ),
            layer(MemoryKind::User, MemorySource::Agents, LayerStatus::Loaded),
            layer(
                MemoryKind::Project,
                MemorySource::Everlasting,
                LayerStatus::Loaded,
            ),
            layer(
                MemoryKind::Project,
                MemorySource::Agents,
                LayerStatus::Loaded,
            ),
        ]
    }

    /// 全开 = 恒等(与 main 行为逐字节一致,AC 回滚锚)。
    #[test]
    fn all_on_is_identity() {
        let layers = four_loaded();
        let out = apply_slot_flags(layers.clone(), &MemorySlotFlags::all_on());
        assert_eq!(out.len(), 4);
        for (a, b) in layers.iter().zip(out.iter()) {
            assert_eq!(a.status, b.status);
            assert_eq!(a.content, b.content);
        }
    }

    /// 关一个槽位:该层 Disabled(content/tokens 清零、path 保留),
    /// 其余层与 Vec 索引不变。
    #[test]
    fn disabled_slot_demotes_only_that_layer() {
        let flags = MemorySlotFlags {
            project_everlasting: false,
            ..MemorySlotFlags::all_on()
        };
        let out = apply_slot_flags(four_loaded(), &flags);
        assert_eq!(out.len(), 4);
        assert!(matches!(out[2].status, LayerStatus::Disabled));
        assert_eq!(out[2].content, "");
        assert_eq!(out[2].tokens, 0);
        assert_eq!(out[2].path, std::path::PathBuf::from("/fake"));
        for (i, l) in out.iter().enumerate() {
            if i != 2 {
                assert!(matches!(l.status, LayerStatus::Loaded), "index {i}");
            }
        }
    }

    /// Missing 层关开关 → Disabled(前端徽标需要区分"没建文件"与
    /// "建了但被关");Error 层不动(错误态优先,便于排障)。
    #[test]
    fn missing_becomes_disabled_error_stays() {
        let layers = vec![
            layer(
                MemoryKind::User,
                MemorySource::Everlasting,
                LayerStatus::Missing,
            ),
            layer(
                MemoryKind::User,
                MemorySource::Agents,
                LayerStatus::Error { reason: "x".into() },
            ),
            layer(
                MemoryKind::Project,
                MemorySource::Everlasting,
                LayerStatus::Missing,
            ),
            layer(
                MemoryKind::Project,
                MemorySource::Agents,
                LayerStatus::Missing,
            ),
        ];
        let flags = MemorySlotFlags {
            user_everlasting: false,
            project_agents: false,
            ..MemorySlotFlags::all_on()
        };
        let out = apply_slot_flags(layers, &flags);
        assert!(matches!(out[0].status, LayerStatus::Disabled));
        assert!(
            matches!(out[1].status, LayerStatus::Error { .. }),
            "Error 不被 Disabled 覆盖"
        );
        assert!(matches!(out[2].status, LayerStatus::Missing));
        assert!(matches!(out[3].status, LayerStatus::Disabled));
    }
}
