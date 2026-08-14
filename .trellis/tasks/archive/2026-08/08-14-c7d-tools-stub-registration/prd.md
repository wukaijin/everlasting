# tools Stub 注册(渐进式披露 D)

## Goal

C7 Phase 2 之 D(触发条件已满足:turn-smoke 实测 tools 占首轮 context 38.5% > 15% 线)。tools[] 初始只发**轻量 stub**(真名 + 一句话描述 + 宽松外壳 schema),另注册常驻 `load_tool_schemas` 元工具,模型按需拉取完整参数契约后真实调用。目标:经典聊天首轮 tools token 从 6773 压到 **≤3500**,省上下文窗口预算(provider 无关)。

用户价值:多 turn 任务每轮省 ~3k 窗口预算,保留更多真历史、延迟 C3 压缩触发。

## Background

- 调研底子:`docs/research/tool-context-progressive-disclosure.md` §3.1(方向② 适用度最高)+ Path 2。C7(R1 度量 + R3 裁剪)已于 2026-08-14 落地归档,`turn_trace.tools_token` 度量链路可直接复用验证收益。
- 实测基线(2026-08-14 turn-smoke):经典 chat 首轮 tools_token=6773 / context_input=17602 = 38.5%。

## Decisions(2026-08-14 brainstorm)

1. **方案 A 保守档**(用户拍板):只 stub 低频重型 **10 个**:`use_ui` / `remember` / `update_checklist` / `web_fetch` / `run_background_shell` / `shell_status` / `shell_kill` / `merge_worker` / `discard_worker` / `request_mode_change`。`ask_user_question` **保持全量**(模型向用户提问的通道,不引入提问前多一轮往返)。核心 7(read/write/edit/shell/grep/glob/list_dir)不动。`use_skill` 移出候选(评审 P2-1,2026-08-14 核实:schema 仅 `skill_name` 单字段 ≈ 百余 token,stub 零收益;且它是 L2 并行白名单(`chat_loop/tools.rs:88` `{read_file,grep,glob,list_dir,use_skill}`)中唯一与候选相交者 — stub 后直呼走并行分支绕过自愈拦截)。**不变量:候选集 ∩ L2 并行白名单 = ∅,单测固化**(结构性防直呼绕过,评审 P1-2)。
2. **app_config 开关 `tools_stub_enabled`,默认开**:一键回退全量 schema(回滚通道)。
3. **适用范围:经典 chat only,gate 是唯一防线**:群聊**复用同一 `run_chat_loop`**(`group_chat_loop.rs:5` 头注释 / `:286` 调用 — 评审 P1-1 核实,修正 C7 归档评审"群聊走独立链"的错误事实),其白名单(`group_chat_prompts.rs:194` `GROUP_CHAT_RESEARCH_TOOLS`)含 `web_fetch`(候选之一)— stubify 与 append 的 gate 必须 `!effective_is_worker && !is_group_chat`(`is_group_chat` 信号 drive.rs:510 现成);worker 同理不 stub(自主可靠性优先)。
4. **粘性 session 级**:loaded-set 记在 AppState 的 session-keyed registry(先例 `BackgroundShellRegistry`),`run_chat_loop` 是 per-request 的不能放 loop-local;加载后不回退(前缀稳定,一次性 invalidation)。
5. **`dispatch_subagent` 不 stub**(本任务范围外):其 def 每 turn 动态重建(subagent enum),单列后续候选。

## Requirements

- **R1 stub 变换环**:`drive.rs` 过滤链第 4 环(mode→workflow→session_type 之后、dispatch append 之后同侧 append),纯函数 `Vec<ToolDef> → Vec<ToolDef>`:候选集内且未 loaded 的工具**原地替换**为 stub(真名 + 一句话描述含"先调 load_tool_schemas"指引 + `input_schema = {"type":"object"}`,保序);已 loaded 保持全量;非候选不动。
- **R2 `load_tool_schemas` 元工具**:不进 `builtin_tools()`(避免渗入 worker/群聊种子集),在 drive.rs stubify 后条件 append(开关开 && 非 worker && 非群聊,放 dispatch_subagent append 同侧之后),同 dispatch append 模式。chat_loop 按名拦截:输入 = 工具名列表(或 `"all"`),写入 loaded-set,tool_result 返回完整 schema JSON 文本。
- **R3 直呼自愈**:模型未 load 就直呼 stub 工具时,chat_loop **serial 路径顶部**统一拦截(先于 :857/:1011/:1099 既有拦截),返回 `is_error: true` tool_result 附完整 schema + "schema now loaded, retry",并顺手写入 loaded-set。并行路径无需拦:候选 ∩ L2 并行白名单 = ∅(Decision 1 不变量)。
- **R4 开关**:每 request 读一次 `get_config_value(pool, "tools_stub_enabled")`(best-effort,缺省 = 开);关 → 第 4 环直通 + 不 append `load_tool_schemas`。
- **R5 registry 生命周期**:`delete_session` 时清理 loaded-set(对齐 `kill_all_for_session` 接线点);daemon 重启丢 loaded-set 可接受(下一条消息重新按需 load,一轮往返成本)。
- **R6 度量**:不改 C7 R1 链路,`turn_trace.tools_token` 自然反映 stub 后体积。

## Acceptance Criteria

- [ ] AC1 `scripts/turn-smoke.sh` 实测:开关开、经典 chat、首轮无 load 调用,`tools_token ≤ 3700`(基线 6773;2026-08-14 用户拍板从 ≤3500 上调,实测 3677)。
- [ ] AC2 e2e:模型调 `load_tool_schemas` 后能按返回的真实 schema 成功调用该工具(至少一个 stub 工具真实跑通)。
- [ ] AC3 不回归:23 工具各自适用场景仍可用(mode/workflow/session_type 过滤不变;群聊白名单不变;`ask_user_question` 等全量工具行为不变);`cargo test -p everlasting --lib` 全绿。
- [ ] AC4 粘性:同一 session 第二条用户消息(新 request)后,已 loaded 工具仍全量下发;`delete_session` 后 loaded-set 清空。
- [ ] AC5 开关:关 `tools_stub_enabled` 后,首轮 tools_token 回到 ~6773 水平,`load_tool_schemas` 不再出现。

## Out of Scope

- 方向① Anthropic Tool Search / ③ invoke_tool 黑盒 / ④ 两阶段侦察 turn(research §3.3/§3.4/Path 3)。
- R2 Anthropic cache 断点(C7 Phase 2 ②,等原生 provider)。
- 群聊 / worker 路径 stub 化;`dispatch_subagent` stub 化(动态 enum,后续候选)。
- memory 指令块治理(BACKLOG §3.1,另行排期)。
