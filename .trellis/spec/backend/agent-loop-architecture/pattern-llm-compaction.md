# Pattern: LLM 摘要式上下文压缩(C3,2026-08-18)

> 来源:任务 `08-18-llm-context-compaction`(PRD/design/review 走
> `.trellis/tasks/08-18-llm-context-compaction/`,现实现
> `agent/compaction.rs` + `drive.rs` C3 块)。

C3 压缩从"机械丢组"升级为**两层结构 + 兜底链**:超触发线(0.85×window)时,
被压区由 LLM 生成 handoff 式结构化摘要回填,近期消息(保留区,15-25k token)
逐字存活;摘要失败/仍超时降级机械丢组(原 `compact_messages` 行为不变)。

## 数据契约(load-bearing)

- **摘要行** = 普通 `messages` 行,`role='user'`,`metadata.kind =
  "compaction_summary"`(无 migration,复用 B1 的 metadata 列)。
- **content 与 text 两列必须同值写纯摘要正文**(对齐锚在 text 列 ——
  rehydrate 回发 `text` 列原文;in-context 折叠从 content 重建)。
  回填前缀话术(SUMMARY_CONTEXT_PREFIX)只加在 in-context 构建时,
  **绝不落库**(评审 P1-2:进 `<prior-summary>` 会滚雪球 + 污染 D2 搜索)。
- **cutoff_seq = 被压区末行的真实 seq(精确值)**,是水位折叠点;
  `preserve_from_seq = cutoff + 1`。**禁止"摘要行 seq-1"近似** —— 那是
  当前输入行的 seq,折叠会吞掉保留区(PR2 check P1)。
- **水位语义**:context = `[最新摘要行] + [seq > cutoff 且 kind ≠
  compaction_summary 的行]`。保留区/当前输入/后续回答天然跨请求存活;
  旧摘要行被增量合并吸收(kind 过滤防重复出席)。
- **D3 自愈**:`edit_user_message` cascade 删掉摘要行 → 倒序"找现存
  最新"自然回退次新水位,零专门代码。

## 对齐与 seq 契约

- **水位替换查 DB,不信 wire**(wire 层 `ChatMessage` 无 metadata):
  `apply_compaction_watermark(wire, db_rows)`;对齐前提 = 前端
  `reloadAfterFinalize` 保证 wire 与 DB 行 1:1;idx±1 内容重对齐容忍
  单行位移;彻底失败 → `watermark_miss` warn + fail-open(回 main 行为)。
- **cutoff 精确计算**(`compressible_cutoff_seq`):无折叠 =
  `db_rows[cut - synthetic_prefix_len - 1].seq`;有折叠 = `seq >
  prior.cutoff 且 kind≠summary` 的过滤后缀数行;退化(待压区只剩 prior
  摘要)= 传递 prior.cutoff。对齐失效 → Err → 摘要失败路径(不白付
  completion)。
- **摘要行 insert 吃 loop 的 seq 游标、返回推进值**,绝不独立
  `MAX(seq)+1`(messages 主键 `(session_id, seq)` 会与 loop 后续
  persist 撞号);`permission_ctx.turn_seq` 重指到 assistant 行保持
  审计引用准确。
- **prior-summary 检测用 `SummaryAnchor` 循环内穿参**(`DriveTurnOutcome`
  线程模式,同 `loop_hit_count`),**不用位置猜测** —— 合成头布局随
  memory/skills 有无漂移(摘要实际落位 1/2/3)。

## Gate 与 scope

`llm_compaction_enabled`(config 缺省 on,fail-open)&& `!effective_is_worker`
&& `!group_chat`(session_type 判定,同 memory digest gate 口径,init.rs
一处罩三路径)。worker/群聊不进水位替换与摘要路径。

## 降级链与熔断

1. LLM 摘要(旁路 completion:无 tools、禁 thinking 采集、4k 输出兜底、
   `retry_open` 包裹;`Ok(ChatEvent::Error)` 与 `Err` 都算失败 —— 漏
   接 Ok(Error) 会把半截文本当完整摘要落库);
2. 摘要失败/空输出/落库失败 → 机械丢组(`compact_messages` 原样);
3. 摘要后仍 > 0.95×window(巨尾)→ 机械兜底 → 仍超 `StillOver`
   fail-fast(RULE-A-002,不发请求);
4. 熔断:`CompactionRegistry`(进程级 OnceLock 单例,同
   `memory::digest::registry()` 先例 —— `run_chat_loop` 24+ 参签名是
   硬约束,AppState 穿不进 loop),连续 3 次失败跳过摘要直达机械,成功
   清零,`delete_session_inner` 清理。

## 观测

`CompactResult.method`(`Summary`/`Mechanical`/`None`)+ `summary_usage`;
`compaction_json`(trace.rs **手工 json!**,扩字段要动 Rust + record +
TS 三处)带 method/summary_usage;摘要 usage **不混入**
`update_last_turn_usage`(主 turn 口径);TS 侧 `?? "none"` 兼容旧回看行。

## 边界情况

- 空待压区(cut == synthetic_prefix_len)→ 直走机械;
- 摘要后机械兜底真丢了消息 → anchor 置 None(防同 loop 二次压缩误把
  摘要消息当 anchor 跳过 transcript 输入);
- 图片:被压区退出 context(transcript 渲染 `[image attached: <file>]`
  占位);保留区照常;`images_token` 口径自动跟随请求内容。
