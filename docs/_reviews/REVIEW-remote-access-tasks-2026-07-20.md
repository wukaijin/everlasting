# 远程访问改造 Trellis Task 体系 — 设计评审

> **评审日期**:2026-07-20
> **评审范围**:3 个 Trellis task(parent `07-20-remote-access-multi-channel/` + child 1 `07-20-remote-access-transport-abstraction/` + child 2 `07-20-remote-access-daemon-split/`),均 status=planning
> **评审类型**:设计评审 + 任务体系架构审计(pre-`task.py start` review)
> **评审基线**:commit `74edc71`(2026-07-20),task 目录创建于 2026-07-20 14:49-14:55;`docs/REMOTE-ACCESS-RESEARCH.md` 已吸纳本次 review + DeepSeek 评审甄别后的修正
> **评审模型**:MiniMax-M3
> **对照基准**:`docs/_reviews/REVIEW-remote-access-research-2026-07-20.md`(MiniMax-M3 上一步 review)+ `docs/_reviews/REVIEW-remote-access-research-deepseek-v4-pro.md`(DeepSeek-v4-pro second review)+ `docs/_reviews/review-triage-2026-07-20.md`(两份 review 的独立甄别,内嵌于 parent task 的 `research/`)+ `docs/ARCHITECTURE.md §4/§5`(daemon 化与 Channel Adapter 决策原文)

---

## 0. 总体评价

**综合评分:★★★★ (4/5)——任务体系拆分合理、triage 甄别质量高、对两份 review 的吸收做得扎实;但有 4 处结构性 / 流程配置问题需要在 `task.py start` 前补正,否则 Phase 1 实施时会卡 review gate 或在 Phase 1/2 边界处发生接口漂移。**

这是一份**少见的、高质量的评审吸收**——3 个 task 的 PRD / design.md / implement.md 对上游两份 review(我的 MiniMax-M3 + DeepSeek-v4-pro)做了独立甄别,**不照单全收,逐项核验后采纳/部分采纳/驳回**。这是项目其他 task 处理 review 时从未见过的(triage doc 即便在 `trellis-check` 子流程的标准里也属少见高质量)。

三个**架构优点**值得肯定:

1. **Parent / child 拆分契合路径设计**——parent 持有调研产出 + 跨 child 决策,2 child 各自独立 ship,符合"Phase 1 零风险铺路 + Phase 2 中等风险交付"的双层节奏。Decision D1("单一协议统一")在 parent 显式记录,不被 child 各自重新决定。
2. **Transport-abstraction 三段串行 + 每段独立回滚**——P1.1 / P1.2 / P1.3 各自独立 commit + revert 能力,体现"早期 ship、失败早退"的方法论。回滚点 §"P1.1 完成后 commit: feat(transport): add Transport interface..." 这种**精确到 commit message 的回滚设计**值得学习。
3. **WSL→Windows 转发作为 child 2 的核心验收**——parent D4 + child 2 R11 + ROADMAP P2.5 都强调 WSL→Windows 验证,这是项目 WSL-first 设计的真实落地(DeepSeek §8.6 提的"WSL 浏览器访问链路"被采纳为 Phase 2 第一验证关)。

**但有以下 7 个问题需要在 task `start` 前补正或确认**:

| 级别 | 问题 | 性质 | 影响 |
|---|---|---|---|
| **P0-A** | child 2 `daemon-split` 只有**骨架 PRD**(行 5 显式说"待 Phase 1 完成后 brainstorm"),无 design.md / implement.md。trellis 工作流"复杂任务必须 PRD + design + implement 三件套"在 `task.py start` 会卡 review gate | 流程结构性 | start 卡壳,或需要手动跳过 gate |
| **P0-B** | parent PRD D1"**所有 client 统一走 HTTP/SSE,Tauri `#[tauri::command]` IPC 入口废弃为死代码**"是 ARCHITECTURE 级强决策(否决了 in-process fallback),需要 Carlos 显式 ack + 同步到 `ARCHITECTURE.md §4` 目标态图 | 决策透明度 | 后续要反对向 in-process fallback 时付出双倍代价 |
| **P0-C** | 3 个 task 的 `implement.jsonl` / `check.jsonl` 都是默认 `_example` 占位(254 字节,1 行模板),**实际为空**。sub-agent dispatch 时无 spec/research 上下文 | 工作流配置 | 派发 `trellis-implement` / `trellis-check` 时无 `.trellis/spec/` 指引 |
| **P1-A** | 所有 3 个 task 优先级全标 `P2`,但 transport-abstraction **承接 MiniMax P0-D(emit 散点 6 处)+ P1-A(`apply_ui_diff` 误读)**——P0/P1 findings 的紧迫性被低估 | 优先级标定 | 队列排序错排;按项目其他 P0 债做法应升 P1 |
| **P1-B** | transport-abstraction design.md §3.3 显式承诺"Phase 2 用途:新增 `HttpSseSubagentSink` 实现同一 trait,subagent 路径零改动(只换 sink 注入)",但 child 2 daemon-split PRD 的 R1-R15 **完全没有提到 `HttpSseSubagentSink` 类型** | 设计漂移 | Phase 2 实施者可能用不同拆分(如把 subagent 事件合到 `HttpSseSink`)导致 Phase 1 接口反向不兼容 |
| **P1-C** | daemon-split R6 SSE 部分"**mpsc bounded buffer(64-256,实施时实测)**反压;定期 `: ping` 心跳"——lost segment 策略(每事件 `id:` + `Last-Event-ID` 重发)、客户端在线状态检测(30s ping + 60s 超时) 都没写 | 模糊点 | Phase 2 上线后才暴露,断网重连漏事件 |
| **P2-A** | parent PRD Acceptance Criteria 含"ARCHITECTURE §4 触发条件更新为'真浏览器远程访问诉求确定实施时'" + "ROADMAP B10 拆为 B10a/B10b/B10c"两项**跨文档同步工作**——没有显式 owner / 时机;且 Child 2 是 park 状态时这两项可能与 Phase 2 实施不同步 | 跨文档一致性 | 文档同步被遗忘 |

下面对每个发现做事实核验(所有 P0/P1 断言均通过文件 Read 二次核验)。

---

## 1. P0-A:child 2 `daemon-split` 缺 design.md / implement.md(trellis 流程会卡 review gate)

### 1.1 现状核验

```bash
$ ls /usr/local/code/github/everlasting/.trellis/tasks/07-20-remote-access-daemon-split/
check.jsonl       (254B, _example 占位)
implement.jsonl   (254B, _example 占位)
prd.md            (6.1k, 骨架 PRD)
task.json         (616B, status=planning)
```

**没有 design.md 和 implement.md**。

child 2 prd.md 行 5 显式声明:

> **状态:骨架 PRD。Phase 2 细节(完整 design.md / implement.md)待 Phase 1 完成、协议方向经 dogfooding 验证后,再进 brainstorm 补齐。**

### 1.2 工作流的影响

trellis 工作流(参见 `.trellis/workflow.md` 与 `get_context.py --mode phase --step 1.1` 引导):

> 复杂任务必须 PRD + design.md + implement.md 三件套,`task.py start` 前 review gate 会检查三件套存在。

parent PRD D1 又说"parent 本身不直接实施代码",所以三件套缺失的责任落在 child 2 头上。但 child 2 的设计/实施文档是有意延后(§1.3 分析)。

### 1.3 这是有意还是疏漏?

**有意**(很可能):
- child 2 prd.md 行 5 + parent prd.md 行 11-14 都明示"先 PRD 立项,Phase 1 完成后补 design / implement"
- 逻辑上:daemon 化取决于 Phase 1 transport 抽象的实际产出,sink trait 名字、事件 schema、HttpSseSink 实现细节等都需要 Phase 1 真实落地后才有依据
- 提前写 design 容易被 Phase 1 实际产出反向打脸

**但有矛盾**:
- parent PRD Acceptance Criteria 第 2 条"Child 2 完成并 archive(本机浏览器可访问 + 79 command 行为一致 + WSL→Windows 验证通过)"——无法在 `task.py start` 时验证 child 2 是否会达到该标准,因为**没有 design / implement 对照基线**
- 一旦 Phase 1 实际产出与 child 2 PRD 假设不符,child 2 需要重做 → Phase 2 启动延期

### 1.4 建议

**选项 A(推荐)**:`task.json.dev_type` 标 `"lightweight"` 或 `"scaffold"`,让 trellis 工作流识别"先 PRD 立项,Phase 1 完成后 rewrite"的合法状态——这是 child 2 现状的事实陈述。

**选项 B**:在 child 2 任务目录同时写一个 `design.md.staging.md`,**明确标注"Phase 2 design 雏形,待 Phase 1 完成后 rewrite,Phase 2 start 前替换为本文件"**——给 trellis 工作流一个占位文件以过 review gate,同时保持"会重写"的语义。

**选项 C**:把 child 2 标 `status: parked`(或类似 trellis 支持的状态),直到 transport-abstraction archive 后再正式 activate。

**推荐 A** + 一个明确的 "Phase 1 archive + 协议 dogfooding ≥ 1 周 + then brainstorm child 2 design.md" gate 句写在 child 2 prd.md 顶。这是**最简洁、最对得起父任务决策的方案**。

---

## 2. P0-B:parent PRD D1"单一协议统一"是 ARCHITECTURE 级强决策,需 Carlos 显式 ack

### 2.1 D1 全文(parent prd.md 行 30-41)

> **决策**:Phase 2 完成后,**所有 client(Tauri GUI / 浏览器 / 未来的 Electron)统一走 HTTP/SSE 连 daemon**;Tauri 内嵌的 `#[tauri::command]` IPC 入口废弃为死代码。
>
> **否决方案**:后端 Facade(service 层 + Tauri/HTTP 双 adapter 长期并存,GUI 保留 in-process 离线 fallback)。
>
> **理由**:
> - 79 个 command 维护两套入口的协议 drift 成本太高(测试翻倍、type sync 复杂)
> - 单用户场景下 HTTP 一跳延迟可忽略(localhost sub-ms)
> - opencode 已验证"只有 HTTP server"可行
>
> **代价**:daemon 挂了 GUI 就废,无 in-process fallback。**接受这个代价**——daemon 是单进程 + systemd/launchd 自动重启,可靠性足够;若 dogfooding 发现"daemon 频繁挂"再回来补 in-process fallback(届时 Phase 2 已有 service 层雏形,补成本可控)。

### 2.2 强决策的影响范围

| 影响维度 | 现状 | D1 后的状态 |
|---|---|---|
| `#[tauri::command]` 入口 | 79 个,活代码 | **dead code**(仍编译,但无 caller) |
| Tauri GUI 离线能力 | 启动失败 → daemon fallback?有 | 启动失败 = GUI 不可用 |
| 开发体验 | `pnpm tauri dev` 启动失败可单独排查 | daemon 没起 = `pnpm tauri dev` 跑不起来 |
| 反悔成本 | N/A | 需重写 transport 层(die 已经封装在 `AppHandleSink` 还要恢复) |

### 2.3 MiniMax 评估

D1 的**方向**与 `REVIEW-remote-access-research-2026-07-20.md` §6.2 路线图 Phase 2 后的"GUI 也走 daemon"建议一致。
D1 的**否决理由**("drift 成本高 + localhost sub-ms")合理,与 opencode 模式对得上。
D1 的**代价评估**("daemon 挂了 GUI 就废")与 review 的 P1-C(dual-pool 写竞争)、§6.2 "立即可做第一步"(锁定 Tauri 版始终可用)对齐。

**但 D1 是 ARCHITECTURE 级决策,不是单一阶段的过渡决策**——它的"废弃 79 个 command"是**永久性方向选定**,不是"先做、错了再回来"。这种决策需要:

1. **ARCHITECTURE.md §4 同步更新**:从"目标态图示"拓展为"目标态实施路线",否则 ARCHITECTURE.md 与 parent PRD 互相矛盾
2. **决策记录**:在 IMPLEMENTATION.md §4 ADR 或 `docs/REMOTE-ACCESS-RESEARCH.md` §6.3 内加"Carlos ack 日期"
3. **回退路径显式记录**:D1 末段"若 dogfooding 发现 daemon 频繁挂再回来补 in-process fallback"——这条回退路径需要前置架构条件,即"Phase 2 实施时 service 层雏形"需要在 child 2 PRD 内显式作为回退路径的孵化器(目前没有)

### 2.4 建议

- 在 `ARCHITECTURE.md §4` 找到 daemon 化触发条件章节,**新增段"目标态实施路线"**,引用 parent PRD D1 作为权威决策,并说明"79 个 `#[tauri::command]` 入口在 Phase 2 archive 时转为 dead code"
- parent PRD D1 段后加一行 "**Carlos 决策日期:2026-07-20**(research-triage 已采纳)"
- 在 child 2 PRD 的"## Open Questions" 加 **Q0**:"Phase 2 实施时是否同步孵化 in-process fallback 的 service 层雏形?(D1 回退路径的前置条件)"
- review-triage doc 把 D1 标"已采纳"+ 链接到 ARCHITECTURE 对应更新位置,形成闭环

---

## 3. P0-C:3 个 task 的 `implement.jsonl` / `check.jsonl` 是空 stub(sub-agent dispatch 无 spec 上下文)

### 3.1 现状核验

```bash
$ cat 07-20-remote-access-multi-channel/implement.jsonl
{"_example": "Fill with {\"file\": \"<path>\", \"reason\": \"<why>\"}. Put spec/research files only — no code paths. Run `python3 .trellis/scripts/get_context.py --mode packages` to list available specs. Delete this line once real entries are added."}

$ cat 07-20-remote-access-transport-abstraction/check.jsonl
{"_example": "Fill with {"file": ... "}. ... Delete this line once real entries are added."}

$ cat 07-20-remote-access-daemon-split/implement.jsonl
{"_example": "Fill with {"file": ... "}. ... Delete this line once real entries are added."}

$ cat 07-20-remote-access-daemon-split/check.jsonl
{"_example": "Fill with {"file": ... "}. ... Delete this line once real entries are added."}
```

### 3.2 影响

按 trellis 工作流,`trellis-implement` / `trellis-check` sub-agent dispatch 时会读 jsonl 取 spec/research 文件清单作为 system prompt 上下文。**空 jsonl = sub-agent 只看 prd.md / design.md / implement.md,不会自动加载 `.trellis/spec/` 下的 guideline,也没有 research 上下文**。

对照 archived 已完成任务的填充规范(`07-03-subagent-per-agent-model-ui/implement.jsonl`)——应至少 3-5 条 `{file, reason}` entries。

```bash
$ cat 07-03-subagent-per-agent-model-ui/implement.jsonl
{"file": ".trellis/spec/backend/subagent-runs-schema.md", "reason": "阶段 1b 加 model_display 列的权威 schema..."}
{"file": ".trellis/spec/backend/agent-loop-architecture.md", "reason": "run_subagent / resolve_worker_provider / dispatch_subagent 链路..."}
{"file": ".trellis/spec/backend/database-guidelines.md", "reason": "阶段 0 新表 subagent_model_overrides + CREATE TABLE IF NOT EXISTS 幂等惯例..."}
```

### 3.3 建议(child 1 立即可填,child 2 待 design 补完后填)

**child 1 transport-abstraction `implement.jsonl`**(建议 5 条):

```jsonl
{"file": ".trellis/spec/backend/event-sink-trait.md", "reason": "P1.3 emit 散点收敛参考 ChatEventSink trait 抽象模式 + SubagentEventSink 设计对照(若 spec 不存在,本任务须沉淀一份)"}
{"file": ".trellis/spec/frontend/transport-pattern.md", "reason": "若 Phase 1 沉淀的 pattern 写入此 spec(见 implement.md 末尾 §Phase 3.3 spec update)"}
{"file": "docs/REMOTE-ACCESS-ROADMAP.md", "reason": "P1.1-P1.3 实施步骤与 ROADMAP Phase 1 段落对拍一致"}
{"file": "docs/_reviews/REVIEW-remote-access-research-2026-07-20.md", "reason": "MiniMax P0-D/P1-A 的采纳点确认"}
{"file": ".trellis/spec/backend/agent-loop-architecture.md", "reason": "agent::chat pre-flight error path 入口定位 + sink 接入点参考"}
```

**child 1 transport-abstraction `check.jsonl`**(建议 4 条):

```jsonl
{"file": ".trellis/spec/backend/event-sink-trait.md", "reason": "校验 SubagentEventSink trait 抽象后 subagent collector 双通道语义未破坏"}
{"file": "docs/REMOTE-ACCESS-ROADMAP.md", "reason": "校验 P1.1-P1.3 验收清单与 ROADMAP Phase 1 标准对拍"}
{"file": ".trellis/spec/backend/test-model-contract.md", "reason": "测试惯例:vitest run / cargo test / cargo clippy 的标准执行模式"}
{"file": ".trellis/spec/frontend/vitest-mock-pattern.md", "reason": "vi.mock 写法与 22 个测试文件的 mock 改造一致性(若不存在,看已有 .test.ts 文件惯例)"}
```

**parent task** — implement.jsonl 不必填(parent 本身不直接实施)
**child 2 daemon-split** — implement.jsonl / check.jsonl 待 design.md 补完后填(可在本任务 P0-A 决策后)

---

## 4. P1-A:task 全标 P2,但承接 MiniMax P0/P1 findings 紧迫债

### 4.1 现状

所有 3 个 task 的 `task.json.priority` = `"P2"`(P2 = Project default,见 `task.py:7` 的 `--priority P0|P1|P2|P3` 参数定义)。

### 4.2 P2 vs P0/P1 语义区分

- **task `priority`** = 项目调度优先级(trellis 队列排序)
- **review `P0/P1/P2`** = findings 严重度(P0 = 事实错误,P1 = 遗漏,P2 = 改进建议)

两者不同但有相关性:review 的 P0/P1 通常应该承接为 task 的 P0/P1 优先级(除非多方权衡后刻意延后)。

### 4.3 transport-abstraction 承接 P0 findings 的紧迫性

| 承接的 finding | 严重度 | 任务的紧迫性 |
|---|---|---|
| MiniMax P0-D(emit 散点 6 处需收敛) | P0 事实错误 | **高** — 不处理,Phase 2 换 transport 时 6 处会原样变 6 类直调 SSE,drift 风险 |
| MiniMax P1-A(`apply_ui_diff` 误读需纠正) | P1 遗漏 | 中 — 影响 protocol 化设计 |
| MiniMax P1-B(SSE backpressure 风险) | P1 遗漏 | 中 — 已采纳部分 |
| MiniMax P1-D(`listen` 语义错位) | P1 设计 | 中 — 已采纳方案 B |

按项目一贯做法(`REVIEW-agent-loop-full-audit-2026-06-14` 的 shell env_clear / process_group 是 P0 债,后被 RULE-E-001/002 立项),**P0 债通常升 P1 优先级**(因为是已知的技术债,优先级高于未排期的功能)。

### 4.4 建议

- **transport-abstraction**:升 `priority: P1`(承接 P0 债但范围明确可控,估时 ~2-3 天)
- **daemon-split**:维持 `P2`(执行时间 ≥ 2 周 + 0.5 E2E,依赖 Phase 1 完成才能启动,不是 P0 级)
- **parent**:维持 `P2`(调度协调,非实施)

如果认定 emit 散点是 P0 安全债(从"RCE"维度),可进一步升 P0(但 emit 散点本身不直接造成 RCE,主要是 protocol drift 风险,**升 P1 更合理**)。

---

## 5. P1-B:design.md §3.3 承诺的 `HttpSseSubagentSink` 没在 child 2 PRD 出现(设计漂移)

### 5.1 transport-abstraction design.md §3.3 原文

> **Phase 2 用途**:daemon 化时,新增 `HttpSseSubagentSink` 实现同一 trait,subagent 路径零改动(只换 sink 注入)。

transport-abstraction design.md 给出了**具体的 trait 名字 + 实现类型 + 行为契约**——这是 Phase 1 对 Phase 2 的**对外承诺**。Phase 2 实施时,这个 trait 必须按承诺的形状实现,否则 Phase 1 已交付的 sink 注入路径会与 Phase 2 daemon 实际期望不匹配。

### 5.2 child 2 daemon-split PRD 检索

```bash
$ grep -i "HttpSseSubagentSink\|SubagentEventSink" \
    .trellis/tasks/07-20-remote-access-daemon-split/prd.md
# 无结果
```

daemon-split prd.md 的 R1-R15 完全没有提到这两个类型。意味着 Phase 2 实施时,**subagent event 走 SSE 的具体路径没有任何上下游对齐**——这有"实施时定"的风险,届时 Phase 1 已交付、`HttpSseSubagentSink` 名字已固化,但 daemon 实施者可能用不同的拆分(如把 subagent 事件合到 `HttpSseSink: ChatEventSink`),导致**接口反向不兼容**。

### 5.3 可能的对立方案

| 方案 | Phase 1 承诺 | Phase 2 可选 |
|---|---|---|
| **A** | `SubagentEventSink` 是独立 trait,Phase 2 新建 `HttpSseSubagentSink` 实现 | 与承诺一致 ✅ |
| **B** | subagent 事件合到 `HttpSseSink: ChatEventSink`(扩展 ChatEventSink 加 subagent 方法) | 与承诺不一致 — Phase 1 抽象白做 |
| **C** | 完全去掉 sink trait,Phase 2 直接在 subagent 路径里判 `cfg(daemon)` | Phase 1 抽象白做 |

任何 Phase 2 实施方案与 Phase 1 承诺不一致,都会**反向破坏 Phase 1 已交付的 6 处散点收敛成果**——它们之所以收敛到 `SubagentEventSink` trait,就是为了 Phase 2 能 swap 实现。

### 5.4 建议

在 child 2 prd.md 的"## Dependencies" 或 "## Open Questions" 加显式说明:

> **承接 transport-abstraction 抽象契约**(详见 `.trellis/tasks/07-20-remote-access-transport-abstraction/design.md` §3.3):
> - `SubagentEventSink` trait 已有 Phase 1 抽象,Phase 2 必须新增 `HttpSseSubagentSink: SubagentEventSink` 实现,**不**合并到 `HttpSseSink: ChatEventSink`
> - `AppHandleSubagentSink: SubagentEventSink` 与父 agent loop 的 `AppHandleSink: ChatEventSink` 并列,各自独立注入
>
> **Open Question Q0**:`SubagentEventSink` 与 `ChatEventSink` 是否在 trait 边界层有共同父 trait?(实施期可考虑)

---

## 6. P1-C:daemon-split R6 SSE 部分缺 lost segment 策略 + 客户端在线检测

### 6.1 现状(daemon-split prd.md R6)

> **R6** SSE endpoint `GET /api/v1/stream/{session_id}`:`HttpSseSink` 实现,emit → SSE 推送;`session_id → Vec<SseSender>` 路由表;**mpsc bounded buffer(64-256,实施时实测)**反压;定期 `: ping` 心跳。

### 6.2 应明确但漏了的点

**a) Lost segment 策略**

- 客户端断网(EventSource 自动重连)后,daemon 不知道 client 错过哪些事件
- 常见做法:server 给每条事件加 `id: <uuid>` 字段,client 端 EventSource 通过 `Last-Event-ID` header 带回最近收到的 id;daemon 据此从保留 buffer 重发
- 现行 R6 完全未提
- MiniMax review §P1-B "SSE backpressure 风险"只提了 buffer 膨胀,没提断网重连

**b) 客户端在线状态检测**

- 单 TCP 连接断开时,daemon 多久发现?通常靠 TCP keepalive(分钟级 vs SSE 期望秒级)
- 反过来,客户端多久发现 daemon 停了?通常靠 SSE `event: ping` 心跳
- 现行 R6 提了"定期 `: ping` 心跳",但没明确**ping 间隔 + 超时定义**

**c) 多 client 订阅同一 session 时的扇出策略**

- R6 "session_id → Vec<SseSender>"已涵盖 ✅ —— 这一点处理得当

**d) 大 message backpressure 阈值**

- R6 写了 mpsc 64-256 调优 ✅
- 但 MiniMax review §6 提到的"5MB `tool_result` 走 SSE chunked 不截断"应在 R6 验收里出现(目前只在 R14 E2E harness 里 ✅)

### 6.3 建议

R6 补 3 句:

> **R6 补-1**:每条 SSE 事件带 `id: <uuid>` 字段;客户端 EventSource 重连时通过 `Last-Event-ID` 自动重发错过事件。daemon 端为每 session 维护 `VecDeque<{id, event}>` 最近 N=1024 条环形 buffer(具体数字 P2.5 期间按 token 速率调优)。
>
> **R6 补-2**:daemon 端 30 秒间隔发 `:\n\n`(SSE comment frame 作为 ping)+ 每条 SSE 连接独立 timeout(60 秒无 ping 响应则关闭);client 端 60 秒未收到任何 frame 触发 reconnect。
>
> **R6 补-3**:5MB `tool_result` 在 SSE chunked transfer 下不截断(单条 message 上限 8MB,超过则按 hunk 拆分;具体阈值 R6 实施时确定)。

---

## 7. P2-A:跨文档同步工作未明确 owner / 时机

### 7.1 parent PRD Acceptance Criteria 第 5、6 条

```
- [ ] [ARCHITECTURE.md §4] 触发条件更新为"真浏览器远程访问诉求确定实施时"
- [ ] [ROADMAP B10] 拆为 B10a/B10b(近期)+ B10c(远期),档位调整
```

这是两份跨文档同步工作,但:

- 没有显式 owner(Carlos 自己、child 1 owner、还是 child 2 owner?)
- 没有显式时机(Phase 1 完成时?Phase 2 完成时?Archive 时?)
- 与 child 2 的 park 状态不协调 — 如果 child 2 在 Phase 2 才补 design,那 B10c 远期部分可能也在 Phase 2 才动

### 7.2 建议

parent PRD Acceptance Criteria 第 5、6 条按以下方式修订:

```
- [ ] **[Phase 1 archive 前]** Carlos:CARLOS 更新 ARCHITECTURE.md §4 触发条件为"真浏览器远程访问诉求确定实施时";ROADMAP B10 拆为 B10a/B10b,Phase 1/2 进入第二/三档;(owner: Carlos;触发: child 1 archive 时)
- [ ] **[Phase 3 远期启动时]** Carlos:更新 ROADMAP B10c(认证 + 跨设备远程)进入第四档,触发条件沿用现有"BACKLOG §6 飞书或跨设备远期项实施时";(owner: Carlos;触发: 远期任务开始时)
```

---

## 8. 与上游两份 review 的对齐情况

### 8.1 已被采纳且处理得当

| 上游 finding | task 中体现 | 评价 |
|---|---|---|
| **MiniMax P0-A**(57→80) | parent R4 "**79 个 command**" | ✅ 实测 79 行(`lib.rs:155-333`),采纳精准 |
| **MiniMax P0-B**(9→10) | parent R5 "**10 类 SSE 事件**" | ✅ |
| **MiniMax P0-C**(22→21) | transport-abstraction R2 "**21 个非测试文件**" + R3 "**22 个 *.test.ts**" | ✅ 数字全对(triage 独立核验后确认) |
| **MiniMax P0-D**(散点 6 处) | transport-abstraction R6 **枚举 6 处路径** + design.md §3.1 图示 + implement.md P1.3.1-5 checklist + SubagentEventSink trait 抽象 | ✅ **最关键发现被结构化吸收** |
| **MiniMax P1-A**(`apply_ui_diff` 误读) | parent prd.md 没显式提,但 transport-abstraction R7 "subagent 路径**不能简单合并**到 AppHandleSink" 暗示吸取了"严格走 trait 不魔改"的教训 | ⚠️ 隐含,未显式 — 可在 child 1 prd.md 加一句"apply_ui_diff 当前 Ok 路径已结构化,协议化时无需改签名" |
| **MiniMax P1-B**(SSE backpressure) | daemon-split R6 mpsc bounded + 心跳,triage 标"不照搬 Vercel 20ms tick"(P1-B 部分驳回) | ✅ 采纳 + 部分驳回合理 |
| **MiniMax P1-C**(dual-pool) | daemon-split R9 "**GUI 不开本地 SqlitePool**" + parent R6 跨 child 要求 | ✅ 比 MiniMax 更彻底(直接禁,不是 readonly) |
| **MiniMax P1-D**(listen 语义) | transport-abstraction design.md §1.1 注释 "Tauri 全局广播,SSE 按 session 订阅 — 错位在 httpTransport 内部消化"+ 方案 B 单 EventSource + 分发表 | ⚠️ 选了方案 B(MiniMax 方案 A "subscribe(sessionId, ...)" 被驳),有理由但未深入论证 |
| **MiniMax P2-A**(触发条件拆双条款) | triage doc 显式驳回(过度工程化) | ✅ 驳回合理 |
| **MiniMax P2-B**(E2E harness) | daemon-split R12-R15 + 工作量估"2-3 周 + 0.5 周 E2E" | ✅ 已上调 |
| **MiniMax 边角 11.1**(`pick_project_dir` 浏览器等价物) | daemon-split R10 "**手动输入项目路径文本框**" | ✅ 采纳,纠正了 doc 的 `webkitdirectory` 错误等价物 |
| **MiniMax 边角 11.2**(URL snake_case vs camelCase) | 不强制 camelCase,沿用 snake_case(transport-abstraction design.md §1.1 + daemon-split R4) | ✅ 驳回合理 |
| **MiniMax 边角 11.3**(daemon 启动 SQLite 检测) | triage 标"驳回,WAL 模式天然支持多进程"(实际措辞不严谨,见 §8.4)+ R9 已禁 GUI 开 db pool | ⚠️ 见 §8.4 详评 |
| **DeepSeek §8.6 WSL→Windows 浏览器** | parent D4 + child 2 R11 + ROADMAP P2.5 | ✅ **出色采纳**,项目 WSL-first 设计的真实落地 |
| **DeepSeek §8.4 oneshot → RPC** | child 2 R7 "axum handler 通过 `Extension<Arc<AppState>>`" | ✅ |
| **DeepSeek §8.2 GUI 不开 db** | child 2 R9 | ✅ |
| **DeepSeek 风险 10 单二进制部署** | child 2 R3 "内嵌静态文件 server (`tower-http::services::ServeDir`) 指向 `app/dist/`" | ✅ |
| **DeepSeek §8.1 测试策略完全缺失** | child 2 R12-R15 详尽 + parent Acceptance Criteria 含 E2E 验证 | ✅ |

### 8.2 triage 拒绝项的合理/不合理评估

| 上游 finding | triage 结论 | MiniMax 立场 | 谁对 |
|---|---|---|---|
| **MiniMax P0-C listen=8** | "实际真实 listen 只在 4 文件"(注:triage 也确认 grep regex 误把 utils 注释当成 listen 调用) | 我承认 grep regex 不严谨 — 但 transport-abstraction design.md §2.2 表格里的 "streamController ✓(6 个) + permissions ✓(1) + projects ✓(1) + subagentRuns ✓(2) = **10 listen 调用在 4 文件**" 是对的 | **triage 对** + 我的 design.md 表格也对了(grep regex 需要更严格) |
| **MiniMax P0-D "subagent 故意绕过 sink"** | "triage:作者有意,不是疏漏;结论对、原因揣测不准" | 我原文确实给两种可能("可能是有意" + "也可能是早期忘了迁移");triage 已确认是**有意的设计** | **triage 对**(动机层面);**结论也采纳**(P1.3 处理散点) |
| **MiniMax P2-A 触发条件双条款** | "过度工程化" | 我认为重要架构决策需要精准触发条件 | **triage 对**(单条款 OR 三个远期项即可) |
| **MiniMax 边角 11.3 SQLite WAL 检测** | "WAL 模式天然支持多进程" + 与 P1-C 重复 | 我坚持 WAL 不天然支持并发写(只支持多读 + 单写) | **triage 措辞不严谨(WAL 不是天然多进程写)**,**但结论对**(GUI 不开 db pool 就压根不竞争) |
| **DeepSeek §1.2 "教科书级 IPC 盘点"** | "误判:接受了 57/9/22 错误数字" | 完全对 — 基于错误前提的"优秀"评价打折扣 | **triage 对** |
| **DeepSeek §10.1-2 command_palette 重复注册** | "误读:实测 `command_palette::list_commands` 与 `panel::list_subagents` 是完全不同函数" | 完全对 — 我核验过 lib.rs 也是 80 个 unique 注册 | **triage 对** |

### 8.3 三处"隐含未显式"

虽然不是 P0/P1 问题,但三处上游评审的具体建议在 task 里**隐含但未显式记录**:

1. **MiniMax P1-A**(`apply_ui_diff` 改 `kind` enum 而不是改函数签名)— transport-abstraction PRD 没显式提,应加一句
2. **MiniMax P1-D** listen 选方案 B(单 EventSource + 分发表)— design.md §1.1 注释提了,但**没论证为什么否决方案 A**(MiniMax 提议的 subscribe(sessionId, ...))
3. **MiniMax 边角 11.3** SQLite WAL 措辞不严谨 — triage doc 没在"驳回"段独立标注"triage 文字瑕疵但结论对",未来追溯时容易引起混淆

### 8.4 triage 措辞瑕疵:WAL 模式"天然支持多进程"实际是简化版

triage doc 第 56 行:

> **过度工程化**,单条款够用...

实际不是这一行,而是第 56 行附近的某条:

> MiniMax 驳回 / 边角 11.3 / daemon 启动 SQLite 文件持有检测 / 与 P1-C/DeepSeek §8.2 重复,**且 WAL 模式天然支持多进程**,方向错

SQLite **WAL 模式允许多个 reader + 单个 writer**——不是"天然支持多进程写"。多个进程同时写需要应用层排队(`SQLITE_BUSY` + `busy_timeout`)。

但**因为 child 2 R9 已要求 GUI 全走 RPC 不开本地 db pool,该错话不影响结论**——daemon 是唯一 writer,GUI 不写。这条**不影响任务正确性,只是措辞不严谨**。

### 8.5 建议补的显式标注

| 隐含项 | 建议位置 | 建议内容 |
|---|---|---|
| P1-A `apply_ui_diff` | transport-abstraction prd.md 末尾 Notes 加一句 | "apply_ui_diff 当前 Ok 路径已结构化(8 个错误路径全 `Ok({ok:false, kind, error})`),协议化时**无需改函数签名**(`Result<_, String>` 实际不走 Err);`kind: String` 升 enum 可作为可选改进,不阻塞 Phase 1" |
| P1-D 方案 B 论证 | transport-abstraction design.md §1.1 注释补一段 | "**为何不选方案 A**(subscribe(sessionId, ...))?方案 A 会破坏 streamController 当前按 event name 订阅、按 requestId 在 store 内部分发的两层逻辑——Tauri 端按事件名订阅是天然的,强迫它按 sessionId 订阅会要求 streamController 拆为 per-session 实例,改动面更大。方案 B 在 httpTransport 内部消化错位,Tauri 端零改动" |
| WAL 措辞 | review-triage doc 驳回段补一句 | "**注**:此处的'天然支持多进程'是简略说法,WAL 实际允许多读 + 单写;但因 R9 已要求 GUI 不开 db pool,daemon 是唯一 writer,该简化不影响结论" |

---

## 9. 整体可执行性评估

| 维度 | 评分 | 备注 |
|---|---|---|
| Parent / child 拆分合理性 | ★★★★★ | 2 child 边界清晰、各自独立实施 + 验证 |
| Child 1 PRD/Design/Implement 三件套完整度 | ★★★★½ | 完整 + 详细,缺 `implement.jsonl` 填充 |
| Child 2 PRD/Design/Implement 三件套完整度 | ★★★ | 骨架 PRD 符合"延后设计"逻辑,但没标 lightweight → 流程会卡 |
| review 甄别质量 | ★★★★★ | triage doc 是项目少见的高质量甄别,采纳/驳回均独立核验 |
| 与 MiniMax review 对齐 | ★★★★½ | 12 条 findings 中 11 条恰当处理(1 处隐含);P1-A 未显式记录 |
| 与 DeepSeek review 对齐 | ★★★★★ | WSL 转发 / oneshot RPC / 单二进制部署 / GUI 全 RPC 四项重大补充全部落地 |
| 流程配置(`implement.jsonl` / `check.jsonl`) | ★★ | 全部空 stub,sub-agent dispatch 时无 spec 上下文 |
| 优先级标定 | ★★★ | 全 P2 与 P0/P1 findings 不匹配 |
| 决策透明度(D1 与 ARCHITECTURE §4) | ★★★ | D1 是 ARCHITECTURE 级决策但未对应 §4 / 未 ack 记录 |
| 跨文档同步(Parent Acceptance Criteria 5、6) | ★★★ | 未明确 owner / 时机 |
| Phase 1 ↔ Phase 2 接口契约(HTTP SSE sink chain) | ★★★½ | `HttpSseSubagentSink` 跨 phase 接口未在 child 2 显式承接 |
| SSE 协议完备性 | ★★★½ | 反压 + 心跳已采纳,缺 lost segment / 客户端超时 |

**整体:★★★★ (4/5)**——任务体系本身设计扎实,triage 甄别质量尤其值得称赞。4 处 P0 + 3 处 P1 补正后可正常进入 `task.py start`。

---

## 10. 行动清单(按优先级)

### P0 — `task.py start` 前必须补正(影响流程能否启动)

- [ ] **P0-A.1 child 2 task.json `dev_type` 标 `"lightweight"` 或新建 `status: parked`**(选项 A,推荐)
- [ ] **P0-A.2 child 2 prd.md 顶部加"Phase 1 archive + 协议 dogfooding ≥ 1 周 then brainstorm child 2 design.md"gate 句**
- [ ] **P0-B.1 parent PRD D1 段后加"Carlos 决策日期:2026-07-20"**
- [ ] **P0-B.2 ARCHITECTURE.md §4 加"目标态实施路线"段,引用 parent D1 作为权威决策**
- [ ] **P0-B.3 child 2 prd.md "## Open Questions" 加 Q0 关于"in-process fallback service 层孵化"**
- [ ] **P0-C.1 child 1 transport-abstraction `implement.jsonl` 填充 5 条 spec entry(见 §3.3 建议清单)**
- [ ] **P0-C.2 child 1 `check.jsonl` 填充 4 条 spec entry**
- [ ] **P0-C.3 parent task 不填 implement.jsonl(parent 不直接实施)**

### P1 — 实施前强烈建议补正(影响 Phase 1/2 边界 + 协议完备性)

- [ ] **P1-A.1 transport-abstraction 升 `priority: P1`**(承接 MiniMax P0-D)
- [ ] **P1-B.1 child 2 prd.md "## Dependencies" 段加 transport-abstraction 接口契约承接(含 `HttpSseSubagentSink`)**
- [ ] **P1-C.1 child 2 R6 补 3 句(lost segment 重发 + ping 心跳超时 + 大 message 阈值)**
- [ ] **P1-A.2 或保留 P2 但写明"承接 §XX P0 债"避免错排**
- [ ] **P1 在 transport-abstraction design.md §1.1 加"为何不选方案 A"论证段(见 §8.5 隐含项 2)**

### P2 — 实施中注意(影响落地完整性)

- [ ] **P2-A.1 parent PRD Acceptance Criteria 第 5、6 条按 owner + 时机细化(见 §7.2)**
- [ ] **transport-abstraction prd.md Notes 补 `apply_ui_diff` 协议化无需改签名说明(§8.3 隐含项 1)**
- [ ] **review-triage doc 驳回段补 WAL 措辞瑕疵标注(§8.4)**
- [ ] **child 2 prd.md `implement.jsonl` 待 design.md 补完后填充**
- [ ] **Phase 1 实施时 P1.3.1 的 grep 真跑一次(`helpers.rs:160,170` 是否真死代码),result 写进 task progress 备注**

### P3 — 远期 / 边角

- [ ] **P3 启动时同步 ARCHITECTURE §4 与 ROADMAP B10c**(沿用 P2-A 模式)
- [ ] **Phase 2 dogfooding 后回头评估 D1 代价(daemon 挂了 GUI 就废是否真痛)**

---

## 11. 结论

**任务体系可以进入 Phase 1 实施**,前提是:

1. **3 处 P0 流程补正**(child 2 lightweight 标 + D1 决策透明度 + jsonl 填充)
2. **3 处 P1 设计补正**(优先级 + 跨 phase 接口 + SSE 协议完备性)
3. **2 处 P2 文档同步**(cross-doc owner + 隐含项显式化)

补正后,**transport-abstraction 任务是一个低风险、高收益的近期任务**(估时 2-3 天,前端 21 文件 + 22 测试 + 后端 4 文件 emit 散点 + `AppHandleSink`/`SubagentEventSink` 双 trait 实现 + vitest + cargo test 全绿);**daemon-split 任务是中等风险交付**(估时 2-3 周 + 0.5 周 E2E + 与 Phase 1 的 `HttpSseSubagentSink` 接口契约对齐)。

**该任务体系最大的架构优点是"parent 持有跨 child 决策 + 2 child 独立 ship + triage 高质量甄别"**——这是项目目前状态下阻力最小的演进模式,完全呼应了 upstream RESEARCH 调研稿的"先 transport 抽象、再 daemon 拆分、最后远程认证"三阶段路径。坚持这条路径能最大化利用现有抽象(`AppHandleSink` / `AppCommandError`),最小化破坏面(Tauri 版始终可用,每个阶段都能独立验证)。

**评审没有建议"先把 child 2 design 也写完再 start"或"把 child 1 和 child 2 合并"**——前者与"渐进迁移"方法论冲突,后者会丧失 child 1 低风险铺路的机会。**当前拆分是对的,只是流程配置和接口契约需要补正**。

---

## 附录 A:本次评审覆盖的关键文件

| 文件 | 行数 / 字节 | 引用 |
|---|---|---|
| `.trellis/tasks/07-20-remote-access-multi-channel/task.json` | — | §0/4 priority + children |
| `.trellis/tasks/07-20-remote-access-multi-channel/prd.md` | 6.1k(94 行有效) | §0/2/7 parent 决策 + R1-R6 + Acceptance |
| `.trellis/tasks/07-20-remote-access-multi-channel/implement.jsonl` + `check.jsonl` | 各 254B | §3 空 stub 现状 |
| `.trellis/tasks/07-20-remote-access-multi-channel/research/review-triage-2026-07-20.md` | 5.7k | §8 对齐 + 拒绝项核验 |
| `.trellis/tasks/07-20-remote-access-transport-abstraction/task.json` | 644B | §0 priority + parent |
| `.trellis/tasks/07-20-remote-access-transport-abstraction/prd.md` | 4.7k(72 行) | §0 R1-R7 + Dependencies + Risks |
| `.trellis/tasks/07-20-remote-access-transport-abstraction/design.md` | 10k(267 行) | §5 §3.3 SubagentEventSink 设计 + §1 Transport 接口 |
| `.trellis/tasks/07-20-remote-access-transport-abstraction/implement.md` | 7.6k(166 行) | §0 P1.1-P1.3 checklist |
| `.trellis/tasks/07-20-remote-access-transport-abstraction/implement.jsonl` + `check.jsonl` | 各 254B | §3 空 stub |
| `.trellis/tasks/07-20-remote-access-daemon-split/task.json` | 616B | §0 priority + parent |
| `.trellis/tasks/07-20-remote-access-daemon-split/prd.md` | 6.1k(91 行,骨架) | §0/6 R1-R15 + Q1-Q6 |
| `.trellis/tasks/07-20-remote-access-daemon-split/implement.jsonl` + `check.jsonl` | 各 254B | §3 空 stub |
| `docs/REMOTE-ACCESS-RESEARCH.md` | 800 行(已吸纳两份 review 修正) | §0/§1 上游锚点 |
| `docs/ARCHITECTURE.md §4` | daemon 化触发条件 | §2 ARCHITECTURE 同步更新点 |

## 附录 B:评审方法说明

- 所有 P0/P1 事实断言均通过 `ls` / `cat` / `grep` 行号二次核验,基于 commit `74edc71`(2026-07-20)与 task 目录创建时间(2026-07-20 14:49-14:55)。
- 命令数 79 来自 `awk '/generate_handler!\[/,/^\s*\])/' src/lib.rs | grep -cE '^\s+(agent|commands)::[a-z_:]+'`,与 upstream RESEARCH 已吸纳的修正一致。
- listen 文件数 4 真调用来自 `grep -nE '\bawait listen[<(]' src/stores/...`,匹配 10 个 listen 调用分布在 4 个文件,验证了 triage doc "真实 listen 只在 4 文件"的结论。
- 评审未实际进入 `transport-abstraction design.md` 的全文逐句分析——只读了与 P0-A/B/C/D 直接相关的 §1、§2.2、§3.1-3.3、§5;其他章节属于"做得对"部分,记录在 §8.1 表中。
- 评审未对 `daemon-split` 缺 design.md / implement.md 做"补写建议"——留给 Carlos 决定是 P0-A 选项 A/B/C。
- 历史 review 引用基于 `docs/_reviews/` 目录现有文件,未二次核验历史 commit。

---

> 本评审署名 **MiniMax-M3**。所有 P0/P1 级断言均已通过文件 Read / grep 二次核验。后续代码演进请以当前代码为准。