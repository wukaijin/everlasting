# 统一 token 预算表 + 关卡⑤硬卡(上下文预算统一治理)

## Goal

把上下文窗口从"各机制自治"变为"一张统一预算表管到底":每轮请求发出前统一估算请求总占用,超预算时在关卡⑤(send 前最后一道检查)按既定优先级静默裁剪,裁剪行为全程可观测(实发口径),裁尽仍超才 fail-fast。

**用户价值**:长 session 不再出现"窗口被低价值内容(超大 @文件、历史重复图)静默挤满 → 模型质量劣化/丢指令";@文件度量盲区闭合;压缩触发线漏计 tools/system 的口径洞修复;所有裁剪有 audit/trace 可查。

**来源**:BACKLOG §3.1 缓解手段后两条("统一 token 预算表" + "关卡⑤硬卡"),三切片度量齐备后 BACKLOG 明确标注"可排期"。

## Background(代码探索 2026-08-19 + start 前评审修正,锚点相对 app/src-tauri/src/ 与 app/src/)

### 请求构造链(关卡⑤落点)

- 每 request 一次 `prepare_loop_state`(`agent/chat_loop/init.rs:92`):C3 水位替换(`init.rs:446`)→ memory 指令块头对(`init.rs:482`)→ skill listing(`init.rs:561`)→ system prompt(`init.rs:626`)→ @文件注入(`at_file::inject_at_tokens`,`init.rs:839`)→ 附件转 ImageRef(`init.rs:848`)。
- 每 turn 一次 `drive_turn`(`agent/chat_loop/drive.rs:82`):head_sha/system 刷新(`drive.rs:180-190`)→ C3+ 压缩块(`drive.rs:215-453`)→ turn_messages APPEND 组装(`drive.rs:537-724`,checklist/后台 shell 通知/recall/breadcrumb 均为**独立合成消息追加**,不改既有 message 文本)→ 图片 resolve(`drive.rs:735`)→ tools 过滤链+stubify(`drive.rs:752-826`)→ tools_token 估算(`drive.rs:851`)→ `provider.send(system, messages, tools)`(`drive.rs:891-901`)。

### 关键结构事实(评审 F1 修正后的口径)

- **memory 指令块头对与 skill listing 是合成消息,插在 messages 里**(`init.rs:512-534` User/Assistant 头对 insert(0/1);skill listing 独立合成 user message,`init.rs:536+`);@文件展开文本与图片块也在 messages。因此 `estimate_messages_tokens(&messages)` **已含 memory/skill/@文件/图片/历史**。
- **口径洞的真身**:C3+ 触发判断(`drive.rs:221-231`)的 `tokens_pre = estimate_messages_tokens(&messages)` 漏计的是与 messages **并列发送**的 **tools[] 与 system prompt**(stub 后 tools ~3.7k、全量 ~6.8k,system ~2-3k)。大窗口(200k)下 0.85 线余量通常吸收得了;**小窗口模型(32k/64k)下占比可观**,请求可能在 messages 未达触发线时整体超窗。机械 `compact_messages`(`drive.rs:384`)无 gate(群聊/worker 也保护),口径同此。
- **@文件注入是每 request 重新展开的请求副本**(`init.rs:825-830` 注释:DB 存原始 `@relpath` 为 SoT,重载后按**当前文件内容**再展开;`inject_at_tokens` 就地改内存 messages,`at_file.rs:298` 遍历全部 user message)。注入链无任何 token 计数,唯一约束是 read_file 的 50KB 字节截断(`tools/read_file.rs:201`);manifest 只记行数(`at_file.rs:470`),图片 `tokens_est`(`at_file.rs:485`)仅展示。**推论:裁剪/度量所依赖的注入区间标记必须是同请求内的临时产物,落 DB 必 stale。**
- 既有三切片度量(tools/memory/images)为本地 cl100k 估算(`memory/tokens.rs:50`)、请求前计数、`ChatEvent::Done` 落 `turn_trace`(`db/trace.rs:96`,worker `skip_persist` 不写);`context_input_tokens` 来自 provider usage(事后值)。`estimate_messages_tokens` 对无精估尺寸的图片 pad 6400 字符 ≈ **1600 tok/张**(`context.rs:234/237`,精估值走 attachments 元数据)。
- config 惯例:DB `app_config` KV(`db/config.rs:37`),fail-open 缺省开;豁免口径统一 `!worker && !群聊`(digest `init.rs:420-436` / stub+元工具 `drive.rs:772/824/832`)。
- 前端:`TurnCard.vue` 三 cell(`:365-404`),整卡 `contextUtilPct`(`:139-151`)硬编码 `CONTEXT_WINDOW_REF = 200_000`,非 per-model;`contextWindow` 前端字段已有(ChatInput.vue:167)。

## Requirements

### WP1 统一度量与口径

- **R1 @文件切片度量 + 注入区间标记**:`inject_at_tokens` 扩展——对全部 user message 的注入正文 `count_tokens`,并返回**每条消息内**的注入区间(spans:`{msg_idx, start, end, path, tokens}`,同请求临时产物经 loop state 传递,**不落 DB**);聚合值落 `turn_trace.at_files_token` 新列;TurnCard 新 cell。
- **R2 system 切片度量**:system prompt 本体 + skill listing 合成消息计数(归因口径,物理上 skill listing 在 messages 内),落 `turn_trace.system_token`;至此归因 breakdown 完整。
- **R3 统一总量估算与口径修正**:`budget.rs::estimate_request_tokens(system, tools_json, messages)` = count_tokens(system) + count_tokens(tools_json) + `estimate_messages_tokens(messages)`——**messages 已含 memory/skill/@文件/图片/历史,不再单独加计任何切片**(评审 F1);C3+ 触发线(0.85)、摘要 postcheck(0.95)、机械 `compact_messages` 的"当前占用"三处全部切换到该总量。tools 过滤链+估算需在压缩判断前完成(时序重排,见 design §3)。
- **R4 预算可观测(实发口径)**:`turn_trace.context_window` 新列(请求时窗口快照);TurnCard 弃用硬编码改行内值(NULL 回退 200_000);预算行 = **实发**统一总量 vs 窗口 + 归因切片占比条(残差 = 总量 − tools − memory − @files − images − system,钳 0)——trace 各 token 列一律记**裁剪后实发值**(= 预裁值 − 各臂 freed 的算术差,见 R8),与 provider `context_input` 可比。
- **R5 烟测**:`scripts/turn-smoke.sh` 报告列扩展(at_files/system/context_window)。

### WP2 关卡⑤硬卡

- **R6 预算门**:drive_turn 内 send 前统一估算 > `0.95×window` 触发静默裁剪;gate = `context_budget_enabled`(DB config,fail-open 缺省 on)&& `!worker && !群聊`。
- **R7 裁剪引擎(优先级既定,非破坏性——只作用于请求副本,不动 DB/registry,下轮重算)**:
  1. 旧轮次 @文件正文(非当前 turn 注入)→ 按 R1 的同请求 spans 在副本上替换为占位行(spans 失配时跳过该 span,fail-open 不裁);
  2. 旧轮次历史图片 → 降级为 B1 占位文案先例(模型知有图未发,防幻觉);
  3. memory 已加载节 → 本请求指令块视图回退目录态(`MemoryDigestRegistry` 不动,窗口持续紧则每轮等效回退);
  4. 当前 turn 注入的 @文件/图片**不裁**(是要处理的活)。
- **R8 裁剪可观测(实发口径)**:`AuditKind::ContextBudgetTrim`(enum 变体无 migration,payload `{arms: [{kind, count, tokens_freed}], over_by, pre_total, post_total, window}`)+ 非持久化 `ChatEvent`(前端瞬时提示,`Retrying` 先例)+ TracePanel 徽标;trace 各切片列按"预裁 − freed"改记实发值。
- **R9 兜底**:裁尽仍超线 fail-fast,错误信息含各切片 breakdown(对齐 RULE-A-002 StillOver 语义,stop_reason=`context_over_budget`)。

## Acceptance Criteria(实施验收 2026-08-19)

- [x] AC1 单测:统一总量 = system + tools + messages 三部件之和;归因切片互不重叠且之和 ≤ 总量(`budget::tests::estimate_request_tokens_equals_three_parts_sum` + `attribution_slices_inside_messages_do_not_exceed_total`)。
- [x] AC2 触发线口径:tools+system 挤窗场景在统一总量 > 0.85 时触发压缩——单测半边(`tools_and_system_overhead_crosses_trigger_when_messages_under`,extra=0 旧口径不触发对照)+ 集成半边(`tests_agent_loop/budget.rs::tools_and_system_squeeze_triggers_mechanical_compaction`,自校准夹具环境无关)。
- [x] AC3 硬卡裁剪:enforce_budget 单测六件覆盖优先级/非破坏性/当前 turn 保护/APPEND 稳定/失配 fail-open。**范围注记**:全 loop 逼出臂级行为在所有确定性构造下会被 C3 StillOver(0.5 target)抢先中止,闸门真实触发面是压缩后 APPEND 增长——接线以 no-misfire 集成锁(常态请求零干扰),臂级语义以单测锁。
- [x] AC4 可观测(实发):audit + ChatEvent + TurnCard 预算行/占比条;`context_window` 行内值驱动(32k 窗口 50% vs 旧行回退 8% 测试锁定)。
- [x] AC5 裁尽仍超 → fail-fast:Error turn + `context_over_budget` 标识 + 各切片 breakdown(单测锁文案;形态对齐 RULE-A-002 Error+abort,stop_reason 语义在错误文本中)。
- [x] AC6 gate:worker/群聊豁免(gate 同 digest/compaction);开关缺省 on;no-misfire 集成锁常态零干扰。
- [x] AC7 live 烟测:重编 daemon 实跑——system_token=795 / context_window=200000 / at_files NULL(零注入语义正确)三新列落值,tools 3996 / mem 2080 与基线吻合。WP2 臂级 live 触发不可构造(见 AC3 注记),观测链路(chip/徽标/审计)有前端测试覆盖。
- [x] AC8 全量验证:后端 1869+1 既有 flaky(subagent guard,复跑即过,与 main 基线同款);前端 1122 全绿;vue-tsc 0 err;clippy 零新增(4 个基线既有,stash 对照核实);fmt 干净。

## Key Decisions

- **D1 任务形态**:单任务两 WP,不拆度量先行阶段(2026-08-19 用户定)。
- **D2 超线行为**:静默裁剪 + 审计 + 裁尽 fail-fast 兜底,不走软询问/直接拒发(2026-08-19 用户定)。
- **D3 裁剪优先级**:旧轮次 @文件 → 旧轮次历史图 → memory 已加载节;当前 turn 注入不裁;数据先于指令(2026-08-19 用户定)。
- **D4 预算模型**:全局线 + 优先级表,不暴露 per-slice 配额 config;预算线 0.95 对齐 `SUMMARY_POSTCHECK_RATIO`,压缩 0.85 触发线改统一口径后仍是第一道防线,硬卡是最后防线。
- **D5 豁免口径**:硬卡 gate 跟随 digest/compaction(`!worker && !群聊`);机械压缩保持无条件(口径修正惠及群聊/worker)。
- **D6 非破坏性**:裁剪只改请求副本(`turn_messages` clone + 指令块副本),DB messages / StubRegistry / MemoryDigestRegistry 均不动,下轮重算。
- **D7 时序重排**:tools 过滤链 + 估算挪到压缩判断前(纯集合操作,依赖仅 head_sha 刷新,与压缩块零依赖)。
- **D8 估算口径(评审 F1)**:总量按**发送部件**加法(system + tools + messages),归因切片从 messages 内部归因——两类口径分开,永不互相加计。
- **D9 trace 实发口径(评审 F2)**:trace 记裁剪后实发值(预裁 − freed),预算行与 provider usage 可比;预裁值只活在 audit payload。
- **D10 spans 同请求临时性(评审 F5 裁定)**:注入区间标记由注入过程在本请求内产出并消费,不落 DB——@文件每 request 重展开,DB spans 必 stale;失配 fail-open。

## Out of Scope

- worker turn_trace 度量盲区(`skip_persist` 不写行)——独立小额 follow-up。
- provider usage 精确值反向校准本地估算(偏差治理)。
- per-slice 配额 config knobs / 注入期 @文件预防性上限(硬卡事后裁剪已覆盖,注入期上限另评)。
- 群聊/worker 硬卡接入;softcap 第四臂 handoff 联动(另案)。
- Anthropic cache 断点(R2,等原生 provider)。

## Risks / Deferred

- **触发线口径修正的回归面**:统一口径使 0.85 更易触发(补上了 tools+system)——有意修复,但既有压缩测试需全量复跑校准;单测锁"messages 部件自身超线"场景行为不变。
- **tools 过滤链时序重排**涉及 drive.rs 主干,需 stub 粘性 registry 语义回归测试。
- cl100k 本地估算与 provider 计量存在系统性偏差(无精估图片 pad ≈1600 tok/张)——预算线 0.95 留 5% 余量吸收。

## Review Resolution(start 前评审 2026-08-19,独立核实后处置)

- **F1(采纳,高)**:原估算公式重复加计 memory——核实 `init.rs:512-534` memory 头对确在 messages 内;连带修正 Background 口径洞描述(漏计的只有 tools+system,非 memory/@文件/图片)。公式改三部件加法(D8)。
- **F2(采纳)**:trace 预裁/实发未定义——裁定 D9:trace 一律实发(预裁 − freed 算术差),预算行与 `context_input` 可比。
- **F3(采纳)**:ZCode 属 sub-agent-dispatch 平台(`workflow.md:186`),inline 豁免为 Codex-only——已 curate `implement.jsonl` / `check.jsonl`(各 ≥3 条真实 spec 条目)。
- **F4(采纳)**:图片 pad 640 → 6400 字符 ≈1600 tok(`context.rs:234`),PRD 已改。
- **F5(部分采纳)**:span 偏移鲁棒性担忧方向正确,但核实 @文件为每 request 重展开(`init.rs:825-830`),**原设计"注入时落 DB metadata spans"不可行(评审未点破此层),评审建议的"按 manifest 重建定位"需重读文件、贵且有竞态,均不采纳**;裁定 D10 同请求临时 spans + 失配 fail-open + AC3 追加 APPEND 用例(采纳)。
- **F6(采纳)**:AC8 基线改"以 start 时 main 实测为准"。
