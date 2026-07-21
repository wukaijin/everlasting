# 两份 Review 的独立甄别报告

> 来源:[REVIEW-remote-access-research-2026-07-20.md](../../../../docs/_reviews/REVIEW-remote-access-research-2026-07-20.md)(MiniMax-M3)+ [REVIEW-remote-access-research-deepseek-v4-pro.md](../../../../docs/_reviews/REVIEW-remote-access-research-deepseek-v4-pro.md)(DeepSeek-v4-pro)
> 甄别日期:2026-07-20
> 用途:本 parent task 的两个 child 实施时,sub-agent 会读这份文件了解"哪些 review 意见被采纳、哪些被驳回、为什么"。

## 总体判断

| 维度 | MiniMax-M3 | DeepSeek-v4-pro |
|---|---|---|
| 风格 | 强势、攻击性强、抓数字错误 | 温和、建设性、补新维度 |
| **事实准确率** | **高** —— 多数 load-bearing 断言核验为真 | **中** —— 接受了文档错误数字,但有误读 |
| 价值类型 | 纠错(数字/结构性错误) | 补全(WSL 网络/token 安全等文档没覆盖的点) |
| 独立性 | 自己重跑了 grep + 行号核验 | 主要基于文档内部逻辑推演 |

**关键:两份 review 互补,不冲突。MiniMax 是"事实核查员",DeepSeek 是"设计顾问"。但两份都有错的地方,不能照单全收。**

---

## 采纳清单(已修正进 RESEARCH.md)

### 来自 MiniMax(全部核验为真)

| 编号 | 断言 | 核验结果 | 修正 |
|---|---|---|---|
| P0-A | 命令数 57 → 实际 ~80 | ✅ 真,实测剔除注释后 = **79** | RESEARCH §1.2a 全表重写 |
| P0-B | 事件数 9 → 10(标题与表格矛盾) | ✅ 真,笔误 | §1.2b 标题改 10 |
| P0-C | 前端文件数 22/21/4 偏 | ✅ 方向真,实测:import 21 / invoke 24 / **listen 真实调用只在 4 文件**(MiniMax 说 8 是把注释也算进去了) | §1.5a 重写 |
| P0-D | "所有 emit 收敛到 sink" 是错的 | ✅ **本次最重要发现**:实测 6 处直调散点 | §1.2b + §1.6 重写,这是 Phase 1 P1.3 的依据 |
| P1-A | `apply_ui_diff` 从不返回 Err | ✅ 真,8 个错误路径全 `Ok({ok:false,...})` | §1.2c 改为"kind 升 enum,签名无需改" |
| P1-B | SSE backpressure 风险 | ✅ 合理补充(具体数字未验证,不照搬) | 新增 §3.1e |
| P1-D | listen 签名语义错位 | ✅ 观察真,但 MiniMax 方案 A 会破坏 streamController;采纳 **DeepSeek 方案 B** | §4.2 |

### 来自 DeepSeek(MiniMax 没覆盖的有价值补充)

| 断言 | 价值 | 修正 |
|---|---|---|
| §8.6 WSL 浏览器访问链路 | ✅ **本次最有价值新增**,Phase 2 第一验证关 | 新增 RESEARCH §3.3a WSL 段 + ROADMAP P2.5 |
| §8.4 oneshot → RPC 化 | ✅ Phase 2 核心难点,原文档一笔带过 | RESEARCH §4.3 步骤 7 补序列图 |
| §8.2 数据库迁移(GUI 全走 RPC) | ✅ 比 MiniMax P1-C 更彻底 | RESEARCH §4.3 步骤 5 + ROADMAP P2.4 |
| §3.2 token 存储 XSS | ✅ 合理安全考量 | Phase 3 远期设计参考 |
| §六 GUI 自动管理 daemon | ✅ 开发体验设计 | ROADMAP P2.4 |
| §7 风险 10 静态文件 server | ✅ 单二进制部署 | RESEARCH §4.3 步骤 6 + ROADMAP P2.4 |
| §8.1 测试策略完全缺失 | ✅ 真 | RESEARCH §6.4 + ROADMAP 各子阶段验证标准 |

---

## 驳回清单(核验为假或过度)

### MiniMax 驳回

| 编号 | 断言 | 驳回理由 |
|---|---|---|
| P0-D 措辞 | "作者有意绕过 sink ... 但 doc 没提"暗示疏漏 | **部分误读**:subagent 绕过 sink 是有设计原因(collector vs 父 loop sink 双路径),不是"忘了迁移"。结论(要处理散点)对,原因揣测不准确 |
| P2-A | 触发条件拆双条款 §4-A/§4-B | **过度工程化**,单条款够用 |
| 边角 11.2 | URL kebab / body camelCase 强制统一 | 违反现有 snake_case 约定(`chat-event` / invoke 参数),不采纳 |
| 边角 11.3 | daemon 启动 SQLite 文件持有检测 | 与 P1-C/DeepSeek §8.2 重复,且 WAL 模式天然支持多进程,方向错 |
| P0-C listen=8 | 把注释里的 `listen<>` 算进真实调用 | **实测真实 `listen()` 调用只在 4 个 store 文件**,另 4 个是注释 |

### DeepSeek 驳回

| 断言 | 驳回理由 |
|---|---|
| §1.2 IPC 盘点"教科书级" | **误判**:接受了 57/9/22 错误数字,基于错误前提的"优秀"评价打折扣 |
| §10.1-2 command_palette "同函数重复注册" | **误读**:实测 `command_palette::list_commands` 与 `panel::list_subagents` 是完全不同的函数,功能服务于重叠面板,非重复注册 |
| §七 风险 8.5 BackgroundShell 难度调低为"中" | 部分对但理由不全:忽略了前端查看 shell 状态的跨进程需求 |
| §六 listen 签名 `() => void` vs `Promise<() => void>` | 吹毛求疵,保持 Promise 统一签名便于异步 transport |

---

## 对 child task 实施的指导

### child 1(transport-abstraction)注意点

- **P1.3 收敛 emit 散点时,subagent 路径不能简单合并到 `AppHandleSink`**——要保留 collector 双通道语义(MiniMax P0-D 误读点 + 我核验确认)。
- **listen 真实调用只在 4 文件**(streamController / permissions / projects / subagentRuns),不是 MiniMax 说的 8 文件。
- **`apply_ui_diff` 不需要改签名**(MiniMax P1-A),只是 `kind: String` 可选升 enum。

### child 2(daemon-split)注意点

- **WSL localhost forwarding 是第一验证关**(DeepSeek §8.6),P2.5 必须实测。
- **oneshot 跨进程转换**:axum handler 通过 `Extension<Arc<AppState>>` 调 `permission_store.resolve()` 命中 daemon 进程内 sender(DeepSeek §8.4 + RESEARCH §4.3 步骤 7 序列图)。
- **GUI 全走 RPC,不开本地 db pool**(DeepSeek §8.2),彻底消除 dual-pool。
- **daemon 内嵌静态文件 server**(`tower-http::ServeDir`)实现单二进制部署(DeepSeek 风险 10)。
- **SSE backpressure**:bounded mpsc + 心跳,具体数字实施时实测,不照搬 MiniMax 的"Vercel AI SDK 20ms tick"(P1-B 未验证)。

---

## 补充:REVIEW-remote-access-tasks-2026-07-20(MiniMax 第三轮,task 体系评审)甄别

> 来源:[docs/_reviews/REVIEW-remote-access-tasks-2026-07-20.md](../../../../docs/_reviews/REVIEW-remote-access-tasks-2026-07-20.md)
> 这份 review 针对 3 个 Trellis task 本身(非 RESEARCH.md)。甄别关键:**review 的多数 P0 基于"Trellis 脚本会硬卡"的假设,但实际核验脚本并不硬卡**。

### 核验:review 的 P0/P1 真实严重度

| review 断言 | review 严重度 | 实际核验 | 真实严重度 |
|---|---|---|---|
| **P0-A** child 2 缺 design/implement 会"卡 review gate" | P0 | `task.py` 全文 grep `design.md\|implement.md\|lightweight\|dev_type` **零命中**;`dev_type` 字段无人读,workflow §1.4 是 advisory(人 review)非脚本硬 gate | **P2**——不阻塞 start,child 2 骨架 PRD 是有意为之 |
| **P0-B** D1 需 Carlos ack + 同步 ARCHITECTURE §4 | P0 | ack 是合理(决策透明度);ARCHITECTURE 同步是真正的有效点 | **P1**——ack 已补(parent prd.md),ARCHITECTURE 同步在 Phase 1 archive 时做 |
| **P0-C** jsonl 空 stub 影响 sub-agent dispatch | P0 | seed `_example` 自动跳过(task_context.py:118);空 jsonl **不阻塞** start,只是 sub-agent 少了自动注入的 spec | **P1**——child 1 已填 5+4 entries,child 2 待 design 后填 |
| **P1-A** priority 全 P2 不当 | P1 | priority 字段**纯 metadata**,task.py 无任何调度逻辑读它(只 `--priority` 参数 default P2,list 不排序) | **P3**——纯标签,不影响行为,**不改** |
| **P1-B** HttpSseSubagentSink 接口漂移 | P1 | **有效点**——design.md §3.3 对 Phase 2 有承诺,child 2 PRD 没承接 | **P1**——✅ 已采纳,child 2 prd.md Dependencies 补承接契约 |
| **P1-C** SSE lost segment + 超时 | P1 | 有效点,属 Phase 2 设计细节 | **P2**——✅ 已采纳,child 2 prd.md R6.1/R6.2/R6.3 + Q-SSE-1 |
| **P2-A** 跨文档同步无 owner | P2 | 有效点 | **P2**——✅ 已采纳,parent prd.md 验收项加 [Phase 1 archive 时] owner/时机 |

### 驳回

| review 断言 | 驳回理由 |
|---|---|
| **P0-A "task.py start 会卡 review gate"** | 实测 task.py 无任何 design/implement 检查。workflow §1.4 措辞是"complex tasks must exist and be reviewed",**review 是人做的,不是脚本 gate**。child 2 作为骨架 PRD + 显式声明"Phase 1 完成后补 design"是合法状态,不需要标 lightweight/parked |
| **P0-C 紧迫性** | 空 jsonl 不阻塞任何流程,workflow 1.3 说 seed 会被"自动跳过"。child 1 已填(启动实施前填是合理工程实践),但把它列为 P0 夸大了影响 |
| **P1-A priority 升级** | priority 在本项目 task.py 里是纯标签字段,无调度语义。升 P1 只是给人看,不改变任何行为。**不改** |
| **§8.4 "WAL 天然支持多进程"措辞瑕疵** | review 自己也承认"不影响结论"。WAL 确实是多读+单写,但因 R9 已要求 GUI 不开 db pool,daemon 是唯一 writer。措辞简化不影响任务正确性 |

### 已采纳并落地的修正

- ✅ parent prd.md D1 段加"Carlos 决策日期 2026-07-20" + ARCHITECTURE §4 同步说明
- ✅ parent prd.md 验收项 5/6 加 [Phase 1 archive 时] owner/时机
- ✅ child 2 prd.md Dependencies 加 transport-abstraction 接口契约承接(HttpSseSubagentSink)
- ✅ child 2 prd.md R6 补 R6.1(lost segment)/ R6.2(ping 超时)/ R6.3(大 message 阈值)
- ✅ child 2 prd.md Open Questions 加 Q0(in-process fallback 孵化)+ Q-SSE-1(buffer 大小)
- ✅ child 1 implement.jsonl 填 5 entries + check.jsonl 填 4 entries(validate 通过)

### 甄别总判断

这份 review **事实核验质量依然高**(行号准、grep 可复现),但**对 Trellis 工作流的脚本行为有误判**——它把 workflow.md 的 advisory 措辞当成硬 gate,导致 P0-A/P0-C/P1-A 三个"P0/P1"被夸大为阻塞性问题。实际核验脚本后,真正阻塞的是 0 个,有价值的改进是 4 个(已全部落地)。

**与上游两份 review 一致的质量特征**:事实层面可信,推断层面需独立核验。这次的"脚本行为核验"是关键——没核验就会照单全收"P0 阻塞"的判断,浪费精力在标 lightweight 这种无意义动作上。
