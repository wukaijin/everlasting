# 评审意见分类（MiniMax-M3 + DeepSeek-v4-pro）

> 评审日期：2026-07-26。两份评审独立收敛于 4 个核心议题（高信号）。
> 本文档记录每条意见的独立判断：采纳 / 部分采纳 / 不采纳 + 理由。

## 核心议题（两份都提）

### 议题 1：resume 硬前置降级？🔴 待用户拍板

- **评审立场**（两份一致）：resume 是 epic 最大风险，建议降级为「增强项」，review v1 不带 resume 跑通。
- **我的判断**：**不同意降级，但接受加 fallback 路径**。
  - 分歧理由：降级建议忽略了两点 —— (a)「resume 必须先做」是用户在 brainstorm 里明确的产品决定（省 token），不是工程师强加；(b) resume 不只省 prd 重读，省的是整轮 reviewer 会话历史（含 codebase 探索过程）。
  - 接受的部分：MiniMax 建议的「C1 fallback：resume 不可用时降级全新派」稳妥。C1 PRD 加此 fallback，但默认仍走 resume。
- **处置**：⚠️ 这条涉及产品优先级，**最终由用户拍板**（见文末 Open Decision）。

### 议题 2：层次 2 单点失败 + final_text fallback ✅ 采纳

- **评审立场**（两份一致）：主 LLM 提炼可能漏/错，需 reviewer 原始 final_text 作验证路径。
- **我的判断**：**完全采纳**。层次 2 的核心风险就是提炼失真，而 reviewer final_text 已持久化在 subagent_runs，零成本做 fallback。
- **处置**：
  - C2 R3：每条 finding 带 `source_run_id`，可跳转 reviewer 原始 final_text。
  - C2 Notes：「不直接读 final_text」改为「主要数据源 review-state.json，final_text 作验证/fallback」。
  - C3 reviewer.md：约束输出便于主 LLM 提炼（已有，强化）。

### 议题 3：resume 的 stale context ✅ 采纳

- **评审立场**（两份一致）：resume 续接 messages 含旧 prd 内容，reviewer 可能基于旧引用判断。
- **我的判断**：**采纳**。这个风险 real（生成评审提示词时我也意识到了）。
- **处置**：
  - C1 R4：resume clarification 升级为结构化对象（current_state / changes_since_last / this_round_purpose）。
  - C3 reviewer.md：system prompt 加「若上轮对话引用与当前文件矛盾，以当前文件为准」。

### 议题 4：模型失败/超时 + 无限回环 🟡 部分采纳

- **模型失败**：✅ 采纳。多模型并发必须处理 N-1 成功。review-state.json schema 加 `status` 字段（completed/failed/timed_out/...），wf-synthesize 指引主 LLM 处理部分失败。
- **无限回环 cap**：🟡 部分采纳。**不采纳** MiniMax 的 `max_rounds:5` 硬编码到 workflow.json（引擎层不该管，validate 不认此字段）；**采纳** DeepSeek 的 convergence_note 软引导（wf-synthesize 要求主 LLM 每轮给收敛评估 + 主动建议定稿）。

## 采纳的遗漏点（低成本高价值）✅

| # | 遗漏点 | 来源 | 处置 |
|---|---|---|---|
| 1 | reviewer 应读代码做「设计 vs 实现一致性」检查 | DeepSeek 4.6 | C3 reviewer.md 明确评审范围含项目代码（工具集已是 read/grep/glob，改 prompt） |
| 2 | review-state.json schema 完善 | MiniMax §6.1 | 采纳核心字段：schema_version、finding_id（稳定 id）、source_run_id、status、per-round dimensions。增强字段（diff_from_previous/seen_by）MVP 不做 |
| 3 | model 用稳定 id 而非 display_name 作 map key | MiniMax 4.12 | schema models key 改 model_id，display_name 仅展示 |
| 4 | 写文件原子化 + 前端解析降级 | 两份 | tmp+rename 防撕裂读；前端 try-catch 防白屏 |
| 5 | revising 加 change log | DeepSeek 决策6 | review-state.json 加本轮修订摘要（弥补砍 review.md 后「改了什么无记录」的缺口） |
| 6 | reviewer 中途模型变更 | DeepSeek 4.4 | round 加 models_present，矩阵按出现过的模型并集渲染列 |

## 不采纳的意见（含理由）🔴

| # | 意见 | 来源 | 不采纳理由 |
|---|---|---|---|
| 1 | 先做两个 spike（重合度 + resume PoC）再动 | MiniMax §7 | 项目是学习 harness 工程，spike 的「重合度>70% 就砍」会扼杀学习价值；多模型编排本身值得做 |
| 2 | C1 提升为独立 epic | MiniMax §2.1 | 语义之争，不影响执行（无论叫什么 C1 都先做且独立验收） |
| 3 | review/dev 共享 task 读写竞争加锁 | MiniMax §4.10 | 设计是 review 在 dev 之前（串行），不存在并发；写文件原子化（已纳入）可顺带缓解 |
| 4 | 跨 provider 选 reviewer 作硬约束 | DeepSeek §1 | 好建议但不该硬约束（用户可能只配 1 provider）；改为 skill 提示 |

## 行号问题（MiniMax §0）⚠️ 部分属实

| 项 | 判断 |
|---|---|
| `build_worker_messages` 位置（dispatch.rs:645 → mod.rs:642） | ✅ 属实，需修 |
| SynthesisRound 行号（127 → 130） | ✅ 偏差，design 阶段统一校正 |
| 「dispatch.rs:1632/1653 不存在」 | ❌ MiniMax 自己错了 —— 这两行是真实 allowed_roles 调用，文件 2601 行（非 MiniMax 说的 2400） |
| 「dispatch 零消费 coordination」表述歧义 | ⚠️ 论断正确但锚点指向 roles_by_state 易误解，需澄清 |

**处置**：修 build_worker_messages 位置；其余行号 design 阶段精确 read 时统一校正。

## Open Decision（需用户拍板）

**议题 1（resume 硬前置降级）**：
- 选项 A（维持现状）：resume 硬前置，C1 先做；C1 加 fallback（resume 不可用降级全新派）。
- 选项 B（采纳评审建议）：resume 降级为增强项，review v1 全新派跑通，C1 并行或后做。

我的推荐是 A（尊重原 brainstorm 决定 + fallback 缓解风险），但这是产品优先级判断，由用户定。
