# BUGLIST-GROUP-CHAT — 群聊 headless 实跑缺陷跟踪

> **来源**:2026-09-05 经 daemon HTTP API(`:7456`)headless 开一场 6 人群聊实跑暴露的问题——session `082add5c-a98a-431a-936b-a764eea54ce5`(主题「Everlasting 目前还欠缺什么功能」,moderator MiniMax-M3,参与者 glm-5.3 / GLM-5.3-Flash / deepseek-v4-flash 混编),全程约 19 分钟 70 条消息,覆盖创建 → 开场 → 6 人发言 → 收官共识全链路。
> **甄别方式**:逐项对照源码考证(定位到行级根因)+ daemon 日志交叉 + HTTP/SQLite 只读抽查;区分「真缺陷 / 表现达标」。
> **用法**:同主 [BUGLIST.md](./BUGLIST.md) —— §2 是待修复清单,修复一项就把状态改 ✅ 并填提交引用;§3 本批判定表现达标,不再重开。编号 GC-x。
> **证据锚点**:session 永久在库(`load_session` 可复核 seq 级证据);转录导出 `out/group-chat-everlasting-gaps-20260905.md`(本地 `out/` 不入库);daemon 日志 `$XDG_STATE_HOME/dev.everlasting.app/daemon.log`(滚 3 代)。

---

## 1. 结论速览

| 判定 | 数量 | 条目 |
|---|---|---|
| A. 真缺陷,进 §2 跟踪修复 | 7 | GC1、GC2、GC3(P1);GC4、GC5(P2);GC6、GC7(P3) |
| B. 表现达标,关闭(§3) | 4 | 工具白名单零泄漏、role_history 隔离、权限拒绝自适应、中断自愈重试 |

主线判定:**群聊编排本身能跑通高质量讨论,短板集中在「讨论生命周期的可观测性」(GC1/GC2)与「无人值守适配」(GC3)**——这两组决定了群聊能否被可靠地自动化驱动;GC4/GC5 是体验与健壮性打磨。

> **2026-09-05 修复**:GC1-GC7 全部修复(同日提交,详见 §2 各条「修复」行)。回归锚点:后端
> `tests_group_chat`(busy 探针/熔断/max_rounds/前缀剥离/复用清空)、`tests_group_chat_prompts`
> (剥离/改写/检测纯函数)、`permissions/tests_ask`(无人值守快速拒)、`daemon/sse`(观察者跟踪)、
> `db/sessions_tests/session_crud`(生命周期列 round-trip);前端 `streamController.test.ts`
> (error 终结 + notice)。
>
> **2026-09-05 修复后 live 实跑验证**(session `60bcb778`,daemon 重启新二进制,moderator
> MiniMax-M3 + 3 参与者混编 anthropic/openai,两轮:整场讨论 ~16 分钟/31 条 + 同 session 快速
> 收官复用;转录 `out/group-chat-gc-fix-verify-20260905.md`,本地不入库):
> - **GC1**:全程 318 个轮询采样(含所有轮间空隙与 120s 权限等待)busy 回落 **0 次**;结束后
>   busy=false 不复活(旧二进制轮间必翻 false)。
> - **GC2**:结束后 sessions 行 `stop_reason=group_chat_end` 可查;第二轮复用时旧值先清后写
>   (live 验证复用清空语义)。
> - **GC3**:本场恰好有一次越界 grep 触发 ask;因 tunnel 远程浏览器在线(观察者在场),走了
>   attended 路径等满 120s + 人工 deny 应答通道正常——实证「GUI 在线窗口 ≥120s 不受影响」的
>   另一半契约(无人值守 8s 快速拒由单测锁定)。
> - **GC4**:34 行零仅前缀空消息;实跑抓到第一版「遇 thinking 即停」漏网缺陷并即日修正(见
>   GC4 条目补丁行)。
> - **GC5**:本场零错误轮,熔断未触发(正确);错误轮呈现/熔断语义由单测锁定。
> - **GC7**:`discussion_summary` 一等字段两次落库(第一轮完整共识清单 + 第二轮快速确认),
>   单次 session 查询直接可读。

---

## 2. 待修复清单(A 类)

> 状态:⬜ 待修复 · 🔧 修复中 · ✅ 已修复(填提交) 。按严重级别排序。

| 编号 | 级别 | 问题 | 根因位置 | 状态 | 修复提交 |
|---|---|---|---|---|---|
| GC1 | **P1** | `busy` 信号是轮次粒度而非整场讨论粒度,外部观测者误判「已结束」 | `group_chat_loop.rs:319,514` + `session_active_request` | ✅ | `5a64f8ee` |
| GC2 | **P1** | 讨论终止原因(stop_reason)不落库,事后不可查 | `db/trace.rs:35`(TurnTraceRow 无此列) | ✅ | `5a64f8ee` |
| GC3 | **P1** | 无人值守时权限审批死区:越界路径 ask 硬等 120s 超时按拒 | `permissions/ask.rs:20`(ASK_TIMEOUT 常量) | ✅ | `5a64f8ee` |
| GC4 | P2 | moderator 自指 `@` 前缀逐轮累积 + 产生仅前缀空消息 | `group_chat_prompts.rs:72-82`(own-row verbatim) | ✅ | `5a64f8ee` |
| GC5 | P2 | `[生成出错中断]` 标记作为普通发言进入后续轮视野,自愈靠模型不靠机制 | `helpers.rs:482` + 编排器无错误轮熔断 | ✅ | `5a64f8ee` |
| GC6 | P3 | turn_trace wire 字段命名(camelCase)无集中文档,API 消费者易踩 | `db/trace.rs:38-62` 注释级约定 | ✅ | `5a64f8ee` |
| GC7 | P3 | end_discussion 共识清单对 API 藏于 tool_result,无一等字段 | `MessageItem.vue:113` 仅 GUI 侧渲染 | ✅ | `5a64f8ee` |

### GC1 `busy` 轮次粒度,轮间翻 false(P1)

- **现象**:外部轮询器在讨论进行到第 3 分钟采到 `busy=false` 误判整场结束退出;daemon 日志同一时刻之后仍有参与者流式生成(16:44:31 `busy=false msgs=4` → 16:47 deepseek 仍在 SSE 出块)。轮与轮之间空隙可持续数十秒。
- **根因**:`run_group_chat_loop` 是外层编排器,每个 speaker 回合单独调一次 `run_chat_loop`,各自在 `session_active_request` 注册/注销(`group_chat_loop.rs:319`、`:514` 构造 deps;`CallerRole.skip_session_active=false`)→ 轮间(reload 消息 + `role_history` 重建 + LLM 首字节延迟)map 清空,`busy` 翻 false。`list_sessions` 的 busy 表达的是「有无活跃 chat request」,不是「整场编排是否在跑」。
- **影响**:API 驱动方/remote 客户端/断线重连的 GUI 无法区分「轮间空隙」与「讨论已结束」。GUI 在线时靠 SSE done 事件兜底,但 SSE 不回放历史。
- **修复方向**:编排级状态一等化——session 暴露 `orchestration_state`(running / ended / stop_reason / rounds_used),或编排期间整体持有 busy。
- **修复(2026-09-05,`5a64f8ee`)**:采用「编排期间整体持有 busy」——F1 队列驱动器同款先例:内层每 speaker 的 `run_chat_loop` 改传 `skip_session_active + skip_cancellations` 双 true(`chat_inner` 的注册在 spawn 前一次完成、跨轮存活),编排器在**唯一退出点**统一清 `cancellations[rid]` + `session_active_request[sid]`。附带收益:轮间隙用户 Stop / 删除会话也能找到 token 取消整场。观测语义与 GC2 合成:`busy=true` → 进行中;`!busy + stop_reason` → 已结束(原因);`!busy + NULL` → 未跑过。
- **回归验证**:headless 驱动群聊全程轮询 `list_sessions`,busy/编排态在轮间不回落;讨论结束后状态落 ended 且不再复活。→ 测试锚点 `tests_group_chat::group_chat_busy_holds_across_turns_and_lifecycle_persists`(BusyProbeSink 在每个 chat 事件时刻快照 busy,断言全程含轮间 Speaker 事件无一次回落;退出后两 map 清空)。

### GC2 讨论终止原因不落库(P1)

- **现象**:事后 `list_turn_traces` 无法回答「这场为何结束」——正常收官、30 轮帽截断、取消、错误在持久层不可区分;唯一推断手段是「有没有 end_discussion 的 tool_result」。
- **根因**:`TurnTraceRow`(`db/trace.rs:35` 起)字段全景只有 token 系列 + compaction/loop_hint/breadcrumb JSON + created_at,**没有 stop_reason 列**;`group_chat_end` / `max_rounds` 只存在于 SSE done 事件的 `stop_reason` 里,不持久化。
- **影响**:审计、复盘、自动化驱动都缺「讨论生命周期」锚点;与 GC1 是同一观测性缺口的两面,可同一 PR 收口。
- **修复方向**:stop_reason(至少 group_chat_end / max_rounds / cancelled / error)落 turn_trace 主行或 sessions 行。
- **修复(2026-09-05,`5a64f8ee`)**:落在 **sessions 行**(一场讨论一行的生命周期锚点,比 per-seq 的 turn_trace 主行更贴合「这场为何结束」):迁移加 `sessions.stop_reason TEXT`(+`discussion_summary`,见 GC7);`run_group_chat_loop` 启动时 `clear_group_chat_lifecycle` 清残留(复用会话不误报上一场的原因),退出时 `finalize_group_chat_lifecycle` 写入四值之一(`group_chat_end` / `max_rounds` / `cancelled` / `error`——cancel 路径 Done 抑制但 DB 仍留痕)。`SessionSummary` 与 `SessionRow` 均暴露 `stop_reason`(wire snake_case,sessions 域约定)。
- **回归验证**:分别触发正常收官与 MAX_ORCHESTRATION_ROUNDS 截断,事后查询能区分两者。→ 测试锚点 `tests_group_chat::group_chat_max_rounds_persists_stop_reason`、`group_chat_error_breaker_halts_after_consecutive_error_turns`、`group_chat_second_run_clears_stale_stop_reason`、`db/sessions_tests::group_chat_lifecycle_columns_round_trip`(四值 post-hoc 可区分 + 复用清空)。

### GC3 无人值守权限审批死区:120s × N(P1)

- **现象**:moderator 开场把仓库路径幻觉成 `/home/user/everlasting/docs`(cwd 之外)→ 触发 permission ask → 无 GUI 在线应答 → `ASK_TIMEOUT=120s` 超时按拒,连续两次,开场白卡约 4 分钟(占全程 19 分钟的 21%)。cwd 内读取全程静默放行,只有越界路径触发 ask;deny 本身无害——moderator 收到拒绝后自行绕过继续点名。
- **根因**:ask 机制假设「有人在看」(SSE `permission:ask` 事件 + snapshot `pending_interaction` + `permission_response` 代批,积木齐全),但不感知订阅者存在与否;超时是全局常量(`permissions/ask.rs:20`),不按 session/场景可配;群聊工具白名单本就只读(`group_chat_prompts.rs:195`),为一条幻觉路径等满 120s 收益为负。
- **修复方向**(可组合):(a) 无活跃 SSE 订阅者时快速拒(5-10s);(b) 群聊场景路径越界快速 deny 而非 ask;(c) `ASK_TIMEOUT` 按 session/场景可配。
- **修复(2026-09-05,`5a64f8ee`)**:采用 (a) 通用方案——`ChatEventSink` 新增 `has_live_observer()`(默认 `true` 保守:Tauri GUI/全部测试 sink 不变),仅 daemon 的 `HttpSseSink` 覆盖为 `SseRegistry::subscriber_count() > 0`。`ask_path` 每次调用前解析一次:无观察者 → 超时臂取 `min(UNATTENDED_ASK_TIMEOUT=8s, ask_timeout())`(`min` 保证测试的 task-local 覆写仍权威),deny 原因带 `no live observer` 标记且保持 `permission timed out after` 前缀(worker 分支判别从等值比较放宽为前缀匹配)。`ask_no_timeout` 用户开关优先级仍最高(显式「永不超时」压过在场检测)。8s 宽限内新连上的观察者可从 replay buffer 看到 ask 并经 `permission_response` 应答,oneshot 臂先到先赢。
- **回归验证**:headless 无订阅者时越界路径工具调用 <10s 返回 deny;GUI 在线时审批窗口仍 ≥120s,人工 allow/deny 路径不受影响。→ 测试锚点 `permissions/tests_ask.rs::unattended_ask_denies_fast_with_named_reason`、`attended_ask_keeps_classic_timeout_reason`(GUI 路径逐字节不变)、`unattended_worker_ask_denies_fast_and_discriminates`、`daemon/sse.rs::http_sse_sink_has_live_observer_tracks_subscribers`。

### GC4 `@moderator:` 自指前缀逐轮累积(P2)

- **现象**:moderator 持久化文本的自指前缀滚雪球——seq32 一连 → seq41 两连 → seq48/57/68 四连 `@moderator: @moderator: …`;另产生两条仅含前缀的空消息(seq9 `@moderator:`、seq38 `@陈曦-前端:`)。
- **根因**:已知问题的残留。`group_chat_prompts.rs:72-76` 文档化了双重前缀风险,修法是「他人行改写不带 `@` 前缀、归属交 wire 层 `apply_speaker_prefix`」;但**自己的行按 invariant 1 原样保留**(`:80-82`,Anthropic 签名回传需要)。模型一旦开始自称 `@moderator:`,下一轮看到自己历史带前缀就再模仿一个,无人剥离;prompt 层缓解(教模型用 @ 称呼**他人**)管不住「称呼自己」。
- **影响**:转录脏、GUI 观感差、轻微 token 浪费;仅前缀空消息污染消息流。
- **修复方向**:持久化前(或回放时)剥离 own-leading `@<self>: `——text 块级操作,不碰 thinking 签名,修复面小;或 wire 层对 own row 检测自指前缀并告警。
- **修复(2026-09-05,`5a64f8ee`)**:持久化前剥离——`group_chat_prompts.rs::strip_own_prefix_blocks` 纯函数(text 块级,thinking/签名/tool 块不碰),调用点在 `drive.rs` 收尾持久化之前(`current_speaker` 有值才启用,经典聊逐字节不变):反复剥离第一个 Text 块的 `@<speaker>:`(ASCII `:` 与全角 `:` 都容忍)至净文本;剥空的块移除,整条剥空的 turn 走既有空轮分支不落库(根除仅前缀空消息);他人 `@` 称呼与正文不动。选持久化侧而非回放侧:own-row verbatim 是模型模仿链的输入源,存库干净 = 后续 own-history 视角干净,雪球从源头断。
- **修复后 live 实跑补丁(2026-09-05 同日,session `60bcb778`)**:重启 daemon 实跑验证抓到第一版语义缺陷——剥离只扫**前导** Text 块、遇首个非文本块即停,而 Anthropic 交错序常为 `[thinking, text, tool_use]`(该场 seq28/29 实锤两行漏网),thinking 在前时前缀漏过。修正为**跳过非文本块找第一个 Text 块**再剥(找到正文头即停,后续 text 块属 body 不动),补 seq28/29 形态单测锁定。
- **回归验证**:连跑多轮群聊,moderator/参与者持久化文本无自指前缀累积;不再产生仅前缀空消息。→ 测试锚点 `tests_group_chat_prompts::strip_own_prefix_*`(单层/四层累积/全角冒号/仅前缀塌空/他人称呼不动/thinking 在前跳过/body 不动)+ `tests_group_chat::group_chat_strips_own_prefix_on_persist`(端到端:双重前缀剥净、仅前缀 turn 零落库)。live:2026-09-05 实跑 34 行零仅前缀空消息;前缀剥离路径由实跑形态单测锁定(第二场模型未自称,无可剥样本)。

### GC5 错误标记进入后续发言者视野(P2)

- **现象**:赵拓-后端一轮 `[生成出错中断]`(`helpers.rs:482` ERROR_MARKER)被持久化为该参与者的「发言」(seq28);moderator 识别中断并重新点名,本次自愈成功——但靠的是 moderator 模型的理解力,不是机制保证。
- **背景**:根因大类(错误重试死循环烧光 30 轮帽)在 08-04 重写已修,有前科案例与测试锚点——`group_chat_loop.rs:19` 引用 DB `d7fe451c`(Anthropic 2013 → ERROR_MARKER 死循环)、`tests_agent_loop/basic.rs:152`;`8be4687f` 为弱模型抢 moderator 身份案例。遗留面:错误标记作为普通文本进入后续轮历史,依赖各角色自行正确解读。
- **修复方向**:编排器对 ERROR_MARKER 轮计数/熔断(连续 N 次 → 终止并落 stop_reason=error,与 GC2 联动);或 `role_history` 改写时把错误标记行剔除,由编排器以系统注记替代。
- **修复(2026-09-05,`5a64f8ee`)**:两条都做。(1) **熔断**:编排器在每个 speaker 回合后 reload 并检测该 speaker 最新 assistant 行是否带 ERROR_MARKER(`speaker_last_turn_errored`,raw reload 而非 role_history,不受 (2) 改写影响);连续 3 轮(`MAX_CONSECUTIVE_ERROR_TURNS`)→ 终止,终端 `Done{stop_reason:"error"}` + sessions 行落 `error`(与 GC2 联动)。计数语义:错误轮 +1;**只有参与者的干净发言轮重置**(真正的内容进展);moderator 的干净仲裁轮**不**重置——否则「每个被点名者 provider 都挂、moderator 点名正常」的交替模式永不触发、烧满 30 轮(实测踩中后修正);单个自愈错误(seq28 形态)计 1 后被下一干净轮清零,不受罚。前端 `streamEvents` 终结白名单与 `groupChatNotice` 同步加 `error`。(2) **视野改写**:`role_history` 他人行分支把 ERROR_MARKER 改写为显式 `[系统注记:…]`(部分文本保留 + 注记;仅 marker 行塌缩为无内容注记);own-row verbatim(invariant 1)不动。
- **回归验证**:人为使某参与者模型连续失败(400/断流),重试有上限且最终终止原因可查;其他参与者视野中错误轮呈现为系统注记而非「某人说了句话」。→ 测试锚点 `tests_group_chat::group_chat_error_breaker_halts_after_consecutive_error_turns`(3 连错熔断 + stop_reason=error + M2 视角无裸 marker 有注记)+ `tests_group_chat_prompts::role_history_rewrites_other_speaker_error_marker_to_system_note` / `role_history_keeps_own_error_marker_verbatim` / `speaker_last_turn_errored_keys_on_latest_assistant_row` + 前端 `streamController.test.ts::GC5 终端 error`。

### GC6 turn_trace wire 命名无集中文档(P3)

- **现象**:API 消费者按 snake_case 猜字段名查询 `list_turn_traces`,返回行字段全 None(本次实踩);实际 wire 是 camelCase(`runId`/`toolsToken`/`memoryToken`…,`db/trace.rs:38-62` 各字段注释注明)。
- **根因**:daemon HTTP API 的双向命名约定(请求体 snake_case、行负载 camelCase)只存在于代码注释,无集中 API 文档。
- **修复方向**:daemon API 文档(或 OpenAPI/JSON Schema 导出)写明命名约定与各 Row 的 wire 字段名。
- **修复(2026-09-05,`5a64f8ee`)**:新增 [docs/DAEMON-API.md](./DAEMON-API.md) ——命名约定总则(请求体恒 snake_case;响应行按域分:sessions 域 snake_case、trace/providers 域 camelCase,含历史成因)+ `TurnTraceRow` 全字段 camelCase 对照表(即本次实踩点)+ sessions 域关键字段 + 群聊生命周期消费指南(GC1/GC2 的 busy×stop_reason 推导表、GC7 的 discussion_summary)+ GC3 无人值守审批窗口说明。源码为权威,文档漂移以源码为准。
- **回归验证**:按文档字段名裸 curl 可正确读出目标字段(含 GC2 落地后的 stop_reason)。→ 文档即交付;`stop_reason`(sessions 域 snake_case)由 `group_chat_lifecycle_columns_round_trip` 锁定。

### GC7 end_discussion 总结对 API 藏于 tool_result(P3)

- **现象**:整场最有价值的共识清单只存在于 end_discussion 的 tool_result 内容里(seq69,`{"cwd":…,"result":"## 共识清单…"}`),`load_session` 无一等字段。
- **根因**:GUI 侧有 `DiscussionSummaryCard` 专门渲染(`MessageItem.vue:113` 起替换通用 ToolCallCard),体验已覆盖;API 消费者只能翻 content blocks 找 tool_result 再 JSON 解包。
- **修复方向**:`LoadedSession`/session 行带 `discussion_summary`(终局时回填),或独立查询端点。
- **修复(2026-09-05,`5a64f8ee`)**:session 行一等字段——迁移加 `sessions.discussion_summary TEXT`;`end_discussion::execute_intercept` 把 summary 捕获进 `GroupChatTurnState.end_summary`,编排器在正常收官退出时随 stop_reason 一起 `finalize_group_chat_lifecycle` 落库;`SessionRow.discussion_summary` 经 `load_session` 直接可读(一次 API 调用取总结,无需解析 tool_result)。非收官退出(max_rounds/cancelled/error)不写;每次编排启动清残留。
- **回归验证**:讨论结束后一次 API 调用直接取到总结文本,无需解析 tool_result。→ 测试锚点 `tests_group_chat::group_chat_busy_holds_across_turns_and_lifecycle_persists`(总结原文 round-trip)+ `group_chat_error_breaker_halts_after_consecutive_error_turns`(非收官不写)+ `db/sessions_tests::group_chat_lifecycle_columns_round_trip`。

---

## 3. 判定表现达标(B 类,关闭)

以下四项本次实跑验证通过,属已知设计或已有修复生效,除非产品主张变更不再重开:

- **工具白名单零泄漏**:群聊全程无 shell/write 类工具混入——`group_chat_prompts.rs:195` 只读白名单(R1 后新增 builtin 工具不自动进入群聊,`8be4687f` 类弱模型滥用不可复发)。
- **role_history 隔离无串台**:无参与者误认自己是 moderator(08-04 身份混淆根因类别,本次 6 人 × 混编 3 模型未复发)。
- **权限拒绝不炸循环**:越界路径 deny 后 moderator 自适应绕过继续主持(ask 通道语义正确;慢的问题归 GC3)。
- **中断自愈**:`[生成出错中断]` 后 moderator 检测并重新点名成功(design D7 fallback 生效;机制加固归 GC5)。

---

## 4. 求证衍生修复(2026-09-06,GC-fix live 转录复盘)

对 09-05 验证 session(`60bcb778`)整场转录做逐条求证,发现三个 GC 清单之外的真实缺陷(论断核查结论:GC1-GC7 相关共识 P0「打断最小语义」依然成立,但其论据中「participant 回合 max_turns=1」「压缩只可能在 moderator 回合触发」两条与代码不符——群聊压缩总 gate 显式排除群聊 `chat_loop/init.rs`(`!worker && !群聊`),群聊任何回合都不做摘要压缩,超线直走机械截断;「当前 speaker 只缺露出」亦过时,`ChatEvent::Speaker` 08-04 起每轮已发):

### D1 群聊 prompt 无 cwd——主持人错路径烧 10 分钟(A 类)

- **现象**:moderator(MiniMax-M3)开场三轮工具调用全部使用幻觉绝对路径 `/home/user/everlasting/...`,5 次 root 外审批 ask × 120s attended 超时,seq1→seq9 烧掉 10 分 08 秒(占整场 16 分钟 2/3),研究预算耗尽后凭记忆开题,直接导致其「群聊是 N session 拼场」错误前提。同场 deepseek/GLM flash 参与者用相对路径全部免审批成功。
- **根因**:群聊 prompt 是 `system_prompt_override` 完全替换,而 `- Working directory:` 行在经典 prompt(`system_prompt.rs`)里——群聊模型对项目路径**零 in-band 信息**,只能猜。模型侧 seq5 曾从工具报文 envelope 的 `cwd` 字段学到正确路径,seq7 又退回——注意力跟随弱是放大器,不是根因。
- **修复(2026-09-06)**:`GroupChatCtx` 新增 `project_root`(`build_group_chat_ctx` 从 `SessionRow.current_cwd` 回填);moderator/participant prompt 注入 `## Project context` 块(working directory + 「优先相对路径,绝对路径出 root 需审批」教法);空 cwd 优雅降级为无该节。
- **回归锚点**:`tests_group_chat_prompts::speaker_prompts_carry_working_directory_when_known` / `speaker_prompts_omit_project_context_when_root_unknown`。

### D2 grep 工具相对 glob 恒零命中(通用工具 bug,A 类)

- **现象**:live 实跑中参与者 grep `{"glob":"app/src/**/*.vue","pattern":"打断|插话|preempt|中断"}`(未传 path)返回 "No matches",据此下了「全部 .vue 零命中」的错误论断;实际 6 个 .vue 文件命中。
- **根因**:rg 的 `--glob` 匹配的是它**打印的路径**。工具把 root 解析成绝对路径传给 rg → rg 打印绝对路径 → 根相对 glob(`app/src/**`)永远失配。path 缺省 `"."` 也被 resolve 成绝对 cwd,故「缺省 path + 相对 glob」必挂(本机复现:同款命令 exit=1,`**/*.vue` 即正常)。
- **修复(2026-09-06)**:root 为目录时改用 `cmd.current_dir(root)` + 传 `.` 作搜索根——rg 打印根相对路径,glob 语义即符合直觉;输出统一经 `rewrite_paths_to_relative` 剥 `./` 前缀(files 模式此前不重写,现一并收口)。非目录 root(单文件/缺失路径)保留直传,避免 `current_dir` spawn 失败误报「rg 不在 PATH」。
- **回归锚点**:`tools::grep::tests::relative_glob_matches_with_default_path` / `relative_glob_matches_with_relative_path`。

### D3 两处过时注释仍在散布 participant `max_turns=1`(文档债,已实际误导)

- **现象**:live 实跑中认真读代码的参与者(deepseek)得出「participant 回合 max_turns=1,永远撞不响压缩阈值」——前提与实际相反(`group_chat_loop.rs` 实际调用 `max_turns=Some(20)`,08-07 R3;moderator 才是 1)。
- **根因**:`group_chat_loop.rs` 模块文档 §6 与 `chat_loop.rs` 软卡分支注释双双仍写 1(且互相引用),08-07 R3 改 20 后未同步——模型读的是真代码里的假注释,非幻觉。
- **修复(2026-09-06)**:两处注释更正为「participant 20 / moderator 1,08-07 R3」。
- **第四处同类病灶(2026-09-06 第二场 live 群聊现场发现并修复)**:`group_chat_loop.rs` 模块文档 Fallback 段仍描述「no-nominate 时 round-robin 一轮」——该 fallback 08-06 已移除(会派错 participant 致角色塌缩),现状是重试 moderator 直到 MAX_ORCHESTRATION_ROUNDS;更正为现状描述。该场讨论的共识清单本身把「假注释是喂给群聊参与者的毒数据」立为 RULE 候选,见该场 `discussion_summary`。
