# Review — tools Stub 注册(渐进式披露 D)

> 评审日期:2026-08-14。评审对象:`prd.md` / `design.md` / `implement.md`(status=planning,实施前评审)。
> 方法:对三件套引用的代码事实逐条核验——过滤链接入点与 `effective_is_worker` gate(`agent/chat_loop/drive.rs`)、拦截点与执行路径(`agent/chat_loop/tools.rs` 并行 L2 / serial 分流、`ask_user_question` / `request_mode_change` 等拦截型工具)、`builtin_tools()` 清单与各候选工具 def 形态(`tools/mod.rs` + 各 `tools/*.rs`)、registry 先例(`background_shell/mod.rs`)、session 删除接线点(`commands/sessions.rs::delete_session_inner`)、群聊是否复用 `run_chat_loop`(`agent/group_chat_loop.rs`)、开关读点(`db/config.rs:37`)、AC1 基线来源(journal-4 + turn-smoke.sh)。

## 总体评价

方案方向正确、收益诚实(候选 11 选定为低频重型、`ask_user_question` 全量保留),**代码落点与三件套描述基本对上**(第 4 环位置、拦截点 ~:857、registry 先例、`get_config_value` 读点、worker gate 均核实无误),回滚通道(`tools_stub_enabled` 开关 + 单 commit)设计到位。AC 可测性整体成立。

**结论:修正 2 个 P1 后放行实施**(1 处群聊 gate 遗漏且红线对照表有一行事实错误 + 1 处直呼自愈拦截点覆盖不全),P2 建议顺手处理。

## ✅ 核验通过(证据确凿)

| 声明 | 核验结果 |
|---|---|
| 过滤链三环 mode→workflow→session_type 于 `drive.rs:511-517`,dispatch append 在 `:546-559`(gate `!effective_is_worker`) | **精确**:第 4 环插入位置(三环后、dispatch append 前)与 design 一致;`turn_tool_defs` freeze 于 `:560` |
| `is_group_chat` 信号在 `drive.rs:510` 已现成(`loaded_session.session.session_type == GroupChat`) | **精确**:零成本复用,gate 加 `!is_group_chat` 即可 |
| `ask_user_question` 拦截于 `chat_loop/tools.rs:857` | **精确**:serial 路径内 `if name == "ask_user_question"` → `execute_blocking`;`request_mode_change` 拦截在 `:1011`、`request_task_state_transition` 在 `:1099` |
| L2 并行路径(`tools.rs:84`)`is_parallel_eligible` 白名单含 `use_skill` | **精确**:`use_skill` 恰在 11 候选内(见 P1-2/P2-1) |
| `builtin_tools()`(`tools/mod.rs:138`)注册含候选 11 全部 | **精确**:web_fetch/use_skill/update_checklist/run_background_shell/shell_status/shell_kill/merge_worker/discard_worker/remember/ask_user_question(排除)/use_ui/request_mode_change 均在场 |
| `get_config_value`(`db/config.rs:37`)签名 `(pool, key) -> Result<Option<String>>` | **精确**:每 request 读一次的成本与语义成立;缺省 on 的 fail-open 语义安全 |
| registry 先例 `BackgroundShellRegistry`(`background_shell/mod.rs:248` trait + `:303` DefaultRegistry) | **精确**:session-keyed in-memory map 形态匹配;不抽 trait(YAGNI)合理 |
| `delete_session_inner`(`commands/sessions.rs:280`)内已有 `background_shells.kill_all_for_session`(`:348`)接线点 | **精确**:`clear(sid)` 对齐该点;daemon 重启丢 loaded-set 可接受(prd R5) |
| 群聊**复用** `run_chat_loop`(`group_chat_loop.rs:286` moderator 轮 / `:478` participant 轮) | **精确**:见 P1-1,design 红线对照表第 3 行与此矛盾 |
| 群聊白名单 `group_chat_tool_defs`(`group_chat_prompts.rs:194/206`)= 5 研究工具 + 2 仲裁工具 | **精确**:`GROUP_CHAT_RESEARCH_TOOLS` 含 **web_fetch**(11 候选内),moderator 加 nominate/end |
| AC1 基线 6773/38.5% 来源 | **精确**:journal-4.md:369 记录 C7 live 烟测 `tools_token=6773 / context_input=17602`;`turn-smoke.sh` 已报 tools_token/占比,AC1 可测 |
| AC4 粘性测试前提(同 session 跨 request) | **成立**:`run_chat_loop` 每 request 调一次,registry 挂 AppState,loaded-set 跨 request 存活 |
| execute_tool 链路无 schema 校验(`tools/mod.rs:424-459` match by name) | **精确**:stub 后 `{"type":"object"}` 直呼会直接真实执行,参数错只回普通 error → 自愈分支的必要性成立 |

## ⚠️ 需修正的问题(按严重度排序)

### 🔴 P1-1 — 群聊 gate 遗漏,且红线对照表第 3 行事实错误

**位置**:design 决策 3 + §5 红线对照表第 3 行("群聊走 `group_chat_loop.rs` 独立链,不经 `turn_tool_defs`")。

**问题**:群聊**并非**独立链——`group_chat_loop.rs:286` / `:478` 两处都调用 `run_chat_loop`,stubify 作为 drive.rs 第 4 环会在群聊传入的白名单(`group_chat_prompts.rs:194`)上执行,而白名单里 **`web_fetch` 恰在 11 候选内**。gate 只写 `!effective_is_worker` 时,群聊每轮 `web_fetch` 被 stub、`load_tool_schemas` 被 append 进 speaker 的 turn defs——直接违反决策 3"适用范围:经典 chat only",白名单语义被污染(虽同 session 共享 loaded-set、load 一次后恢复全量,不致破坏)。

**建议**:stubify 与 append 的 gate 各加 `&& !is_group_chat`(`drive.rs:510` 现成信号);同时改正 design 红线对照表第 3 行(群聊复用的是同一 `run_chat_loop` body,gate 是唯一防线)。

### 🔴 P1-2 — 直呼自愈拦截点覆盖不全:`use_skill` 走 L2 并行路径会绕过

**位置**:design §2 直呼自愈分支(标在 serial `~:857` 旁)。

**问题**:`dispatch_tool_calls` 先走 L2 并行路径(`tools.rs:84`,`is_parallel_eligible` 判定)再走 serial(`:439`)。**`use_skill` 在 L2 并行白名单内**(`:88`)且是 11 候选之一——stub 后模型直呼 `use_skill` → 命中并行分支 → `:317` 直接 `execute_tool` 真实执行,**自愈拦截不触发**:loaded-set 不写、schema 不回灌;`execute_tool_inner` 不做 schema 校验,模型只拿到普通 error,无 "schema now loaded, retry" 指引。同理 `request_mode_change` 等拦截型工具 stub 直呼也会绕过(它们 `execute_blocking` 本身可用,影响小)。

**建议**:自愈判断提到 `dispatch_tool_calls` 入口——并行/serial 分流之前遍历 `tool_calls` 统一拦(一处覆盖全部路径);或并行任务内加同款分支。

### 🟡 P2-1 — 建议把 `use_skill` 移出候选(一行修复,风险面同时消失)

**位置**:prd 决策 1 候选清单。

**问题**:`use_skill` 的 schema 仅 `skill_name` 一个 string 字段、description 很短(`tools/use_skill.rs:25-41`),stub 收益接近零,却恰在 L2 并行白名单里——是 P1-2 唯一会真实踩坑的工具。若从 `STUB_CANDIDATES` 拿掉(11→10),P1-2 的风险面与复杂度同时消失;若坚持保留,则 P1-2 的入口统一拦是硬前提。

### 🟡 P2-2 — AC1 可达性偏乐观,实现第 1 步后先粗估再跑 live

**位置**:AC1(≤3500)。

**问题**:6773 基线中 tools 大头集中在超长 behavior-guide description(`use_ui` 的 description 是整页行为指南;`update_checklist`/`web_fetch`/`request_mode_change` 的 def 文件均 20k+ 字节)。候选 11 全 stub 成一句话后合计估计 ~500 token + `load_tool_schemas` def(~200-300)+ 未动核心工具,**≤3500 大概率能过但非板上钉钉**。

**建议**:implement 第 1 步(stub 模块)落地后先用 `serde_json::to_string(&stubified_defs)` + `memory::tokens::count_tokens` 粗算一轮,超线再调候选集或 description,别等 live 烟测才暴露。

### 🟡 P2-3 — 工具顺序稳定性写进注释

**位置**:design §1 `STUB_CANDIDATES` 表。

**建议**:`STUB_CANDIDATES` 的顺序即 stub 工具在 `tools[]` 中的相对位置,改动会动前缀缓存断点(C7 R3.2 稳定性前提)——在 `stub.rs` 注释里写明;`load_tool_schemas` append 建议放在 `dispatch_subagent` append 之后(与现有 append 同侧),避免无谓的顺序扰动。

## 放行建议

1. 修 P1-1(stubify/append gate 加 `!is_group_chat` + 改正 design 红线表第 3 行)——改文档 + 一行 gate;
2. 修 P1-2(自愈拦截提至 `dispatch_tool_calls` 入口统一分支,或采纳 P2-1 移出 `use_skill` 候选);
3. P2-1/P2-2/P2-3 按 implement 顺序顺手处理(候选微调、粗估验证、顺序注释)。

修正后按 implement.md 执行顺序(纯函数 → registry → 接线 → 拦截 → 测试 → live)开工,无需重新 brainstorm。
