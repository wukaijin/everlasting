# Design — tools Stub 注册(渐进式披露 D)

> 配套 `prd.md`。方案 A 保守档(11 候选)+ `load_tool_schemas` 拦截元工具 + session 级粘性 + app_config 开关。

## 0. 数据流(一图)

```
turn 开始(drive.rs)
  ├─ 三环静态过滤(mode → workflow → session_type)        [现有,不动]
  ├─ R4:每 request 读一次 tools_stub_enabled(缺省 on)
  ├─ 第 4 环 stubify(开关 on && !effective_is_worker && !is_group_chat):
  │     候选集 ∧ 未 loaded → stub(真名+一句话+{"type":"object"})
  │     已 loaded → 全量保持
  ├─ append dispatch_subagent def                          [现有,不动]
  ├─ append load_tool_schemas def(同上 gate,dispatch 同侧之后)[新]
  └─ freeze → provider 下发 → R1 tools_token 估算          [现有,自然反映 stub 体积]

模型调 load_tool_schemas(names | "all")
  → chat_loop 拦截(ask_user_question 同款)→ 写 loaded-set
  → tool_result = 完整 schema JSON 文本
  → 下一 turn 起该工具全量下发(粘性)

模型未 load 直呼 stub 工具
  → chat_loop serial 路径顶部拦截(单点)→ 写 loaded-set
  → tool_result = is_error:true + 完整 schema + "schema now loaded, retry"
```

## 1. Stub 形态(`tools/stub.rs` 新模块)

```rust
pub const STUB_CANDIDATES: [&str; 10] = [ /* prd Decision 1 的 10 个名字 */ ];

/// 纯函数:候选集内未 loaded 的替换为 stub。
/// 实现遍历输入 Vec 原地替换(保序 — tools[] 顺序与 builtin_tools()
/// 注册序一致,顺序扰动会动 provider 前缀缓存;const 数组只是集合,
/// 其顺序不影响输出顺序)。
pub fn stubify(tools: Vec<ToolDef>, loaded: &HashSet<String>) -> Vec<ToolDef>
```

- stub ToolDef:`name` 不变;`description` = 一句话语义摘要 + `Call load_tool_schemas(["<name>"]) to load its full parameter schema before use.`(一句话摘要手工维护在 `stub.rs` 的 const 表里,不从原 description 裁剪 — 确定性);`input_schema` = `{"type":"object"}`。
- **不变量单测**:`STUB_CANDIDATES ∩ {read_file, grep, glob, list_dir, use_skill}(L2 并行白名单)= ∅` — 防未来候选或并行白名单扩员重新引入"直呼绕过 serial 自愈拦截"的洞。
- **静态度量单测**(评审 P2-2):classic-chat 工具集(builtin 23 − R3 裁 2 − workflow 裁 2 + dispatch def)过 stubify 后 `serde_json::to_string` + `count_tokens` 估算,断言 ≤ 3700(**2026-08-14 用户两轮拍板从 ≤3000 上调**;首轮改 3500 后实测仍超:极短摘要 stub 10 个含 JSON 包装 330(非预估 190)+ dispatch 生产 5 模型 enum 984(非预估 500),合计 3675、live 实测 3677;AC1 线随用户拍板定 3700,保留 stub 语义摘要。基线 6773 → 3677:省 3096,-45.7%,tools 占首轮 context 38.5% → 26%)— 第 1 步就锁定可达性,不等烟测暴露。

## 2. `load_tool_schemas` 定义与拦截

- **def**:不进 `builtin_tools()`(那会渗入 worker 种子集 `prepare_worker` 与群聊前的全集),在 drive.rs stubify 后 append:`name = "load_tool_schemas"`,`input_schema = {"type":"object","properties":{"tool_names":{"type":"array","items":{"type":"string"},"description":"tool names to load, or [\"all\"]"}},"required":["tool_names"]}`。
- **拦截**(`chat_loop/tools.rs` **serial 路径顶部**,先于 :857 ask_user_question / :1011 request_mode_change / :1099 request_task_state_transition 既有拦截):`name == "load_tool_schemas"` → 解析 `tool_names`(未知名字以 error 文本列出合法名;`"all"` = 候选集全量)→ 写 registry → tool_result 文本 = `serde_json::to_string_pretty(&[ToolDef])`。纯读 + registry 写,无 Tier 4 语义(Tier 5 静默 Allow,同 `remember`)。
- **直呼自愈**(同位置,execute 前统一拦):`STUB_CANDIDATES.contains(name) && registry 未含 && 开关 on` → 写 registry → 返回 `is_error: true` tool_result(完整 schema + retry 指引)。**并行路径(L2,tools.rs:84)无需拦**:候选 ∩ 并行白名单 = ∅(§1 不变量单测固化)。注意:此分支需要知道"本 turn 该工具确实是 stub 下发"(即走了第 4 环),gate 条件与 drive 侧同源(开关 && !worker && !group_chat),避免关掉开关后误拦真实调用。

## 3. Loaded-set registry(`state.rs`)

- `AppState.stub_loaded: Arc<StubRegistry>`,`StubRegistry = RwLock<HashMap<String /*session_id*/, HashSet<String>>>`,方法:`get(&sid) -> HashSet`(clone)、`extend(&sid, names)`、`clear(&sid)`。
- 先例:`BackgroundShellRegistry`(`background_shell/mod.rs:114`)。不抽 trait — 只有一个 in-memory 实现,YAGNI。
- 生命周期:`delete_session_inner` 里 `clear(sid)`(对齐 `kill_all_for_session` 接线点);daemon 重启自然清空(prd R5 接受)。
- chat_loop 拦截点拿 registry:经 `run_chat_loop` 参数传递(同 `subagent_cache` / `group_chat_state` 的穿参模式),不塞 ToolContext(拦截不走 execute_tool_inner)。

## 4. 开关(app_config)

- key `tools_stub_enabled`,`"false"` 才关(缺省/读失败 = 开;fail-open 语义安全 — stub 不删能力只延迟披露)。
- 读点:`run_chat_loop` 顶部每 request 一次 `db::config::get_config_value(pool, "tools_stub_enabled")`(`db/config.rs:37`),不 per-turn 读。
- 设置 UI 不动(MVP 用 DB 直改/后续补 UI;开关本身是回滚通道,低频)。

## 5. 红线对照(落地前确认不违反)

| 红线 | 落点 | 对照 |
|---|---|---|
| tools[] 同 session 连续 turn 稳定(C7 R3.2) | 粘性:加载后不回退 | 一次性 invalidation(load 后下一 turn),之后稳定 ✓ |
| worker 不得见 dispatch_subagent 嵌套 | stubify/append gate `!effective_is_worker` | 同款 gate(drive.rs:544 先例)✓ |
| 群聊白名单 `group_chat_tool_defs` 不变 | **群聊复用同一 `run_chat_loop`**(`group_chat_loop.rs:5/286`,评审 P1-1 核实 — 修正 C7 归档评审"独立链"错误事实);白名单 `GROUP_CHAT_RESEARCH_TOOLS`(`group_chat_prompts.rs:194`)含候选 `web_fetch` | **gate 是唯一防线**:stubify/append 均加 `!is_group_chat`(信号 drive.rs:510 现成)✓ |
| L2 并行只读 batch 不变 | stub 工具真实执行仍走原 eligibility | 拦截只发生在 load/直呼两个名字分支 ✓ |
| Plan 模式砍写工具 | stubify 在三环**之后** | Plan 已砍的不会复活 ✓ |
| 交互工具不裁能力(C7 R3.1 原则) | `ask_user_question` 保持全量 | 方案 A 明确排除 ✓ |

## 6. 风险 + 回滚

| 风险 | 缓解 |
|---|---|
| 模型不看 stub 指引、反复直呼不 load | 直呼自愈分支:第一次直呼即回灌 schema 并 loaded,无需模型听话 |
| 某候选工具实际高频(如 web_fetch),load 往返变常态 | 观测:turn_trace + 审计 load 调用频次;候选集是 const 表,一PR 可调 |
| 未来候选扩员撞上 L2 并行白名单(直呼绕过自愈) | §1 不变量单测 `候选 ∩ 并行白名单 = ∅` 固化,撞上即测试红 |
| 每 request 一次 config 读 | 单行 kv 读,μs 级;且与 workflow_enabled(session 列)同数量级 |
| 前端 TracePanel tools 占比突变 | 非破坏:数值变小是预期;无前端改动 |

回滚 = `tools_stub_enabled = "false"`(运行时直通);代码回滚单 commit(新模块 + 三处接线)。

## 7. 不做(Phase 3+ / OOS,见 prd)

`dispatch_subagent` stub 化(动态 enum)、worker/群聊 stub 化、load 频次治理(按使用历史预载)、设置 UI 开关。
