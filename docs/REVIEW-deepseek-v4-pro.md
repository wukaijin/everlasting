# Everlasting 设计审查报告

> 审查日期：2026-06-04
> 审查模型：deepseek-v4-pro
> 审查范围：`README.md` + `docs/` 下 5 份设计文档（DESIGN / ARCHITECTURE / TECH / IMPLEMENTATION / BACKLOG），共约 1800 行
> 审查角度：完备性、一致性、可行性、风险、架构质量、技术选型、实施策略、文档质量

---

## 一、总体评价

这是一套**工程思维清晰、自洽度较高**的个人项目设计文档。5 维拆分（需求/架构/技术/实现/候选）覆盖了从"为什么做"到"怎么做"的完整链路，16 关卡请求生命周期设计和 8 步实施路线图让文档具备了可执行性。

**核心优势**: 边界感强（明确 5 个"不做"）、决策有记录（13 条决策日志）、交叉引用丰富（文档间可追溯）、技术选型有理有据。

**核心不足**: 测试策略缺失、数据模型未定义、安全模型不成体系、部分风险的缓解措施偏理论化。

下面从 8 个维度展开分析。

---

## 二、架构设计 — 16 关卡审查

### 2.1 设计亮点

| 关卡 | 亮点 |
|------|------|
| ③ Channel 入口 | 消息去重 + 鉴权 + 路由，三层防护分离清晰 |
| ⑤ Context 构造 | 4 层 Memory + Role + Skill 描述的分层加载，是 harness 的核心战场 |
| ⑧ 决策分叉 | Mode 检查在 tool 执行前拦截，Plan/Review/Yolo 的行为差异定义清楚 |
| ⑨ Tool 权限 | 静态规则（路径前缀）+ 动态规则（LLM 推理结果）+ 用户偏好三层防御 |
| ⑪ Git 联动 | 隐式关卡，后台持续运行，不打断主流程 |

### 2.2 潜在问题

**a) 关卡 ⑤ → ⑨ 之间存在信息断层**

⑤ 构造了 tool 白名单，⑨ 再检查一次。两次过滤的逻辑是什么关系？是 ⑤ 做粗筛、⑨ 做细筛，还是两层独立？文档没有说清楚。如果 ⑤ 已经过滤了但 ⑨ 还有自己的规则，两者冲突时以谁为准？

**建议**: 明确 ⑤ 是"给 LLM 看的 tool 列表"（影响 LLM 行为），⑨ 是"真正执行前的守门人"（影响安全）。两者角色不同，不存在冲突——但如果 ⑨ 拒绝了一个 ⑤ 放行的 tool，LLM 会困惑。

**b) ⑬ 循环检测算法过于简单**

Jaccard 相似度 > 0.9 作为阈值在实践中可能误判。例如 agent 连续改 3 个不同文件的 import，tool_use 的参数不同但语义相似——不应该算循环，但 Jaccard 可能会漏。反之，agent 反复调 `shell("cargo test")` 等待测试通过，参数完全相同但行为合理——应该给更多次机会而不是 5 次就打断。

**建议**: 把 tool name 和 exit code 加入判定维度。同一 tool + 同一 exit code 重复 N 次才触发，而不是只看参数相似度。

**c) ⑮ Channel 输出的消息合并策略**

"相邻 token 合并（50ms 内多条合并成一条）"——这个阈值对 GUI 没问题，但对飞书（需要 patch 消息）是否适用？飞书的 API 延迟可能远超 50ms，合并反而可能导致消息更新不及时。

**建议**: 合并策略改为 per-channel 可配置。

### 2.3 横切关注点评审

**2.5.5 Context 超限降级**的保护顺序有一个隐含假设：system_prompt + role + memory 总能塞进 context window。但如果 4 层 memory 已经 2K tokens、Role prompt 2K、system prompt 2K，在 Claude Haiku（200K window）完全没问题，但在 Ollama 本地模型（常见 8K-32K window）下，仅这三项就占掉了大半。降级策略只考虑了"超限后怎么丢"，没考虑"按模型能力动态调整加载量"。

**2.5.7 LLM Provider 限流**的跨 session 共享令牌桶——如果 3 个 session 同时跑，每个都在打 Anthropic API，按 tier 1 的 50 RPM 限制，令牌桶的公平分配策略是什么？平均分配还是先到先得？没说。

---

## 三、技术选型审查

### 3.1 高风险项

| 依赖 | 风险 | 评级 |
|------|------|------|
| **rig-core 0.38.1** | 预 1.0，breaking change 频繁。0.38 → 0.39 可能改 Agent trait 签名 | 中 |
| **git2-rs** | worktree API 不完整，文档提到可能 spawn 命令兜底 | 中 |
| **rmcp 0.16.0** | MCP 协议本身还在演进（2025-2026 年规范多次更新） | 中 |
| **Tauri 2 + WSLg** | 组合未经充分验证，WSLg 对 WebKitGTK 的兼容性是未知数 | 高 |

### 3.2 选型一致性检查

- ✅ Rust 生态闭环：rig-core / rmcp / git2-rs / sqlx / tokio 都是纯 Rust
- ✅ 前端不引入重型 UI 框架，自己攒 + shadcn primitives
- ⚠️ `react-diff-viewer` 已 3 年未更新（最后发布 2023 年），React 19 兼容性未知。应考虑 `diff-view` 或自写
- ⚠️ `nucleo` 的文档极少，API 不稳定。备选 `skim`（`fzf` 的 Rust 端口）更成熟

### 3.3 缺失的依赖

- **测试框架**: 文档未提及 `rstest` / `tokio::test` / `wasm-bindgen-test`
- **日志/追踪**: 未提及 `tracing` / `opentelemetry`（对调试 16 关卡的分布式行为至关重要）
- **基准测试**: 未提及 `criterion`（context 构造的 token 计数性能需要基准）
- **前端测试**: 未提及 Vitest / Playwright

---

## 四、实施路线图审查

### 4.1 步骤顺序的合理性

8 步路线图整体逻辑正确：先跑通（步骤 1-2），再结构化（步骤 3-4），再平台适配（步骤 5），再高级功能（步骤 6-7）。

但存在一个**关键风险**:

**步骤 5（WSL 体验）被标注为"建议抽出来作为步骤 0"**，但实际排在步骤 4 之后。文档自己承认步骤 1-4 都"假设 Tauri 跑得通"。如果步骤 5 验证失败，步骤 1-4 的代码需要回滚多少？答案取决于 Tauri 的问题在哪一层——如果是 WSLg 渲染问题，前端代码不受影响但 IPC 架构要重评估；如果是 WebKitGTK 在 WSLg 下根本不可用，整个方案要重新选型。

**建议**: 把步骤 5 的"WSLg hello world 验证"明确为一个不超过 2 小时的 spike，在一开始就做。

### 4.2 步骤间依赖的隐藏假设

| 步骤 | 隐藏假设 | 风险 |
|------|----------|------|
| 1 | Anthropic API key 可用 | 低（个人开发者通常有） |
| 3 | sqlx 编译期 SQL 检查不阻塞开发迭代 | 中（schema 频繁变更时 CI 慢） |
| 4 | 目标项目是 git 仓库 | 低（设计有约束） |
| 6 | portable-pty 在 WSL 内可用 | 中（PTY 在 Linux 没问题，但在 WSL 内的行为需验证） |
| 7 | Claude Desktop 支持 MCP stdio 协议 | 低（已支持） |

### 4.3 Daemon 化的时机模糊

文档说 daemon 化的触发条件是"飞书 channel 决定实施"或"长跑任务被打断是不是真痛"。但即使不做飞书，以下场景也需要 daemon：

- 用户关 GUI 但想后台跑 test suite
- Session 切换时前一个 session 的 shell 命令还在跑
- 用量统计需要跨 session 聚合

这些场景在 MVP 阶段就会出现。**建议**: 把 daemon 化从"飞书触发"提升为"步骤 5 之后必做"，不依赖飞书决策。

---

## 五、风险管理的深度审查

### 5.1 已识别风险的缓解措施评估

| 风险 | 缓解措施 | 评估 |
|------|----------|------|
| Rig 0.x breaking change | 锁版本 | ✅ 够用 |
| Tauri 2 在 WSLg 下的 bug | fallback VNC/X11 | ⚠️ VNC 体验极差，不应作为正式方案。应增加"XWayland 直接转发"作为首选 fallback |
| Linux sandbox 不可用 | landlock / firejail / 应用层黑名单 | ⚠️ landlock 需要内核 5.13+，需验证 WSL2 内核版本。firejail 需要 root。应用层黑名单是最低保障但最容易被绕过 |
| 上下文爆炸 | 压缩 / 裁剪 / 摘要 | ⚠️ 方案是"早期裁剪老消息，后期 LLM 摘要"。LLM 摘要本身消耗 token——用 2K tokens 摘要换 10K tokens 空间，净收益取决于压缩比。需要基准测试 |
| 循环检测 | Jaccard 相似度 | ⚠️ 见 §2.2(b) |

### 5.2 未识别但重要的风险

**a) SQLite 并发写入瓶颈**

SQLite 在 WAL 模式下支持并发读，但写是串行的。如果 GUI 在写 session 状态、daemon 在写 token 用量、agent 在写 message——三者同时写，可能出现 `SQLITE_BUSY`。sqlx 默认不重试，需要显式配置 busy_timeout。

**b) git worktree 清理失败**

Session 异常退出（kill -9、断电）后，worktree 目录残留。下次启动时扫描到孤儿 worktree 怎么处理？文档没说清理策略。

**c) Tauri 2 IPC 的序列化开销**

16 关卡中，②（Tauri IPC）和 ⑮（Channel 输出）都涉及 serde 序列化。大消息（如 tool_result 返回 50KB 的 shell 输出）的 JSON 序列化/反序列化开销可能成为 UI 卡顿的瓶颈。

### 5.3 风险严重度矩阵

```
        发生概率
        高    中    低
影响   ┌─────┬─────┬─────┐
  高   │ 上下文 │ WSLg │ 沙箱  │
       │ 爆炸  │ 兼容 │ 不可用│
       ├─────┼─────┼─────┤
  中   │ Rig  │worktr│ 飞书  │
       │ break│ee API│ 限速  │
       ├─────┼─────┼─────┤
  低   │ LLM  │  —   │  —   │
       │ 断连 │     │      │
       └─────┴─────┴─────┘
```

---

## 六、文档体系本身的质量审查

### 6.1 优点

- **交叉引用密集且准确**: DESIGN ↔ ARCHITECTURE ↔ TECH ↔ IMPLEMENTATION ↔ BACKLOG 之间有大量 `[ARCHITECTURE.md §2.2 第⑨关]` 级别的精确引用
- **版本语义说明**: 在 DESIGN §3.3 和 BACKLOG §0 两处提醒了"v1 在不同上下文含义不同"
- **决策日志时间戳**: IMPLEMENTATION §4 每条决策都有日期和原因
- **"什么不做"比"做什么"更详细**: DESIGN §3.5 列出了 5 个明确不做的方向

### 6.2 不足之处

**a) 缺少测试策略文档**

7 份文档，0 处提及测试。对于自研 agent core 的项目，至少要回答：

- ⑨ 权限检查怎么测？（mock tool call + 各种路径组合）
- ⑬ 循环检测怎么测？（模拟 LLM 返回 + 构造重复序列）
- ⑩ Tool 执行怎么测？（文件系统 sandbox / tempdir）
- 前端组件怎么测？（Vitest + React Testing Library？）
- E2E 吗？（Playwright 跑 Tauri？不太现实，但至少要说明为什么不做）

**b) 数据模型缺失**

SQLite 被提及 15+ 次，但没有一次给出 schema。文档说"SQLite 是唯一存储"，但没有一张表定义。ARCHITECTURE 中提到 `session_instructions`、`memories`、`images`、`attachments` 等表名，但字段、索引、关系都没有。这导致一个很实际的问题：无法评估 schema 是否支持 16 关卡中所需的查询。

**c) 安全模型不成体系**

安全相关的讨论分散在 BACKLOG §8.3（按功能列风险表）、ARCHITECTURE §2.2 ⑨（权限检查）、DESIGN §5.1（风险表中的 sandbox 行）。缺少一个统一的安全模型章节，回答：

- 信任边界在哪（用户输入 ↔ LLM ↔ 文件系统 ↔ 网络）
- 最小权限原则怎么落地
- 什么样的 tool call 需要用户确认，什么样的自动放行
- prompt injection 的防御纵深（不止 BACKLOG §8.3 中分散提的几点）

**d) 性能指标缺失**

文档没有定义任何性能目标。例如：

- 用户按回车到看到第一个 token 的延迟目标？（<500ms? <1s?）
- SSE token 渲染帧率？（60fps 逐字还是 10fps 批量？）
- Session 切换加载历史消息的延迟？（100 条消息 < 200ms?）
- 项目扫描（@文件补全）的冷启动延迟？

**e) 前端架构描述薄弱**

整个系统架构中前端只被描述为"React 18 + Vite + shadcn/ui"。但 16 关卡中有 4 关是前端相关（①②⑭⑯），ARCHITECTURE 对前端的描述仍然停留在 Tauri event 层面。缺少：

- 前端组件树（App > ProjectPane > SessionList > ChatView > InputBox）
- 前端状态管理（Zustand store 的结构是什么样的）
- 前端错误处理（event 监听失败、tauri.invoke 超时、SSE 流中断的 UI 表现）

### 6.3 文档间不一致

| 位置 | 内容 | 问题 |
|------|------|------|
| README.md 行 18 | "✅ 14 关卡架构细化" | ARCHITECTURE 已扩展为 16 关 |
| DESIGN.md §3.3 开头 | "以 9 项为基础，补 3 项" | BACKLOG 是 7 个方向，数字对不上。实际上 BACKLOG 的 7 个方向展开后对应 DESIGN 的 12 项，但表述让人困惑 |
| ARCHITECTURE.md 行 4 | "请求生命周期的 14 道关卡" | 正文已改为 16 关 |
| IMPLEMENTATION.md §2.5 | 步骤 5 标注为 [MVP] | 但此步被描述为"验证 Tauri 在 WSLg 跑得通"，更像是 spike 而非 MVP 功能 |

---

## 七、候选功能（BACKLOG）的合理性审查

### 7.1 五层模型的正确性

BACKLOG §0 的五层分层（输入层→指令层→输出层→拓扑层→触达层）在概念上是合理的，但**层间边界有模糊处**：

- `/command` 放在输入层（§1.3 输入触发），但同时是 Skill 的用户调用入口（§2 与 /command 的关系）。它到底是用户交互方式（输入层）还是 agent 能力（指令层）？
- 生成式 UI（§5 输出层）的 `button` primitive 触发 Tauri command——这个 button 到底是"agent 呈现结果"还是"用户新输入通道"？

### 7.2 各功能评估

| 功能 | 评估 |
|------|------|
| §1 图片/@文件/command | 设计扎实。图片的 resize→hash→去重 流程成熟。@文件的风险（路径遍历）有应对。是 MVP 之后最应该做的 |
| §2 Skill | 与 Anthropic skill 规范对齐，设计克制。但 `use_skill` 虚拟 tool 的描述列表如果太长（20+ skill），LLM 选择困难 |
| §3 Memory | 4 层记忆设计合理，与 Claude Code 的 CLAUDE.md 生态对齐。2K token 上限是实用的约束 |
| §4 多角色/多模式/编排 | v1 只做 role + mode 无编排——这个克制是对的。编排是另一个产品 |
| §5 生成式 UI | 约束式 vs 自由式的选择正确。4 种 primitive (button/selector/diff/code_block) 足够 MVP。开关默认关是好的保守策略 |
| §6 飞书 | 技术方案扎实（Channel Adapter + Daemon 化）。但**使用场景存疑**：个人在飞书里跟自己的 agent 聊天？如果只是"远程遥控"，云端同步（§7）的 REST API 可能就够了 |
| §7 云端同步 | 方案克制（只 push 摘要、Cloudflare Workers + D1）。隐私设计合理。但"远程遥控"和"飞书 channel"有功能重叠：都是在外网操作 agent。应该二选一 |

### 7.3 飞书 vs 云端同步的功能重叠

飞书和云端同步都在解决"不在电脑前怎么用 agent"的问题，但路径不同：
- 飞书 = IM 作为交互界面（实时双向）
- 云端同步 = REST API 作为状态层（异步只读为主）

如果只做云端同步 + web 管理页，能得到飞书 80% 的远程能力，且不需要 Daemon 化。反过来只做飞书，能得到实时性但牺牲了 REST API 的通用性。

**这个问题文档没有讨论，但值得在实施前做决定。**

---

## 八、与参考项目的对标审查

根据 docs/README.md 的必读参考清单，以下是设计文档相对参考项目的对齐度：

| 参考项目 | 学到什么 | 设计文档是否体现 |
|----------|----------|:---:|
| anthropics/claude-agent-sdk-python | agent loop 的结构 | ✅ 16 关卡中的 ⑥→⑫→⑥ 循环对标了 SDK 的 query loop |
| All-Hands-AI/OpenHands | 事件流通信、前端组件 | ⚠️ 事件流设计有（Tauri event），但前端组件架构未展开 |
| 0xPlaygrounds/rig | Agent 抽象 | ✅ TECH §2 有详细选型分析 |
| modelcontextprotocol/rust-sdk | MCP 协议实现 | ✅ MCP 只外暴露的决策有专章 |
| cline/kanban | worktree + auto-commit | ✅ ARCHITECTURE §3 有 worktree 决策 |
| Aider-AI/aider | repo map、token 优化 | ❌ repo map 概念未提及。这是 Aider 的核心竞争力——让 LLM 理解项目结构。Everlasting 目前依赖 @文件手动补全，缺少自动项目感知 |
| cline/cline | modes 状态机 | ✅ BACKLOG §4.2 有 5 种 mode 的状态定义 |

**关键缺失: Repo Map**

Aider 的 repo map 是自动生成项目结构概览，让 LLM 知道"有哪些文件、函数、类"。Everlasting 的 @文件补全是"用户告诉 LLM 去读哪个文件"，这是两种不同的哲学：
- @文件 = LLM 被动，等用户喂
- repo map = LLM 主动，自己决定读什么

对于 vibe coding 场景，repo map 可能是比 @文件更重要的功能——agent 需要自己探索项目结构。建议在 BACKLOG 中加入"自动项目感知"方向。

---

## 九、改进建议（按优先级）

### 高优先级（实施前应解决）

1. **补充数据模型（SQLite schema）** — 哪怕只是初版 ER 图。至少定义 `projects`、`sessions`、`messages`、`tool_calls` 四张核心表
2. **制定测试策略** — 单元测试（Rust 端）、组件测试（React 端）、集成测试（IPC 层）的范围和目标
3. **明确架构版本号** — 修复 14 关 vs 16 关的文档不一致，统一为当前设计版本
4. **完成步骤 0（WSLg 验证）** — 在投入步骤 1-4 之前，先用 Tauri hello world 验证 WSLg 可用

### 中优先级（MVP 阶段解决）

5. **补充前端架构** — Zustand store 结构、组件树、错误处理策略
6. **定义性能目标** — 首 token 延迟、session 切换延迟、流式渲染帧率
7. **明确 Daemon 化时机** — 不依赖飞书决策，独立评估 daemon 化的必要性
8. **repo map 可行性评估** — 研究 Aider 的 repo map 方案，评估是否需要加入 BACKLOG

### 低优先级（v1 阶段解决）

9. **安全模型专章** — 整合分散在各文档的安全讨论
10. **飞书 vs 云端同步二选一** — 评估功能重叠，决定优先级
11. **react-diff-viewer 替代方案** — 确认 React 19 兼容性，必要时换 `diff-view` 或自研
12. **性能基准测试计划** — context 构造的 token 计数、大消息序列化开销

---

## 十、总结

这套设计文档的**质量对于一个个人项目的设计阶段来说是出色的**。它的核心价值在于：

- 16 关卡把 harness engineering 的核心问题空间化、可讨论化
- 13 条决策日志让每个技术选择都有据可查
- "什么不做"定义清楚，防止范围蔓延

但它目前停留在"架构师视角"——讲清楚了系统怎么搭，但对"怎么验证搭对了"（测试）、"数据长什么样"（schema）、"跑得多快算够"（性能目标）着墨太少。

**一句话**: 设计足以指导 MVP 开发，但启动前需要补上数据模型和测试策略两块拼图。

---

> 本报告随设计文档演进更新。任何重大架构变更后应重新审查。
