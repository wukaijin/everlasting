//! tools Stub 注册(渐进式披露 D,C7 Phase 2,08-14-c7d-tools-stub-registration)。
//!
//! 经典 chat 的 `tools[]` 初始只发**轻量 stub**(真名 + 极短摘要 +
//! 宽松外壳 schema `{"type":"object"}`),另注册常驻 `load_tool_schemas`
//! 元工具,模型按需拉取完整参数契约后真实调用。目标:首轮 tools token
//! 从 6773 压到 ≤3700(AC1,2026-08-14 用户拍板;静态度量单测锁 ≤3700,
//! 校准依据见测试注释)。
//!
//! 三条结构不变量(单测固化):
//! 1. `STUB_CANDIDATES` **不含** `use_skill`(评审 P2-1:schema 单字段,
//!    stub 零收益;且它是 L2 并行白名单 `{read_file,grep,glob,list_dir,
//!    use_skill}` 中唯一与候选相交者 — 移出后候选集 ∩ 并行白名单 = ∅,
//!    并行路径无需拦截,直呼自愈只在 serial 顶部单点完成)。
//! 2. `stubify` **原地替换保序** — `tools[]` 顺序与 `builtin_tools()`
//!    注册序一致,顺序扰动会动 provider 前缀缓存断点(C7 R3.2 稳定性
//!    前提;`STUB_CANDIDATES` 只是集合,其 const 顺序不影响输出顺序)。
//! 3. stub 工具真实执行仍走原 eligibility/权限链(拦截只发生在
//!    load_tool_schemas / 直呼自愈两个名字分支)。
//!
//! 适用范围 gate(drive 侧 stubify + append、chat_loop 侧拦截同源):
//! `开关开 && !effective_is_worker && !is_group_chat` — 群聊复用同一
//! `run_chat_loop`(`group_chat_loop.rs:286/:478`)且白名单含候选
//! `web_fetch`,worker 自主可靠性优先不 stub。

use std::collections::{HashMap, HashSet};

use tokio::sync::RwLock;

use crate::llm::types::ToolDef;

/// 候选 stub 工具集(prd Decision 1 的 10 个,方案 A 保守档)。顺序是
/// 集合语义,不影响 `stubify` 输出顺序(见模块注释不变量 2)。
pub const STUB_CANDIDATES: [&str; 10] = [
    "use_ui",
    "remember",
    "update_checklist",
    "web_fetch",
    "run_background_shell",
    "shell_status",
    "shell_kill",
    "merge_worker",
    "discard_worker",
    "request_mode_change",
];

/// L2 并行只读白名单(chat_loop/tools.rs `is_parallel_eligible`)。
/// 不变量单测:`STUB_CANDIDATES ∩ PARALLEL_WHITELIST = ∅` — 防未来
/// 候选或白名单扩员重新引入「stub 直呼走并行分支、绕过 serial 自愈
/// 拦截」的洞(评审 P1-2)。
///
/// `#[allow(dead_code)]`:生产代码只读 `STUB_CANDIDATES`,白名单仅被
/// 测试引用 — 但它是**不变量的权威定义**,必须与候选集同源常驻
/// (白名单真实值在 `chat_loop/tools.rs` 注释里,这里固化断言用)。
#[allow(dead_code)]
pub const PARALLEL_WHITELIST: [&str; 5] = ["read_file", "grep", "glob", "list_dir", "use_skill"];

/// 每工具超短语义摘要(手工维护,不从原 description 裁剪 — 确定性;
/// 摘要末尾统一附 load_tool_schemas 指引)。长度受 AC1 预算约束
/// (静态度量单测锁定 ≤3700,2026-08-14 用户两轮拍板)。
const STUB_DESCRIPTIONS: [(&str, &str); 10] = [
    ("use_ui", "Render interactive UI cards in the chat."),
    ("remember", "Write a long-term memory."),
    ("update_checklist", "Replace the current checklist."),
    ("web_fetch", "Fetch a URL."),
    ("run_background_shell", "Start a background shell."),
    ("shell_status", "Query a background shell."),
    ("shell_kill", "Kill a background shell."),
    ("merge_worker", "Merge a worker's branch."),
    ("discard_worker", "Discard a worker's branch."),
    ("request_mode_change", "Request a mode switch."),
];

/// Stub 形态的一句话描述:极短语义摘要 + 明确的「先 load」指引。
/// 长度受 AC1 预算约束(静态度量单测锁定,见下;极短摘要方案经用户
/// 拍板,2026-08-14)。
fn stub_description(name: &str) -> String {
    let summary = STUB_DESCRIPTIONS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, d)| *d)
        .unwrap_or("(no summary)");
    format!("{summary} load_tool_schemas([\"{name}\"]) first.")
}

/// 纯函数:候选集内**未 loaded** 的工具**原地替换**为 stub(真名 +
/// 一句话描述 + 宽松外壳 schema),已 loaded 保持全量,非候选不动。
/// 保序(遍历输入 Vec 替换,顺序与 `builtin_tools()` 注册序一致 —
/// 前缀缓存前提,C7 R3.2)。
pub fn stubify(tools: Vec<ToolDef>, loaded: &HashSet<String>) -> Vec<ToolDef> {
    tools
        .into_iter()
        .map(|mut t| {
            if STUB_CANDIDATES.contains(&t.name.as_str()) && !loaded.contains(&t.name) {
                t.description = Some(stub_description(&t.name));
                t.input_schema = serde_json::json!({"type": "object"});
            }
            t
        })
        .collect()
}

/// `load_tool_schemas` 元工具 def。**不进** `builtin_tools()`(那会渗入
/// worker 种子集 `prepare_worker` 与群聊前的全集);由 drive.rs 在
/// stubify 后按 gate(`开关 && !worker && !群聊`)条件 append。
pub fn load_tool_schemas_def() -> ToolDef {
    ToolDef {
        name: "load_tool_schemas".to_string(),
        description: Some(
            "Load the full parameter schema for one or more stubbed tools (schemas are \
             omitted until requested). Pass the tool names to load, or [\"all\"] for every \
             stubbed tool; the full schema JSON is returned."
                .to_string(),
        ),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "tool_names": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Tool names to load, or [\"all\"] for every stubbed tool."
                }
            },
            "required": ["tool_names"]
        }),
    }
}

/// Session → loaded-set 注册表(design §3)。挂 `AppState.stub_loaded`,
/// 跨 request 存活(同 session 第二条用户消息后已 loaded 工具仍全量
/// 下发 — 粘性,AC4);`delete_session` 清理;daemon 重启自然清空
/// (prd R5 接受:下一条消息重新按需 load,一轮往返成本)。
///
/// 不抽 trait — 只有一个 in-memory 实现(YAGNI);先例
/// `BackgroundShellRegistry` 是 session-keyed 进程级 registry 的同类,
/// 但那是 `Arc<dyn>` + async 方法,这里是普通 struct(纯内存 HashMap,
/// 无 I/O,不需要 dyn 抽象)。
#[derive(Default)]
pub struct StubRegistry {
    inner: RwLock<HashMap<String, HashSet<String>>>,
}

impl StubRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// 返回 session 当前已 loaded 的工具名集合(clone)。
    pub async fn get(&self, session_id: &str) -> HashSet<String> {
        self.inner
            .read()
            .await
            .get(session_id)
            .cloned()
            .unwrap_or_default()
    }

    /// 把名字并入 session 的 loaded-set(幂等)。
    pub async fn extend(&self, session_id: &str, names: impl IntoIterator<Item = String>) {
        let mut guard = self.inner.write().await;
        let entry = guard.entry(session_id.to_string()).or_default();
        for n in names {
            entry.insert(n);
        }
    }

    /// 清空 session 的 loaded-set(`delete_session` 接线点,对齐
    /// `kill_all_for_session`)。
    pub async fn clear(&self, session_id: &str) {
        self.inner.write().await.remove(session_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::ToolDef;

    /// 全量真实 builtin 工具集(23 个,含全部候选)。
    fn all_builtins() -> Vec<ToolDef> {
        crate::tools::builtin_tools()
    }

    // ------------------------------------------------------------------
    // stubify 纯函数形态
    // ------------------------------------------------------------------

    #[test]
    fn candidate_unloaded_replaced_with_stub_shape() {
        let tools = all_builtins();
        let stubbed = stubify(tools, &HashSet::new());
        for name in STUB_CANDIDATES {
            let def = stubbed
                .iter()
                .find(|d| d.name == name)
                .unwrap_or_else(|| panic!("candidate {name} missing from stubified set"));
            assert_eq!(def.name, name, "stub 保留真名");
            let desc = def.description.as_ref().unwrap();
            assert!(
                desc.contains("load_tool_schemas"),
                "stub 描述须含 load_tool_schemas 指引: {name}"
            );
            assert_eq!(
                def.input_schema,
                serde_json::json!({"type": "object"}),
                "stub schema 应为宽松外壳: {name}"
            );
        }
    }

    #[test]
    fn loaded_candidates_keep_full_schema() {
        let tools = all_builtins();
        let loaded: HashSet<String> = STUB_CANDIDATES
            .iter()
            .take(3)
            .map(|s| s.to_string())
            .collect();
        let stubbed = stubify(tools, &loaded);
        for name in &loaded {
            let def = stubbed.iter().find(|d| d.name == *name).unwrap();
            assert_ne!(
                def.input_schema,
                serde_json::json!({"type": "object"}),
                "已 loaded 工具应保持全量 schema: {name}"
            );
        }
    }

    #[test]
    fn non_candidates_untouched() {
        let tools = all_builtins();
        let stubbed = stubify(tools, &HashSet::new());
        for name in [
            "read_file",
            "write_file",
            "edit_file",
            "shell",
            "grep",
            "glob",
            "list_dir",
        ] {
            let def = stubbed.iter().find(|d| d.name == name).unwrap();
            let full = all_builtins();
            let full_def = full.iter().find(|d| d.name == name).unwrap();
            assert_eq!(
                def.description, full_def.description,
                "非候选描述不动: {name}"
            );
            assert_eq!(
                def.input_schema, full_def.input_schema,
                "非候选 schema 不动: {name}"
            );
        }
    }

    #[test]
    fn stubify_preserves_order() {
        // 保序 = tools[] 顺序稳定(前缀缓存前提)。全量集过 stubify 后
        // 顺序与 builtin_tools() 注册序逐位一致。
        let full = all_builtins();
        let original_order: Vec<String> = full.iter().map(|d| d.name.clone()).collect();
        let stubbed = stubify(full, &HashSet::new());
        let stubbed_order: Vec<String> = stubbed.iter().map(|d| d.name.clone()).collect();
        assert_eq!(stubbed_order, original_order, "stubify 必须原地保序");
    }

    #[test]
    fn ask_user_question_not_in_candidates() {
        // prd Decision 1:模型向用户提问的通道保持全量(不引入提问前
        // 多一轮往返)。
        assert!(
            !STUB_CANDIDATES.contains(&"ask_user_question"),
            "ask_user_question 必须排除在候选外"
        );
    }

    // ------------------------------------------------------------------
    // 不变量单测(评审 P1-2):候选集 ∩ L2 并行白名单 = ∅
    // ------------------------------------------------------------------

    #[test]
    fn candidates_disjoint_from_parallel_whitelist() {
        for c in STUB_CANDIDATES {
            assert!(
                !PARALLEL_WHITELIST.contains(&c),
                "候选 {c} 撞上 L2 并行白名单 — stub 直呼会走并行分支绕过 \
                 serial 自愈拦截;必须移出候选或扩白名单(评审 P1-2)"
            );
        }
    }

    // ------------------------------------------------------------------
    // 静态度量单测(评审 P2-2):classic-chat 集 stubify 后 ≤ 3000
    // (AC1 ≤3500 留 live 余量,第 1 步就锁定可达性)
    // ------------------------------------------------------------------

    /// classic-chat 真实首轮工具集:builtin 23 − R3 裁 2(nominate/end)
    /// − workflow 裁 2(create_task/request_task_state_transition)+
    /// dispatch_subagent 动态 def + load_tool_schemas def。应用
    /// R3/R4 后 stubify,count_tokens ≤ 3960。
    ///
    /// **预算校准(2026-08-14,用户两轮拍板)**:
    /// 1. 原 design ≤3000 在 Edit 模式(AC1 实测模式)数学不可达 —
    ///    核心 9 工具全量 2261 + dispatch 真实 def 984(实测)= 3245,
    ///    零 stub 已超 3000。静态线首轮改 3500,stub 走「极短摘要 +
    ///    load 指引」。
    /// 2. 3500 仍不可达:极短摘要 stub 10 个含 JSON 包装 = 330(非
    ///    预估的 190),dispatch 生产 5 模型 enum = 984,合计 3675,
    ///    实测 turn-smoke = 3677。**AC1 线随用户拍板调 3700**,保留
    ///    stub 语义摘要(177 tok 差距不值得牺牲「模型 load 前知道
    ///    工具干嘛」)。基线 6773 → 3677:省 3096,削减 45.7%,
    ///    tools 占首轮 context 38.5% → 26%。
    /// 3. **3700 → 3900(2026-08-17,D2②)**:注册 `search_history`
    ///    (agent 驱动跨 session 全文搜索)实测 +178 tok → 3855。该
    ///    工具非 stub 候选(3 参数 schema;recall 类工具首次直用优
    ///    于先 load 再重试),且 stub 化也只省 ~140(3715 仍超线)—
    ///    线随新增注册工具基线整体平移,沿用校准 2 的「实测 > 预估
    ///    线」先例。若未来再注册新工具逼近 3900,优先评估扩
    ///    STUB_CANDIDATES 而非继续平移。
    /// 4. **3900 → 3960(2026-08-21,b1-image-followups R3)**:
    ///    read_file description 增补图片块说明(向 LLM 披露新能力,
    ///    ~+48 tok → 实测 3903)。read_file 是核心工具不 stub 化;
    ///    披露文案已精简到最短。线随既有工具描述增长平移。
    /// dispatch 校准:生产 `model_briefs = list_models`(实测 5 模型),
    /// 用真实 display_name 列表模拟(不是 2 模型的低估值)。
    #[tokio::test]
    async fn static_token_budget_classic_chat_first_turn() {
        let tools = crate::tools::filter_tools_for_session_type(
            crate::tools::filter_tools_for_workflow(all_builtins(), false),
            false,
        );
        let mut defs = stubify(tools, &HashSet::new());
        // dispatch_subagent def 是每 turn 动态重建的(subagent enum +
        // model enum)。用真实 `definition_with_cache`(空项目 = 2
        // builtin available line)+ 生产 5 模型 display_name 复刻
        // drive.rs 生产路径(实测 984 tok)。
        let cache = crate::agent::subagent::SubagentCache::arc();
        let tmp = tempfile::TempDir::new().unwrap();
        let models: Vec<crate::agent::subagent::ModelBrief> = [
            ("a0d9a925-a548-4ba9-a175-77d288e73346", "MiniMax-M2.7"),
            ("42274366-4447-47c7-957b-c2832b0edf52", "MiniMax-M3"),
            ("fd926ed1-67e7-4678-92c1-2ece904a3f9d", "glm-5.2"),
            ("b8d0abc2-f894-4850-ad76-2d66558923a1", "deepseek-v4-flash"),
            ("594d87b7-5734-4584-b5da-db3bd5182284", "glm-5.3"),
        ]
        .iter()
        .map(|(id, name)| crate::agent::subagent::ModelBrief {
            id: id.to_string(),
            display_name: name.to_string(),
        })
        .collect();
        let dispatch = crate::agent::subagent::definition_with_cache(
            &cache,
            tmp.path().to_str().unwrap(),
            None,
            &models,
        )
        .await;
        defs.push(dispatch);
        defs.push(load_tool_schemas_def());
        let json = serde_json::to_string(&defs).unwrap_or_default();
        let tokens = crate::memory::tokens::count_tokens(&json).await;
        assert!(
            tokens <= 3960,
            "classic-chat 首轮 stubified tools[] 估算 {tokens} tok > 3960(AC1 线,校准史见上)"
        );
    }

    // ------------------------------------------------------------------
    // StubRegistry
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn registry_get_extend_clear() {
        let r = StubRegistry::new();
        assert!(r.get("s1").await.is_empty());
        r.extend("s1", ["web_fetch".to_string(), "remember".to_string()])
            .await;
        let loaded = r.get("s1").await;
        assert!(loaded.contains("web_fetch") && loaded.contains("remember"));
        // session 隔离
        assert!(r.get("s2").await.is_empty());
        // 幂等 extend
        r.extend("s1", ["web_fetch".to_string()]).await;
        assert_eq!(r.get("s1").await.len(), 2);
        // clear
        r.clear("s1").await;
        assert!(r.get("s1").await.is_empty());
    }
}
