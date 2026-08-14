## Scenario: tools Stub 注册(渐进式披露 D,C7 Phase 2,2026-08-14)

> 配套 task `08-14-c7d-tools-stub-registration`。把低频重型工具的完整
> schema 从首轮 `tools[]` 移出,换轻量 stub + `load_tool_schemas` 元
> 工具按需拉取。目标:经典 chat 首轮 tools token 6773 → ≤3500。

### 1. Scope / Trigger

- Trigger:C7 R1 度量显示 tools 占首轮 context 38.5% > 15% 线。
- 方案 A 保守档(用户拍板):只 stub **10 个低频重型工具**
  (`use_ui` / `remember` / `update_checklist` / `web_fetch` /
  `run_background_shell` / `shell_status` / `shell_kill` / `merge_worker` /
  `discard_worker` / `request_mode_change`);核心 7 工具
  (read/write/edit/shell/grep/glob/list_dir)与 `ask_user_question`
  (提问通道,不引入提问前多一轮往返)不动;`use_skill` **移出候选**
  (schema 单字段 stub 零收益,且是 L2 并行白名单中唯一与候选相交者 —
  移出后候选 ∩ 并行白名单 = ∅,直呼自愈无需拦并行路径)。

### 2. Stub 形态(`tools/stub.rs`)

```rust
pub const STUB_CANDIDATES: [&str; 10];   // 集合语义,顺序不影响输出
pub fn stubify(tools: Vec<ToolDef>, loaded: &HashSet<String>) -> Vec<ToolDef>;
pub fn load_tool_schemas_def() -> ToolDef;
```

- stub ToolDef:`name` 不变;`description` = 极短语义摘要 + `load_tool_schemas([name]) first.`;
  `input_schema` = `{"type":"object"}`(宽松外壳)。**原地替换保序** — tools[]
  顺序与 `builtin_tools()` 注册序一致,顺序扰动会动 provider 前缀缓存断点(C7 R3.2)。
- 静态度量单测:classic-chat 集 stubify 后 `count_tokens ≤ 3700`(**2026-08-14
  用户两轮拍板从 ≤3000 上调**:首轮改 3500 后实测仍超 — 极短摘要 stub 10 个
  含 JSON 包装 330(非预估 190)+ dispatch 生产 5 模型 enum 984(非预估
  500),合计 3675、live 实测 3677;AC1 线随用户拍板定 3700。基线 6773 →
  3677:省 3096,-45.7%,tools 占首轮 context 38.5% → 26%)。
- 不变量单测:`STUB_CANDIDATES ∩ {read_file,grep,glob,list_dir,use_skill} = ∅` —
  防未来候选/白名单扩员重新引入「直呼绕过 serial 自愈拦截」的洞。

### 3. `load_tool_schemas` 契约

- **不进** `builtin_tools()`(避免渗入 worker 种子集 `prepare_worker` 与群聊前
  的全集);drive.rs stubify 后、dispatch_subagent append 之后同侧 append
  (gate 同 stubify)。`input_schema` = `{"tool_names": ["..."] | ["all"]}`。
- **拦截**(chat_loop/tools.rs **serial 路径顶部**,先于 ask_user_question /
  request_mode_change / request_task_state_transition 既有拦截):解析
  `tool_names`(`"all"` = 候选集全量;未知名字以 error 文本列出合法名)→ 写
  session-keyed loaded-set registry → tool_result = 完整 schema pretty JSON。
  纯读 + registry 写,无 Tier 4 语义(Tier 5 静默 Allow,同 `remember`)。
- **直呼自愈**(同位置,execute 前统一拦):`STUB_CANDIDATES.contains(name)`
  && registry 未含 && 开关 on → 写 registry → `is_error: true` tool_result
  (完整 schema + "schema now loaded, retry")。并行路径(L2)**无需拦**:
  候选 ∩ 并行白名单 = ∅(不变量单测固化)。
- gate(与 drive 侧 stubify/append 同源):`开关 && !permission_ctx.is_worker
  && group_chat_state.is_none()`。

### 4. 粘性 registry(`tools/stub.rs::StubRegistry`)

- `AppState.stub_loaded: Arc<StubRegistry>`,`RwLock<HashMap<session_id,
  HashSet<tool_name>>>`,`get` / `extend` / `clear`。跨 request 存活
  (同 session 第二条用户消息后已 loaded 工具仍全量下发 — AC4);
  `delete_session_inner` 清空(对齐 `kill_all_for_session` 接线点);
  daemon 重启自然清空(下一条消息重新按需 load,一轮往返成本)。

### 5. 开关(app_config)

- key `tools_stub_enabled`,`"false"` 才关(缺省/读失败 = 开;fail-open 语义
  安全 — stub 不删能力只延迟披露)。读点:`run_chat_loop` 顶部每 request 一次
  `db::config::get_config_value(pool, "tools_stub_enabled")`,不 per-turn 读。

### 6. 红线(落地确认不违反)

| 红线 | 落点 |
|---|---|
| tools[] 同 session 连续 turn 稳定(C7 R3.2) | 粘性:加载后不回退(一次性 invalidation) |
| worker 不得见 dispatch_subagent 嵌套 | stubify/append gate `!effective_is_worker` |
| 群聊白名单 `group_chat_tool_defs` 不变 | **群聊复用同一 `run_chat_loop`**(`group_chat_loop.rs:286/:478`);白名单含候选 `web_fetch` — gate 加 `!is_group_chat` 是唯一防线 |
| L2 并行只读 batch 不变 | 候选 ∩ 并行白名单 = ∅(不变量单测);stub 工具真实执行仍走原 eligibility |
| Plan 模式砍写工具 | stubify 在三环(mode→workflow→session_type)**之后** |
| 交互工具不裁能力(C7 R3.1 原则) | `ask_user_question` 保持全量 |

### 7. 测试要点

- stubify 纯函数:候选替换形态 / loaded 保持 / 非候选不动 / 保序 / 不变量 /
  静态度量(stub.rs 8 测)。
- 集成(agent/tests_agent_loop/stub.rs 5 测):load_tool_schemas 拦截 +
  粘性(跨 request)/ 直呼自愈 / 开关关全量无元工具 / worker 不 stub /
  **群聊不 stub**(gate `!is_group_chat` 回归锚)。
- 既有测试默认 `tools_stub_enabled=false`(make_harness 写入)— 直呼候选工具
  的旧语义不受新拦截影响;stub 专项测试显式开。
