# PRD — N12 压缩无进展熔断 + 摘要取消收口 + retry 测试锚

> 来源:BACKLOG 附录 C N12(dsh 对比调研衍生候选)。专项调研已完成(2026-09-27,task `09-27-n12-agent-loop-semantics-research`,`research/n12-semantics-gap-analysis.md`,下称【调研】),本 PRD 直接固化其立项建议,范围三件、单 PR 收口。

## 背景

dsh harness 三条 agent-loop 语义经对照核验后,落地价值集中在压缩链:

- **件③(实锤缺口,主体)**:C3+ 压缩熔断(`CompactionRegistry` 连续 3 次失败跳过摘要直达机械)只测「摘要 LLM 机制是否坏了」,不测「压缩是否无进展」。不收敛分支(摘要净增长 / 巨尾)下,`SummaryOutcome::Applied` 每轮 `record_success` 清零计数,用户每次重发都烧一次摘要旁路 completion,**永不收敛**——与关卡⑤硬卡「压缩后仍超预算」(`StillOver`)组合成真实的 token 浪费路径。
- **件②(良性交错窗口,顺手)**:摘要 LLM 成功返回后、`insert_compaction_summary` 落库后、请求发出前,存在毫秒级取消交错窗口——turn 取消但摘要行已落库、水位已推进。数据自洽但取消语义不纯。
- **件①(已满足,防退化)**:`retry_open` 循环内克隆重发、零重装配,但无测试锚钉住;未来若把重试上移到 chat_loop 层会无声退化(重跑装配)。

## 需求范围

### R1 件③:压缩无进展熔断(主体,【调研】方案 B)

- **进展判定**:C3 压缩块结束时,若「本 turn 进了摘要路径 &&(水位未推进 ‖ `tokens_after` ≥ `tokens_before`)」记一次 no-progress。判定量只用已有信号(`SummaryAnchor.cutoff` 与本 turn 新 cutoff 比较 + `tokens_pre`/`tokens_after`),零新估算。
- **熔断动作**:连续 2 次 no-progress → 粘性跳过摘要路径,直达机械丢组(后续 turn 仍 `StillOver` 则维持原 Error,不再烧摘要 LLM)。
- **解除条件**:水位推进即解除(新消息大量进待压区自然复位)——与 dsh「表面替换世代推进才允许重试」语义对齐,不用时间窗。
- **范围边界**:熔断只罩摘要路径;机械丢组无 LLM 成本照跑;不改变 `StillOver` fail-fast 形态(Error turn + abort 不发请求);与既有「连续 3 次失败」熔断正交(两维度并存,互不清零对方语义)。
- **落点**:`CompactionRegistry`(进程级 OnceLock 单例)加第二维度或 reason 字段;drive.rs C3 块尾收进展判定。

### R2 件②:摘要落库前取消检查(【调研】选项 A)

- `insert_compaction_summary` 落库前加 `is_cancelled` 检查,命中则丢弃已生成摘要、返回 `SummaryOutcome::Cancelled`(取消语义纯化;代价 = 白付一次摘要 LLM,窗口毫秒级、概率极低,接受)。
- 不破既有契约:取消不计熔断(`Cancelled` → 两维度都不计)。
- spec `pattern-llm-compaction.md` 记录该边界(取消×摘要交错:落库前的取消检查是有意行为)。

### R3 件①:retry 零重装配测试锚

- 单测:`MockProvider` 构造网络错误序列,记录每次 `send` 收到的 messages(如 hash),断言 retry 循环 N 次调用全等——钉住「循环内零装配」不变量。纯测试,零产品代码改动。

### R4 spec 回填

- `pattern-llm-compaction.md` 降级链补「无进展熔断」节:判定式、粘性、解除条件、与既有熔断正交关系;件②边界一段。

## 非目标

- 不做【调研】方案 A(`StillOver` 时也 `record_failure`——语义糊且防不住不收敛路径)、方案 C(仅观测标记)。
- 不改 `StillOver` fail-fast 形态、不动机械丢组降级路径。
- 不引入时间窗解除、不新增 token 估算。
- 不动装配期其他取消检查点分布(装配段无显式取消检查是现状架构,【调研】件②已判定无严重形态)。

## 验收标准

| # | 标准 | 验证 |
|---|------|------|
| AC1 | 不收敛场景(摘要 Applied 但水位未推进或总量未降)连续 2 turn 后,第 3 turn 不再调用摘要 LLM(直达机械) | 单测:MockProvider 断言摘要 completion 调用次数 |
| AC2 | 熔断期间水位推进(新 cutoff > 旧 cutoff)后,摘要路径恢复 | 单测 |
| AC3 | 既有「连续 3 次失败」熔断行为不变(与 no-progress 维度正交,`Failed` 计数不被 no-progress 干扰,反之亦然) | 单测 |
| AC4 | 摘要 LLM 成功后、落库前取消 → 摘要行不落库,turn 以取消收场;取消不计任何熔断维度 | 单测 |
| AC5 | `retry_open` 网络重试 N 次收到的 messages 逐次全等(零重装配锚) | 单测 |
| AC6 | 收敛路径回归不破:待压区逐 turn 蚀尽、正常 Applied 压缩、手动 `/compact`、既有压缩全套单测绿 | `cargo test -p everlasting --lib` 全量 |
| AC7 | spec `pattern-llm-compaction.md` 含无进展熔断节 + 取消边界段;与实现一致 | 文档评审 |

## 约束

- 改动面集中在 `app/src-tauri/src/agent/drive.rs`(C3 块)+ compaction registry 所在模块 + `llm/retry.rs` 测试 + spec 文档。
- C3 块是最热路径之一,回归必须全量钉(AC6)。
- 规模预估 ~30-60 行产品代码 + 单测(【调研】口径)。

## 依赖与风险

- 前置依赖:无。
- 主要风险:C3 块内判定时机(块尾 vs `Applied` 分支)影响「本 turn 进了摘要路径」口径——以【调研】行号证据(drive.rs:552/633/705-736/521)为准,实施时复核当前行号可能有漂移。
