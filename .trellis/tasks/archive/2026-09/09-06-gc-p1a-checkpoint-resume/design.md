# Design — 群聊 P1a checkpoint 落库与续跑

> 决策来源:prd.md「已定决策」Q1-Q3 + 三项技术推荐。锚点均为 2026-09-06 实读。

## 1. 架构总览

```text
                      ┌────────────────────────────────────────────┐
                      │ run_group_chat_loop (group_chat_loop.rs)   │
   chat_inner ───────▶│  round head: drain injects → preempt check │
   (ChatEntry{        │            → UPSERT checkpoint(round,streak)│
    resume:Option)    │  speaker turns (moderator / participant)   │
        │             │  GC5 streak 变化 → UPSERT checkpoint       │
   resume_group_chat  │  退出: finalize lifecycle                  │
   (命令/路由/GUI)     │        stop_reason∈{cancelled,error}→ 留行 │
        │             │        其余终局            → DELETE 行     │
        └────────────▶└────────────────────────────────────────────┘
                                                                      │
   boot: AppState::load_from_dir ── recover_group_chat_checkpoints ───┘
         (stop_reason=NULL 且有行 → 'interrupted',幂等,不覆盖终态)
```

三块改动互相独立可分 PR:**DB 层**(表 + 4 函数 + sweep)→ **编排器 + 入口**
(resume 语义)→ **GUI + 文档**。

## 2. DB 层

### 2.1 表(schema.rs,随既有 `CREATE TABLE IF NOT EXISTS` 块追加)

```sql
CREATE TABLE IF NOT EXISTS group_chat_checkpoints (
  session_id   TEXT PRIMARY KEY,
  round        INTEGER NOT NULL,
  error_streak INTEGER NOT NULL DEFAULT 0,
  started_at   TEXT NOT NULL,   -- RFC3339,行生命周期内不可变
  updated_at   TEXT NOT NULL
);
```

- 一场讨论至多一行(session_id 主键,upsert 语义)。
- FK/级联:跟随仓内既有 sessions 引用表的实际先例(messages 等——实施时核对
  schema.rs 现状,有 FK 就 `REFERENCES sessions(id) ON DELETE CASCADE`,没有就
  依赖 sweep 的天然 join + delete_session 侧显式清理,与先例一致即可)。

### 2.2 函数(落 `db/sessions/session_crud.rs` 群聊 lifecycle 区,紧邻
clear/finalize)

| 函数 | 语义 |
|---|---|
| `upsert_group_chat_checkpoint(pool, session_id, round, error_streak)` | `INSERT ... ON CONFLICT(session_id) DO UPDATE SET round, error_streak, updated_at`——**不动 started_at**(行生命周期 = 一场讨论;新场由 R1 的开跑删行保证 started_at 重置) |
| `get_group_chat_checkpoint(pool, session_id) -> Option<GroupChatCheckpoint>` | resume 命令校验 + 测试用 |
| `delete_group_chat_checkpoint(pool, session_id)` | 终局退出 + 新场开跑 |
| `recover_group_chat_checkpoints(pool) -> RecoverReport` | boot sweep 两步:① 标中断 `UPDATE sessions SET stop_reason='interrupted' WHERE stop_reason IS NULL AND id IN (SELECT session_id FROM group_chat_checkpoints)`——**不写 `updated_at`**(见 §7 权衡);② 清孤儿 `DELETE FROM group_chat_checkpoints WHERE session_id IN (SELECT id FROM sessions WHERE stop_reason IN ('group_chat_end','preempted','max_rounds'))`(自愈「finalize 成功但删行失败」的残留,`recover_interrupted_messages` 的 orphan 修复同思路)。返回两计数供日志 |

类型 `GroupChatCheckpoint { session_id, round: i64, error_streak: i64, started_at,
updated_at }` 进 db/types.rs(SessionRow 区)。

### 2.3 sweep 挂点

`AppState::load_from_dir` → `load_inner` 内的崩溃恢复块(state.rs:367-405,
`reap_orphaned_runs` / `recover_interrupted_messages` 两个 warn-only 幂等先例
之后、catalog 构建之前)——daemon bin 与 Tauri Full 共享,且先于 backup / HTTP
handler(既有 ordering note:recovery 先跑,任何请求观察不到 pre-recovery 状态)。
失败仅 warn 不阻断启动。

### 2.4 FK

跟随全仓先例(messages / turn_trace / scheduled_tasks 均
`REFERENCES sessions(id) ON DELETE CASCADE`,schema.rs:176 等):checkpoint 表
同款 CASCADE,delete_session 自动清行。

## 3. 编排器(group_chat_loop.rs)

### 3.1 新参数

```rust
pub struct GroupChatResume { pub start_round: usize }   // streak 恒归零,不携带
```

`run_group_chat_loop(..., resume: Option<GroupChatResume>)`(暂定尾参追加,调用点
仅 chat.rs:703 一处)。

### 3.2 循环改动

- `let start_round = resume.as_ref().map(|r| r.start_round).unwrap_or(0);`
  `for round in start_round..MAX_ORCHESTRATION_ROUNDS`。
- round 0 转录分支:`if round == 0 && resume.is_none() { messages.clone() } else {
  reload_messages(...) }`——resume 空尾条绝不进 D-D 持久化路径。
- 开跑序列(在 clear_group_chat_lifecycle 旁):`resume.is_none()` 时先
  `delete_group_chat_checkpoint`(对称清场;与 clear 一样 best-effort warn)。
- **轮头 upsert**:在 inject drain / preempt 检测之后、moderator 轮之前——
  `upsert(round, consecutive_error_turns)`。crash 窗口 = 当前轮内,至多重跑该轮。
- **streak 同步**:GC5 计数每次变更后(moderator 侧 / participant 侧检查点)
  `upsert(round, streak)`。写放大 ≤ 3 次/轮 × 30 轮,SQLite 本地,忽略不计;
  不做变更检测(无条件写,简单优先)。
- **退出**:finalize 后按 `stop_reason_str` 分流——`cancelled` / `error` 留行
  (upsert 最终轮值),`group_chat_end` / `preempted` / `max_rounds` 删行。
  cancel 路径同样适用(行保留 = 可续跑,正是 Q2 语义)。

### 3.3 moderator 恢复指令(group_chat_prompts.rs)

`pub fn moderator_resume_instruction() -> &'static str`——`moderator_wrapup_instruction`
同款追加式;命中条件 `resume.is_some() && round == start_round`(仅恢复后首个
moderator 轮)。文案要点:讨论因中断在第 N 轮恢复;继续主持既有议题,勿重新开场、
勿重述已完成的发言;按 nominate/end 既有规则推进。**已知边缘(评审 P2-3,接受)**:
该轮 moderator 若未 nominate(空转轮)指令即消失——后续轮靠转录上下文自愈,
不加「保留到首次实际提名」的复杂度。

### 3.4 重跑粒度(评审 P2-1,明示)

轮头 upsert 意味着崩溃窗口 = 当前轮内至上一轮完成:若崩溃发生在上一轮全部落库
之后、本轮 upsert 之前,resume 会**重跑完整上一轮**——reload 全量历史,重跑的
moderator 看到自己上一轮的仲裁文本,该轮发言在转录中近似重复。轮头粒度的固有
取舍(Out of Scope 已接受),AC3 live 观察重复度。

### 3.5 恢复后的竞态与残留

- controls 条目是编排器入口新建的(默认 false/空),无陈旧 preempt/inject 残留。
- resume 进入也走 `clear_group_chat_lifecycle`——'interrupted'/'cancelled'/'error'
  旧值先清,再由本轮终局重写(与 GC2 复用语义一致)。
- 续跑再 crash:轮头 upsert 重建行 + 下次 boot sweep 再标 interrupted,幂等闭环。
- roster 续跑时按当前 metadata 重解析(文档明示:中断期间改参与者配置会生效)。
- 双 resume 竞态(评审 P2-2):3a 兜底能收敛(后到者 cancel 先到者、先到者
  finalize cancelled 落行、最终被后到者收官值覆盖;窗口内 busy=true 使消费方
  不误判)——经典 chat 同款竞态面,接受;GUI 按钮提交后防抖(§5)消双击主因。

## 4. 入口层

### 4.1 ChatEntry 扩展(chat.rs)

```rust
pub(crate) resume_group_chat: Option<usize>,   // Some(start_round);全部既有构造点传 None
```

- 路由临界区:resume 请求 messages 为空 → injectable=false 天然跳过注入分支;
  群聊分支 break 'routing 走 legacy(3a 防御取消 + 认领 + preflight + spawn 全复用)。
  并发双 resume 的竞态面 = 经典 chat 同款(3a 兜底),接受。
- spawn 群聊分支:`resume: entry.resume_group_chat.map(|r| GroupChatResume {
  start_round: r })` 透传。

### 4.2 resume_group_chat 三件套

- **`commands/chat.rs`(或新 commands/resume.rs)`resume_group_chat_inner(state,
  session_id)`**:① load 校验 session 存在且 `session_type == group_chat`;
  ② !busy(session_active_request);③ `get_group_chat_checkpoint` 存在;
  ④ round < MAX_ORCHESTRATION_ROUNDS;⑤ **stop_reason 不为终局三值**
  (`group_chat_end` / `preempted` / `max_rounds`——评审 P1-1:行存留是
  best-effort,「finalize 成功但删行失败」的残留会让 API 直调推翻已收官场的
  summary,与 Q2 冲突;该兜底 + sweep 孤儿清理双保险);⑥ 构造
  `ChatEntry{messages: vec![], resume_group_chat: Some(row.round), ...}` 调
  chat_inner。
- **daemon 路由** `POST /api/v1/agent/resume_group_chat`(routes/agent.rs,chat
  路由同款 sink 构造),body `{session_id}`,返回 ChatAcceptance。
- **Tauri command** `resume_group_chat(session_id)`:注册**两处**——lib.rs
  `invoke_handler`(lib.rs:211,真注册面)+ commands/mod.rs `all_command_names()`
  (mod.rs:92,测试名单;评审勘误:mod.rs 并非 invoke_handler 所在)。
- 四类校验失败错误文案:非群聊 session / 讨论进行中 / 无可续跑断点 / 预算已耗尽。

## 5. GUI 层

- **TS 数据通路(评审 P0-1,必修)**:`SessionSummary`(chat.types.ts:466)与
  `LoadedSession.session`(streamRehydrate.ts:76)现**均无** `stop_reason` 字段
  (wire 有值、类型不可见,vue-tsc 必拦)——补 `stop_reason?: string | null`
  (通知文案需要则一并补 `discussion_summary?: string | null`)。
- **刷新时点(评审 P0-1B)**:`finalizeRequest` 只本地翻 `summary.busy=false`,
  `reloadAfterFinalize` 只重拉消息缓冲——终态 stop_reason 不进内存 `sessions[]`,
  按钮不出现直到下次 list_sessions。修法选①:`reloadAfterFinalize` 在
  load_session 返回后把 `loaded.session` 的受控字段(stop_reason /
  discussion_summary / updated_at 等)合并回对应 `sessions[]` 条目(08-31
  「finalize 后 DB 权威重拉」先例注释同区)。
- **可见性门**:`session_type==='group_chat' && !busy && stop_reason ∈
  {'interrupted','cancelled','error'}`。
- **按钮**:群聊 chip 区(M3 打断按钮同区),可续跑态出现;**提交后防抖**
  (disable 至 acceptance/busy 翻转——评审 P2-2:点击到首个 SSE 事件前无本地
  忙标记,双击即双 resume);点击 → `transport.resumeGroupChat(sessionId)` →
  `Started` 受理 → SSE 照常跟随(streamController 零改动,rid 为新请求)。
- **通知文案**:中断态一行通知「讨论已中断,可续跑」——**不带时间戳**(精确
  时刻在 checkpoint.updated_at,不随 list_sessions 出wire;评审 P1-2 回避
  时间源分歧),并明示「发送新消息将开始新讨论,不再续跑」(评审 P2-4)。
- 前端测试:按钮门矩阵(stop_reason 四态 × busy 两态抽界)、受理路径、防抖、
  通知文案、finalize 合并回写。

## 6. 兼容与迁移

- 新表 `CREATE TABLE IF NOT EXISTS` = 存量库 probe no-op,零数据迁移。
- wire 纯叠加:stop_reason 新值 `interrupted`(消费方「!busy + stop_reason≠null
  → 终态」派生天然吸收);ChatAcceptance 不变(resume 返回 Started)。
- 旧版 daemon / 旧 MCP 面对新库:表无人读,零影响。

## 7. 权衡记录

- **行存留编码可续跑性**(而非独立 resumeable 列):一行一状态,门 = 行在 + !busy;
  stop_reason 只作展示。避免「行在但 stop_reason=cancelled 到底可不可续」双真源。
- **轮头粒度 checkpoint**(而非 speaker 轮后/消息级):转录本就持久,丢的只有
  「当前轮进行到哪」;重跑一轮的代价(一次发言)远低于细粒度断点的复杂度。
- **boot sweep 写库**(而非读时派生):list_sessions 免 join(热路径),M1/M2
  消费方零改动;代价 = 中断标记延迟到下次启动(而中断的定义就是「进程没了」,
  标记时机天然只能在重启时)。
- **sweep 不写 `sessions.updated_at`**(评审 P1-2):SessionList 按 updated_at
  分组排序 + 展示时间(SessionList.vue:20/187)——写 boot 时刻会让全部 crashed
  session 一起顶到侧栏最前且显示「刚刚」;不写则 updated_at ≈ 最后消息落库时刻
  ≈ 真实中断时刻(轮头密集,误差一轮内)。通知文案不带时间戳,回避时间源分歧。
- **stop_reason 终局拒绝 + sweep 孤儿清理双保险**(评审 P1-1):「行存留编码
  可续跑性」依赖删行 best-effort 成功;命令侧第五校验堵 API 直调,sweep 侧
  清残留行恢复编码可信。
- **3.2 streak 同步的无条件写**:变更检测的分支复杂度 > 每 micro-write 的成本。

## 8. 回滚

三 PR 各自可独立 revert;DB 层 revert 后表残留无害(无人读写);GUI revert 后
resume 面退化为 API-only。
