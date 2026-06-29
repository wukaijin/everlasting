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
- [ ] 软拦截死循环防护:同一 verified pitfall 一个 session 内只拦 1 次,二次命中降级注脚 + 正常执行(不卡到 MAX_TURNS)
- [ ] 卫生 job:相似度 >0.7 的两条 → 合并(hit_count 累加),不重复
- [ ] 长期不命中的记忆 → demoted,不再参与召回
- [ ] cargo test 全绿

## Technical Approach

方向 + 4 个原待决项已收敛定档,完整技术设计见 [`design.md`](./design.md)(§2 决策清单 / §3 两路 recall filter 纠正 / §4 软拦截数据流),执行计划见 [`implement.md`](./implement.md):

- **hint round loop 改法**(本 task 最复杂):verified + `trigger_key` 完全命中 → 短路 `execute_tool`、回灌 `is_error=false` 提示让 LLM 重判;**每坑每 session 软拦截 1 次**(session 级 HashSet 防循环),同坑二次命中降级注脚 + 正常执行
- **状态机晋升阈值**:candidate→active @ `hit_count≥2`;active→verified @ `hit_count≥5` 且创建满 3 天(v1 无跨 session 翻车信号,用存续时长代理"未翻车")
- **Jaccard**:char-trigram 集合,Jaccard >0.7 视为重复(零依赖、语言无关)
- **卫生 job 触发**:事件触发(`insert_memory` 后按计数 + app 启动一次),不引入长驻 interval(项目无此范式)
- **关键纠正**:P2 注释预期"P5 收紧 recall filter 到 ActiveVerifiedOnly"会掐断 candidate 晋升路径 —— P5 反而**保持/放宽** filter,靠低阈值快速晋升控噪(design.md §3)

## Out of Scope

- 向量检索(v2)
- LLM-judge 写入过滤(v2)
- 记忆自动晋升为指令文件(v2)

## 关联

- epic:[`06-29-autonomous-memory/prd.md`](../06-29-autonomous-memory/prd.md) · spike-007 §3 状态机 + §4 软拦截 + §10 待决项
- 技术设计:[`design.md`](./design.md) · 执行计划:[`implement.md`](./implement.md)
