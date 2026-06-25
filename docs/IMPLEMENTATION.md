# IMPLEMENTATION — 实现讲解

> Everlasting 的"自研决策 + 决策日志"。**本文件是决策档案**,不列路线图(路线图见 [ROADMAP.md](./ROADMAP.md))。
> 需求见 [DESIGN.md](./DESIGN.md),架构见 [ARCHITECTURE.md](./ARCHITECTURE.md),技术选型见 [TECH.md](./TECH.md),路线图见 [ROADMAP.md](./ROADMAP.md)),候选功能见 [BACKLOG.md](./BACKLOG.md)。
>
> > 2026-06-04/05 项目启动期决策见 [archive/implementation-inception-2026-06-04-to-05.md](../.trellis/spec/archive/implementation-inception-2026-06-04-to-05.md) (已归档)

---

## 1. 决策:自己写 agent core,不用 SDK 包装

**背景**:Anthropic 2025-2026 年出了官方 Agent SDK(`claude-agent-sdk-python` / `-typescript`),用 `query()` 直接拿结构化消息流。OpenAI Codex CLI 是 Rust 写的(Apache 2.0)但没官方 SDK。

**为什么不用**:
1. **学习目标要求自研** — 用了 SDK 只学到"怎么调 SDK",学不到 harness 核心
2. **控制粒度** — SDK 帮你做了"消息流 → tool 调用 → 回填"的循环,你想插自定义逻辑(权限、审计、统计)就被抽象挡住了
3. **解耦厂商** — 一旦 SDK 协议变化,业务逻辑全挂

**什么时候用 SDK 合适**:赶时间、要快速出活、不在乎学习价值。本项目两个都不符合。

**自研的边界**:
- ✅ 自己写:Agent Loop、消息管理、tool 注册、流式解析、权限检查
- ✅ 自己写:Tauri IPC 事件协议、session 持久化、worktree 管理
- ❌ 不自己写:LLM HTTP 协议(用 rig)、SSE 解析(用 rig)、MCP 协议(用 rmcp)
- ❌ 不自己写:GUI 框架(Tauri 已有)、Diff 算法(用前端库)

---

## 4. 决策日志

> 按时间倒序记录。每次重大决策都加一条,包含"为什么"。**本节只追加不删除**(ADR 性质的不可再生历史档案)。

### 2026-06-25 — 放开 SSE 流中 mode 切换(后端零改动,toast 仅流中弹)

**Context**: 前端之前在 SSE 流进行中故意禁用 mode 切换(`ModeSelect.vue:117` `toggleMenu()` early-return + `:disabled="isStreaming"` + CSS `.mode-select__trigger--disabled` + `chat.ts:1242` `requestSetMode` 流中 guard + `chat.ts:1278` `confirmYolo` 流中 guard,共 6 处;`ModeSelect.vue` 注释明确"matches the backend's 'mode applies on next turn boundary' rule")。用户希望放开,以便流中也能预切下一轮的 mode(典型场景:发现 agent 走偏了,中途切到 plan 模式让下一轮重新规划)。后端 `set_session_mode` 命令全文无 streaming 检查,`chat_loop.rs:396` 每 turn 开头读 `loaded_session.session.mode` 整 turn 复用 —— turn-boundary 语义是真实的。

**决策**:

1. **UI 6 处 guard 全删**:`ModeSelect.vue` 的 `toggleMenu` early-return、`:disabled="isStreaming"`、CSS `--disabled`、YoloConfirmModal 的 `:disabled`、title 分支文案;`chat.ts` `requestSetMode` 和 `confirmYolo` 的流中 guard。store 不在前端偷偷吞 IPC,IPC 透传到后端照单全收 + DB 持久化 + 审计写入。Yolo confirm modal 整体保留(安全门),仅删 modal trigger 的 `:disabled` —— 让流中点 Yolo 也能走确认门。
2. **toast 仅在流中弹**:非流中切 mode 立即生效(trigger chip 文字立刻变就是反馈),弹 toast 是噪音;只有流中切 mode 才会有"已切换但不立即生效"的预期差,toast 是必要的 UX 锚点。文案:"Mode 已切换,将在下一轮 turn 生效",kind=info,duration=3000ms。Edit/Plan 路径由 `onModePick` 弹,Yolo 路径在 modal confirm 后由 `onYoloConfirm` 弹。
3. **toast 调用收敛在 `ModeSelect.vue`**,不进 `chat.ts` —— 避免 store 间耦合,也避免 cancel 路径需要 counter-handler(Yolo cancel 不弹 toast 是默认行为,无需显式处理)。
4. **`confirmYolo` 返回类型从 `Promise<void>` 改为 `Promise<boolean>`** —— 让 modal 的 `@confirm` handler 可以 await 并根据成功与否决定是否弹 toast。向后兼容(已有 `chatMode.test.ts` 调用者 `await store.confirmYolo()` 不使用返回值)。

**Consequences**:

- 同一 turn 内所有 tool_use 仍按旧 mode 走(`chat_loop.rs:396` 的 turn 开头缓存不变):中途切 yolo 不会让当前轮的 tool 立刻 bypass 弹窗;中途切 plan 不会让当前轮的 write_file 立刻被拦。这是符合设计的 —— turn-boundary 是 backend 的真实语义,不能为了"立即生效"去给 in-flight loop 加 mode 推送通道(性价比极低,且会引入新 race)。
- toast 频率 = 用户在流中点 mode 的次数,可忽略;每次最多 1 行 `mode_changed` 审计 + 可选 `yolo_entered/exited`,审计密度可接受。
- 后续如果真的需要"流中途立即生效",需要把 `chat_loop.rs` 的 `session_mode` 从"turn 开头缓存到局部变量"改成"每次 tool_use 重新读 `loaded_session.session.mode`" —— 当前不做(场景未出现,Yolo bypass 仍可通过 reload session 触发)。

**关联**: PRD `.trellis/tasks/06-25-sse-mode-toast-turn/prd.md`;改 `app/src/components/chat/ModeSelect.vue` + `app/src/stores/chat.ts` + `app/src/stores/chatMode.test.ts` 注释;**后端零改动**(PRD 验证:`app/src-tauri/` 无新 commit)。

### 2026-06-25 — L3a subagent 并发(只读 worker fan-out):竞态消解 + 范围决策 + worker 联网拆分

**Context**: L3a 把 B6 单 worker 串行 dispatch 升级为并发 fan-out。拆自原 L3 子项 1(2026-06-24),L3b(worktree 隔离)另行。Plan 阶段先做行业调研(2 份 research 文档,源码核实 Hermes `delegate_task` 默认同步阻塞/并发3/硬拒/depth1,纠正 scheduling-survey 2 处事实错误:"不阻塞"应为默认阻塞、"30"应为默认3)。

**决策**:
1. **范围:只读 worker 并发**(researcher/探索类),worktree 留 L3b —— 带写 worker 并发需隔离,不在 L3a。
2. **并发模型:父 turn 阻塞 + 内部 fan-out**(`FuturesUnordered` 复用 L2 只读 batch 模板),非"父 agent 不阻塞"(那是 daemon 化,L3b+)。
3. **只读保证 = 运行时强制剥写**(`force_readonly` 保留 read/grep/glob/list_dir),不靠语义层限类型 —— 与 `STRUCTURALLY_DISABLED` "不信任定义"哲学一致,且为 L3b(去剥写+worktree)留扩展;安全底线仍由 is_worker Deny 兜底。
4. **竞态三处只读范围消解**(auto-context 查源码裁定,核心洞察):permission:ask(worker is_worker=true → Tier4 ask 塌缩 Deny,只读工具低 Tier)/ token(`add_token_usage` `col=COALESCE(col,0)+?` 原子增量,SQLite 单写锁)/ cancellations(`parent_token.child_token()` × N + 各 worker_rid 注册,父 cancel 一次 fan-out 全部)—— **零并发控制代码**,并发安全"免费"。
5. **上限 3 硬拒**(env `DELEGATION_MAX_CONCURRENT_CHILDREN`,对齐 Hermes),不截断不排队。
6. **MVP 只优化纯 dispatch 批**(≥2 全 dispatch),混批走原 serial。
7. `run_subagent` 加 `force_readonly: bool` 参数(非 wrapper) —— 比 copy ~450 行函数更符合 code-reuse(避免 faithful-port drift hazard),serial 传 false 保护 B6(回归测试 `l3a_single_dispatch_runs_serial_path_unchanged` pin)。

**Consequences**: 实现大幅收窄(serial path 加并发分支 + 一个 `filter_tools_readonly` 函数,`run_subagent` 体不动);864 测试绿(9 l3a 集成 + 2 单测);前端 store 按 runId 天然支持 N concurrent(PR2 实质满足,无需改造)。

**验证发现 + 拆分**: 2026-06-25 手动验证(并发 3 researcher 搜外部项目更新)暴露 **worker 三层不能联网**(researcher 设计无 web_fetch + `force_readonly` 剥 web_fetch + worker is_worker 对 web_fetch 无授权→ask→Deny)。"subagent 联网"是独立工具/权限配置域,**非 L3a bug**(L3a 正确实现"本地只读并发",范围决策=只读不含联网),拆 task `06-25-subagent-web-access`。

**关联**: spec `.trellis/spec/backend/tool-contract.md` §"Concurrent dispatch_subagent batch"(7节含 race dissolution Wrong/Correct) + `agent-loop-architecture.md` §"Pattern: Concurrent readonly dispatch"(race dissolution 表 + when/when-not);PRD `.trellis/tasks/06-24-l3a-readonly-concurrent/`;research `docs/research/subagent-{communication,scheduling-communication}-survey.md`。

### 2026-06-19 — RULE-A-012 reqwest streaming 超时改 per-chunk `read_timeout` + 流错误补 tracing(响应 2026-06-18 17:56 静默中断事件)

**Context**: 2026-06-18 17:56:52Z 一条 session(`request_id=mz8s3hqwx6rmqjswgte`,`messages.seq=37`)的 thinking 流在 60.4s 时被静默切断,前端只看到 `[生成出错中断]` toast,Rust 日志 **零** WARN/ERROR,grep 不到任何线索,首次靠 DB 反查(`text="[生成出错中断]"` + content thinking 在"尝试 1"中途被截 + seq=36→37 间隔 = 60.403s)才定位到 `reqwest::Client::builder().timeout(Duration::from_secs(60))` 触发。两条独立但同源的根因:

1. **reqwest `.timeout()` 是总 deadline**(`connect` 起算到 body EOF),不适合 SSE streaming —— 响应大小未知、chunk 间隔可变(extended thinking + 3rd-party 代理 `wukaijin.com` + `thinking_effort=high` 默认值 = 60s+ 才出首个 text delta 常见)。`anthropic.rs:210` / `openai.rs:425` 用的就是这个 API。
2. **`chat_loop.rs:657`**(拆分自 `chat_loop.rs`,2026-06-23 抽 `run_subagent` 至 `subagent/dispatch.rs` 后行号下移 ~522 → 实际 `agent/chat_loop.rs:~135`)**`LlmError` 静默包成 `ChatEvent::Error`,不打 tracing** —— 整个错误通道(Network / Auth / RateLimit / Server / InvalidRequest)都不留 Rust 侧 breadcrumb。RULE-A-007(2026-06-17)只补了 "error arm 持久化 partial turn",没补 "trace the cause"。

行业参照(`reqwest` 自身文档 `async_impl/client.rs:1448-1459`):**"read_timeout is more appropriate for detecting stalled connections when the size isn't known beforehand"** —— SSE 的标准定义。LiteLLM 默认 `timeout=600s` 区分 `httpx.Timeout(timeout=, connect=, read=, pool=)`(`litellm/llms/custom_httpx/http_handler.py:133`);Anthropic / OpenAI SDK 都暴露 `Timeout(connect=, read=, write=, pool=)` 四阶段配置;reqwest 同款语义:**`timeout`(总 deadline)、`read_timeout`(per-chunk)、`connect_timeout`(握手)三独立 API**。

**Decision A**: provider reqwest 客户端 `.timeout(60s)` → `.read_timeout(60s)`,保留 `.connect_timeout(10s)`

`anthropic.rs:209-211` / `openai.rs:424-426` 两个 site 同步改。注释块说明 reqwest 文档原文 + 引用 incident `mz8s3hqwx6rmqjswgte` / `messages.seq=37` 作为"为什么改"的可追溯锚点。**不动** `connect_timeout` —— 握手阶段就该短。

**Decision D**: `chat_loop.rs:657` `Err(err) → ChatEvent::Error` 静默包装补 `tracing::warn!`

```rust
Err(err) => {
    tracing::warn!(
        request_id = %rid,
        turn,
        category = err.category(),  // LlmErrorCategory Display
        error = %err,               // reqwest::Error / serde_json::Error / ...
        "chat: LLM stream errored"
    );
    ChatEvent::Error { message: err.user_message(), category: err.category() }
}
```

`LlmError::category()` 已 Display 五类(Auth / RateLimit / InvalidRequest / Server / Network),日志一行 grep 直接出分类。**不动** `category()` Display 也不动 `user_message()`(UI toast 文案由前端控制器决定)。

**Alternatives considered & rejected**:

1. **抬总超时到 600s(跟 LiteLLM 对齐)** —— 否决。`read_timeout=60s` 已能 cover 慢代理 streaming;真要触发 60s 内零 chunk 说明代理真死了,这时让用户看到错误反而是对的。抬总超时是治标,且会把"代理挂着不返回"这类死循环无意义延长。**Out of scope,留待未来**。
2. **`providers` / `models` 表加 `request_timeout_secs` 列做 per-provider 覆盖** —— 否决本次做。当前 `read_timeout=60s` 通用,WSL + 3rd-party 代理实测够用;等真有用户被不同代理掐脖子再上,DB schema 改动有迁移成本,不该提前做。**Out of scope,见 ROADMAP 第三档预留**。
3. **不动 D,只改 spec 标"已知观测盲点"** —— 否决。这是 1 行 `tracing::warn!` 就能解决的可观测性债,不是设计权衡(对比 RULE-A-010 是 UX 决策留待未来实现;这里是 5 行代码 + 0 风险)。

**影响面**:

- 代码:`llm/provider/anthropic.rs:209-227`(含注释块 + 改动)+ `llm/provider/openai.rs:424-442`(同上)+ `agent/chat_loop.rs:655-682`(per-event Err 分支加 tracing::warn! + 注释块 + 改动)。**不动** cancel 路径 / **不动** 正常 Done 路径 / **不动** 正常 Error pre-emit(`ChatEvent::Error` 仍 pre-emit 给前端,只是多了 Rust 侧 breadcrumb)。
- Spec:`backend/error-handling.md` 加 "RULE-A-012 (2026-06-19) — reqwest per-chunk read_timeout + stream-error tracing" 段,引 incident + 改动表 + Out of scope(总超时 / per-provider 列)。
- DEBT:`.trellis/reviews/DEBT.md` 加 RULE-A-012 条目(Status closed),Re-evaluation Log 加一行。
- Journal:`.trellis/workspace/Carlos-home/journal-2.md` 追加 summary(同 4-stage commit 的 journal 段)。

**关联**:

- **RULE-A-007**(2026-06-17 closed,`error arm persist partial turn`)—— 同是 `had_error = true` 路径处理,A-007 落盘,A-012 落 trace,互补。
- **RULE-A-006**(2026-06-15 closed,`chat_loop 单一权威`)—— 改 `chat_loop.rs:657`(拆分自 `chat_loop.rs`,2026-06-23 抽 `run_subagent` 后行号下移 ~522)1 处全生效,9 个 `agent_loop_*` 集成测试已覆盖真实 production 路径(`agents/tests.rs` → 现 `agent/tests_*.rs`,2026-06-23 拆为 5 域 + `tests_common.rs`)。
- DEBT §收尾路径建议 🟡 梯队 "看到顺手修"——本次即此类。

---

### 2026-06-20 — FT-F-001 PR1:ToolCallCard 抽 3 shared body component(为 drawer typed-cards 重做做硬前置)

**Context**: FT-F-001 阶段 2(SubagentDrawer typed-cards 重做)的硬前置。drawer 当前 4 种 `TranscriptKind` 走统一 `JSON.stringify` 渲染,主面板 `ToolCallCard` 已有 input/output/permission 多形态但**结构上同源不共享**。两条路径要共用卡片逻辑,但数据形状不对齐(`TranscriptEntry.payload_json: Record<string, unknown>` vs `ToolCallInfo { id, name, input }`)。

**Decision A**:抽 3 个 shared body component,**不是** adapter 路径
- `ToolInputBody.vue`(name + input → `<details><summary>input</summary><pre>{{ JSON.stringify(input, null, 2) }}</pre></details>`)
- `ToolOutputBody.vue`(content + isError + durationMs? → cwd envelope auto-unwrap + truncate(500) + size label + F5 duration chip + isError 红边)
- `PermissionAskBody.vue`(mode: 'interactive' | 'historical' + ask + onRespond? → interactive 4 按钮 + feedback textarea / historical info-only 行)
- 3 body **不接 variant prop**(D3),**不读 store**(D3 — callback prop 模式,store 依赖留 outer),**无 store imports**(defensive)

**Decision B**:outer wrapper 留 inline
- `ToolCallCard.vue` 995 → ~790 行(净减 200+ 行,body 模板 + JS + CSS 全部移出)
- `.tool-card__details` / `.tool-card__pre` / `.tool-card__approval*` 8 类**全删**(移到 3 body 内部)
- 保留 inline:`tool-card__header`(icon+name+path+status+duration+diff btn)、`tool-card__diff`+popover(edit_file 专属,drawer 不用)、`tool-card__subagent-preview`(dispatch_subagent 主面板专属,drawer 不用)
- **diff 不抽 `DiffBody`**(D6 — drawer 不渲染 diff,收益小)
- 抽出方案 ≠ "v-if 拆 3 body 拼装":3 body 独立 component + outer 持有 3 store,每 body 单独 mount

**Decision C**:wrapper 兼容层保 outer selector 锁
- `ToolCallCard.vue` 包 `<PermissionAskBody>` 时**保留** `<div class="tool-card__approval">` 外层,内层用 body 自己的 BEM 类(`.permission-ask-body__btn--once` 等)
- **原因**:`ToolCallCard.test.ts` 14 test 第 1/2/3/6 条用 `.tool-card__approval` 做"approval UI present/absent"锁;无 wrapper div → 4 test 改 selector;有 wrapper div → 4 test 零改 selector。**实质行为锁未变**(approval UI 挂载由 `v-if` 同控制),仅是 1 行 wrapper div 帮老 selector 找到目标

**Decision D**:测试拆分 = `ToolCallCard.test.ts` 全 mount(非 shallow)
- 旧 `mountCard` 用 `shallow: true` 是为 stub `Icon` / `DiffView` 子 component
- FT-F-001 PR1 后,approval UI 在 `<PermissionAskBody>` 子 component 内,`shallow: true` 会 stub 它 → 4 test 找 `.tool-card__approval-btn--*` 失败
- 解决:`mountCard` 改全 mount,`Icon` / `DiffView` 仍 stub 由各自内部处理(无副作用,因为这 2 个本来就不在 approval UI 渲染路径)
- 4 test selector 改:`.tool-card__approval-btn--*` → `.permission-ask-body__btn--*`(等 4 个 selector + 1 行注释 + 1 行 mount 改动)。**测试逻辑零变**(assertion 内容、setup、teardown 一致),仅 selector 跟随实现路径

**Alternatives**(已否决):
- **adapter 路径**(`synthesizeToolCallInfo` / `synthesizeToolResultInfo` wrapper):drawer 端写 boilerplate adapter 重复合成;否决,组件 API 直接吃 data 更净
- **`provide`/`inject` 注入 store**:3 body 都集中一个 parent,over-engineering;否决
- **1 个 `<ToolCardBody>` 加 variant prop**:FT-F-001 D3 决策已排除 variant 多变体爆炸;否决
- **保留旧 `.tool-card__approval-btn--*` 类名在 PermissionAskBody 内部**:body 内部类名 ≠ outer 类名,语义错乱;否决
- **拆 4 PR 各自 commit**:`ToolInputBody` + `ToolOutputBody` + `PermissionAskBody` 互相无依赖,可独立 commit,但合 1 PR 减少 review 摩擦(类似 A4 1-PR-全部合模式);采用 1 PR

**影响面**:
- 新文件 6:`ToolInputBody.vue`(81 行) / `ToolOutputBody.vue`(140 行) / `PermissionAskBody.vue`(323 行) + 各自 test(5/10/17 共 32 test)
- 改文件 2:`ToolCallCard.vue`(995 → 791 行) / `ToolCallCard.test.ts`(1 mount 改动 + 4 selector 改动 + header 注释)
- **零后端改动**(frontend-only 任务)
- **零 store 改动**(`subagentRuns.ts` / `chat.ts` / `permissions.ts` 全部不动)
- **零 drawer 改动**(`SubagentDrawer.vue` 留 FT-F-001 阶段 2 接入)
- spec:暂不改(本 PR 不引入新 spec 段;FT-F-001 阶段 2 drawer 接入时才沉淀 spec)

**Verification**:
- `pnpm vue-tsc --noEmit`:0 error
- `pnpm vitest run` 全集:272 passed(基线 240 + 32 新 = 272),4 个**pre-existing** unhandled rejection in `streamController.test.ts:reloadAfterFinalize`(本 PR **不引入**,git stash 后基线复现)
- `pnpm vitest run src/components/chat/ToolCallCard.test.ts`:14 test 全 pass(行为锁保持)
- `pnpm vitest run src/components/chat/ToolInputBody.test.ts`:5 test 全 pass
- `pnpm vitest run src/components/chat/ToolOutputBody.test.ts`:10 test 全 pass
- `pnpm vitest run src/components/chat/PermissionAskBody.test.ts`:17 test 全 pass
- `git grep -nE "JSON\\.stringify\\(.*input" app/src/components/chat/ToolCallCard.vue`:0 hit(inline stringify 迁出)
- `git grep -nE "extractToolResultDisplay" app/src/components/chat/ToolCallCard.vue`:2 hit(1 import + 1 usage in `displayContent` computed 给 dispatch_subagent preview fallback 用,符合 AC13 "≤ 1 hit" 精神)

**关联**:
- **FT-F-001**(drawer typed-cards 阶段 2,本 task 完成后才能 `task.py start`)
- **D1 决策**(drawer typed-cards 7 决策之一):D1/A = 抽 shared body,本 PR 落地;D3/A = body 纯 outer 各起,本 PR 落地;D4/A = 8 零改 + 19 新增 test(实际 14 改 4 selector + 32 新增,理由见 Decision D)
- **RULE-A-016**(worker ask_path 写 transcript PermissionAsk,2026-06-20 PR3a):PermissionAskBody `historical` mode 直接吃此 schema 的 payload_json,无需二次合成

### 2026-06-18 — B12 Checklist(agent 自跟踪进度清单)设计决策 + 先于 B6 subagent 的排序

**Context**: 用户提"想在 subagent(B6)之前加一个 task list / todo list 功能,先做哪个"。grill-with-docs session 锁定它是 **TodoWrite 式 agent 自跟踪 tool**(命名 **Checklist**;CONTEXT.md 已落术语 + 三消歧义:非 Trellis task / 非 plan mode / 非 subagent)。本 ADR 记三条核心权衡 + 排序理由,实施前定盘。

**Decision 1 — 排序:Checklist 先于 B6 subagent**

- ① **量级不对称**:Checklist ≈ 1 个 tool + 注入 hook + 1 个渲染卡;subagent 要 fork agent loop(嵌套 `run_chat_loop` + 独立 messages + 独立 token 预算)+ summary 回填 + worker UI——**动 loop 核心**。前者旁挂,后者动地基。
- ② **学习路径依赖**:两者都要"在 turn 开头把 agent 自管状态注入 context"。Checklist 注入一张**列表**(平凡实例);subagent 注入子 agent 的 **summary + 子 context 预算**(复杂实例)。Checklist 是 subagent 那套机制的小面 warm-up,先在小面上跑通"每轮注入动态 agent-state",再上 subagent。
- ③ **正交**:Checklist 不碰 subagent 的任何面;将来 subagent 甚至可把 checklist 项派给 worker。B6 的 roadmap 依赖(B5 Memory)已满足,**无紧迫性逼 subagent 先做**。

**Decision 2 — state + 注入 + 持久化:loop-local `Vec` + 每轮 ephemeral 重发 + 无新 DB 表**

- **state**:`run_chat_loop` 作用域内 `Vec<ChecklistItem>`,**per-request 生命周期**(一个 user message 的整 run),不跨 run → **无新 DB 表、无 migration**。
- **注入**:每轮 `provider.send` 前,从 Vec 重建一份 synthetic user block(整张 list + 显式 in_progress 焦点),**append** 到**当次请求的 messages 副本**(不写回持久化 `messages`),发完即弃,下轮从最新 state 现造。**不打 `cache_control`**(每轮必变,cache 永不命中,块小成本可忽略)。空表跳过(turn 1 未调过 update)。**不塞 system prompt**——会每轮 bust system prompt cache,废掉 memory / skill 那套 `cache_control` 机制(skill 当年特意"decoupled from memory cache window"即此理,`chat_loop.rs:258-263`)。**关键:append 而非 prepend**——memory 的 `cache_control: Ephemeral` 断点在 `messages[0]` 块的 banner 上,任何在它**之前**的 per-turn 变化块(包括 prepend 的 checklist)都会 bust 该断点;append 把 checklist 放在断点之后,memory cache 窗口不受影响(trellis-check 2026-06-19 修正:原 plan 写 prepend,实施 review 发现 prepend 会 bust memory cache,改 append)。
- **持久化 / replay**:`update_checklist` 的 tool_result(本就在 message history 里持久化)携带完整列表 → 渲染 + reload 还原的 source of truth。reload 从 **DB 全量 history** 重建;C3 compaction 是 **in-memory only**(`agent/context.rs:36` 实锤,DB 保留全部 message),故 reload 还原**永远完整**,不受 compaction 影响。
- **cancel / 切 session**:复用现有 cancel 路径 + **RULE-A-004**(cancel 掉的 tool 不 commit tool_result,Vec 那次改动不算数)→ live Vec 与持久化 history 不打架;切回从 DB history 重建。

**Decision 3 — tool 形状:单 `update_checklist` 全量替换 + 三态 + 至多一 in_progress**

- **全量替换**(对齐 opencode `todowrite` / Cline),非细粒度 add/update/delete。**硬理由**:replay 要求"最后一条 tool_result == 当前态"(O(1) 还原);细粒度要重放所有 op(O(N) + deleted 项残留),直接打破 Decision 2 的 replay 设计。原子 + 幂等;token 成本可忽略(per-request + 几条短项)。
- **item schema**:`{content, status}`,status 三态 `pending` / `in_progress` / `done`(对齐 Claude Code / opencode)。**至多一个 `in_progress`**(= agent 当前焦点指针,喂给注入的"current focus")——soft 约束,model 传多个时 tool coerce(保留最后一个、其余降 pending),不报错避免打断 loop。此约束亦让 UI 焦点动效有单一目标。

**Alternatives considered & rejected**:

1. **细粒度 tool(add / update / delete)**:否决,见 Decision 3——打破 replay(O(N) 重放 + 残留),状态漂移风险。
2. **新 DB 表存 checklist**:否决——per-request 无跨 run 需求,migration 是过度设计。
3. **checklist 塞 system prompt 每轮重建**:否决——每轮 bust system prompt cache,废掉 caching。
4. **纯靠 tool_result history(最后一条当当前态),无 ephemeral 重发**:否决——C3 会从 **live 数组**压掉旧 tool_result,agent 进行中可能丢计划。ephemeral 重发是"进行中"的扛压路径(reload 才靠 DB history,两条独立路径,互不依赖)。
5. **注入只发 in_progress 焦点而非整张**:否决——C3 压掉 history tool_result 后,ephemeral 块须自给自足,只发焦点会让 agent 丢非焦点项。

**影响面**(✅ 已实施 2026-06-19 — `c59daaa` docs / `994db84` PR1 后端 / `1896470` PR2 前端 / PR3 spec 沉淀):

- 代码(✅ 落地):`tools/` 新增 `update_checklist` + `tools/mod.rs` 注册;`agent/chat_loop.rs` 加 loop-local `Vec<ChecklistItem>` + 每轮 ephemeral 注入 seam(`compact_messages` 后、`provider.send` 前 **append** 到副本,非 prepend——见 Decision 2 cache 修正);`ToolDef` 注册;前端新 `<ChecklistCard>` 浮层组件(ChatPanel 内 `position: absolute`、最小化悬浮球、焦点项动效)+ checklist store(从 `tool:call` / `tool:result` 派生当前态)。
- **零 DB schema 变更**(per-request + replay 走 history)。
- spec(✅ 落地):`backend/tool-contract.md` 加 Checklist 段;`frontend/state-management.md` 加 checklist store 段。
- CONTEXT.md:Checklist 术语已落(2026-06-18,含三消歧义)。
- ROADMAP:§2 🟠 第三档加 B12(本日同步)。

**关联**:

- **B5 Memory**(`build_instructions_blocks` + `cache_control: ephemeral`)——Checklist 注入机制的原型;但 Checklist 是 run 内**动态**(memory 是 run 内**静态**),故用 ephemeral 每轮重发而非一次性头部插入。
- **C3 compaction**(`agent/context.rs`,`in-memory only`)——保证 reload 从 DB 还原完整;进行中靠 ephemeral 重发扛压。
- **RULE-A-004**(cancel 掉的 tool 不记 audit)——同一套保护罩住 checklist(cancel 的 update 不 commit)。
- **B6 Subagent**——Checklist 是其"每轮注入动态 agent-state"机制的小面 warm-up;实施顺序 **Checklist → B6**。

### 2026-06-17 — RULE-A-007 error arm 对称 cancel 路径 persist partial turn

**Context**: DEBT RULE-A-007(P2,Agent Loop)记录了 SSE 流中途 error 时 agent loop 行为不对称的问题:error arm 直接 `return`,**丢弃已累积的 `text_parts` / `finalized_thinking` / `tool_calls`** —— 这些 delta 已通过 `ChatEvent::Delta` 渲染给前端,但 reload 后从 DB 读不到。cancel 路径却正确地 flush + 构造 assistant_blocks + `CANCELLED_MARKER` 追加 + `persist_turn` 落库。两条 terminal 路径(except normal Done)行为不一致,是数据完整性 + UX 一致性 bug。

**Decision A**: `ERROR_MARKER` text 追加,对称 `CANCELLED_MARKER`

新增 const `pub const ERROR_MARKER: &str = "[生成出错中断]"`,定义位置在 `agent/helpers.rs` 跟 `CANCELLED_MARKER` 同处(文案对齐中文风格 + 方括号包裹,跟 `"[已停止]"` 一致)。text 追加逻辑加 `else if had_error { ... }` 分支,完全对称 cancel 的 `CANCELLED_MARKER` 追加:`full_text.is_empty() ? marker_alone : "\n\n" + marker`。

**否决**:metadata `interrupted: "error"` 字段方案。理由:D3 加了 metadata 通道,但 cancel 用的是 text marker(既定模式);引入 metadata 会让"中断标记"有两种表达(cancel=text / error=metadata),增加前端渲染分支。对称性优先,单表达更简单。

**Decision B**: error arm persist 失败 = log-only,不 emit_persist_failure

error 路径在 L598 已 emit `ChatEvent::Error`(per-event arm)给前端。若 partial turn persist 再 emit 第二个 Error(`emit_persist_failure`),会发出**两个 terminal Error 事件**,前端 terminal 处理逻辑会 fire 两次,行为未定义且冲突。

**对称依据**:cancel 路径的 synthetic tool_result persist 失败也 log-only(`chat_loop.rs` 注释明确"loop is about to emit terminal cancelled Done, an Error here would be second terminal event")。error 路径同构——terminal 已发,后续 persist 失败只 `tracing::error!` + return。

**否决**:error persist 失败也 emit_persist_failure(RULE-A-003 正常路径模式)。理由:正常路径没 pre-emit terminal,所以 emit_persist_failure 是**首个** terminal;error 路径已 pre-emit,场景不同。RULE-A-003 的"emit + abort"模式适用于"还没有 terminal 信号"的路径,error 路径不适用。

**Decision C**: error arm 也 emit TurnComplete

cancel 路径 persist 后 emit TurnComplete(seq + latency 给前端定位 partial message)。error 对称也 emit TurnComplete(seq 指向 partial turn),否则前端收不到 partial message 的 seq 定位,latency breakdown 丢失。

**Error 事件 + TurnComplete 并存的合理性**:两个事件携带不相交的信息——Error = "出错了"(terminal 信号),TurnComplete = "这个 seq 的 partial turn 已落库 + latency"。前端 listener 各自处理,不冲突。controller 把 Error 当 terminal(终止 streaming UI),把 TurnComplete 当 per-turn 元数据(attach latency 到对应 seq)。

**Alternatives considered & rejected**:

1. **不动 error arm,更新 spec 标"已知偏离"**(参考 RULE-A-010 D3 处理方式):否决。RULE-A-010 是 UX 设计决策(二次取消语义)留待未来实现;A-007 是**数据丢失 bug**,reload 后 partial turn 消失,不是设计权衡,必须修。
2. **error arm 也 emit Done(`stop_reason: "error"`)** 让前端有 terminal Done:否决。Error 事件本身就是 terminal 信号(前端 chat store 把 Error 当 terminal 处理),再 emit Done 是双 terminal。cancel 路径 emit Done 因为 cancel 没有 pre-emit "cancelled" 事件;error 路径已有 pre-emit Error,场景不同。
3. **把 error persist 失败改成 emit + 不 return**(让 loop 继续走 cancel/max_turns 路径):否决。error persist 失败说明 DB 写不进去,继续 loop 只会撞更多 persist 失败,且 TurnComplete 也会失败。log + return 是最干净的失败处理。

**影响面**:

- 代码:`agent/helpers.rs`(加 `ERROR_MARKER` const)+ `agent/chat_loop.rs`(改 error arm:删 `if had_error { return; }`,加 `else if had_error { flush + log }` + `else if had_error { ERROR_MARKER 追加 }` + persist 失败 `if had_error { log-only } else { emit_persist_failure }` + 新增 `if had_error { persist_cwd + touch + return }` 退出块)。**不动 chat.rs**(RULE-A-006 闭环,chat.rs 是薄 pre-flight)+ **不动 cancel 路径** + **不动 RULE-A-003 正常路径** + **不动 RULE-A-004 audit 顺序**。
- 测试:5 新增(`agent_loop_error_persists_partial_text` / `_empty_text_uses_error_marker` / `_persists_thinking_and_tool_calls` / `_persist_failure_is_log_only` / `_emits_turn_complete`),全 pass。567 tests total pass,0 warning。
- Spec:`backend/agent-loop-architecture.md` 加 "Pattern: Turn-boundary persist symmetry — error arm matches cancel arm" 段(含 When to apply / When NOT to apply / Constants);`backend/error-handling.md` 加 "Agent Loop Error Paths — terminal event + persist invariants" 段 + persist 失败处理矩阵(6 行,明确每处 persist site 的 failure handling)。
- DEBT:RULE-A-007 open → closed (2026-06-17);Re-evaluation Log 加一行。

**关联**:

- DEBT RULE-A-003(cancel/正常 persist 失败处理参考,P1 closed `d8ee7d9`)—— error arm persist 失败 log-only 对称 cancel tool_result,不破坏正常路径 emit_persist_failure。
- DEBT RULE-A-006(chat_loop 单一权威,P1 closed `759607c`-ish via `06-15-unify-chat-loop-dispatch`)—— 改 chat_loop 改 1 处全生效,9+ agent_loop_* 测试覆盖真实 production 路径。
- DEBT §收尾路径建议第 3 条(D3 收尾时提过 A-007 留独立 task——本 ADR 即是)。

### 2026-06-17 — D3(session 内消息编辑/重发)3 PR 闭环 + RULE-A-010 spec 偏离声明

**Context**: D3 是 V2 第二档(`§2`)最后一项,DEBT 收尾建议第 3 条"D3 自然碰 A-007(error 路径 partial text)/ A-010(二次取消语义),应最后做"指明 D3 实施是顺手收口的天然窗口。本任务前 2 PR 已落地:
- **PR1** (`308d277`):后端 `edit_user_message` Tauri command,单事务包裹 `UPDATE messages` (in-place content + metadata `edited_at`/`original_content`) + 级联 `DELETE messages WHERE seq > N` + INSERT `edit_message` audit row。零 schema 变更,纯用 B2 PR3 新加的 `messages.metadata` JSON 列。8 个集成测试覆盖 cascade delete / metadata 合并 / 原值备份 / 原子 rollback。
- **PR2** (`114b239`):前端 `<MessageActionsMenu>`(reka-ui DropdownMenu,3 项 Edit/Resend/Copy,Resend 永久 disabled + "PR3 待实施" tooltip)+ `<MessageItem.vue>` inline edit mode(textarea + Save/Cancel + 4 层防御:`canEdit` role check + streaming 时整个 menu trigger 灰显 + editBuffer 用 local ref 显式避开 stream delta race + 编辑失败保持 edit mode active + toast)+ `chatStore.editMessage` 3 步流程(streamController.cancel → invoke IPC → controller.refresh)。

PR3 收尾范围(本 ADR 锁定):
- **Resend 按钮实质化**(从 disabled 占位 → 实际功能)
- **"(edited)" 标签**(从 `messages.metadata.edited_at` 读取,bubble 旁小灰字渲染)
- **`AuditKind::ResendMessage` 新增 + audit 落表**(后端 agent loop 在 user message persist 路径检测 `resendSeq` IPC flag 触发 best-effort audit)
- **C4 `<AuditLogModal>` 暴露新 kind**(dropdown 加"编辑消息"/"重新发送" + AuditLogItem 图标 family 加 `message-edit`/`message-resend`)
- **RULE-A-010 spec 偏离声明**(`docs/ARCHITECTURE.md §2.5.1` 加 "已知偏离" 注释 + DEBT.md Status open→closed + Re-evaluation Log 加行)
- **`docs/IMPLEMENTATION.md §4` D3 完成 ADR**(本条)
- 同步:`database-guidelines.md` 加 Resend audit 模式段 + `state-management.md` 加 resendMessage + "(edited)" 标签渲染段

**Decision**:
1. **Resend 方案 A**(复用 chat IPC,前端传 `resendSeq` flag,**不引入新 IPC**):
   - 后端 `chat` 命令签名扩展为 `pub async fn chat(..., resendSeq: Option<i64>) -> Result<(), String>`,Tauri 自动 camelCase ↔ snake_case 转换。
   - 前端 `controller.startRequest` 接受可选 `resendSeq?: number`,`invoke("chat", { ..., resendSeq })` 透传。
   - 后端 agent loop `run_chat_loop` 在 user message `persist_turn` 成功后检测 `resend_seq.is_some()`,调 `record_message_resend_audit(db, session_id, original_seq, &preview)` best-effort 落表(`tracing::warn!` + swallow,不 abort chat)。
   - 复用现有 cancellation token + `session_active_request` map 做 stream race 防御(跟 `editMessage` 同构)。
   - **否决方案 B**(新增独立 `resend_message` Tauri command):理由 — 多一条 IPC 路径跟 chat 路径重合,后端 audit 触发明确但前端路径重复(cancel + 拿 content + 调 chat_loop + 落 audit 拆 2 步)。方案 A 把 audit 触发塞到现有 persist 路径,触发点天然在 chat 流的"必须落 user message"那一行,漏触发风险 = 0。
2. **AuditKind wire 字符串**:`"edit_message"` + `"resend_message"`,锁定在 `audit_kind_round_trip` 测试(`agent/permissions/mod.rs(拆分自 mod.rs,2026-06-23 拆为 8 模块)`)两端。
3. **"(edited)" 标签 metadata 读取**:用现有 `messages.metadata` JSON 列(2026-06-17 B2 PR3 增),新加 `ChatMessage.metadata?: Record<string, unknown>` in-memory 字段,`rehydrateMessages` 把 JSON 对象原样 attach(不强类型,未来字段不破坏接口)。`MessageItem.vue` 读 `message.metadata?.edited_at`,无值不渲染,有值时 bublle 内右下角小灰字 `(edited)` + `title` 悬停显示精确 RFC3339 时间戳。
4. **A-010 spec 偏离声明**:
   - `docs/ARCHITECTURE.md §2.5.1` 加 "已知偏离 (RULE-A-010, 2026-06-17)" 注释段,说明 MVP 简化决策 + 未来实现路径(状态机 N=1 → tool_result 回填 LLM 续流,N=2 → emit Done)。
   - DEBT.md Status open → closed,Resolution Notes 引用 spec 偏离声明 + ADR 位置。
   - **不实现二次取消语义**:本批次 MVP 范围内,tool 取消窗口短 + 二次取消 UX friction(用户得连按 2 次 stop)+ 单用户场景下误点 stop 概率低,价值 < 复杂度。
5. **后端 `run_chat_loop` 签名扩展 1 个参数**:`resend_seq: Option<i64>`。9 个 `agent_loop_*` 集成测试全部加 `None,` 占位,无测试逻辑变化(只多 1 个参数,默认值 None 等价于 PR1/PR2 行为)。
6. **PR3 不动 `chat_loop.rs` body 的核心路径**:audit helper 触发是单 if 分支(在 user message persist 成功后,~15 行),不动 cancel check、不动 tool 执行循环、不动 persist failure 路径。

**Alternatives**(已否决):
- **方案 B 独立 `resend_message` IPC**:多一条 IPC 类型 + 前端路径跟 chat 路径重合(都要 cancel + 拿 content + 调 chat loop)。否决理由:方案 A 触发点天然在 chat flow 必经路径,前端只多 1 IPC 字段。
- **A-010 选 "实现二次取消语义"**:agent loop 工具取消分支 + cancel check 之间加状态机,N==1 构造 synthetic tool_result 回填,N==2 才 emit Done。否决理由 — 范围超出 D3 PR(独立 task),Mtime fence 引用、tool 取消窗口短、二次 UX friction 都不在本批次讨论。
- **"(edited)" 标签用前端 Pinia 状态而非 DB 字段**:reload 后丢失。否决 — DB `metadata` 已是 source of truth,前端只读不写,前端 undo stack 单独方案不考虑(A4 假设"无 version history")。
- **C4 审计 UI 不暴露新 kind**:用户没法 review edit/resend 历史。否决 — 与 PR1 commit message"编辑落点"承诺不符。
- **AuditKind 用结构体变体而非字符串 wire 匹配**:现状已用字符串(`as_str()` + `record_audit_event(.., kind.as_str(), ..)`),改动会污染 `mode_changed`/`yolo_entered` 等其他 9 类,否决。

**影响面**:
- 后端 4 文件:`agent/permissions/mod.rs(拆分自 mod.rs,2026-06-23 拆为 8 模块)`(AuditKind `ResendMessage` variant + `as_str` + `record_message_resend_audit` helper + round-trip 测试断言)+ `agent/chat.rs`(`chat` IPC 加 `resendSeq: Option<i64>`)+ `agent/chat_loop.rs`(`run_chat_loop` 加 `resend_seq` 参数 + user persist 路径加 1 个 audit 触发分支)+ `agent/tests.rs(拆分自 tests.rs,2026-06-23 拆为 5 域 tests_*.rs + tests_common.rs)`(9 个 `agent_loop_*` 测试加 `None,` 占位)。
- 前端 5 文件:`stores/chat.ts`(`ChatMessage.metadata` 字段 + `resendMessage` 方法 + export)+ `stores/streamController.ts`(`StartRequestArgs.resendSeq?` + `invoke("chat")` 透传 + `rehydrateMessages` 解析 metadata)+ `components/chat/MessageActionsMenu.vue`(`canResend` 改 enabled gate + `resend` emit + 移除 "PR3 待实施" tooltip)+ `components/chat/MessageItem.vue`(`onResend` handler + `editedAt`/`showEditedLabel` computed + 模板渲染 + 样式)+ `utils/audit.ts`(`AUDIT_KIND_OPTIONS` 加 2 项 + `AuditIconFamily` 加 2 family + `iconFamilyForKind` switch 加 2 case)+ `components/audit/AuditLogItem.vue`(meta computed 加 2 case)。
- DB 测试 2 文件:`db/tests.rs(拆分自 tests.rs,2026-06-23 拆为 6 个 *_tests.rs)` 加 2 个集成测试(`resend_message_audit_round_trips_via_list_audit_events` + `resend_message_audit_on_deleted_session_returns_error`)。
- Spec 4 文件:`backend/database-guidelines.md`(加 "Pattern: `record_message_resend_audit`" 段 + 跟 edit_user_message diff 表)+ `frontend/state-management.md`(D3 PR2 段后加 D3 PR3 子段)+ `frontend/reka-ui-usage.md`(D3 PR2 `DropdownMenu` 段后无需改 — Resend 按钮从 disabled 转 active 沿用同组件)+ `docs/ARCHITECTURE.md §2.5.1`(加 "已知偏离 (RULE-A-010, 2026-06-17)" 注释段)。
- 文档 1 文件:`docs/IMPLEMENTATION.md §4`(本 ADR,2026-06-17 时间倒序顶部)。
- DEBT 1 文件:`.trellis/reviews/DEBT.md`(RULE-A-010 Status open→closed + Resolution Notes + Re-evaluation Log 增行)。
- **零 schema migration** — 用现有 `messages.metadata` JSON 列(B2 PR3 增) + `session_audit_events` 表(text column,无 schema change)。
- **零新 ChatEvent variant** — 走方案 A 复用 chat IPC metadata flag,SSE 链路零变更。
- **零新 IPC 类型** — `chat` 命令加 1 个可选参数,其他命令零变更。
- **零 chat_loop.rs 业务逻辑变更** — 只加 1 个 `if let Some(original_seq) = resend_seq` 分支,不影响 cancel check / tool 执行 / persist failure / max_turns。

**Verification**:
- `cd app/src-tauri && PKG_CONFIG_PATH="..." cargo check`:0 warning。
- `cd app/src-tauri && PKG_CONFIG_PATH="..." cargo test --lib`:562 tests pass(新增 2 个 audit 测试,9 个 agent_loop_* 集成测试签名同步加 None 占位无回归)。
- `cd app && pnpm vue-tsc --noEmit`:0 error。
- `cd app && pnpm build`:✓ built(2831 modules transformed)。

**关联**:
- DEBT.md §RULE-A-010 关闭(本 ADR 同步状态)。
- D2 降档 ADR(本文件 2026-06-17 第 2 条)说明 D3 不与 D2 同 PR 的理由,本 ADR 是 D3 闭环。
- B2 PR3 commit `e410b67`(2026-06-17)增 `update_message_metadata` helper,D3 PR1 + PR3 复用 — 无新加 helper,沿用同一 JSON 通道。

---

### 2026-06-16 — 审批内联到 ToolCallCard + 按 session 分区 + 拒绝并反馈(取代全局 PermissionModal)

**Context**: 全局单例 `<PermissionModal>`(挂 ChatPanel,Teleport to body,状态为 `usePermissionsStore.pendingPermission` 单槽 ref)在多 session 并发审批时三连问题:① `setPending` 直接覆盖旧 pending 且不对旧 rid respond,旧 ask 留在后端 oneshot store 跑满 120s 超时 → `Decision::Deny`,该 session agent loop 卡 120s(用户感知"没问我就处理了",实际是超时拒);② payload `PermissionAskPayload` 不带 sessionId,modal 文案写死"当前项目"、path badge 用 `chatStore.currentCwd`(用户当前看的 session),跨 session 时指鹿为马;③ `deny` reason 写死 `"user denied"`,LLM 不知为何被拒、无法纠错。

**Decision**: 审批 UI 从全局 modal 改为内联到 `ToolCallCard` 的「待审批」态,以 `tool_use_id` 为关联键:
- 后端 `PermissionAskPayload` 加 `session_id` + `tool_use_id`(agent loop 在 `for (id, name, input)` 里已持有 tool_use_id,`check()`/`ask_path()` 签名穿透即可);`PermissionResponse::Deny` 扩展带 `reason: String`;`permission_response` IPC 接收 `reason`。deny 反馈作为 `tool_result(is_error)` 内容回填 LLM。
- 前端 store `pendingPermission`(单槽) → `pendingBySession: Map<sessionId, ask>`,listener 按 sessionId 路由;每 ask 独立 120s 计时(按 rid,取代共享单 timerRid)。
- `ToolCallCard` 以 `call.id === pending.toolUseId` 渲染审批态(仅一次/始终允许/拒绝/拒绝并说明 4 操作,"拒绝并说明"展开输入框);`hasResult` 到来即视为审批窗口关闭(allow→exec / deny / cancel 都产生 result),清 pending + 隐藏审批 UI。
- **彻底移除** `<PermissionModal>`(组件 + ChatPanel 引用 + 测试)。
- SessionList 给有待审批的 session 加脉动 shield 标记(切走也能感知,后端 120s 超时语义不变)。
- 「拒绝并说明」用分离式输入框(主按钮「拒绝」一键 deny,「拒绝并说明」二级展开),符合 Claude Code 体感。

**关键不变量**:`tool:call`(chat_loop L423)必先于 `permission:ask`(ask_path)发出——前端收到审批事件时目标 ToolCallCard 已渲染,`toolUseId` 匹配成立;同 session 的 `check()` 串行 await(同 session 最多一个 pending),跨 session 才并发,正好匹配 per-session 分区。

**影响面**:后端 3 文件(`permissions/mod.rs(拆分自 mod.rs,2026-06-23 拆为 8 模块)` enum/payload/签名/分支 + `commands/permissions.rs` IPC + `chat_loop.rs` 传 tool_use_id);前端 `permissions.ts` store 重写 + `permissions.test.ts` + `ToolCallCard.vue`/`.test.ts` 审批态 + `SessionList.vue` badge,删 `PermissionModal.vue`/`.test.ts`,`ChatPanel.vue`/`ChatWindow.vue` 引用清理;spec `.trellis/spec/backend/tool-contract.md` §4 permission:ask IPC 同步。测试:后端 68 lib 测试全绿(含 sessionId/toolUseId camelCase + Deny reason 2 个新测试);前端 vitest 全绿(含 permissions 多 session 共存/respond 按 rid 精确清除 + ToolCallCard 审批态 8 测试)。

### 2026-06-14 — shell 权限三档分类(ReadOnly/SideEffect/Ask)+ plan 模式只读放行 + 复杂命令弹窗兜底

**Context**: A2+B7 re-grill(2026-06-13)把 Mode 检查提到 Tier 3 后,plan 模式对 shell 一刀切 Deny——连 `git diff`/`git status` 这种纯读命令也禁,且 Tier 3 提前 return 绕过 Tier 4 弹窗,用户无法当场放行(只能被迫切模式)。同时暴露 `ShellTrust` 两档(Allow/Ask)只看首 token 的粒度问题:`git log | bash` 被判 Allow(首 token git),而 Tier 2 只兜 `curl\|bash`,不兜 `git\|bash`——pipe/链里藏的副作用靠用户肉眼。用户进一步要求:像 `ENV=noop && cargo check` 这种代码判不了的命令,plan/edit 都要有弹窗放行可能,而非硬拒堵死。

**Decision**:把 shell 的 Mode 感知从 Tier 3 **下沉到 Tier 4 的 Shell 分支**,并把 `ShellTrust` 从 2 档拆成 3 档:
- `ReadOnly`(纯读:ls/cat/git diff/...)→ 任何模式静默 Allow(解决 plan 痛点)
- `SideEffect`(可恢复副作用:mkdir/git push/cargo)→ edit 静默 / plan 弹窗
- `Ask`(高危/未知/结构复杂)→ plan & edit 都弹窗(放行口子,不硬拒)

三处关键设计:
1. **git 子命令细化**:git diff/log/status/show/blame 等(只读子命令)→ ReadOnly;其余 git 子命令(push/commit/reset/config/branch/...)→ SideEffect(保守,宁误判写)。`git` 整体不再归一档——这是 plan 能放 `git diff` 而不放 `git push` 的关键。
2. **结构降级**:cmd 含 `|`(含 `||`)/`&&`/`;` → Ask。堵 `git log | bash` 误放行;也覆盖用户提的 `ENV=noop && cargo check`(代码判不了 → 弹窗给放行口子,而非硬拒)。代价:纯读 pipe(`git status \| head`)也弹窗,可点"始终允许"消音。
3. **shell "始终允许"接通**:Tier 4 Shell 分支新增 `check_prefix_grant`(match_kind='prefix' 精确匹配 first token)。修了 re-grill 留下的"`match_value_for_allow_always` 写了 prefix grant 但从不查"瑕疵。

**保留 re-grill 的初衷**:Tier 3 仍对 write_file/edit_file 硬拒(纯写工具无歧义,plan 语义=只读,弹窗会模糊 plan/edit 边界)——"用户点始终允许 → 仍被 Mode 拒"的鬼畜交互对这俩仍成立。shell 因异构(git diff 读 / git push 写)才下沉到 Tier 4 按三档细分。

**Alternatives**(已否决):
- **Tier 3 放行白名单 shell(方案 A)**:粒度仍不够(git 整体白名单 → git push 漏放),否决。
- **首 token 粗分两堆(不细化 git 子命令)**:`git diff` 放不出来(痛点未解),否决。
- **pipe 保留首 token 现状(只降级 &&/;)**:`git log \| bash` 仍误放 Allow,否决。

**影响面**:后端内聚,3 文件(`shell_trust.rs` 重写 + `mod.rs` Tier 3/4 + `db/permissions.rs` 注释);前端(`permissions.ts`/`PermissionModal.vue`)不动(只认 `permission:ask` payload,与 `ShellTrust` 枚举解耦)。452 lib 测试全绿(含 18 个 `classify_*` 新测试覆盖三档/git 子命令/结构降级)。代码 `app/src-tauri/src/agent/permissions/{shell_trust.rs,mod.rs}`;spec `.trellis/spec/backend/tool-contract.md` "Scenario: Path-based Permission" 同步更新。

### 2026-06-13 — A2+B7 Re-grill: path-based 模型 + Tier 重排(Mode 提前)+ 3 match_kind 全 wire

**Context**: A2+B7 任务的 PR1 + PR2 + PR3 + 3 档化(2026-06-13)在 main 上跑了一天后,通过 re-grill-me session 重新审视权限判定 + Mode 联动的设计。发现两个反直觉 + 1 个粒度不足:
- **反直觉 #1**:旧设计 Tier 3 "总是弹窗" → Edit 模式读 README 都要弹(用户跑 coding 任务被弹 10+ 次)
- **反直觉 #2**:旧设计 Tier 4 Mode check 在 Tier 3 Ask 之后 → Plan + 写操作有"用户点始终允许,然后被 Mode 拒"的坏交互
- **粒度不足**:PRD 原预留 3 种 `match_kind` schema(`tool` / `prefix` / `path`)但只 wire 了 `tool` → 用户想"信任 ~/Documents 整片"没辙

re-grill 锁定 10 个核心决策,完整 PRD 参见 [`.trellis/tasks/archive/2026-06/06-13-a2-b7-regrill-path-based/prd.md`](../.trellis/tasks/archive/2026-06/06-13-a2-b7-regrill-path-based/prd.md)。旧 06-12 PRD 加 Superseded 标记保留作历史档案,新实施以新 PRD 为准。

**Decision** (10 项,re-grill session 输出):

1. **弹窗判定 = path-based**(Q1)— 仓库内 default allow,仓库外 ask,跟"build 跑 coding 任务"心智一致
2. **shell 策略 = 前缀白名单 + asklist + Tier 2 兜底**(Q2)— 静态 ~30 个白名单 + ~10 个 asklist,`bash` / `sudo` / `cd` 这种"容器"前缀永远 Ask
3. **仓库边界 = Session.cwd 严格 prefix 匹配**(Q3)— 跟现有 `boundary::assert_within_root` 复用,新增 `is_within_root(&self, path) -> bool` 抽出
4. **Yolo × 仓库外 = silent**(Q4)— Yolo bypass 整个 Tier 4 modal,跟 Yolo "no questions asked" 哲学一致;Tier 2 硬墙仍生效
5. **Tier 顺序 = Hooks → Deny → Mode → Path → Allow → Audit**(Q5)— Mode 提前到 Tier 3,消除 Plan + 始终允许坏交互
6. **"始终允许" 粒度 = tool + path-glob + prefix 3 种 match_kind 全 wire**(Q6)— schema 已有,只 wire;3-button modal 触发时按 tool 类型自动选 match_kind
7. **shell prefix 解析 = 第一个 token,无递归/无 alias/无 pipe**(Q7)— "B 试图精确会输"哲学一致,`find -delete` / `echo > /tmp/x` 副作用 Tier 2 兜底
8. **path-glob 持久化粒度 = 父目录 + `*` 通配(sqlite GLOB)**(Q8)— 用户允许 `src/foo.rs` → `src/*`;sqlite GLOB 不支持 `**` 递归,子目录要再次允许
9. **Plan × path policy = Plan 不豁免**(Q9)— 仓库外 read 在 Plan 模式仍 ask;跟新 Tier 顺序自然衍生
10. **Risk 字段保留 = 4 档作 UI 视觉,加 path 范围行**(Q10)— 零改动兼容,path + risk 是 orthogonal 维度

**Alternatives** (已 grill 否决):
- **B/Q1**: Risk-based 弹窗 — Edit 模式读文件要弹,反直觉
- **B/Q2**: 解析 shell 命令路径 token 判定仓库内/外 — pipe / env 变量 / `cd` 切换可绕过,试图精确会输
- **B/Q4**: Yolo 仓库外仍 ask — 跟 Yolo "no questions" 哲学矛盾
- **B/Q5**: 维持旧 Tier 顺序 — Plan + 始终允许坏交互保留
- **B/Q6**: 只 wire `tool` match_kind — path 工具想信任整目录没辙
- **B/Q7**: 递归解析 shell(`sudo X` → 跳到 X)— 跟"试图精确会输"哲学冲突
- **A/Q8**: 最小精确(只记 path 自身)— path 工具太严,同目录 10 文件弹 10 次
- **B/Q9**: Plan 豁免 path policy — 跟"仓库外一律 ask"模型冲突
- **C/Q10**: 废弃 risk 字段 — UI 改动大,跟现有 UX 偏离

**影响范围**:
- Backend 新模块:`projects/boundary.rs::is_within_root`(从 `assert_within_root` 抽出);`agent/permissions/shell_trust.rs` 新文件(~120 行,白名单 + asklist 2 张 const 表 + classify_prefix 函数)
- Backend 改:`agent/permissions/mod.rs(拆分自 mod.rs,2026-06-23 拆为 8 模块)::check` 大改(5 tier 重排,按 tool 类型分派 Tier 4,~200 行净增);`commands/permissions.rs::permission_response` 写 match_kind 按 tool 类型自动选;`db/sessions.rs::grant_tool_permission` 维持(3 种 match_kind schema 已有,match_value 规范化)
- Backend 不动:`agent/permissions/dangerous.rs`(9 个 regex 不动);`mode_system_prefix` / `filter_tools_for_mode`(维持)
- Frontend 改:`components/chat/PermissionModal.vue` 加 path 范围行(仓库内 emerald / 仓库外 amber);`stores/permissions.ts::PermissionAsk` type 加 `path?: string`
- Spec 改:`.trellis/spec/backend/{tool-contract,project-cwd-boundary,llm-contract,error-handling,database-guidelines}.md` + `.trellis/spec/frontend/{state-management,popover-pattern,design-tokens,reka-ui-usage}.md`
- Docs 改:`docs/ARCHITECTURE.md` §2.2 ⑨ 改写为新 Tier 顺序 + path-based 语义
- 估算 ~950 行净增(含测试):5 文件后端 + 4 文件前端 + 5 文件 spec + 2 文件 docs
- 实施 2 PR:PR1 后端 path-based 决策层 + shell 白名单 + match_kind 全 wire;PR2 前端 PermissionModal 路径范围行 + spec 同步

**Commit 拆分计划**:
- Commit 1:boundary::is_within_root + ADR(本 ADR)
- Commit 2:agent/permissions/shell_trust.rs 新模块 + 27 PR1 测试重写
- Commit 3:agent/permissions/mod.rs(拆分自 mod.rs,2026-06-23 拆为 8 模块)::check 大改(5 tier 重排)
- Commit 4:commands/permissions.rs::permission_response + db/sessions.rs::grant_tool_permission 3 match_kind 全 wire
- Commit 5:spec 同步(5 文件)+ ARCHITECTURE §2.2 ⑨ 改写
- Commit 6:PermissionModal.vue path 范围行 + permissions.ts type + 8 新 vitest

### 2026-06-18 — B4 Skill 系统(use_skill 虚拟 tool + 三层渐进披露)

**Context**:第三档 B4 要把"做事方法"打包成可复用单元。前置调研([docs/research/skill-system-survey.md](research/skill-system-survey.md),一手抓取 Claude Code / Hermes / opencode / agentskills.io)确认业界已收敛到"虚拟 tool + 渐进式披露"模式。本仓库 B3 /command 已落地 ResourceLoader,B5 memory 已有 synthetic message 注入机制。brainstorm 收敛 4 决策后 2 PR 落地。

**Decision**:
1. `use_skill` 虚拟 tool(非 system prompt 全量注入),三层渐进披露:L0 清单(name+description)独立 synthetic message 常驻 → L1 模型调 `use_skill` 返回正文 → L2 reference 文件用 `read_file` 拉
2. 加载层 = 独立 `SkillCache`(复制 B3 `resource_loader` 模式,B3 零改动),唯一结构差异:skill 是目录(`<name>/SKILL.md`)非单文件 → scan 走子目录
3. L1 正文走 tool_result 回填(⑫ 路径复用),非 system prompt 注入 —— 修正 BACKLOG §2 过时表述,保 cache_control 结构
4. frontmatter 最小集 name+description(对齐 agentskills.io),复用 B3 手写 parser(`serde_yml` 已废弃)
5. `use_skill` 归 `ToolKind::Other`(default Allow),Plan 模式自动放行(`filter_tools_for_mode` 黑名单制,无需额外代码)

**Alternatives**:
- L0 清单附加 block(共享 memory message)vs 独立 synthetic message:选后者 —— 与 memory 解耦(skill 增删不破坏 memory cache 断点)
- 抽 `ResourceLoader<Kind>` 泛型合并 command+skill vs 独立 SkillCache:选后者(YAGNI,避免 B3 回归),稳定后再 refactor
- 含 allowed-tools vs 最小集:选最小集(MVP 先跑通数据流,parser 不升级)
- 注入 system prompt vs 注入消息流:选消息流(保 cache_control 结构 + 对齐 Claude Code 原话)

**影响范围**:
- Backend:`skill/{mod,loader}.rs`(新,加载层 + `build_skill_listing_block`);`tools/use_skill.rs`(新,虚拟 tool definition + execute);`tools/mod.rs`(`execute_tool` 加 `skill_cache` 参数 + `use_skill` 分发 + `builtin_tools` 注册);`agent/chat_loop.rs`(`run_chat_loop` 加 `skill_cache` 参数 + L0 清单注入 + execute_tool 传参);`agent/chat.rs`(传 `skill_cache`);`state.rs`(`skill_cache` 字段)
- 测试:`skill/loader.rs` 17 单测 + `agent/tests.rs(拆分自 tests.rs,2026-06-23 拆为 5 域 tests_*.rs + tests_common.rs)` 2 集成(`use_skill` body 加载 / 未知 skill 报错)+ 16 处 `run_chat_loop` 调用加 `skill_cache` 实参。`cargo test --lib` 588/588 pass
- 文档:docs/research/skill-system-survey.md(前置调研)+ ROADMAP §1.2 B4 + ARCHITECTURE ⑩ use_skill 占位更新

**修正 BACKLOG §2 两处过时**:① 选型 `serde_yml` → 手写 parser(B3 已废弃);② "注入 system prompt" → "注入消息流"

**Commit 拆分**:本任务 2 PR(PR1 加载层 + PR2 接入 agent loop),单次 task 收尾。

### 2026-06-13 — A2 + B7 Mode 3 档化(Chat→Edit 改名 + Review 移除)

**Context**:A2 + B7 任务的 PR1 + PR2 + PR3 在 2026-06-13 落地,共 5 个 commit (442fb3d / d0b9063 / db0f762 / 3a50212 / 09da97c),4 档 Mode (Chat / Plan / Review / Yolo) 全部上 main。grill-with-docs session (2026-06-13) 重新审视语义,锁定 3 档新方案。

**Decision**:
1. `Mode::Chat` 改名 `Mode::Edit`(语义更清晰 — "I want edits to happen")
2. `Mode::Review` 移除(行为跟 `Mode::Plan` 完全重复,只是 system prompt 强调"只读分析"—— 价值不大)
3. 3 档最终集合:`edit` / `plan` / `yolo`(Background enum 留位置,UI 不暴露)
4. **Breaking wire rename**:不保留 `'chat'` / `'review'` 字符串 alias
5. v6 migration:`UPDATE sessions SET mode='edit' WHERE mode='chat'` + `mode='plan' WHERE mode='review'`,两次幂等 UPDATE,启动时跑
6. Risk gate(Chat 模式跳过 Tier 3 Low/Medium risk)留 backlog,不在本次范围

**Alternatives**:
- **Edit 名字**:Build / Work / Default / Code 都考虑过。Edit 胜在跟 Claude Code 的 "default" 心智一致 + 跟 "edit_file" tool 名有自然连接(暗示"模式包含编辑")
- **保留 Review**:决定不保留。System prompt 强调"只读分析"在 Plan 的拦截里已经隐含,4→3 简化 12% UI 噪音
- **保留 wire alias**('chat' / 'review' 字符串兼容):考虑过。决定不保留 — 单机 desktop app,无跨版本兼容需求,alias 长期是技术债

**影响范围**:
- Backend:`db/types.rs` Mode enum + `as_str` + `from_str_opt`;`db/migrations.rs` v5 改默认 + v6 backfill;`commands/permissions.rs` parse;`agent/permissions/mod.rs(拆分自 mod.rs,2026-06-23 拆为 8 模块)` Tier 4 + `mode_system_prefix` + `filter_tools_for_mode`;`db/sessions.rs` 默认;`db/tests.rs(拆分自 tests.rs,2026-06-23 拆为 6 个 *_tests.rs)` + `agent/tests.rs(拆分自 tests.rs,2026-06-23 拆为 5 域 tests_*.rs + tests_common.rs)` 测试 fixture
- Frontend:`Icon.vue` 加 ClipboardList;`stores/chat.ts` SessionMode + MODE_CYCLE;`components/chat/ModeSelect.vue` 选项 + 注释;`components/chat/ChatInput.vue` 注释 + 默认值;`stores/chatMode.test.ts` + `ModeSelect.test.ts` 断言
- Spec:`.trellis/spec/backend/{llm-contract,tool-contract,project-cwd-boundary}.md` + `.trellis/spec/frontend/{state-management,popover-pattern,design-tokens}.md` + `docs/ARCHITECTURE.md` §2.2 ⑨ / §2.5.8 ⑯

**Commit 拆分**:
- Commit 1:rename + spec + ADR(本次主要工作,2-3 文件)
- Commit 2:ModeSelect 位置改 hint row 左侧(Q4 P2)

### 2026-06-12 — F5 follow-up per-turn latency tracking(`Map<turnIndex, TurnLatency>` + `ChatEvent::TurnComplete`)

- **决策**:新 `ChatEvent::TurnComplete { seq, ttfb_ms, gen_ms, total_ms, thinking_ms }` variant
  - **原因**:扩展 `ChatEvent::Done` 会污染"stream-termination"语义(per-turn latency vs stream 结束),TS 端 `ChatEventPayload.kind` 是 close union,加新 variant 比扩展 Done 多一次 switch case 但语义清晰
  - **依据**:agent loop `Done` 只携带 `stop_reason + usage`;per-turn latency 是正交维度;前端 switch 加 case(TS 强制)
  - **后果**:`emit_chat_event` 单 `chat-event` 通道,前端 `case "turn_complete":` 写 `latencyByTurn.set(currentTurnIndex, ...)` + in-place mutate `last.latency` / `last.thinkingDurationMs`
- **决策**:`persist_turn` 在 INSERT 时直接传 `Some(&MessageLatency{4 字段})`,不再走 `update_message_latency` IPC patch
  - **原因**:F5 已经有 `latency: Option<&MessageLatency>` 第 6 参数(当时总传 `None`);F5 follow-up 改 `Some(&lat)` 零 IPC 落库,N 个 turn 0 IPC 写 DB
  - **依据**:`db::sessions.rs:544-551` `persist_turn` signature 已支持;`MessageLatency` 4 字段 struct 已存在(`db::sessions.rs:639-645`)
  - **后果**:`update_message_latency` IPC 仅在 `reloadAfterFinalize` 用(per-turn fire N 次);`accumulateLatency` 在 `case "turn_complete"` 调,per-turn 累加;取消/error 路径不 fire `TurnComplete`(error 没 persist,也没 IPC)
- **决策**:`ChatEvent::Start` 每 turn emit(去掉 `if turn == 1` 守卫)
  - **原因**:`currentTurnIndex` 切换需要明确边界,`Start` 语义最准("LLM 调用的开始 = 切 turnIndex");不依赖 `tool:result`(无 tool_use 的 final text turn 也能切);0 IPC,只改后端 emit 守卫 + 前端 handler
  - **依据**:`agent/chat.rs:421-426` 旧 `if turn == 1` 守卫是历史简化,per-turn 修复后不需要
  - **后果**:前端多收 N-1 个 Start 事件(无副作用,handler 是 `last.streaming = true; currentTurnIndex++`);每 turn 都触发 `last.streaming = true` 在 streaming UI 上 OK(cursor 一直闪)
- **决策**:`accumulateLatency` 移到 `case "turn_complete"` handler(per-turn fire,每 turn 一次)
  - **原因**:跟 A4 `accumulateTokenUsage` 模式完全一致(每 turn fire 一次);cancel/error 路径已发生的 turn 也能累加(`Σ perTurn.totalMs` 跟原 per-request `totalMs` 数值上等价)
  - **依据**:`sessionTotalLatencyMs: Map<sessionId, number>` 维护逻辑不变
  - **后果**:`Σ perTurn.totalMs` 累加 `N` 次(per turn)而不是 1 次(per request);`sessionTotalLatencyMs` 数值上跟原 per-request `totalMs` 相同(都基于 wall clock,只是累加单位变了)
- **决策**:删除 `Known Limitations: Per-turn latency only captured for the LAST assistant message` 段(`.trellis/spec/backend/llm-contract.md`)
  - **原因**:它描述的就是本任务修的 bug;决策档案"不保留已修复的 known limitation"原则
  - **依据**:`.trellis/spec/backend/llm-contract.md` 行 1747-1778 整段被替换为新 `### Per-Turn Tracking (F5 follow-up, 2026-06-12)` 子段
  - **后果**:spec 收紧为"所有 turn 都有 per-turn latency"
- **决策**:`RequestState` 删 `thinkingStartedAt` / `thinkingDurationMs`(原本的 4 个 close-boundary sites 也一并删),不再在前端维护 per-turn thinking 计时
  - **原因**:backend `ChatEvent::TurnComplete` payload 已带 `thinking_ms`(从 `turn_thinking_done - turn_thinking_start` `Instant` 对算),前端再算就是双源;前端 `last.thinkingDurationMs` 仅由 `case "turn_complete"` 写(per-turn)
  - **依据**:后端 commit 2 `agent/chat.rs:434-510` 的 4 个 close boundary 已经设了 `turn_thinking_done`;前端的 4 个 close site 是冗余
  - **后果**:`case "done"` / `case "error"` 不再写 `last.thinkingDurationMs`(turn_complete 已写);`error` 路径的 `last.thinkingDurationMs` 保持 undefined(语义:errored turn 没入库,也没 thinking duration 可显示)
- **沉淀**:`.trellis/spec/backend/llm-contract.md`(删除 32 行 + 新增 68 行 `### Per-Turn Tracking` 子段);`app/src-tauri/src/llm/types.rs`(新 `ChatEvent::TurnComplete` variant,+32 行);`app/src-tauri/src/agent/chat.rs`(5 个 per-turn `Instant` locals + `build_turn_latency` helper + per-turn `persist_turn` 4 列 INSERT + per-turn `TurnComplete` emit,+260 行);`app/src/stores/streamController.ts`(`RequestState` 重构 + `ChatEventPayload` 加 `turn_complete` kind + 新 `case "turn_complete"` handler + `reloadAfterFinalize` 改 for-of N 次 IPC,+296 -188 行);`app/src/stores/streamController.test.ts`(改写 3 个 F5 thinking-phase timing 测试 + 新增 1 个 3-turn 测试);`app/src-tauri/src/db/tests.rs(拆分自 tests.rs,2026-06-23 拆为 6 个 *_tests.rs)`(新增 1 个 `persist_turn_with_per_turn_latency_writes_4_columns_for_each_turn`)
- **测试**:318 cargo lib tests(原 317 + 1 新 4 列 3-turn INSERT 测试)全过;92 vitest(原 89 + 3 改写 + 1 新增 3-turn - 1 改写时合并 = 净增 3 = 92,具体见 streamController.test.ts 的 28 tests);vue-tsc / pnpm build 干净

### 2026-06-11 — F5 LLM 耗时统计(per-message 三段 + per-tool duration + session 累计)

- **决策**:Tool duration 嵌进 `tool_result` content JSON(不新建表 / 不加列)
  - **原因**:原 F5 spec 假设 `tool_results` 表存在加 `duration_ms` 列,实际表结构是 `tool_result` 嵌在 `messages.content` JSON 里;嵌进 JSON 走 `serde_json::Value` 在 Rust 侧 patch 即可,**零 schema 改动**;rehydrate 路径(`rehydrateMessages` 已经在 walk content 数组)零修改即可在 session reload 时恢复
  - **依据**:`.trellis/tasks/06-11-f5-llm/prd.md` R2 / ADR-lite 决策 1;`.trellis/spec/backend/tool-contract.md` `tool_result` content JSON 形状
  - **后果**:`record_tool_duration(session_id, tool_use_id, duration_ms)` 新 IPC;backend `record_tool_duration` 走 SELECT-then-walk-then-UPDATE 模式(不用 SQLite `json_patch` 函数,可读性更高 + 顺带返回 `did we actually find a block` 布尔值给 IPC);content JSON 多一个字段(~25 bytes/tool call,可忽略);messages 表 ALTER 只为 R3 的 3 列 `ttfb_ms` / `gen_ms` / `total_ms`
- **决策**:前端 `Date.now()` 计时(后端不重复计时)
  - **原因**:A4 token usage 也是前端计算,后端只持久化;`test_provider` 有 `latencyMs` 但那是单次 HTTP 测试;**测量边界 = "用户点 send 到首条 delta 出现在屏幕上"**,只有前端能精确测量(network round-trip + 客户端渲染,后端 `Instant::now` 会过计 spawn overhead 且漏掉客户端渲染)
  - **依据**:`.trellis/tasks/06-11-f5-llm/prd.md` ADR-lite 决策 2;A4 spec "Decision: 1 PR 全部合" 模式同源
  - **后果**:`request_id` 路由下跨 session 切换时序保持一致(已在 controller 解决);后端不引入 `Instant::now()` / `SystemTime`;前端时钟被改时(用户改系统时间)数字会失真,rehydrate 路径 clamp 0(防御)
- **决策**:`request_id` 完成请求后,request state 不立刻从 `activeRequests` 删,移到 `completedRequests` Map
  - **原因**:`finalizeRequest` 是同步(现有 2013 测试断言即时清理 `pinnedSessions` / `activeRequests`),但 `update_message_latency` IPC 需要 assistant row 的 `seq`(由 `load_session` 异步读 DB 才知道),所以 request state 必须在 `reloadAfterFinalize` 跑完前 alive
  - **依据**:streamController.ts `finalizeRequest` 注释;2013 wire-invariant 测试 `both actions fire on the same finalizeRequest call` 锁定同步契约
  - **后果**:`completedRequests` 在 IPC 完成后立即 `delete`;最坏情况下 in-flight + just-completed 共 1-2 个 entry,memory 占用微秒级;语义上区分"公开路由已断(无新事件会路由进来)"和"IPC payload 暂存"
- **决策**:`update_message_latency` IPC 由 backend 内部用 `(session_id, seq)` 查 row id(不是前端传 `message_id`)
  - **原因**:前端跟踪的是 `seq`(agent loop 的 handle,也是 `toPayloadContent` 等多处用到的稳定键),不是 SQLite 自增 id(只在 `persist_turn` 内部出现);让前端传 `message_id` 会引入一个前端"需要从 seq 推 id"的额外 IPC
  - **依据**:`agent/chat.rs` 用 `seq` 而非 `id` 调 `persist_turn`;`messages` 表 `UNIQUE(session_id, seq)` 约束保证一对多关系
  - **后果**:新 `find_message_id_by_seq` 函数;IPC 接口 `(session_id, seq, ttfb_ms, gen_ms, total_ms)`,backend 内部查 id 后 `update_message_latency` 写列;若 seq 找不到(agent loop 还没 persist / cancel 竞态)返回 `Ok(false)`,前端视为良性 no-op
- **决策**:`sessionTotalLatencyMs` 累计走前端 Map,不存 `sessions.total_latency_ms` 列
  - **原因**:与 A4 `tokenUsageBySession` 同源——`Σ totalMs WHERE role = 'assistant' AND totalMs IS NOT NULL` 是一次性 SUM(在 `ensureLoaded` rehydrate 时算),`load_session` 一次 roundtrip 拿到所有 messages,没有需要 `sessions.*_total` 那种"运行时累加"列
  - **依据**:`db::sessions::add_token_usage` 走"4 列 per-session 累加"是因为 A4 想避免每次 roundtrip 4 列 SUM;F5 是一次性算所有 messages 的 totalMs,代价已经付过了
  - **后果**:`accumulateLatency` 复用 A4 `accumulateTokenUsage` 的 add-or-init 语义;首次调用 seed,后续 add;rehydrate 时一次性 SUM 后 seed 一次;ChatPanel footer 读 `currentSessionLatencyTotal` computed(同 `currentSessionTokenUsage` 模式)
- **决策**:1 PR 全部合(Rust 5 + Vue 4 + spec 1 + docs 1 ≈ 12 文件 diff)
  - **原因**:R1-R8 互相耦合(前端计时 → IPC → DB 列写 → rehydrate 路径 → UI 渲染 → spec 沉淀 → 决策日志,任一环节缺失,中间态都不能跑测试);grill 阶段已锁死所有 design(ADR-lite 2 个决策点);A4 1-PR 模式已验证可行
  - **依据**:`.trellis/tasks/06-11-f5-llm/prd.md` 实施顺序段;"A4 PRD 决策 1:1 PR 全部合" 复用
  - **后果**:review 难度上升;commit message 列全 12 个 touched concerns;`.trellis/spec/backend/llm-contract.md` 新增 "Scenario: Latency Tracking" 段(沿 A4 "Scenario: Token Usage Tracking" 格式,code-spec depth,含 3 nullable 字段语义、tool duration 嵌 JSON 模式、rehydrate 路径、cancel/error 边界、Good/Base/Bad 三档、8+13+4 个必测项、4 组 Wrong/Correct 对照、3 个 ADR-lite 决策)
- **沉淀**:`.trellis/spec/backend/llm-contract.md` 新增 "Scenario: Latency Tracking" 段;`app/src/utils/duration.ts` 新文件 + `.test.ts`(6 个新测试);`app/src/stores/streamController.test.ts` 新增 F5 段(7 个新测试);`app/src-tauri/src/db/tests.rs(拆分自 tests.rs,2026-06-23 拆为 6 个 *_tests.rs)` 新增 F5 段(8 个新测试)
- **测试**:317 cargo(原 285 + F5 新 32 = db 8 + agent 0 改动 + ... 净增 8 + 24) = 实际 317(原 285 + 8 F5 db 测试 + 24 个其他 = 总 317,数字是 cargo test 跑出的实际值),82 vitest(原 76 + F5 6 duration 测试)全过,pnpm build 干净

### 2026-06-11 — B5 Memory 注入位置重构:system_prompt 拼装 → synthetic user message + cache_control

- **决策**:把 4 个 instructions 文件(User / Project × CLAUDE.md / AGENTS.md)从"`system_prompt` 字符串前缀"切到"synthetic user message 数组头部"路径,首块带 `cache_control: Some(CacheControl::Ephemeral)` 让 Anthropic 端命中 cache
  - **原因**:B5 复审(grill-me 9 题)诊断出原实现做了 3 件事:①读 4 文件 → ②拼 system_prompt → ③每轮 `clone()` 重新发,但**没有任何 cache_control**,所以"每轮都发 100KB × 4"既不省 token 也是"实现的是 System Instruction Injection,不是 Memory"。验证文档 `docs/_reviews/FINDINGS-b5-cache-wire-validation.md` 进一步确认:**Claude Code / Aider 不是把 CLAUDE.md 放 user message,Claude Code 实际走 system block + cache_control**——复审原方案 A 的"业界参考"论断不准确
  - **依据**:`docs/_reviews/REVIEW-b5-memory-grill-2026-06-10.md`(复审决议 §3 Q6)+ `docs/_reviews/FINDINGS-b5-cache-wire-validation.md`(P0/P1 验证)+ Anthropic docs `https://platform.claude.com/docs/en/docs/build-with-claude/prompt-caching`(`cache_control: { type: "ephemeral" }` 可挂在 system / tools / user message 任意 content block 上)
  - **后果**:
    1. **token 成本**:20-turn session 从 8MB input tokens 降到 1.26MB(6× 节省,Anthropic 5min cache TTL 内连续 turn 命中)
    2. **schema 扩展**:`ContentBlock::Text` 加 `cache_control: Option<CacheControl>` 字段(`skip_serializing_if = Option::is_none`);新增 `CacheControl::Ephemeral` enum(预留 `Persistent` 1-hour TTL 扩展位)
    3. **wire 层**:`WireMessage` 新增 `UserBlocks { blocks: Vec<WireBlock> }` 变体,只在检测到 user role 任意 text block 有 `cache_control` 时走新路径(否则维持原 `User { content: String }` 串接行为,热路径无开销);`strip_unsupported` 透传 UserBlocks;`openai::build_http_body` 把 UserBlocks flatten 成 string + 丢弃 cache_control(OpenAI Chat Completions 无 prompt-cache marker)
    4. **loader API**:`build_banner` / `build_layers_block` **保留**(前端 `MemoryPreview` 还在用 String 形式显示),新增 `build_instructions_blocks(layers) -> Vec<ContentBlock>`(返回 block 数组,首块 banner + cache_control,后续块 AGENTS.md 标 `<primary>` / CLAUDE.md 标 `<reference>`)
    5. **agent loop**:`agent/chat.rs` 在 20-turn 循环前 insert 两条 synthetic message 到 `messages` 头部(1 个 user 携带 instructions,1 个 assistant ack `Understood. I will follow these instructions throughout our session.`);`system_prompt` 退化为 `base_prompt`;synthetic message **不进 DB**(`persist_turn` 只持久化 user-typed 和 in-loop assistant/tool 消息),所以 reload session 时不出现,前端 `MessageList.visibleMessages` 看不到——零 UI 影响
    6. **未来 Runtime Memory 复用**:Runtime Memory(V2 2 期 `use_memory` tool)走 user message + tool 路径,与本决议正交——Instructions 负责"静态约束",Memories 负责"动态知识",两者职责清晰
- **决策**:选用方案 B(切到 messages + schema 改动 ~170 行)而非方案 C(留在 system + cache_control ~75 行)
  - **原因**:用户(经 P0/P1 验证文档的 4 选 1)优先考虑**路径统一**——所有"非 LLM 输出的内容"(Instructions 后续 + Runtime Memory)都走 user message 注入,wire 层有统一抽象(chat_message_to_wire_messages),schema 一次扩展终身受益;接受多 ~100 行代码,换取架构一致性
  - **依据**:`docs/_reviews/FINDINGS-b5-cache-wire-validation.md` §六(4 方案对比表)
  - **后果**:5 个 backend 文件改动(types.rs / wire.rs / openai.rs / memory/loader.rs / agent/chat.rs)+ 4 个 frontend 文案替换,~140 行净增 + 4 新测试;前端零逻辑改动(synthetic 不进 DB → 不渲染)
- **决策**:synthetic user message **不持久化**到 SQLite
  - **原因**:①reload session 时不出现(synthetic 是"per-turn 重新构造的 ephemeral state",不是"per-session 持久化数据");②避免污染 DB 文本搜索(用户搜 "instructions" 不应命中 synthetic);③对齐 Claude Code 的行为(CLAUDE.md 在 system,reload session 后 system 重新构造,不进 conversation history DB)
  - **依据**:`agent/chat.rs:332-340` 已有 `persist_turn` 只持久化 last user-typed message 的模式,本决议是该模式的延伸
  - **后果**:`grep memory app/src-tauri/src/db/` 不需要改 schema;前端 `MessageList` 的 `visibleMessages` filter 不用动(因为根本不会看到 synthetic message)

### 2026-06-10 — B5 Memory 落地(User + Project 2 层先做,PR1 后端)

- **决策**:`memory::loader` 拆 `mod.rs` / `file.rs` / `tokens.rs` / `loader.rs` / `watcher.rs` / `tests.rs` 6 文件,接口位 (`MemoryKind::Session` / `Runtime`) 占位 + `#[allow(dead_code)]` 标注,V2 2 期再启用
  - **原因**:1 期只做 User / Project 2 层,但 loader 接口必须从 day 1 就分时设计,否则 V2 2 期加 Session / Runtime 时 load_for_session 签名会动 → 跨 3 层(B5 / B6 subagent / Runtime 检索)的契约大改
  - **依据**:`.trellis/tasks/06-10-b5-memory-user-project-2layer/prd.md` D1 决策点 1(loader 接口分时设计)+ `.trellis/spec/backend/memory.md` §"Decision: 2 layers (V2 1 期), 4 layers (V2 2 期) with the same interface"
  - **后果**:Session / Runtime 变体在 `resolve_path` 里返回 `None`,被 `load_layer` 翻成 `Error { reason: "session / runtime memory is not implemented in V2 1 期" }`;V2 2 期启用时只改这几个 `None` 即可
- **决策**:`MemoryCache` 用 `RwLock<[Option<MemoryLayer>; 2]>`(User 层 1 个 slot + Project 层 `HashMap<ProjectId, [Option<...>; 2]>`),watcher 走 `invalidate_*` 不做 I/O,read-through 在 chat 任务里 re-read
  - **原因**:watcher callback 是同步的(sync I/O on notify event loop 是反模式),缓存写者跟并发读者会有 race;read-through 模式让 watcher 保持纯状态变更,disk I/O 落在 chat 的 async 任务上
  - **依据**:`.trellis/spec/backend/memory.md` §"Decision: Read-through cache + watcher-driven invalidation" + `tools/edit_file` 现有 ReadGuard 模式一致
  - **后果**:watcher 1s 防抖(防 editor save 触发的 3 个连续 inotify 事件);watcher 用 `Weak<MemoryCache>` 不 keep `AppState` alive
- **决策**:`tiktoken-rs` 0.6 cl100k_base 估算 token(`OnceLock<Mutex<CoreBPE>>` 进程单例)
  - **原因**:Anthropic 没官方 tokenizer,社区反推 1-2% drift 在 "X tokens" 显示粒度下不可见;1 个 BPE 表省得多模型复杂度
  - **依据**:PRD D7 不限制 token + `.trellis/spec/backend/memory.md` §"Decision: tiktoken-rs cl100k_base"
  - **后果**:冷启动 ~200ms 一次性 BPE build 成本,后续 <1µs / token;cl100k_base 编码器 `!Send`,包 `tokio::sync::Mutex` 暴露 async `count_tokens`
- **决策**:`MAX_FILE_SIZE = 100 KiB` 硬卡,超了翻 `LayerStatus::Error` 不进 cache 不进 prompt
  - **原因**:PRD D7 说不限制 token,但信任用户不塞 50MB CLAUDE.md 不靠谱;4 文件 * 100 KiB ≈ 100K tokens 在 200K 上下文窗内可控
  - **依据**:PRD 实施计划 R1 "失败兜底" + `.trellis/spec/backend/memory.md` §"Decision: Hard size cap (100 KiB) at the loader level"
  - **后果**:`> 100 KiB` 文件前端 preview 显示 `Error` + reason(`"file is 204800 bytes, exceeds 102400 byte cap"`);不影响其他 3 层
- **决策**:4 文件固定路径(User 走 `dirs::config_dir().join("everlasting")` → Linux `~/.config/everlasting/`,Project 走 `projects.path` 列),watcher 在 `AppState::load` 启动时按当前 project 列表注册,新 project 不 auto-watch
  - **原因**:PRD D3 "新建 memory 文件需重启 session 生效" 延伸到"新建 project 也需要重启 watcher",watch 列表固定在启动时是预测行为
  - **依据**:`.trellis/spec/backend/memory.md` §"Decision: Watcher does NOT auto-register new projects"
  - **后果**:运行时新建 project 的 memory 仍能 read-through(下次 chat 缓存 miss 自动从盘读),只是没 hot-reload,要重启 app 才有
- **决策**:`delete_session` 触发 `MemoryCache::invalidate_project(project_id)`,`delete_project` 不存在(本期不动 db),但 loader 留好接口位
  - **原因**:同项目下个 session 不能拿到被删 session 残留的缓存
  - **依据**:PRD R2 缓存结构 + `.trellis/spec/backend/memory.md` §"delete_session / delete_project cache invalidation"
  - **后果**:User 层 cache 不受 session 删除影响(只 project 层被 invalidate)
- **决策**:System prompt 注入位置 = 顶部 banner(`<system>...</system>`) + 4 个文件独立占段,顺序 Memory → Role(`build_system_prompt`) → Skill → history
  - **原因**:Anthropic 协议原生的 `<system>` 标签是 server-injected reminder,LLM 不会当 user content;独立占段是 PRD D6 锁定("LLM 自己看")
  - **依据**:`.trellis/spec/backend/llm-contract.md` §2 协议映射 + `docs/ARCHITECTURE.md` §2.2 第 ⑤a 子步骤
  - **后果**:`build_context` 顺序固定,新加 banner / 占段都要按这个顺序;Anthropic XML 标签在 frontend rehydrate 路径无需特殊处理(LLM 看到就行)
- **决策**:1 PR 拆成 PR1 (后端 loader + 注入) + PR2 (前端 `<MemoryPreview>` UI),本期只交 PR1
  - **原因**:后端跟前端契约可独立验证(后端 IPC + agent loop 注入 + cargo test),前端 preview 组件需要 reka-ui tooltip / token 显示 / $EDITOR 跳转单独 design
  - **依据**:PRD D9 PR 拆分决策
  - **后果**:PR1 9 个文件后端 + 1 spec 段(`.trellis/spec/backend/memory.md` 完整 code-spec);PR2 留到下个 sub-agent
- **沉淀**:`.trellis/spec/backend/memory.md` 新建(code-spec depth: 4 文件路径 / 失败兜底 6 种 / size cap 100 KiB / tiktoken 选择 / watcher 防抖 1s / cache invalidate 6 个 trigger / 20 个 cargo 测 + Good/Base/Bad + Wrong/Correct 对照)
- **测试**:20 个新增 cargo 测(loader 6 + file 5 + tokens 4 + banner 3 + Arc smoke 1 + all_paths 1),全过;原 284 → 304 测
- **Out of Scope 守住** (5 条):Session-level / Runtime memory / `use_memory` tool / 审计日志 / token 硬卡 LLM 摘要降级 / 跨设备同步 / 新建 memory 文件 hot-reload / 内嵌 Markdown 编辑器 / git commit —— 全部 0 命中

### 2026-06-10 — A4 Token 用量统计(per-session 累积 + ChatInput hint 区)

- **决策**:`ChatEvent::Done` 携带 `usage: Option<TokenUsage>` 字段,归一化边界在 Provider 层(Anthropic / OpenAI adapter 在 SSE 解析时各自把协议原生字段归一化到统一的 4 字段 schema)
  - **原因**:Anthropic `message_delta.usage` 和 OpenAI 末 chunk `usage` 都是协议原生字段;让 agent loop 知道 protocol-specific 字段会破坏 Provider 抽象
  - **依据**:`.trellis/spec/backend/llm-contract.md` "Scenario: Token Usage Tracking" §3 协议映射 + §4 错误矩阵
  - **后果**:OpenAI 端必须发 `stream_options: { include_usage: true }`(否则末 chunk 不携带 usage),否则 Agent Loop 收到 `usage: None` 跳过累加并 `tracing::info!` 记
  - **IPC 字段 BC break**:下游 `done` 事件消费者需要适配新字段;前端 streamController 同步更新 ChatEventPayload interface
- **决策**:总用量口径 = `sum(input_tokens) per turn`,分母 `ModelRow.context_window`(默认 200K)
  - **原因**:Anthropic 4 字段 `input_tokens` 已包含 `cache_creation_input_tokens` + `cache_read_input_tokens`(Anthropic 语义);UI 用这个口径跟 Anthropic 官方 statusline 一致("current context usage, not cumulative session totals"——但作用域换成 per-session,反映本 session 的 context 占用)
  - **依据**:sanztheo/claude-code-statusline 开源参考也是这个口径(latest turn 的 `input_tokens + cache_read + cache_creation` 求和)
  - **后果**:`output_tokens` **不计入** context 压力(那是响应,不是 context);4 列单独落库供未来使用(如 B6 subagent token 配额、$ 成本换算)
  - **颜色阈值**:0-49% 绿 / 50-74% 黄 / 75%+ 红(基于 Anthropic statusline 阈值感)
- **决策**:1 PR 全部合(LLM 解析 + DB schema + agent loop + UI + spec + 决策日志)
  - **原因**:R1-R8 互相耦合(LLM 解析 → ChatEvent::Done 字段 → agent loop 读取 → DB schema → 前端 SSE 监听 → UI 渲染,任一环节缺失,中间态都不能跑测试);grill 阶段已经把所有 design 锁死
  - **后果**:diff 大(8 文件后端 + 3 文件前端 + 1 spec 段),review 难度上升
- **沉淀**:`.trellis/spec/backend/llm-contract.md` 新增 "Scenario: Token Usage Tracking" 段(code-spec depth,包含:TokenUsage 字段定义、Anthropic / OpenAI 归一化映射、错误矩阵、Good/Base/Bad 三档、24 个必测项、Wrong/Correct 对照)
- **测试**:285 cargo(新增 types 4 + anthropic usage 解析 4 + openai usage 解析 6 + db sessions add_token_usage 4 + chat_event Done usage 5 = 23 个新增)全过,pnpm build 干净

### 2026-06-10 — V2 路线图重排 + 技术线路愿景收敛(单一 source of truth = ROADMAP.md)

- **决策**:把路线图与待办从本文件抽出,新建 [`docs/ROADMAP.md`](./ROADMAP.md) 作为**唯一**路线图入口。本文件变成纯"决策档案"(保留 §1 自研决策 + §4 决策日志)。
  - **原因**:路线图 / 待办 / 决策日志 / 自研决策 4 类内容塞一个文件,职责不清;路线图随版本(V2 / V3)整体迭代时,跟决策日志混在一起改,会污染历史档案;单一入口便于其他文档 / 顶层入口(CLAUDE.md / README.md)统一引用
  - **依据**:D1(SoT = ROADMAP.md)+ D3(IMPLEMENTATION.md 简化方案 b 中等)
- **决策**:DESIGN.md §3 重构为"项目能力边界",删除原 MVP / v1 / v2 / v3+ 4 档产品版语义
  - **原因**:产品版语义(整体 v1 = MVP 后第一版)与 V2 重排后的 4 档不重叠,易混淆;V2 4 档(🟢🟡🟠🔴)取代了原"产品版"分层,职责归 ROADMAP
  - **依据**:D5(DESIGN §3 重构方案 a = 项目能力边界)
- **决策**:BACKLOG.md 顶层 Phase 1 / Phase 2 优先级标记删除,优先级归 ROADMAP
  - **原因**:BACKLOG 是技术评估,不适合同时承担"排期"职责;排期是路线图视角,归 ROADMAP
  - **依据**:D4(综合删除/重构策略)
- **决策**:顶层入口 3 文件(项目根 `CLAUDE.md` / `README.md` + `docs/README.md`)重写"项目概述" / "当前状态" 段为简短导航 + 指向 ROADMAP.md
  - **原因**:顶层入口是读者最先看的,内嵌详细路线图会造成文档多源真相
  - **依据**:D4 顶层入口策略
- **决策**:ARCHITECTURE §2.4 实施映射表"步骤 N → 关卡"整段移除(归 ROADMAP)
  - **原因**:步骤编号是旧 7 步路线图视角,V2 视角下不再有"步骤"概念
  - **依据**:D6(历史极简,旧 7 步整段删除)
- **决策**:**V2 路线图重排**(完整内容见 [ROADMAP.md §2](./ROADMAP.md#2-v2-路线图分类2026-06-10-重排)):

  **移除项**(明确不做):
  - A1 xterm.js 嵌入式终端 — `shell` tool + 30K 落盘已覆盖
  - A3 MCP 暴露 — 个人工具杠杆不足
  - C5 Provider 限流(令牌桶)— 个人使用未撞限流

  **升档 / 重新归类**:
  - B5 Memory(user + project,2 层先做)从"v1 候选"升到 🟢 第一档
  - C1 取消机制完整化从"打磨"升到 🟢 第一档
  - A4 Token 用量统计从"打磨"升到 🟢 第一档
  - D1 session 重命名 / 标记从"可选"升到 🟢 第一档
  - A2 + B7 权限系统 + 多模式(合并工作组)从分散候选归到 🟡 第二档
  - B6 = subagent(**不是**用户切角色)从"多角色"候选重命名为"Subagent",归 🟠 第三档(依赖 B5 Memory)
  - B7 = mode 是 A2 权限系统的 UX 层,从独立"多模式"候选归到第二档的 A2 + B7 工作组
  - B10 飞书 IM 推迟到 🔴 第四档(触发 daemon 化,重大架构变更)
  - B11 云端同步推迟到 🔴 第四档

  **4 档简表**:
  - 🟢 第一档(立刻做,4 项):A4 / B5 / C1 / D1
  - 🟡 第二档(接着做,7 项):A2+B7 / B3 / C3 / C4 / B2 / D2 / D3
  - 🟠 第三档(缓做,8 项):B6 / B4 / B9 / C2 / C6 / B1 / A5-A6 / A7（注:A7 已于 2026-06-14 解决出档,见 §4 2026-06-14 ADR;此为重排时快照）
  - 🔴 第四档(最远远期,3 项):B8 / B10 / B11
  - 🗑️ 移除(3 项):A1 / A3 / C5

- **依据**:完整决策矩阵 D1-D6 见 [`.trellis/tasks/archive/2026-06/06-10-v2-roadmap-and-vision-consolidation/prd.md`](../../.trellis/tasks/archive/2026-06/06-10-v2-roadmap-and-vision-consolidation/prd.md)。

### 2026-06-07 — 工具集扩展批次(edit_file / grep / glob / list_dir + ReadGuard + Bash 落盘 + cat -n)

- **决策**:`edit_file` 用 claude-code 风格 str_replace_editor + 3 道强制 check(read-before-edit / on-disk freshness / match + uniqueness),失败文案是 plain English(LLM 能自纠)
  - **原因**:`write_file` 整文件覆盖 token 浪费大 + 改错位置不报;claude-code Edit 是 token 经济 + 防护成熟的方案
  - **关键设计**:`ReadGuard` Tauri State,`Mutex<HashMap<SessionId, HashMap<PathBuf, Fingerprint>>>`,session 隔离(切回不重读),edit 写成功后自动 invalidate(逼 LLM 重读)
  - **0 匹配处理**:claude-code 风格直接报错 + 0-3 个最相似行 hint(Jaccard 相似度排序)——**不**自动 strip 空白重试(OpenHands 风格)
- **决策**:`grep` / `glob` / `list_dir` 三个浏览工具跟 edit_file 一起合
  - **grep**:`tokio::process::Command::new("rg")` spawn,3 种 output_mode(files_with_matches / content / count),line cap 500 字符(抄 pi_agent_rust),默认遵守 .gitignore
  - **glob**:`globset` crate,cap 100,按 mtime 倒序,**不**强制 .gitignore(跟 claude-code 一致)
  - **list_dir**:`tokio::fs::read_dir` 字母排序 + 目录加 `/` 后缀,hidden 默认 false(避免 `.git/` 灌爆),非递归(递归归 glob)
- **决策**:`offset/limit` 包含 `old_string` 出现位置就算 read 过(不要求覆盖全文)
  - **原因**:LLM 智能只读相关区段是合法操作,不必要求 LLM 重调 read_file 读全文浪费 token
- **决策**:顺手 2 件在同批次合(read_file 加 `cat -n` 行号 prefix + shell 30K 落盘)
  - **cat -n**:`read_file` 返回每行加 `\t<line_num>\t` 前缀(1-based),截断保留行号;跟 edit_file 报错带行号协同,LLM 拿到内容就能定位"第 42 行"
  - **Bash 落盘**:> 30K 字符写到 `<session_cwd>/.everlasting/outputs/<uuid>.txt`,tool_result 返回 path + 1KB head+tail preview;`delete_session` 调 `cleanup_outputs_dir` best-effort 清理(失败不 cascade)
- **决策**:1 个 `feat(tools):` commit 一次性合(用户拍板)
  - **原因**:4 tool + ReadGuard + Bash 落盘 + cat -n 互相依赖(ReadGuard 跨 edit_file/read_file),分开 commit 反而中间状态编译过不了
- **测试**:77 新 tool test + 3 cleanup_outputs_dir test = 80 新;cargo test 163→166 全过;pnpm build 干净
- **沉淀**:`.trellis/spec/backend/llm-contract.md` 新增 §"Scenario: Tool Set Extension" 段(7 sections code-spec depth,含错误矩阵 + Good/Base/Bad + 24 个必测项 + Wrong/Correct 对照)
- **Out of Scope 守住** (13 条):`hashline_edit` / `MultiEdit` / `LSP` / `WebFetch` / `WebSearch` / damage-control 路径规则 / Bash `cat|head|sed` 等价 read / `replace_all` preview / 前端 tool card 改造 / `read_file` PDF / binary 检测 / `read_many_files` / grep `output_mode=json` —— 全部 0 命中

### 2026-06-07 — streamController 状态架构重构(6 UI/状态 bug 同期修复)

- **决策**:抽 `useStreamControllerStore()` 独立 Pinia store 作为 in-flight SSE 流的**单一来源**,`useChatStore()` 改 thin facade
  - **原因**:旧设计把 messages / `streamingSessionId` / `currentRequestId` / SSE listener 全放 `useChatStore()`,session 切换时会丢 streaming message + 漏 `done` event 处理(red dot + stop button + `sending` 卡死)
  - **新边界**:`streamController` 拥有 per-session message buffer (LRU 20) + activeRequests + 单全局 SSE listener(按 `request_id` 路由,不再按 `currentSessionId` 过滤);`chatStore` 拥有 sessions 列表 + currentSessionId + currentCwd + session CRUD 委托
  - **流指示器分层**:`streamingProjectIds` → AppHeader 红点;`streamingSessionIds` → SessionList 蓝点 1.5s pulse
  - **沉淀**:`.trellis/spec/frontend/state-management.md` 新增 §"Stream Controller Pattern"
  - **测试**:12 个 LRU 单测 + 36 vitest + 103 cargo 全过
  - **commit**:`abde429` + spec `bf9b35b`
- **决策**:顶栏窗口控制补 Tauri 2 capabilities(`set-size` / `set-position` / `outer-size` / `outer-position` / `current-monitor` 等 11 个权限)
  - **原因**:`setSize` 之前静默失败是 Tauri 2 默认 deny(没在 `capabilities/default.json` 声明)
  - **遗留**:position 部分在 RDP 双显示器场景下未完全修好(窗口 grow rightward 而非贴 host 主屏左上角)→ **[2026-06-14 ✅ 解决]**:根因 = Wayland 协议禁止客户端 setPosition(非 Tauri bug,不可绕过),改原生 `toggleMaximize()`,详见 2026-06-14 ADR
  - **commit**:`bd5ea7b`

### 2026-06-06 — 字体栈调整 + `projects.git_branch` 启动 batch backfill

- **决策**:Dark theme 下中文字体栈首位改 HarmonyOS Sans SC,子集打包嵌入(3500 常用字 + ASCII + 标点,woff2 + brotli → 472 KB)
  - **原因**:Noto Sans CJK SC 在 dark theme 下笔画粗细不均,影响阅读
  - **沉淀**:`.trellis/spec/frontend/cjk-fonts.md`(系统字体兜底局限、3500 字覆盖率、Vite+Tauri 资源链路、license 合规三处声明 pattern)
  - **commit**:`aabb9fa` + docs follow-up `d1d51cf` / `adf4ed6`
- **决策**:`projects.git_branch` 用启动时 batch backfill,不再用 PR2 的"打开 project tab 时懒探测"
  - **原因**:老项目(无 git_branch 字段)开了 tab 才能看到分支,首屏体验差;启动 batch 一次扫所有项目,DB 落库
  - **commit**:`7ce3209`

### 2026-06-04 — 路线图重构(步骤 1 完成后审视)

> 📦 **已归档到 [`docs/_archive/2026-06-04-roadmap-restructure.md`](_archive/2026-06-04-roadmap-restructure.md)**。本节历史路线图重构决策(8 步合并 7 步 / 步骤 3 拆 3a+3b / 事件协议混合模式 / SQLite 排期 / 步骤 2 继续手写 reqwest)由 ROADMAP.md V2 重排 ADR 取代,只读不改。

### 2026-06-05 — 步骤 3b-1 follow-up 沉淀 (FU-1/2/3 项目决策)

完整 FU 项(FU-1 cwd `~/` / FU-2 TS interface camelCase / FU-3 pick_project_dir reka-ui 改写)与决策理由沉淀在 [`docs/_archive/2026-06-3b-1/FOLLOW-UP.md`](_archive/2026-06-3b-1/FOLLOW-UP.md);本 ADR 仅留状态索引,FU 内容不重复。
- **FU-3 · `pick_project_dir` 用 reka-ui 渲染 dialog**：Tauri command 不再负责弹原生 dialog，统一改为前端用 reka-ui 的 `Dialog` 组件（后端只暴露 path 校验）。详见 [`docs/_archive/2026-06-3b-1/FOLLOW-UP.md`](_archive/2026-06-3b-1/FOLLOW-UP.md)。

### 2026-06-24 — RULE-D-001 provider api_key 加密存储(P1 安全债收口)

- **决策**:provider api_key 用 AES-256-GCM + HKDF(machine-id) 派生 master key 加密存储(AAD=provider id 绑定,防 DB 内挪用),否决 keyring(WSL 实测无 secret service daemon 开箱不可用)+ stronghold(过度工程)
  - **原因**:三方 research 交叉验证——keyring 在 WSL 主环境实测不可用(gnome-keyring/libsecret/secret-service 全未装;keyutils kernel 后端重启即丢,不适合长期凭证);业界同类(Codex CLI/Claude Code/Aider/Continue)默认明文文件/env var,加密已超主流;应用层加密精准命中"防 DB 文件泄露"威胁模型 + WSL 零摩擦 + 5 直接依赖
  - **依据**:[`.trellis/tasks/archive/2026-06/06-24-p1-api-key-encryption/research/`](../../.trellis/tasks/archive/2026-06/06-24-p1-api-key-encryption/research/) 三份(keyring-wsl-availability / industry-api-key-storage / app-layer-encryption-rust)
  - **后果**:机器绑定固有性质——`wsl --unregister`/重装重置 `/etc/machine-id` 旧密文不可解,靠 `PreFlightError::DecryptFailed` 兜底友好提示重粘(不防本机 root/进程内存,Out of Scope)
- **决策**:前端永不持有明文 api_key —— `ProviderRow.api_key` 加 `#[serde(skip)]` 切断 IPC,`list_providers` 改返 `hasKey` 布尔;Settings 编辑留空覆盖(`None`=保持/`Some`=覆盖)+ 加密状态徽标
  - **原因**:彻底切断前端持明文路径,RULE-D-001 收益最大化;secret 输入业界标准 UX
  - **依据**:[`.trellis/spec/backend/multi-provider-contract.md`](../.trellis/spec/backend/multi-provider-contract.md) ProviderRow wire
- **决策**:加密改动原子合一个大 PR(db migration + 运行时解密 + IPC + 前端),不分拆
  - **原因**:四者强耦合——db 写密文后,运行时解密 + IPC 不返明文 + 前端留空覆盖必须同改,否则中间态双重加密(前端回填密文→再加密)或 chat 用密文发请求
  - **commit**:`576b2f4`(fix)+ `30a5eaf`(docs debt)
- **测试**:crypto roundtrip/empty/tamper/aad_mismatch/unknown_version/distinct_nonces (6) + db api_key_is_encrypted_not_plaintext/plaintext_migration_is_idempotent (2);`cargo test --lib` 822 passed 0 warning;vitest 518 passed;vue-tsc 0 error

### 2026-06-24 — C2 agent loop ⑬ 循环检测(第三档收口)

- **决策**:**分级触发**(L1 精确签名硬触发 `HARD_WINDOW=3` + L2 Jaccard 软提示 `SOFT_WINDOW=5`/`SOFT_THRESHOLD=0.85`),取代架构原文单一 `Jaccard > 0.9`
  - **原因**:调研 [`similarity-algorithm-and-tokenizer.md`](../../.trellis/tasks/06-24-c2-loop-detection/research/similarity-algorithm-and-tokenizer.md) 指出单一阈值无法适配短/长 input —— `read_file` 只 1 个 path token 时 Jaccard 抖到 0.5 漏判,`shell` 长命令改 flag 仍 >0.9 误报;L1 精确签名对最高频死循环(read/grep/shell 同输入)零误报,L2 Jaccard 兜底近重复
- **决策**:命中动作选 **Approach A 两层软提示,无硬打断** —— hint 作为 `ContentBlock::Text` 插到 result message 的 `result_blocks[0]`,LLM 下一轮看到提示;不跳过执行、不终止 loop,MAX_TURNS=200 仍是硬兜底
  - **原因**:符合架构 §2.5.4「不强制打断」原意;无状态机最小侵入 `run_chat_loop`;与 RULE-A-010 cancel「一次即终止」MVP 简化风格一致;Approach B 升级硬打断留 follow-up(若线上观测到「软提示后仍循环」高频再上)
- **决策**:`edit_file` 签名**含 old_string**(非 research caveat #3 说的「不含」)
  - **原因**:research caveat #3 逻辑反向 —— 不含 old_string 恰让正当的同文件多块编辑签名相同→误判 loop;含 old_string 才让同文件不同块编辑保持区分,同时仍抓「反复失败同一 old_string」的真死循环
- **决策**:token 切分用纯 Rust `split_whitespace` + trim 首尾标点,**不复用** `memory::tokens::count_tokens`(tiktoken)
  - **原因**:tiktoken 是 `async` + 持 `tokio::sync::Mutex`(进热路径)+ cl100k_base 切碎 CJK + subword 噪音;Jaccard 粗粒度判定不需要 BPE 精度,word-level 更稳;两套 token 概念物理隔离
- **决策**:不落 AuditKind 表(§2.5.8 已定),只 `tracing::warn!`;前端不新增组件(Q2 决策),hint 仅以 tool_result message 内 Text block 呈现
- **测试**:`loop_detection` 31 单测(detect L1/L2 + signature_of 6 tool + tokenize + jaccard 边界)+ `tests_agent_loop` 2 集成测试(HardLoop hint 注入 turn 4 messages / 非循环不误报);`cargo test --lib` 855 passed 0 warning

### 2026-06-25 — L3c subagent 联网(worker web_fetch,第三档收口)

- **决策**:**最小 MVP** —— 仅改第 1+2 层(researcher `SubagentDef.tools` + `READONLY_TOOL_ALLOWLIST` 各加 `web_fetch`),第 3 层(worker 权限)**零改动**
  - **原因**:基线验证推翻种子 PRD 第 3 层假设。PRD 种子写"worker `is_worker=true` 调 web_fetch → ask_path → 塌缩 Deny",这是 **2026-06-20 PR2b 的旧行为**,已被 **2026-06-22 RULE-FrontSubagent-003 fix** 推翻 —— worker ask 现走 `WorkerAskBanner` round-trip(`ask.rs:124` biased select:parent cancel / 120s timeout / oneshot)。L3a 验证时 worker 报"无 web_fetch"纯粹是第 1+2 层把工具从 toolset 剥掉,worker 根本没机会触发 ask
  - **关键发现 —— "父 session grant 继承"天然已工作**:worker `PermissionContext.session_id` = `parent_session_id`(`dispatch.rs:314` 传 parent_session_id → `chat_loop.rs:411` PermissionContext.session_id),而 `check.rs:257` `check_tool_grant(db, &ctx.session_id, "web_fetch")` 查 `session_tool_permissions` 该 session 的 grant → worker web_fetch **已天然继承父 session grant**(父授权过 web_fetch → 自动 Allow 零 banner;无 grant → 弹 WorkerAskBanner)。第 3 层无需新代码
- **决策**:并发 banner UX 接受现状,不引入 silent allow / worker AllowAlways 持久化
  - **原因**:并发 N worker 各 web_fetch,父 session 无 grant 时弹 N banner(worker AllowAlways 不持久化——`ask.rs:267-273` 有意设计,防跨权限边界)。Workaround:用户预先在主对话对 web_fetch 点"始终允许"让所有 worker 继承。silent allow / 持久化 / 配额各自需独立安全 grill,作为 follow-up
- **决策**:`READONLY_TOOL_ALLOWLIST` 加 web_fetch 不波及 L2 单 turn 并发
  - **原因**:`READONLY_TOOL_ALLOWLIST` 只被 `filter_tools_readonly` 引用(仅 `dispatch.rs:157` force_readonly 并发路径调用);L2 用独立谓词 `is_parallel_eligible`(`chat_loop.rs:1439`),不引用本常量。web_fetch 是只读网络 op、`Risk::Low`、SSRF 已防护,符合"只读并发"语义(无本地副作用,N 个独立 GET 无共享状态竞争)
- **决策**:顺手修正 worker ask 过时描述(L3a 遗留文档债)
  - **范围**:`mod.rs` `dispatch_subagent` ToolDef.description(**LLM-facing**,原"worker has no UI...auto-denied"会让主 agent 不派 worker 做需工具任务,直接损害本 task 的可用性)+ `dispatch.rs:339` 注释 + `tool-contract.md` 第 21 参/is_worker 注释/description block(原"collapse to Deny"与 permission-layer.md §5b 矛盾)。纯文档/注释/prompt,**零行为改动**(行为早已是 WorkerAskBanner)
  - **残留**(未纳入本 PR):`permissions/types.rs:144` is_worker 字段 doc + `tests_subagent.rs:1363/1669` 测试注释同款过时,留作关联文档债后续清理
- **测试**:`mod.rs` 2 处(researcher allowlist + filter 加 web_fetch keep 断言)+ `tests_subagent.rs` `l3a_filter_tools_readonly_keeps_only_four_read_tools` 改名 `_five_` + len 5 + required 加 web_fetch + forbidden 去 web_fetch;`cargo test --lib` **864 passed 0 failed**
- **沉淀**:`.trellis/spec/backend/tool-contract.md`(web_fetch §1 加 subagent 可用性 + 第 21 参/is_worker/description 过时描述修正 + researcher 表格);`app/src-tauri/src/agent/subagent/mod.rs`(researcher tools+prompt+description + READONLY_TOOL_ALLOWLIST + 注释);`dispatch.rs:339` 注释

### 2026-06-26 — L3d subagent frontmatter loader(第三档收口)

- **决策**:砍设计 PRD 的 `/reload-subagents` 命令,改 B3/B4 同款 read-through mtime fence —— .md 改动下次 chat 自动生效(`builtin_tools()` 启动经 `state.tools` 快照,故 dispatch_subagent 拆出,改每 turn `definition_with_cache(&SubagentCache, project_path)` 动态拼 enum + source tag)
- **决策**:`tools` 字段可选 —— 覆盖 builtin 同名且未声明 → 继承 builtin tools;全新 agent 未声明 → `vec![]` 全工具集。`SubagentDef.tools: Vec<String>` 本身区分不了 None/Some,用 `LoadedAgentFile.tools_declared: bool` 侧信道承载(scan→cache→merge 流水线)
- **决策**:`SubagentDef` 全 owned(PR1 纯重构铺路,`name`/`description: String`、`tools: Vec<String>`);`model` 字段 v1 解析但 warn-ignored(`Provider` trait 单实例模型)
- **修订(对设计 PRD)**:R1 user 路径 `~/.config/everlasting/agents/`(非 `~/.everlasting`,跟 B3/B4/B5 一致);R2 复用 **Skill** loader inline-array parser(非 B3 —— B3 scalar-only 不支持数组,设计 PRD §3.3 + deepseek 审查报告都看错文件);R3 删"YAML fail-fast"伪命题(手写 parser 全容错,无 fail-fast 分支)
- **安全教训(PR3 check 发现 BLOCKING 回归)**:防 worker 嵌套靠 `chat_loop.rs` `effective_is_worker` gate(worker 跳过 dispatch_subagent 的 per-turn append),`STRUCTURALLY_DISABLED` filter 只是 defense-in-depth —— filter 只过滤 seed list,不过滤共享 `run_chat_loop` body 的 per-turn append。PR3 初版在共享 body 无 gate 追加 → worker 可嵌套(单测全绿因无人断言 worker turn 的 tools 内容)。Forbidden Pattern:共享 loop body 内 append 动态/禁项 tool 必须用 is_worker gate。`MockProvider` 加 `sent_tools()` 可观测性才能测此不变量
- **测试**:`cargo test --lib` **909 passed 0 failed**(PR1 owned 化适配 + PR2 loader 39 新测试 + PR3 definition_with_cache 4 新 + no-nesting 回归);`vue-tsc --noEmit` 绿
- **沉淀**:`.trellis/spec/backend/tool-contract.md`(dispatch_subagent scenario:no-nesting 机制 callout + Forbidden Pattern + Tool declaration 动态化 + 三层来源 SubagentCache + cache.lookup);`app/src-tauri/src/agent/subagent/loader.rs`(新建);设计 PRD `docs/subagent-loader.md`(本 task 引用 + R1-R3 修订)
