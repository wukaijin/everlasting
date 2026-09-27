# N12 调研:agent-loop 语义吸收三小件 —— 现状缺口 / 改动面 / 取舍

> 调研日期:2026-09-27。对照材料:dsh 调研 [`docs/_history/research/deepseek-harness-survey.md`](../../../../docs/_history/research/deepseek-harness-survey.md) §1.3/§3.1;本项目 LIFECYCLE 16 关卡([docs/LIFECYCLE.md §2](../../../../docs/LIFECYCLE.md))+ `agent-loop-architecture` 系列 pattern spec + 代码实读。
> 三条语义原文(dsh survey §3.1):
> 1. **重试复用装配**:步内重试不再重跑 pre-step/prompt 装配,只重新 prepare + 派生请求。
> 2. **取消原子性**:prepare 或 stream 阶段取消,"commits neither system nor users"(部分准入不落盘)。
> 3. **压缩恢复防死循环**:上下文溢出后,只有剪枝/摘要确实推进了「表面替换世代」才允许重试,否则维持原错误。

## 结论速览

| 件 | 现状判定 | 增量价值 | 建议 |
|----|---------|---------|------|
| ① 重试复用装配 | **已天然满足**(网络重试维度);语义错误重试维度被事前硬卡**设计替代** | 薄 | 降级为测试锚:1 条不变量单测钉住「retry 循环零重装配」 |
| ② 装配期取消不落半准入 | 严重形态(消息重复/丢失/撕裂)**不存在**;唯一交错窗口 = 取消×摘要落库(良性) | 小 | 落库前一行 `is_cancelled` 检查,或显式文档化为有意行为 |
| ③ 压缩恢复防死循环 | **真实缺口**:熔断只有"摘要 LLM 成败"维度,缺"压缩无进展"维度;不收敛分支存在 | **实** | 立项主体:「世代推进判定」防死循环收口(cutoff_seq 为世代等价物),约 30-60 行 |

**总评**:N12 立项范围应从"三小件并重"收窄为「件③为主体 + 件②一行收口 + 件①测试锚」。

---

## 件①:重试复用装配 —— 已满足,设计替代成立

### 现状(核验证据)

`llm/retry.rs:186 retry_open`:重试循环体内是 `provider.send(system.clone(), messages.clone(), tools.clone())` —— 同一装配产物克隆重发,**循环内零装配步骤**。装配(context 构造 / memory 加载 / @文件展开 / C3 压缩 / budget gate)全部发生在 chat_loop 调 `retry_open` 之前(关卡⑤ → ⑥,LIFECYCLE §2.2)。

- 可重试分类 = `Network` / `Server` / `RateLimit`(spec [scenario-retry-backoff §2](../../../../.trellis/spec/backend/llm-contract/scenario-retry-backoff.md));`InvalidRequest`(4xx 非 429)**不重试** → `had_error` 路径 Error turn。
- 首字节后永不重试(安全不变量 R3):tool 副作用只可能在流完成后,首字节前重发 side-effect-free。

### 与 dsh 语义的差距分析

dsh 的"步内重试"覆盖两类触发:网络瞬态 + **语义错误(context overflow → 压缩 → 重试同一步)**。本项目:

- 溢出场景**事前拦截**:压缩块 + budget gate(0.95 硬卡)在 send 之前,`StillOver` 直接 fail-fast 不发请求(drive.rs:705)—— 不存在"发了 400 才知道超"的路径,除非估算与 provider 实际计数失准。
- 失准兜底:4xx InvalidRequest → Error turn → 恢复靠**下一 turn** drive_turn 头部压缩块重跑(重新装配)。跨 turn 重装配是有意的(@文件按当前文件重展开 = 拿最新内容,budget-gate spec 明文「@文件每 request 按当前文件重展开,DB spans 必 stale」)。

**判定**:网络重试维度「复用装配」天然成立;语义错误维度被「事前硬卡 + 跨 turn 重装配」设计替代,且该替代更彻底(不发注定失败的请求)。无实施缺口。

### 改动面(若仍要收口)

1 条单测:构造 `MockProvider` 网络错误序列,断言 retry 循环内收到的 `messages` 与首次一致(比如在 mock 里记录每次 send 的 messages hash,断言 N 次调用全等)。价值 = 防未来把重试逻辑上移到 chat_loop 层(那会重跑装配)时无声退化。成本 < 30 行。

---

## 件②:装配期取消不落半准入 —— 无严重形态,一个良性交错窗口

### 现状(核验证据)

- **架构差异先行**:dsh 是 append-only 事件日志,装配期才决定"提交 system/users";本项目是持久消息架构,user 消息由 chat 命令 **pre-flight 落库**(chat_loop.rs:139-143 注释:`The chat command's pre-flight inserts those entries`),取消后保留——断电不丢消息的有意设计。**这不是缺陷,两架构的准入单位不同**。
- **取消检查点分布**(drive_turn 全文 grep):装配段(压缩块 drive.rs:454-730、budget gate、tools 链)**无显式取消检查**;取消可达点只有三处:① 摘要旁路 completion 内(`send_summary_completion` → `retry_open` 的 biased select → `SummaryOutcome::Cancelled`,drive.rs:2704,**不落库,干净**);② send 阶段主 select(drive.rs:1304);③ tool/C2+ ask 三臂 select(drive.rs:2293)。
- **DB 写原子性**:装配期唯一的 DB 写 = 摘要行 `insert_compaction_summary`(drive.rs:2742,单行单事务 insert,原子)。其余装配(折叠列表构建、folded vec)纯内存。

### 交错窗口(唯一实质发现)

摘要 LLM 成功返回后:clamp → 内存构建 folded → **insert 落库** → `Applied` → 机械兜底 → 到 send 处 cancel 命中 → turn 取消。即:**摘要行已落库、cutoff 水位已推进、但请求未发出**。留下状态 = 历史"已被摘要折叠"而 turn 无输出。

- 影响评估:**良性**。摘要行自洽(独立有效,`kind=compaction_summary`),下一 turn 水位替换正常消费;持久化压缩与手动 `/compact` 同语义(压缩本来就跨 turn 持久);数据无撕裂(单行原子)。
- 语义边界:与用户"取消 = 本 turn 一切无效果"的直觉有出入;与 dsh「取消不提交任何准入」不同——但 dsh 语义在持久消息架构里的直接移植会**丢已生成的摘要**(浪费一次旁路 completion),未必更优。

### 改动面(两个选项)

- **选项 A(收口,一行)**:`insert_compaction_summary` 前加 `if token.is_cancelled() { return SummaryOutcome::Cancelled }`——取消时丢弃已生成摘要。代价 = 白付一次摘要 LLM 调用(取消本来就该尽快生效,这个窗口毫秒级,实际命中概率极低);收益 = 取消语义纯化。
- **选项 B(文档化,零代码)**:在 `pattern-llm-compaction.md` 加一行「取消×摘要交错:落库先行是有意行为(压缩独立于 turn 生命周期),SummaryOutcome::Cancelled 仅覆盖 completion 中途取消」。
- **推荐 A**:窗口毫秒级但代码三行,取消语义纯化值得;顺手在 spec 记边界。

---

## 件③:压缩恢复防死循环 —— 真实缺口,立项主体

### 现状(核验证据)

降级链已完整(pattern-llm-compaction §降级链):摘要(retry_open 包裹)→ 摘要失败/熔断 → 机械丢组 → 仍超 `StillOver` fail-fast(Error turn + abort,不发请求,drive.rs:705-736)。

熔断 `CompactionRegistry`(连续 3 次失败跳过摘要直达机械)的**信号维度**:

- `SummaryOutcome::Applied`(落库+回填双成功)→ `record_success` 清零(drive.rs:552)——**哪怕随后机械兜底产出 StillOver**;
- `SummaryOutcome::Failed`(LLM 错误/空输出/落库失败)→ `record_failure`(drive.rs:633);
- `SummaryOutcome::Cancelled` → 不计(正确);
- **StillOver 分支本身不碰 breaker**(drive.rs:705 只 `record_compaction` trace 观测)。

即熔断只测「摘要机制是否坏了」,**不测「压缩是否无进展」**。

### 缺口推演(跨 turn 循环路径)

场景:巨尾(保留区单条消息 > 窗口)或摘要净增长。用户在 StillOver abort 后重发:

1. drive_turn 头部压缩块重跑:超 0.85 触发线 → 待压区非空(cut > synthetic_prefix_len)→ **又烧一次摘要 LLM**(熔断没触发:上一轮 Applied 已 record_success 清零)→ 摘要后仍超 → 机械兜底 → StillOver → abort。
2. 待压区逐 turn 蚀尽后(cut == synthetic_prefix_len,drive.rs:521 直走机械)→ 稳定 StillOver abort,**不再烧摘要**。

**两条路径**:

- **收敛路径**(常态):逐 turn 烧摘要,直到待压区耗尽。每 turn 一次旁路 completion 的 token + 秒级延迟,收敛但浪费。
- **不收敛路径**(真死循环):pattern 已知边界「待压区极小时摘要正文可能比被压内容更胖(context 净增长)」。水位推进 + tokens_after ≥ tokens_before → 下一 turn 仍超触发线、待压区仍有料 → 每次用户重发烧一次摘要 LLM,**永不收敛**。现熔断信号对此完全失明(Applied = success)。

### 「表面替换世代」等价物

本仓库等价物 = **cutoff_seq 水位**(摘要行 metadata 的被压区末行真实 seq,水位折叠点)。dsh 判定「世代推进才允许重试」映射为:**本 turn 压缩后(水位推进 && 总量下降)才允许下一 turn 再进摘要路径;无进展则粘性跳过摘要,直达机械 + StillOver Error(维持原错误,不再烧 LLM)**。

### 改动面(三方案)

| 方案 | 内容 | 规模 | 缺点 |
|------|------|------|------|
| A 最小 | StillOver abort 时也 `record_failure` | ~5 行 + 单测 | 语义糊:摘要机制没坏,是输入无解;且摘要在 StillOver 前成功会 record_success 清零,计数永不满 3,防不住不收敛路径 |
| **B 直译(推荐)** | 无进展熔断独立信号:C3 块结束时,若「本 turn 进了摘要路径 && (水位未推进 ‖ tokens_after ≥ tokens_before)」记 no-progress;连续 2 次 → 粘性跳过摘要直达机械(StillOver Error 维持原错误);水位推进即解除(新消息大量进待压区自然复位) | ~30-60 行 + 单测(CompactionRegistry 加第二维度或同 registry 加 reason 字段) | 需要在 drive.rs C3 块尾收一个进展判定,动最热路径之一,要钉全回归 |
| C 保守 | 仅观测:StillOver 时记专属 audit/trace 标记 | ~10 行 | 不解决烧 token,只是可诊断 |

**推荐 B**。要点:

- 进展判定用已有量:`SummaryAnchor.cutoff`(驱动侧已知 prior)与本 turn 新 cutoff 比较 + `tokens_pre`/`tokens_after` 比较,零新估算。
- 粘性解除条件用水位推进(而非时间),与「世代」语义严格对齐。
- 熔断范围只罩**摘要路径**(机械丢组无 LLM 成本,照跑),不改变 StillOver fail-fast 形态。
- 单测面:收敛路径(无进展 ×2 后不再调摘要 LLM)、复位路径(水位推进后恢复摘要)、与既有"连续 3 次失败"熔断正交。

### 关联既有机制

- 熔断 registry 已是进程级 OnceLock 单例(pattern §降级链 4),第二维度落点现成。
- 件② 的取消检查若同批做,`SummaryOutcome` 增加一个变体或在 Cancelled 语义内消化,注意「取消不计熔断」既有契约不破。

---

## 立项建议(回填 BACKLOG 用)

N12 实施范围收窄为:

1. **主体**:件③ 方案 B —— 压缩无进展熔断(世代推进判定,cutoff_seq 为世代等价物);
2. **顺手**:件② 选项 A —— 摘要落库前取消检查(一行)+ spec 边界记录;
3. **测试锚**:件① —— retry 零重装配不变量单测。

规模:小任务(单 PR 可收);改动面集中在 `drive.rs` C3 块 + `compaction.rs` registry + spec `pattern-llm-compaction.md`(降级链加"无进展熔断"节)。前置依赖:无。
