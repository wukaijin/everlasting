# Review — 群聊 P1a checkpoint 落库与续跑

> **评审对象**:任务 `09-06-gc-p1a-checkpoint-resume`(planning 阶段,未 `task.py start`)。
> **评审范围**:`prd.md` / `design.md` / `implement.md` / jsonl 清单,逐条对照代码查证
> (Rust 编排器 / 入口 / lifecycle / schema / 前端流控 / M1-M2 脚本 / 文档)。
> **评审结论**:方案根因判断与整体取舍**成立**(PRD 代码取证绝大多数属实,三块独立可分、
> 回滚路径干净),但 R4 的 GUI 门有一条**必须补进 design 才能按 AC5 交付**的数据通路缺口
> (前端 `SessionSummary` 无 `stop_reason` 字段 + 终态后门数据不刷新),另有命令校验缺
> stop_reason 终局拒绝、sweep 时间语义、AC2 表述空转三个 P1。
> **代码基线**:`main@5020792a`。工作树含 .trellis/tasks 未跟踪目录与 .zcode agent 文件
> 改动,与本任务无关。

---

## 0. 结论摘要

| 级别 | 编号 | 问题 | 性质 |
|---|---|---|---|
| 🔴 阻塞 | P0-1 | R4/AC5 前端门读不到 `stop_reason`:`SessionSummary` TS 类型无此字段(vue-tsc 必拦)+ 终态后内存 summary 不刷新(按钮不出现直到重拉 list_sessions) | design 缺口(§5「零新字段」不成立) |
| 🟠 需改 | P1-1 | resume 命令四类校验缺 **stop_reason 终局拒绝**:行存留是 best-effort(删行会失败),残留下「行在 + !busy」会允许续跑已收官的 `group_chat_end` 场、推翻 summary | 与 Q2 冲突的防御缺口 |
| 🟠 需改 | P1-2 | boot sweep 写 `sessions.updated_at` 语义错误:改成 boot 时刻 → 侧栏排序突变 + 「中断于 X」显示重启时间而非崩溃时刻 | design 未定的时间源 + 副作用 |
| 🟠 需改 | P1-3 | AC2「M1 脚本 status」**空转**(脚本无 status 子命令);且 M1 `run` 轮询遇 `interrupted` 返回后 EXIT_BY_STOP_REASON 无键 → 误分类 exit 1 | AC 表述 + 顺手项范围不足 |
| 🟡 需留意 | P2-1 | 轮头粒度的「至多重跑该轮发言」措辞过窄:崩溃在轮完成后 → resume 重跑**完整上一轮**,已落库发言会产生重复副本 | 文档措辞 + AC3 验证预期 |
| 🟡 需留意 | P2-2 | 双 resume / resume×chat 竞态:3a 兜底能收敛,但窗口内 A 被 cancel 成 `cancelled` 可能瞬时覆盖 B 的场 | 前端补 once 防护 + 测试 |
| 🟡 需留意 | P2-3 | 恢复指令仅命中 start_round 首个 moderator 轮;该轮不 nominate 即丢失 | 边缘,可接受 |
| 🟡 需留意 | P2-4 | 可续跑态直接打字发送 = 放弃续跑新开一场(旧转录同 session 仍可见,但清 lifecycle + 删 checkpoint) | UI 文案建议 |
| 🟡 需留意 | P2-5 | implement.jsonl / check.jsonl 仍是 `_example` 占位(workflow 要求 sub-agent 平台 start 前有真实条目) | 流程门禁 |

小勘误:design/implement 引「Tauri 注册(commands/mod.rs:92)」不准——`commands/mod.rs` 只是
`all_command_names()` 名单;真正的 `invoke_handler` 在 `lib.rs:211-218`(两处都要加,别只加一处)。

---

## 1. 查证成立的部分(无需改动)

以下 PRD/design 论断逐条对代码核实,**成立**:

1. **转录即状态、全量持久**:`reload_messages`(group_chat_loop.rs:187-230)从 DB 重建,
   role/speaker 往返;每 speaker turn 经一次 `run_chat_loop`(`max_turns` moderator=1 /
   participant=20)落库,编排器**每轮多次 reload**(moderator 轮前/后、participant 轮前/后)。
2. **编排器易失状态**:DB 侧仅 `sessions.stop_reason` + `discussion_summary` 两个群聊列
   (schema.rs:490-491);round 计数(:339)与 `consecutive_error_turns`(:344)纯局部变量;
   `pending_injects`/`preempt_requested` 纯内存(controls 注册表,编排器入口新建、退出
   remove)。**崩溃确实只剩「中途无痕」这一条路径**。
3. **lifecycle 契约**:`clear_group_chat_lifecycle`(session_crud.rs:925-942,置
   stop_reason/discussion_summary NULL)与 `finalize_group_chat_lifecycle`(:953-976)的
   唯一生产调用者都是 `run_group_chat_loop`(group_chat_loop.rs:322 开头 / :865 退出)。
   **新 chat 静默重启编排器并清 lifecycle 的 M3 P1-1 副作用坐实**:不 busy 的 group_chat
   session 走 legacy → spawn → `run_group_chat_loop` 开头无条件 clear(:316-328)。
4. **入口链与 ChatEntry**:`ChatEntry`(chat.rs:178-203)三个生产构造点 = Tauri chat 命令
   (chat.rs:107)/ daemon HTTP(daemon/routes/agent.rs:61)/ F2 scheduler
   (scheduler/mod.rs:165);群聊分支在 busy+user 尾条时注入、否则 `break 'routing` 落
   legacy(chat.rs:360-413)。resume 空 messages → `injectable=false` 天然走 legacy ✓。
5. **stop_reason 是纯字符串、无 Rust 侧穷举**:`SessionSummary.stop_reason: Option<String>`
   (db/types.rs:454);全仓**无**对 stop_reason 的 enum/from-str/穷举 match —— 新值
   `interrupted` 在 Rust 侧零改动、纯 string 直穿 list_sessions wire。「!busy +
   stop_reason → 终态」消费方派生零 API 变更即可见的设计成立(消费方在 MCP 端
   `isTerminal = !busy && stop_reason != null`,open-ended ✓)。
6. **schema FK 先例**:所有引用 sessions 的表(messages / session_tool_permissions /
   turn_trace / scheduled_tasks)都是 `REFERENCES sessions(id) ON DELETE CASCADE`
   (schema.rs:176/502/525/1076/1326)→ checkpoint 表照抄即可,delete_session 自动清。
7. **boot sweep 挂点**:`AppState::load_from_dir`(state.rs:326)daemon bin 与 Tauri Full
   共享;`load_inner` 内已有两个 warn-only「回收崩溃残留」先例可镜像——
   `reap_orphaned_runs`(:367-377)与 `recover_interrupted_messages`(:393-405,即
   database-guidelines RULE-PERSIST-001 场景)。**P1a 实为 RULE-PERSIST-001 的
   session 级变体**,spec 已有同款形态守则可引。
8. **round-0 转录分支的 resume 修正**:design §3.2「`round == 0 && resume.is_none()`
   → 用传入 messages,否则 reload」是必要且正确的 —— 现有代码 `if round == 0 {
   messages.clone() } else { reload_messages }`(group_chat_loop.rs:404-408),若 resume
   start_round=0 且不加 `resume.is_none()`,会拿空 messages 当历史。
9. **编排器改动锚点成立**:轮头序列 = token 检查(:347)→ inject drain(:362)→ preempt
   检查(:385)→ moderator 轮;GC5 两处计分(moderator :513-525 / participant :734-749);
   退出分流点(:850-863)与 preempt wrap-up 先例(:776-832)——resume 恢复指令与
   checkpoint 退出的插入点都清晰,不碰 turn-taking 主结构。
10. **moderator 恢复指令先例**:`moderator_wrapup_instruction`(group_chat_prompts.rs:
    94-102)是退出条件分支里 `moderator_prompt.clone() + instruction` 的追加式
    (group_chat_loop.rs:779-780),每轮 prompt 由 `moderator_prompt.clone()` 重建
    (:429)——resume 指令同款追加可行。
11. **文档接线目标全部在位**:GCE-ROADMAP §4 P1a 余项行(L78)/ §6 依赖矩阵行(L99)/
    §7 表授权(L110);ROADMAP §1.2 date 序表(P0 行 L114 为先例);DAEMON-API.md §4 三态机
    (L73-76)+ stop_reason 表(L81-85)+ §6.2 终态 done 列表(L202)+ §7 端点清单 —— R6
    触及面与 prd 描述一致。
12. **graceful shutdown 不会误标**:daemon SIGTERM 走 8s drain,cancel token → 编排器
    轮头 break → finalize `cancelled`(daemon/server.rs:482-536)——**只有 SIGKILL/崩溃
    才留下 stop_reason=NULL**,与「interrupted 只在进程死亡路径出现」的设计前提一致;
    AC7 用 SIGKILL 语义模拟 crash 是对的(daemon.sh stop 若是 SIGTERM 会 finalize
    cancelled,测不出 interrupted)。

---

## 2. 🔴 P0-1(阻塞,R4/AC5 必修):GUI 可续跑门读不到 `stop_reason`

### 2.1 两个独立缺口

**缺口 A — `SessionSummary` TS 类型没有 `stop_reason`**。design §5「可见性门:
`session_type==='group_chat' && !busy && stop_reason ∈ {…}`」、且声称
「SessionSummary.stop_reason 已在 wire,零新字段」。前半句对(wire 上 Rust 的
`SessionSummary` 确实序列化 `stop_reason`,db/types.rs:454),**后半句错**:前端类型
`app/src/stores/chat.types.ts:466-574` 的 `SessionSummary` 接口**没有** `stop_reason`
字段(只有 `busy?: boolean` 在 :573);`LoadedSession.session`(streamRehydrate.ts:76-128)
同样不带。list_sessions 的 JSON 运行时含该字段,但 TS 层不可见 → ChatPanel 写
`currentSession.value?.stop_reason` 过不了 vue-tsc(AC6 门禁含 vue-tsc)。R4 按钮门与
interrupted 通知行**照 design 无法编译**。

**缺口 B — 终态后内存 summary 不刷新,门数据不更新**。`stop_reason` 是 session 行字段,
不在 SSE 事件流里回写 `sessions[]`。前端唯一在终态触碰 summary 的地方是
`finalizeRequest`(streamEvents.ts:1336-1370):只做 `summary.busy = false` 本地翻转,
随后 `reloadAfterFinalize` 只重拉**消息缓冲**(load_session → putMessages),不刷
`SessionSummary`。「本端跑完一场(Stop / 熔断 / interrupted 恢复后)→ 立刻看到『续跑』
按钮」的场景需要一次额外的 session summary 拉取(list_sessions 重拉,或 finalize 时
用 load_session 返回的 session 更新 `sessions[]`)。design §5 未定义这个刷新点。

### 2.2 修正建议

- **PR3 清单补**:`chat.types.ts` `SessionSummary` 加 `stop_reason?: string | null`(若
  通知行想带 summary 则一并补 `discussion_summary?: string | null`,但最小集只需
  stop_reason);ChatPanel 门从 `currentSession` 读两字段。
- **刷新时序在 design §5 明示**:任选其一 —— ① `finalizeRequest` 后把
  `reloadAfterFinalize` 的 `load_session` 结果里 session 字段写回 `sessions[]`
  (load_session 已含 session);② 终态时显式 `list_sessions` 重拉一次。注意 08-31
  有「finalize 后 DB 权威重拉」的先例注释(streamEvents.ts:392/478 一带),选①最省。
- 按钮自身加**提交后 once 防护**(disable / 本地 busy 标记),防双击双 resume(P2-2)。

---

## 3. 🟠 P1-1(需改):resume 校验矩阵缺 stop_reason 终局拒绝

design §4.2 的 resume 校验四类:`group_chat` / !busy / checkpoint 行在 / round < MAX。
**没查 stop_reason**。而「行存留编码可续跑性」依赖 `delete_group_chat_checkpoint` 在
`group_chat_end` / `preempted` / `max_rounds` 退出时**必然执行**(R5)——但删行是
best-effort warn + swallow(group_chat_loop.rs:322 同款姿态)。删行一旦失败(或 finalize
写完、删行前崩溃),就会出现「行在 + `stop_reason=group_chat_end`」的残留:GUI 门因
stop_reason 不在可续跑集不显示按钮(✓),但 **API resume_group_chat 直接调会通过四类
校验、续跑一场已正常收官的讨论、推翻刚写的 summary** —— 与 Q2「group_chat_end /
preempted / max_rounds 不可续跑」的已定决策直接冲突。行存留是主要编码,但校验不能
只信它(它可能失守)。

### 修正建议(任选或都做)

- **命令校验补第 ⑤ 类**:`stop_reason ∈ {group_chat_end, preempted, max_rounds}` →
  拒绝(错误文案「该讨论已正常结束,不可续跑」);或等价地放宽为「仅
  stop_reason ∈ {interrupted, cancelled, error, NULL} 可续跑」。
- **boot sweep 顺带孤儿清理**:除标 `interrupted` 外,把「行在 + stop_reason 为终局
  三值」的 checkpoint 行删掉(recover_interrupted_messages 的 orphan 修复同思路),
  自愈删行失败的残留 —— 这样「行在」编码恢复可信。
- implement 的 PR2 单测补一条:**删行失败残留 + 终局 stop_reason → resume 被拒**。

---

## 4. 🟠 P1-2(需改):sweep 写 `updated_at` 的时间语义 + 「中断于 X」时间源未定

design §2.2 的 sweep SQL:`UPDATE sessions SET stop_reason='interrupted', updated_at=?
WHERE stop_reason IS NULL AND id IN (SELECT session_id FROM group_chat_checkpoints)`。
副作用:把被中断 session 的 `updated_at` 改成 **boot 时刻**(重启时间),而非崩溃时刻:

- **侧栏排序突变**:SessionList 按 updated_at 排,重启后所有 crashed session 会一起顶到
  最前,且显示「刚刚更新」——误导。
- **prd Goal 的「中断于 X」**:若 UI 通知行用 `SessionSummary.updated_at` 当 X,显示的是
  「重启标记时刻」而非真实中断时刻。真实中断时刻 ≈ checkpoint 行的 `updated_at`
  (每轮头 upsert,group_chat_loop 轮内崩溃前最后一次写),但 checkpoint 数据不随
  list_sessions 出,前端拿不到。

### 修正建议

- **sweep 不动 `sessions.updated_at`**(只写 stop_reason):sessions.updated_at 由消息
  落库路径维护,崩溃时它 ≈ 最后一条消息时刻 ≈ 中断时刻(轮头密集,误差一轮内),
  「中断于 {updated_at}」即真实最后活动。侧栏不突变。
- 若想要精确到轮的时间:把 checkpoint.updated_at 暴露(或 R4 通知文案只写「讨论已中断,
  可续跑」不写时刻,回避时间源)。至少 design 要**定死 X 从哪来**,现在完全未定义。

---

## 5. 🟠 P1-3(需改):AC2 的「M1 脚本 status」空转 + run 遇 interrupted 误分类

- **`group-chat-run.mjs` 没有 `status` 子命令**:usage 只有 `projects / models /
  presets / run`(scripts/group-chat-run.mjs:670-690)。AC2「M1 脚本 `status` 不改代码
  即可报出中断态」有一半无法验收。MCP `discussion_status` 侧成立(`isTerminal = !busy
  && stop_reason != null` 是 open-ended,group-chat-mcp.mjs:82-84)。
- **M1 `run` 轮询对 interrupted 的处理会误导**:poll 循环 `if (s.stop_reason) return`
  (:517)会把 interrupted 当正常结束返回,但 `EXIT_BY_STOP_REASON` 无该键(:46-51)→
  `exit code 1`(归入「脚本自身错误」bucket)、`--cleanup` 跳过(行为上 session 保留
  反而是对的,但归因错)、转录 note 缺失。AC7 之后的脚本化 resume 若复用 run 的轮询,
  会看到误导性 exit 1。

### 修正建议

- R6 的顺手项从「status 输出提示」扩为:`EXIT_BY_STOP_REASON` 加 `interrupted`(exit 0
  或专属码)+ 转录 note 提示「session 可经 resume_group_chat 续跑」。AC2 表述改为
  「MCP `discussion_status` 与 `group-chat-run.mjs run` 轮询不改判定逻辑即可报出
  中断态」,并把上述 exit 处理列为 R6 项。

---

## 6. 🟡 P2 留意项

- **P2-1 崩溃窗口措辞**:design「至多重跑中断所在轮的发言」过窄。崩溃若发生在 N-1 轮
  完成之后、N 轮头 upsert 之前 → checkpoint round=N-1 → resume 重跑**完整 N-1 轮**,
  该轮已落库的 moderator/participant 发言在转录里出现**重复副本**(reload 全量历史,
  重跑的 moderator 看到自己上一轮仲裁文本)。这是轮头粒度的固有取舍(prd Out of Scope
  已接受「至多重跑中断所在轮的发言」),但措辞应改为「至多重跑中断所在轮(可能含该轮
  已落库发言的重复副本)」,AC3 live 时观察转录重复度,必要时后续加 speaker 级断点。
- **P2-2 双 resume 竞态**:resume 走 legacy,3a 防御取消只防已注册在途;并发双 resume
  各自注册 → 后到者 cancel 先到者,先到编排器 finalize `cancelled`(行保留),后到者
  clear 后续跑 —— 收敛,但先到者的 finalize 若落在后到者 clear 之后,会把后到者进行中
  的场瞬时写成 `cancelled`(busy=true 使消费方不误判,最终被后到者收官值覆盖)。design
  §4.1「经典 chat 同款 3a 兜底,接受」可保留,但 GUI 按钮**必须加提交后 disable**
  (preempt 按钮用 `isCurrentSessionStreaming` 推断,resume 按钮点击到首个 SSE 事件前
  无本地忙标记,双击即双 resume)。
- **P2-3 恢复指令单轮机会**:命中条件 `resume.is_some() && round == start_round`;
  该轮 moderator 若未 nominate(`continue` 到 round+1,group_chat_loop.rs:541-565 注释
  说 retry 实为下一轮)→ 指令丢失。边缘可接受;若要稳,可把指令保留到 resume 后首次
  实际进入 moderator turn 再清。
- **P2-4 可续跑态发消息 = 放弃续跑**:可续跑态(如 interrupted)用户直接在输入框打字发送
  → 不 busy → 新开一场 → `run_group_chat_loop` 开头 clear lifecycle + R1 删 checkpoint
  (resume.is_none)→ 被中断场不再可续(旧转录同 session 仍可见)。R4 说「正常发送仍走
  新开一场」是设计意图,但建议 interrupted 通知行旁加一句「发送新消息将开始新讨论,
  不再续跑」,避免用户误以为打字即续跑。
- **P2-5 jsonl gate**:`implement.jsonl` / `check.jsonl` 仍是 `_example` 占位。workflow
  (`.trellis/workflow.md` L444)要求 sub-agent 平台 start 前两清单有真实 curated 条目;
  任务 status 仍 planning,start 前需补。
- **小勘误**:design/implement 引「Tauri 注册(commands/mod.rs:92)」——`commands/mod.rs`
  只维护 `all_command_names()` 名单;`invoke_handler` 在 `lib.rs:211-218`(chat / cancel /
  preempt 都在那)。resume 命令两处都要加。

---

## 7. 文档接线核对(R6,目标全部在位)

- `docs/GROUP-CHAT-API-ROADMAP.md`:§4 P1a 行(L78)、§6 矩阵行(L99)、§7 授权(L110)。
- `docs/ROADMAP.md` §1.2 date 序表(P0 行 L114 先例,追加一行)。
- `docs/DAEMON-API.md`:§4 三态机(L73-76,补 interrupted 态 + 可续跑集)、stop_reason
  表(L81-85,补值 + boot sweep 语义)、§6.2 终态列表(L202)、§7 端点清单;新增
  `POST /api/v1/agent/resume_group_chat` 端点文档。
- `.trellis/spec/backend/agent-loop-architecture/pattern-group-chat-preempt-inject.md`:
  P1a 段落账(implement 收官前自查已列);`database-guidelines.md` RULE-PERSIST-001 场景
  与本任务同构,可交叉引用。
- 新表 FK:CASCADE 跟随先例,delete_session 自动清行(design §2.1 已留核对项,结论应
  定为 CASCADE)。

---

## 8. 总评

这份 planning 的代码取证质量高(行号基本精确、机制描述与实现一致),决策链条
(Q1 全量 / Q2 行存留编码 / Q3 新表 + boot sweep)与仓库既有模式(schema FK 先例、
load_inner 的 warn-only 恢复 pass、preempt 三件套、wrapup 指令追加式)严丝合缝,
轮预算继承 / streak 归零 / round-0 reload 等细节都想到了。**方向无需调整**;开工前需把
P0-1(前端 stop_reason 数据通路)补进 design §5 + PR3 清单,P1-1/P1-2/P1-3 三处按上述
修订进 design/implement/AC,即可 `task.py start`。三 PR 独立可分的结论维持。
