# epic: agent 自主记忆系统

> Trellis epic `06-29-autonomous-memory` · 来源 4 轮需求探讨 + [docs/spikes/007-agent-autonomous-memory-plan.md](../../../docs/spikes/007-agent-autonomous-memory-plan.md)(完整设计 + 对 spike-005/006 的吸收对比)
> 5 个阶段 child:`06-29-am-p1-storage` / `06-29-am-p2-readwrite` / `06-29-am-p3-tool-recall` / `06-29-am-p4-event-reflect` / `06-29-am-p5-quality`

## Goal

把当前 `memory/` 模块从"开发者手写的指令文件加载"(4 个固定 CLAUDE.md/AGENTS.md 全量注入每个 session)升级为 **agent 自主产生、跨 session 召回的经验库**。典型场景:agent 多次因路径问题调用测试命令失败、成功后记住这个坑,在另一个 session 遇到同类操作时第一时间想到规避。

本质跃迁:不是"加几张表",而是 **谁产生内容 + 什么时刻进入 context** 都变了——从"开发者写、全量吃"到"agent 写、按需召回"。

## 现状 vs 目标

| | 现状(指令文件) | 目标(自主记忆) |
|---|---|---|
| 谁产生 | 开发者手写 | agent 自主 + 旁路事件 |
| 进入 context | 每 session 全量 | 按需召回(背景注入 + 工具执行前) |
| 跨 session | 否(每次重读文件) | 是(DB 持久) |
| 性质 | 静态 config | 动态经验库 |

## 核心矛盾(设计的生死线)

选定组合 **"agent 全自主写 + 背景召回强制注入"** 会放大噪音:全自主写倾向"该记就记"→ 库膨胀;背景召回把记忆强制塞进每个 session prompt → 噪音被自动分发。故:
- **写入端**必须装质量漏斗(状态机),不是写入审批
- **召回端**必须挑对时机 + **精确率优先**(漏一条能用 recall 补,注入一条错的污染整个回答)

## 设计要点(详见 spike-007)

### 写入
- **两路径**:主 agent `remember` tool(含用户显式)→ `candidate`;旁路事件 reflection(连续 ≥2 次同名工具失败后成功)→ 直接 `active`
- **状态机**:`candidate → active → verified`,靠 `hit_count` 自动晋升;老化/覆盖 → `demoted`
- pitfall 强制结构化 `trigger_key`(`tool + command_pattern + path_globs`)
- **异步卫生 job**:dedup(Jaccard>0.7)/ 降权 / 冲突标记
- **写入安全网**(吸收 spike-005):敏感过滤 / 路径泛化 / 长度 ≤500 / 频率控制 / `source_ref` 溯源

### 召回(两层 + 两套检索)
- **层 1 · session 开始**(`chat_loop.rs:537 build_instructions_blocks`):FTS5(title+content+tags,bm25)+ scope/project 过滤,注入同一 synthetic user message,token ≤500
- **层 2 · 工具执行前**(`permissions/check.rs` Tier1):`trigger_key` 精确匹配 pitfall,verified 强命中 → 软拦截重判 / active 弱命中 → 注脚兜底

### 心智模型
记忆是 **经验非规则**:注入措辞降格为提示,矛盾记忆共存明示,agent 当下裁决。

## schema 概要(完整 DDL 见 P1)

`autonomous_memories` 表(scope/kind/status/title/content/tags/trigger_key/source_ref/confidence/hit_count/last_used_at/...)+ `autonomous_memories_fts`(FTS5 虚拟表)。id 自增 + memory_id UUID 分离(FTS5 content_rowid 需整数)。

## Hook 落点(5 处,详见 spike-007 §6)

| 点 | 位置 | 复用管线 |
|---|---|---|
| A · session 开始召回 | `chat_loop.rs:537` | instruction blocks + cache_control |
| B · 工具执行前召回 | `permissions/check.rs` Tier1 | 5-tier 工具执行前拦截链 |
| C · 连续失败→成功监听 | `chat_loop.rs:1717` emit_tool_result | ToolResultPayload 信号 |
| D · turn 结束(预留) | `chat_loop.rs:1406` persist_turn | v1 不做 |
| E · DB 加表 | `migrations.rs:55` | subagent_runs 建表范式 |

## 阶段依赖图

```
P1 存储底座(无依赖,基础)
 ├─► P2 手工读写闭环(session 开始召回 + remember tool + UI)
 ├─► P3 工具执行前召回(trigger_key + 注脚)
 │     └─► P5 质量层(verified 软拦截 + 状态机晋升 + 卫生 job)
 └─► P4 事件驱动自动写入(旁路 reflection)
```

执行序:P1 → P2(最小可见价值)→ P3 → P4 → P5。P2/P3/P4 均只依赖 P1,可灵活排序。

## 关键决策(4 轮探讨定档 + 吸收)

1. 召回主力 = **背景召回注入**(非纯 agent 主动 recall)——解决"未知的未知"
2. 写入 = **agent 全自主写 + 显式 remember**——不做 Tier4 ask 审批(分歧于 spike-006)
3. 工具执行前命中分档 = verified 软拦截重判 / active 注脚兜底
4. pitfall 强制结构化 `trigger_key`(精确召回)
5. 吸收 spike-005 写入安全网 + spike-006 FTS5/前端落点;**不**吸收 spike-006 Tier4 ask / 纯主动模型(详见 spike-007 §11)

## v1 边界(明确不做,留 v2)

- 向量库 / embedding(tag + trigger_key + FTS5 够 v1)
- LLM-judge 写入过滤
- session 结束整体 reflection
- global 记忆层(user / project 两级先)
- `recall_memory` 主动深挖 tool(留扩展位)

## Acceptance Criteria(epic 级,各 child 有自己的细化)

- [x] P1-P5 全部 child 完成 + archive(5/5 done)
- [x] 端到端:连续失败→成功 → 自动产出 pitfall → 另一 session 跑同类命令 → 工具执行前命中 → 第一时间规避(verified 软拦截)——代码层面 P3↔P4↔P5 三层闭环 + 集成测试(`agent_loop_p5_soft_block_*`)锁定;真实 LLM 手动跑 app 验证 deferred
- [x] 记忆库可经 UI 查看/删除/pin(MemoryPreview 扩展,P2)
- [x] 召回不污染主 loop(token 预算 `RECALL_TOKEN_BUDGET=500` + trigger_key 精确匹配,精确率优先)
- [x] **实现后**落 `.trellis/spec/backend/`(`memory.md` Scenario 2 补 P5 contract + 修正 4 处过时,2026-06-29)

## Out of Scope

- daemon 化 / 多进程记忆共享
- 跨用户 / 多设备记忆同步
- 记忆自动晋升为指令文件(CLAUDE.md)
- 记忆的 LLM 语义压缩 / 重写

## 关联

- 完整设计:[docs/spikes/007-agent-autonomous-memory-plan.md](../../../docs/spikes/007-agent-autonomous-memory-plan.md)
- 同主题另两份(吸收对比见 spike-007 §11):[005](../../../docs/spikes/005-agent-memory.md) / [006](../../../docs/spikes/006-agent-autonomous-memory.md)
- child tasks:`06-29-am-p1-storage` / `06-29-am-p2-readwrite` / `06-29-am-p3-tool-recall` / `06-29-am-p4-event-reflect` / `06-29-am-p5-quality`
