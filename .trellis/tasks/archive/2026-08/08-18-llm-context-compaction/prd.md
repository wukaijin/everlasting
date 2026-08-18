# C3 摘要式上下文压缩 — 取代机械丢组,支撑去硬卡

## Goal

把 C3 的"机械丢组"升级为 **LLM 结构化摘要压缩**:超触发线时,被压缩区由 LLM 生成 handoff 式摘要回填,近期消息逐字保留,原始历史落库无损。为后续任务(去 MAX_TURNS 硬卡、手动 /compact、handoff)铺管道。

## Background

- 现状:C3(06-12)纯机械贪心丢组,丢掉的早期上下文无语义保留;长 session 早期决策彻底丢失。
- 上位目标:去掉单聊 200-turn 硬卡的前提是上下文可无限续(压缩保语义),本任务是第一块。
- 调研:7 家工具(Claude Code / Codex / Gemini CLI / opencode / Cline / Roo / OpenHands / Aider)+ Manus/Anthropic 通用原则,见 `research/02-05`。

## Requirements

- R1 **自动触发**:turn 边界估算超 0.85 窗口时自动压缩;仅主 loop 单聊 scope(`!worker && !群聊`,gate 同 memory digest 模式)。
- R2 **摘要回填**:被压缩区 → LLM 结构化摘要(模板含用户意图/决策/文件/错误/进行中状态/下一步,双逐字锚点),以 synthetic user 消息回填,插在 B5 memory 头对之后。
- R3 **持久化**:摘要消息落 `messages` 表(metadata kind=compaction_summary + cutoff 水位);后续请求历史重建时按水位替换(查 DB,不信 wire),DB 原始行不删。
- R4 **保留区**:近期消息逐字保留(`clamp(15k, 窗口×10%, 25k)` 预算,组边界对齐,最后 typed user 轮必入);RULE-A-001 配对原子性不变量保持。
- R5 **兜底链**:摘要失败 → 现机械丢组 fallback;仍超 → StillOver fail-fast(RULE-A-002 不变);连续 3 次失败熔断(同 session 粘性)。
- R6 **增量合并**:多级压缩时输入带 prior-summary,冲突 "conversation wins"。
- R7 **可观测**:compaction_json 扩摘要维度(usage/模型/水位);TracePanel 现有 ContextCompacted 展示不回退。
- R8 **缓存安全**:摘要消息位置 ≥ 2,不 bust memory cache 断点;压缩动作尽量 turn 边界一次性。

## Non-Goals(后续任务,本任务不做)

- 手动 `/compact` 命令入口;
- MAX_TURNS 软卡化 / 去硬卡本体;
- handoff 跨 session 接力;
- worker / 群聊 scope;
- chat 流内摘要 UI 卡片(摘要消息先只服务 context 与 D2 搜索);
- microcompact 独立层(Q5);
- 摘要模型配置面(Q2 留位)。

## Acceptance Criteria(草案,brainstorm 后定稿)

- [ ] AC1:超线 turn 触发摘要,请求 context = 合成头 + 摘要 + 保留区 + 当前输入;`compaction_json` 记录 before/after/method/摘要 usage。
- [ ] AC2:同一 session 第二次请求**不重付摘要**(水位替换生效,零额外 LLM 调用),且**保留区与本请求用户提问跨请求存活**(修订 2026-08-18:按 cutoff_seq 折叠,非按摘要行位置——PR2 check P1 发现原语义会丢保留区)。
- [ ] AC3:二级压缩(跨请求或同 loop 内)产出增量合并摘要(prior-summary 来自循环内 `SummaryAnchor`,可验证)。
- [ ] AC4:摘要 LLM 失败注入测试 → 回退机械丢组,turn 正常完成;连续 3 次 → 熔断标记。
- [ ] AC5:**被压缩区原始行不删**(仅新增摘要行;落库无损);D2 搜索仍能命中被压缩内容,且摘要行本身可被命中(纯摘要,无前缀话术污染)。
- [ ] AC6:tool_use/tool_result 配对不变量测试(保留区边界扫描)全过。
- [ ] AC7:mock 端到端 + turn-smoke live 实测一次真实压缩(前后 context_input 对比)。
- [ ] AC8:群聊/worker session 不触发新路径(gate 测试)。

## 决议记录(2026-08-18,用户批准)

- **Q1 持久化形态 → 甲**:摘要消息落 `messages` 表(metadata kind=compaction_summary),后端按水位替换,前端 wire 零改动。水位判定查 DB(`loaded_session.messages`),不信任前端 wire(wire 层 ChatMessage 无 metadata 字段)。
- **Q2 摘要模型 → session 主模型**(MVP);独立/廉价模型配置面留 follow-up。
- **Q3 摘要粒度 → 增量合并**:输入带 prior-summary,冲突规则 "conversation wins"。
- **Q4 阈值 → 0.85 触发**(原 0.80),保留区 `clamp(15k, 窗口×10%, 25k)` 逐字,验收线 target ≤ 0.60。
- **Q5 microcompact 前置层 → 不做**(C7D stub + memory digest 已治前置大头;机械丢组 fallback 覆盖)。
- **Q6 摘要可见性 → MVP 仅 trace + D2 搜索命中**;chat 流内摘要卡片与手动 /compact 同期 follow-up。前端仅加"最低渲染"防困惑(metadata.kind 判断,低调系统样式行,不算卡片)。

## Notes

- 复杂任务:需补 `design.md`(摘要调用时序/水位替换算法/DB schema 增量)+ `implement.md` 后再 `task.py start`。
- 关联 spec:`.trellis/spec/backend/` 的 agent-loop-architecture / database-guidelines / token-usage-tracking。
