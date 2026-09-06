# Implement — 群聊 P1a checkpoint 落库与续跑

> 三个 PR 各自独立可提交;PR1(DB)→ PR2(编排器+入口)有依赖,PR3(GUI+文档)
> 依赖 PR2 的 wire。验证命令统一从仓库根跑;Rust 测试需 PKG_CONFIG_PATH(见
> AGENTS.md / docs/HACKING-wsl.md 坑 1)。

## PR1 — DB 层:表 + 四函数 + boot sweep

- [x] schema.rs 追加 `group_chat_checkpoints` 建表块(FK 走 CASCADE,全仓
      sessions 引用表先例已核:schema.rs:176 messages 等)
- [x] db/types.rs:`GroupChatCheckpoint` struct
- [x] db/sessions/session_crud.rs:upsert / get / delete /
      `recover_group_chat_checkpoints`(两步:标中断 **不写 updated_at** + 清
      孤儿行,SQL 见 design §2.2)
- [x] state.rs `load_inner` 崩溃恢复块(:367-405,
      reap_orphaned_runs / recover_interrupted_messages 之后):调 recover,
      两计数 `info!`,失败 warn 不阻断
- [x] 单测(db/sessions_tests/session_crud.rs,clear/finalize round-trip 用例旁):
      upsert round-trip + started_at 不可变;delete;sweep 只写 stop_reason=NULL
      的行、**不动 updated_at**、不覆盖已有终态、孤儿行(终局 stop_reason 残留)
      被清、返回计数;session 删除级联清行

验证:`cargo test -p everlasting --lib "session_crud"` → 全量 `--lib`

## PR2 — 编排器 resume 语义 + resume 命令

- [x] group_chat.rs:`GroupChatResume { start_round }`
- [x] group_chat_loop.rs:尾参 `resume: Option<GroupChatResume>`;
      `start_round` 进入 for;round-0 分支加 `&& resume.is_none()`;
      开跑 `resume.is_none()` → delete checkpoint(best-effort);
      轮头 upsert;GC5 两处 streak 变更后 upsert;退出按 stop_reason 分流
      留行/删行(cancelled/error 留,其余删)
- [x] group_chat_prompts.rs:`moderator_resume_instruction()`(追加式,对齐
      `moderator_wrapup_instruction` 形态与测试)
- [x] chat.rs:`ChatEntry.resume_group_chat: Option<usize>`(全部既有构造点补
      None——routes/agent.rs、Tauri command、scheduler fire 路径,构造点逐一
      grep 确认);spawn 群聊分支透传 GroupChatResume
- [x] commands:`resume_group_chat_inner`(**五类校验**:非群聊 / busy / 无行 /
      round≥MAX / stop_reason 终局三值,错误文案见 design §4.2)
      + Tauri command 注册**两处**:lib.rs `invoke_handler`(lib.rs:211)+
      commands/mod.rs `all_command_names()`(mod.rs:92)
- [x] daemon routes/agent.rs:`POST /agent/resume_group_chat`(chat 路由同款
      sink 构造;Body {session_id};返回 ChatAcceptance)
- [x] 编排器单测(tests_group_chat.rs,既有 fake provider / MockEmitter 架子):
      - resume 进入 start_round:moderator 首轮吃 reload(非空尾条)、prompt 带
        恢复指令、后续轮不带
      - 轮头 upsert 与 streak 同步(崩溃模拟:直接断言行值随轮推进)
      - 退出分流:cancelled/error 留行(round=终局轮)、group_chat_end/
        preempted/max_rounds 删行
      - 新场开跑删旧行(复用 session 二跑不残留旧 checkpoint)
      - 命令五类校验拒绝路径(含:**手造「终局 stop_reason + 残留行」→ resume
        被拒**——模拟删行失败残留,评审 P1-1 回归锚)
- [x] daemon 路由测试:resume 端点 happy path + 校验错误(仿 cancel.rs 路由
      测试形态)

验证:`cargo test -p everlasting --lib "tests_group_chat"` → 全量 `--lib` +
clippy -D warnings + fmt

## PR3 — GUI + 文档 + 脚本

- [x] chat.types.ts `SessionSummary` + streamRehydrate.ts `LoadedSession.session`
      补 `stop_reason?: string | null`(+ 通知需要的 `discussion_summary`)
- [x] streamEvents.ts `reloadAfterFinalize`:load_session 返回后把
      `loaded.session` 受控字段合并回 `sessions[]` 对应条目(评审 P0-1B 刷新点)
- [x] transport/http.ts:`resumeGroupChat(sessionId)`
- [x] chat store / chatSendActions:resume action(受理 Started → 既有 SSE 跟随;
      streamController 零改动)
- [x] ChatPanel.vue 群聊 chip 区:可续跑态门(design §5)+ 「续跑」按钮
      (提交后防抖)+ interrupted 通知行(无时间戳 + 「发新消息将开始新讨论」提示)
- [x] 前端单测:按钮门矩阵(busy × stop_reason 四态)、action 受理、防抖、
      通知文案、finalize 合并回写
- [x] DAEMON-API.md:stop_reason 值表补 `interrupted`(语义:进程级中断,可续跑,
      boot sweep 时点)+ `/agent/resume_group_chat` 端点 + lifecycle 三态机补
      中断态与可续跑集
- [x] scripts/group-chat-run.mjs:`EXIT_BY_STOP_REASON` 补 `interrupted`(专属
      exit code,现四值外下一空位)+ 转录 note「session 可经 resume_group_chat
      续跑」;scripts/group-chat-mcp.mjs `isTerminal` 的 JSDoc「四值枚举」顺手
      更新(代码 open-ended 零改动)
- [x] GCE-ROADMAP §4/§6:P1a/P1b 落账(M3 信任底座余项收口);ROADMAP §1.2 加行

验证:`cd app && pnpm test` + `vue-tsc` + `pnpm build`

## Live 验证(AC7,PR2 合入后、收官前)

1. `scripts/daemon.sh start`(根 target/release 二进制,勿用 app/src-tauri/target
   陈旧产物——08-21 B1 教训)
2. `node scripts/group-chat-run.mjs run --preset arch --topic "..."` 起一场;
   中途(speaker 轮进行中)**`kill -9 $(cat <pidfile>)` 直杀**——注意
   `daemon.sh stop` 是 SIGTERM 走 graceful shutdown → 编排器 finalize
   `cancelled`,**测不出 interrupted**(评审 §1.12 核实;pidfile 路径见
   daemon.sh PID_FILE);可另跑一场用 daemon.sh stop 做 cancelled 对照
3. 重启 → sqlite3 -readonly 查 stop_reason='interrupted' + checkpoint 行
   (round 停在中断轮)+ sessions.updated_at 未被 sweep 改写
4. curl POST /agent/resume_group_chat → 轮询 busy/stop_reason → group_chat_end;
   discussion_summary 完整;checkpoint 行已删;观察重跑轮转录重复度(AC3)
5. 转录导出留档 `out/group-chat-p1a-resume-<date>.md`

## 风险点 / 回滚

- **group_chat_loop.rs 是全任务最高风险文件**(M3/08-04 多轮重写史):改动全部
  加在既有锚点旁(轮头序列、GC5 检查、退出尾部),不动 turn-taking 主结构;
  回归锚 = `cargo test --lib "tests_group_chat"`(pattern spec 要求)。
- prompt 结构变更(恢复指令)按 pattern 要求跑一场 live(AC7 即覆盖)。
- PR 各自可 revert(见 design §8);PR1 revert 后表残留无害。
- ChatEntry 加字段的构造点遗漏 → 编译期兜住(pub(crate) struct 无默认构造)。

## 收官前自查

- [x] AC1-AC7 逐条对账(证据:测试名 + live 转录)
- [x] spec 回写:pattern-group-chat-preempt-inject.md 补 P1a 段(或独立
      pattern-group-chat-checkpoint);database-guidelines RULE-PERSIST-001 场景
      与本任务同构(load_inner 三恢复 pass 并列),交叉引用
- [x] start 前:implement.jsonl / check.jsonl 各补真实 curated 条目(本会话为
      inline 实施,workflow 对 sub-agent 平台才强制——顺手满足门禁,评审 P2-5)
- [x] journal 记录 + `/trellis:finish-work`
