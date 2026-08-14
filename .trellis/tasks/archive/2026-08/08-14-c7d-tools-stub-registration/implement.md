# Implement — tools Stub 注册(渐进式披露 D)

> 配套 `prd.md` + `design.md`。执行顺序:纯函数 → registry → 接线 → 拦截 → 测试 → live 验证。

## 执行顺序

1. **`tools/stub.rs` 新模块**(纯函数,零接线风险):
   - `STUB_CANDIDATES: [&str; 10]`(prd Decision 1;**不含 `use_skill`** — 评审 P2-1)+ 每工具一句话摘要 const 表。
   - `pub fn stubify(tools: Vec<ToolDef>, loaded: &HashSet<String>) -> Vec<ToolDef>`(**原地替换保序** — tools[] 顺序稳定是前缀缓存前提)。
   - `pub fn load_tool_schemas_def() -> ToolDef`。
   - 单测:候选替换形态 / loaded 保持 / 非候选不动 / 保序断言。
   - **不变量单测**(评审 P1-2):`STUB_CANDIDATES ∩ {read_file,grep,glob,list_dir,use_skill} = ∅`。
   - **静态度量单测**(评审 P2-2):classic-chat 集(builtin 23 − R3 裁 2 − workflow 裁 2 + dispatch def)stubify 后 `count_tokens(to_string(...))` ≤ 3700(**2026-08-14 用户两轮拍板从 ≤3000 上调**,依据见 design §1 校准注记)— AC1 可达性第 1 步锁定,不等 live。
2. **`state.rs` StubRegistry**:
   - `StubRegistry(RwLock<HashMap<String, HashSet<String>>>)` + `get/extend/clear`。
   - `AppState` 加字段 `stub_loaded: Arc<StubRegistry>`(构造点对齐 background_shells)。
   - `delete_session_inner`(`commands/sessions.rs:280`,kill_all_for_session 接线 :348 旁)接 `clear(sid)`。
3. **drive.rs 接线**(第 4 环 + append):
   - `run_chat_loop` 顶部:读 `tools_stub_enabled`(每 request 一次,best-effort 缺省 on)。
   - 三环过滤后:`if stub_on && !effective_is_worker && !is_group_chat { stubify }`;dispatch append 之后同侧 append `load_tool_schemas_def()`(**gate 必须含 `!is_group_chat`** — 评审 P1-1:群聊复用 run_chat_loop,白名单含 web_fetch)。
   - registry 经 `run_chat_loop` 参数或既有 state 引用传入(对齐 `subagent_cache` 穿参)。
4. **`chat_loop/tools.rs` 拦截**(**serial 路径顶部**,先于 :857/:1011/:1099 既有拦截):
   - `load_tool_schemas`:解析 `tool_names`(`"all"` → 候选集;未知名 error 文本列合法名)→ `registry.extend` → tool_result = pretty JSON。
   - 直呼自愈:`STUB_CANDIDATES.contains(name) && stub_on && !worker && !group_chat && !registry.contains` → `registry.extend` → `is_error: true` tool_result(完整 def + "schema now loaded, retry with correct arguments")。完整 def 从 `builtin_tools()` 现查(名字索引)。并行路径不拦(候选 ∩ 并行白名单 = ∅,单测已固化)。
5. **审计**:`load_tool_schemas` 调用记 audit(对齐 `record_tool_executed_audit` 现有模式,kind 复用或新增 `AuditKind` — 看 21 类里有无合适只读档;MVP 可复用 tool 常规执行审计)。
6. **测试**:
   - `stubify` 纯函数单测(见 1)。
   - 拦截单测:MockProvider 跑一轮 `load_tool_schemas` 调用,断言 tool_result 含 schema + registry 写入;直呼自愈分支同理。
   - 粘性:同 session 两次 request,第二次 turn_tool_defs 含全量 def(复用现有 drive 测试模式)。
   - 不回归:`filter_tools_for_mode/workflow/session_type` 现有测试不动全绿;worker 不 stub 断言;**群聊不 stub 断言(gate `!is_group_chat`,评审 P1-1 的回归锚)**;群聊测试不动。
7. **live 验证**(`scripts/turn-smoke.sh`,AC1/AC5):
   - 开关开:首轮 tools_token ≤ 3700(用户拍板;实测 3677)。
   - 手测一轮让模型调 `load_tool_schemas` + 真实调用 stub 工具(AC2)。
   - 开关关:tools_token 回 ~6773,无 load_tool_schemas。
8. **spec 沉淀**:`.trellis/spec/backend/token-usage-tracking.md` 加 D scenario(stub 形态 + 粘性不变量 + 开关);`tool-contract.md` 加 `load_tool_schemas` 契约。

## 验证命令

```bash
cargo test -p everlasting --lib "tools::stub"          # 纯函数
cargo test -p everlasting --lib "chat_loop"            # 拦截 + 粘性
cargo test -p everlasting --lib                        # 全量(勿加 --test-threads=1)
./scripts/turn-smoke.sh                                # live AC1
# AC5:sqlite3 写 app_config tools_stub_enabled='false' 后再跑 turn-smoke
```

## 风险文件 / 回滚点

- 改动集中:`tools/stub.rs`(新)+ `state.rs` + `drive.rs`(第 4 环)+ `chat_loop/tools.rs`(拦截)+ `delete_session_inner` 一行。单 commit 可整体回滚;运行时开关即软回滚。

## task.py start 前检查

- [x] prd/design/implement 三件套齐
- [x] 实施前评审(review.md)4 问题已全部处置进三件套(P1-1 群聊 gate / P1-2+P2-1 移出 use_skill / P2-2 静态度量 / P2-3 保序注释)
- [x] 用户批准(2026-08-14,经评审修正后交棒实施)
