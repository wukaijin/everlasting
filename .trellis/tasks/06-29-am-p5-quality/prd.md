# P5: 质量层 — verified 软拦截 + 状态机晋升 + 卫生 job

> child `06-29-am-p5-quality` · parent [`06-29-autonomous-memory`](../06-29-autonomous-memory/prd.md) · 详见 [spike-007 §3 状态机 + §4 软拦截分档](../../../docs/spikes/007-agent-autonomous-memory-plan.md)
> 前置:P3(注脚档已搭好 Tier1 拦截点)。质量收口,兑现"第一时间规避"(verified 软拦截)+ 库健康(状态机 + 卫生 job)。

## Goal

三块质量机制:
1. **verified 软拦截重判**:pitfall 升 verified 且 trigger_key 完全命中 → 回灌 pitfall 让 LLM 多想一轮(动 loop,兑现"规避")
2. **状态机自动晋升**:`candidate → active → verified`,靠 `hit_count` + 复核;老化/覆盖 → `demoted`
3. **异步卫生 job**:Jaccard >0.7 dedup 合并 / 低 hit+老降权 / 矛盾冲突标记

## In Scope

- verified 软拦截 hint round:P3 的 Tier1 召回命中 verified + 强匹配时,回灌提示让 LLM 重判(而非直接执行/注脚)
- 状态机晋升规则落地(消费 P1 `update_status`/`bump_hit_count`):candidate 被 recall 命中 → active;active 多次命中 + 未翻车 → verified
- 卫生 job(异步,不阻塞主 loop):dedup(Jaccard>0.7 合并,hit_count 累加)/ 降权 / 冲突标记
- 冲突记忆共存明示:注入时若发现矛盾,提示 agent"存在相悖记忆,请自行判断"(经验非规则)

## Acceptance Criteria

- [ ] pitfall 多次命中(hit_count 达阈值)→ 自动升 verified
- [ ] verified pitfall 工具执行前强命中 → 软拦截生效(LLM 重判,非直接执行)
- [ ] 卫生 job:相似度 >0.7 的两条 → 合并(hit_count 累加),不重复
- [ ] 长期不命中的记忆 → demoted,不再参与召回
- [ ] cargo test 全绿

## Technical Approach(方向,实现细节延后)

- **hint round 的 loop 改法**(本 task 最需设计的细节):verified 命中后如何干净地把"提示+原 tool_use"变成 LLM 重判的回合,不破坏 turn 结构/cancel 语义——实施时设计,spike-007 §10 待决项之一
- 状态机晋升阈值(hit_count 多少升 active/verified)——实施时定
- Jaccard 文本相似度实现(trigram? 分词?)——实施时定
- 卫生 job 触发时机(定时? 事件?)——实施时定

## Out of Scope

- 向量检索(v2)
- LLM-judge 写入过滤(v2)
- 记忆自动晋升为指令文件(v2)

## 关联

- epic:[`06-29-autonomous-memory/prd.md`](../06-29-autonomous-memory/prd.md) · spike-007 §3 状态机 + §4 软拦截 + §10 待决项
