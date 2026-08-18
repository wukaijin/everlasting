# Pattern: LLM 摘要式上下文压缩(C3,2026-08-18)

> 来源:任务 `08-18-llm-context-compaction`(PRD/design/review 走
> `.trellis/tasks/08-18-llm-context-compaction/`,现实现
> `agent/compaction.rs` + `drive.rs` C3 块)。手动 `/compact` 入口
> 由 `08-18-manual-compact-command` 增补(见文末 §手动入口)。

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

## 手动 /compact 入口(08-18-manual-compact-command)

`compact_session` 命令(`commands/sessions.rs` `_inner` + Tauri +
daemon route + FE `CMD_TO_DOMAIN` 四处注册)→ `agent/compaction.rs::
run_manual_compaction`(空闲期编排,与 auto 路径共享 prompt/保留区/
落库/熔断纯函数)。与 auto 的契约差异:

- **seq = MAX(seq)+1 仅在空闲期合法**:命令层查
  `session_active_request` 拒绝 in-flight(streaming 中不排队不取消,
  toast 引导先 Stop)后,才无并发 persist、无撞 `(session_id, seq)`
  主键风险——active loop 内仍必须吃 loop 游标(上文 seq 契约)。
- **gate 链在命令层**:群聊(session_type)拒绝、
  `llm_compaction_enabled=false` 拒绝(回滚开关罩住手动——关掉时
  init 水位替换也停,写了摘要行无用);worker 不可从 session 行判定
  (请求期 flag),接受该限制(命令面向单聊会话)。
- **不受 0.85 触发线限制**(用户主动收窗),但空待压区(水位后
  常规历史全进保留区)→ `NothingToCompress` 报错,零 LLM 调用。
- **熔断**:不查 `is_tripped`(手动可解救 tripped session),失败照
  记 `record_failure` / 成功 `record_success`(与 auto 共享信号)。
- **失败 = 零 DB 写入 + 用户可见错误**(手动发生在 turn 边界外,无
  in-loop context 可修,机械丢组无持久化语义;下一次请求超线时
  drive_turn 既有降级链自然接管)。
- **metadata 增量**:`trigger: "manual"` + `focus`(直输 rest-of-line
  自由文本,注入 prompt 头部 `FOCUS INSTRUCTIONS FROM THE USER` 块,
  `build_compaction_prompt` 第 4 参,auto 路径恒 None);无 turn 上下文
  → 不写 `compaction_json`,观测靠 metadata + 命令响应载荷
  (`ManualCompactionOutcome`:cutoff/tokens before-after/usage/model)。
- **共用 helper**:摘要旁路 completion(retry_open + 剥壳)抽为
  `send_summary_completion`,auto(drive.rs)与 manual 同源。

已知边界(接受):待压区极小(历史几乎全在一个巨尾保留组)时,摘要
正文可能比被压内容更"胖"(context 净增长)——用户主动行为,后续
增量合并会吸收;auto 路径同构但罕见(触发线保证待压区一般较大)。

前端契约:builtin 命令集在 `resource_loader.rs BUILTIN_COMMANDS`
(含 `argument_hint` 字段),handler 统一 `ChatInput.vue::
executeBuiltin`(palette 选中与直输 `/xxx`+Enter 拦截
`utils/slashCommand.ts matchBuiltinCommandInput` 同落一处,两者
不漂移);成功后 `reloadSessionMessages`(= done 后 reload 同款
管线)让摘要行走既有 `kind=compaction_summary` 渲染。
