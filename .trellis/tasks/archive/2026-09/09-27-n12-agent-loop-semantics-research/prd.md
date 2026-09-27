# N12 调研:agent-loop 语义吸收三小件(重试复用装配/装配期取消/压缩防死循环)

## Goal

按 BACKLOG 附录 C.2 立项前调研要求,核验 dsh 调研衍生的三条 agent-loop 语义(重试复用装配 / 装配期取消不落半准入 / 压缩恢复防死循环)与本仓库现有实现的差距,产出「现状缺口 + 改动面 + 取舍」三段式结论,回填附录 C,为 N12 是否立项及范围裁定提供依据。

## Requirements

- 逐条对照 LIFECYCLE 16 关卡 + `agent-loop-architecture` 系列 pattern spec + 代码实读(非只读 spec):
  - A5+ `retry_open` 的重试粒度与重装配行为;
  - C1 取消在装配期(压缩块)的落库行为、user 消息落库时机;
  - C3+ 压缩降级链、`CompactionRegistry` 熔断信号维度、StillOver 后跨 turn 行为;
- 明确「表面替换世代」在本仓库的等价物(cutoff_seq 水位?)及防死循环判定映射;
- 结论回填 `docs/BACKLOG.md` 附录 C N12 行。

## Acceptance Criteria

- [x] `research/n12-semantics-gap-analysis.md` 产出三段式结论(现状缺口 / 改动面 / 取舍),每条判定附代码级证据(文件:行号)
- [x] 「表面替换世代」等价物明确(cutoff_seq 水位)且给出判定映射(水位推进 && 总量下降)
- [x] 三件各自给出实施/不实施建议与改动面量级
- [x] BACKLOG 附录 C N12 行回填调研结论

## 结论(供立项裁定)

- **件① 重试复用装配:已满足**(`retry_open` 克隆参数重发,循环内零装配;语义错误重试被事前硬卡 + 跨 turn 重装配设计替代)—— 降级为 1 条不变量测试锚。
- **件② 装配期取消:无严重形态**(唯一交错窗口 = 取消×摘要落库,良性)—— 一行收口(落库前 `is_cancelled`)+ spec 边界记录,或文档化。
- **件③ 压缩防死循环:真实缺口**—— 熔断只有「摘要 LLM 成败」维度,缺「压缩无进展」维度;不收敛分支存在(摘要净增长,既有熔断信号失明)。推荐方案 B(世代推进判定,~30-60 行),为 N12 立项主体。

**立项建议范围**:件③ 主体 + 件② 一行 + 件① 测试锚;单 PR 小任务,改动面 `drive.rs` C3 块 + `compaction.rs` registry + `pattern-llm-compaction.md`。

## Notes

- 调研产物:[research/n12-semantics-gap-analysis.md](./research/n12-semantics-gap-analysis.md)
- 本任务是 research 任务,不含代码改动;实施任务立项后另行创建。
