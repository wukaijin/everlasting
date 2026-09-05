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

---

## 2. 待修复清单(A 类)

> 状态:⬜ 待修复 · 🔧 修复中 · ✅ 已修复(填提交) 。按严重级别排序。

| 编号 | 级别 | 问题 | 根因位置 | 状态 | 修复提交 |
|---|---|---|---|---|---|
| GC1 | **P1** | `busy` 信号是轮次粒度而非整场讨论粒度,外部观测者误判「已结束」 | `group_chat_loop.rs:319,514` + `session_active_request` | ⬜ | — |
| GC2 | **P1** | 讨论终止原因(stop_reason)不落库,事后不可查 | `db/trace.rs:35`(TurnTraceRow 无此列) | ⬜ | — |
| GC3 | **P1** | 无人值守时权限审批死区:越界路径 ask 硬等 120s 超时按拒 | `permissions/ask.rs:20`(ASK_TIMEOUT 常量) | ⬜ | — |
| GC4 | P2 | moderator 自指 `@` 前缀逐轮累积 + 产生仅前缀空消息 | `group_chat_prompts.rs:72-82`(own-row verbatim) | ⬜ | — |
| GC5 | P2 | `[生成出错中断]` 标记作为普通发言进入后续轮视野,自愈靠模型不靠机制 | `helpers.rs:482` + 编排器无错误轮熔断 | ⬜ | — |
| GC6 | P3 | turn_trace wire 字段命名(camelCase)无集中文档,API 消费者易踩 | `db/trace.rs:38-62` 注释级约定 | ⬜ | — |
| GC7 | P3 | end_discussion 共识清单对 API 藏于 tool_result,无一等字段 | `MessageItem.vue:113` 仅 GUI 侧渲染 | ⬜ | — |

### GC1 `busy` 轮次粒度,轮间翻 false(P1)

- **现象**:外部轮询器在讨论进行到第 3 分钟采到 `busy=false` 误判整场结束退出;daemon 日志同一时刻之后仍有参与者流式生成(16:44:31 `busy=false msgs=4` → 16:47 deepseek 仍在 SSE 出块)。轮与轮之间空隙可持续数十秒。
- **根因**:`run_group_chat_loop` 是外层编排器,每个 speaker 回合单独调一次 `run_chat_loop`,各自在 `session_active_request` 注册/注销(`group_chat_loop.rs:319`、`:514` 构造 deps;`CallerRole.skip_session_active=false`)→ 轮间(reload 消息 + `role_history` 重建 + LLM 首字节延迟)map 清空,`busy` 翻 false。`list_sessions` 的 busy 表达的是「有无活跃 chat request」,不是「整场编排是否在跑」。
- **影响**:API 驱动方/remote 客户端/断线重连的 GUI 无法区分「轮间空隙」与「讨论已结束」。GUI 在线时靠 SSE done 事件兜底,但 SSE 不回放历史。
- **修复方向**:编排级状态一等化——session 暴露 `orchestration_state`(running / ended / stop_reason / rounds_used),或编排期间整体持有 busy。
- **回归验证**:headless 驱动群聊全程轮询 `list_sessions`,busy/编排态在轮间不回落;讨论结束后状态落 ended 且不再复活。

### GC2 讨论终止原因不落库(P1)

- **现象**:事后 `list_turn_traces` 无法回答「这场为何结束」——正常收官、30 轮帽截断、取消、错误在持久层不可区分;唯一推断手段是「有没有 end_discussion 的 tool_result」。
- **根因**:`TurnTraceRow`(`db/trace.rs:35` 起)字段全景只有 token 系列 + compaction/loop_hint/breadcrumb JSON + created_at,**没有 stop_reason 列**;`group_chat_end` / `max_rounds` 只存在于 SSE done 事件的 `stop_reason` 里,不持久化。
- **影响**:审计、复盘、自动化驱动都缺「讨论生命周期」锚点;与 GC1 是同一观测性缺口的两面,可同一 PR 收口。
- **修复方向**:stop_reason(至少 group_chat_end / max_rounds / cancelled / error)落 turn_trace 主行或 sessions 行。
- **回归验证**:分别触发正常收官与 MAX_ORCHESTRATION_ROUNDS 截断,事后查询能区分两者。

### GC3 无人值守权限审批死区:120s × N(P1)

- **现象**:moderator 开场把仓库路径幻觉成 `/home/user/everlasting/docs`(cwd 之外)→ 触发 permission ask → 无 GUI 在线应答 → `ASK_TIMEOUT=120s` 超时按拒,连续两次,开场白卡约 4 分钟(占全程 19 分钟的 21%)。cwd 内读取全程静默放行,只有越界路径触发 ask;deny 本身无害——moderator 收到拒绝后自行绕过继续点名。
- **根因**:ask 机制假设「有人在看」(SSE `permission:ask` 事件 + snapshot `pending_interaction` + `permission_response` 代批,积木齐全),但不感知订阅者存在与否;超时是全局常量(`permissions/ask.rs:20`),不按 session/场景可配;群聊工具白名单本就只读(`group_chat_prompts.rs:195`),为一条幻觉路径等满 120s 收益为负。
- **修复方向**(可组合):(a) 无活跃 SSE 订阅者时快速拒(5-10s);(b) 群聊场景路径越界快速 deny 而非 ask;(c) `ASK_TIMEOUT` 按 session/场景可配。
- **回归验证**:headless 无订阅者时越界路径工具调用 <10s 返回 deny;GUI 在线时审批窗口仍 ≥120s,人工 allow/deny 路径不受影响。

### GC4 `@moderator:` 自指前缀逐轮累积(P2)

- **现象**:moderator 持久化文本的自指前缀滚雪球——seq32 一连 → seq41 两连 → seq48/57/68 四连 `@moderator: @moderator: …`;另产生两条仅含前缀的空消息(seq9 `@moderator:`、seq38 `@陈曦-前端:`)。
- **根因**:已知问题的残留。`group_chat_prompts.rs:72-76` 文档化了双重前缀风险,修法是「他人行改写不带 `@` 前缀、归属交 wire 层 `apply_speaker_prefix`」;但**自己的行按 invariant 1 原样保留**(`:80-82`,Anthropic 签名回传需要)。模型一旦开始自称 `@moderator:`,下一轮看到自己历史带前缀就再模仿一个,无人剥离;prompt 层缓解(教模型用 @ 称呼**他人**)管不住「称呼自己」。
- **影响**:转录脏、GUI 观感差、轻微 token 浪费;仅前缀空消息污染消息流。
- **修复方向**:持久化前(或回放时)剥离 own-leading `@<self>: `——text 块级操作,不碰 thinking 签名,修复面小;或 wire 层对 own row 检测自指前缀并告警。
- **回归验证**:连跑多轮群聊,moderator/参与者持久化文本无自指前缀累积;不再产生仅前缀空消息。

### GC5 错误标记进入后续发言者视野(P2)

- **现象**:赵拓-后端一轮 `[生成出错中断]`(`helpers.rs:482` ERROR_MARKER)被持久化为该参与者的「发言」(seq28);moderator 识别中断并重新点名,本次自愈成功——但靠的是 moderator 模型的理解力,不是机制保证。
- **背景**:根因大类(错误重试死循环烧光 30 轮帽)在 08-04 重写已修,有前科案例与测试锚点——`group_chat_loop.rs:19` 引用 DB `d7fe451c`(Anthropic 2013 → ERROR_MARKER 死循环)、`tests_agent_loop/basic.rs:152`;`8be4687f` 为弱模型抢 moderator 身份案例。遗留面:错误标记作为普通文本进入后续轮历史,依赖各角色自行正确解读。
- **修复方向**:编排器对 ERROR_MARKER 轮计数/熔断(连续 N 次 → 终止并落 stop_reason=error,与 GC2 联动);或 `role_history` 改写时把错误标记行剔除,由编排器以系统注记替代。
- **回归验证**:人为使某参与者模型连续失败(400/断流),重试有上限且最终终止原因可查;其他参与者视野中错误轮呈现为系统注记而非「某人说了句话」。

### GC6 turn_trace wire 命名无集中文档(P3)

- **现象**:API 消费者按 snake_case 猜字段名查询 `list_turn_traces`,返回行字段全 None(本次实踩);实际 wire 是 camelCase(`runId`/`toolsToken`/`memoryToken`…,`db/trace.rs:38-62` 各字段注释注明)。
- **根因**:daemon HTTP API 的双向命名约定(请求体 snake_case、行负载 camelCase)只存在于代码注释,无集中 API 文档。
- **修复方向**:daemon API 文档(或 OpenAPI/JSON Schema 导出)写明命名约定与各 Row 的 wire 字段名。
- **回归验证**:按文档字段名裸 curl 可正确读出目标字段(含 GC2 落地后的 stop_reason)。

### GC7 end_discussion 总结对 API 藏于 tool_result(P3)

- **现象**:整场最有价值的共识清单只存在于 end_discussion 的 tool_result 内容里(seq69,`{"cwd":…,"result":"## 共识清单…"}`),`load_session` 无一等字段。
- **根因**:GUI 侧有 `DiscussionSummaryCard` 专门渲染(`MessageItem.vue:113` 起替换通用 ToolCallCard),体验已覆盖;API 消费者只能翻 content blocks 找 tool_result 再 JSON 解包。
- **修复方向**:`LoadedSession`/session 行带 `discussion_summary`(终局时回填),或独立查询端点。
- **回归验证**:讨论结束后一次 API 调用直接取到总结文本,无需解析 tool_result。

---

## 3. 判定表现达标(B 类,关闭)

以下四项本次实跑验证通过,属已知设计或已有修复生效,除非产品主张变更不再重开:

- **工具白名单零泄漏**:群聊全程无 shell/write 类工具混入——`group_chat_prompts.rs:195` 只读白名单(R1 后新增 builtin 工具不自动进入群聊,`8be4687f` 类弱模型滥用不可复发)。
- **role_history 隔离无串台**:无参与者误认自己是 moderator(08-04 身份混淆根因类别,本次 6 人 × 混编 3 模型未复发)。
- **权限拒绝不炸循环**:越界路径 deny 后 moderator 自适应绕过继续主持(ask 通道语义正确;慢的问题归 GC3)。
- **中断自愈**:`[生成出错中断]` 后 moderator 检测并重新点名成功(design D7 fallback 生效;机制加固归 GC5)。
