# Implement — F1 消息队列(用户连发档 MVP)

> 前置:design.md §1-§8。执行前先走 `trellis-before-dev`(backend + frontend spec)。

## 有序清单

### PR1 后端队列 + 入队分流

- [x] 1. 新建 `app/src-tauri/src/agent/message_queue.rs`:`QueuedMessage` 结构(id: uuid/text/attachments/enqueued_at/priority 占位)+ per-session `VecDeque` 容器 trait + 内存 impl(照 `background_shell::BackgroundShellRegistry` 模板);`SESSION_QUEUE_MAX = 20`;`drain_all` 破坏性出队 + `remove(id)` / `recall(id)` 按 uuid 寻址;单元测试(FIFO 序 / 上限拒 / 清空 / not-found)。
- [x] 2. `AppState` 加 `session_message_queues` 字段(state.rs,数据面组内,catalog-after-db 不变式不触碰)。
- [x] 3. `chat_inner` 统一数据路径改造(design §2):所有发送一律入队;「查忙 + 入队 + 注册/spawn」收进同一把路由锁临界区(**临界区内不得 await**);忙 → 返回 `{queued:true, position}`;闲 → 注册 rid + spawn 驱动器,响应保持 unit 形状。group_chat 与 defense-in-depth 路径保持取消替换(替换时清队列)。开关 `message_queue_enabled`。
- [x] 4. 驱动器续轮循环(spawn 体改造):正常结束 → drain 非空则 persist(drained)为初始 user 输入再进 `run_chat_loop`;cancelled → 清空;break 条件按 design §4 矩阵(错误终止/续轮上限触顶 → 队列保留)。**只在真结束 emit Done**;每次续轮内层 run 开始前 emit 新 `ChatEvent::TurnContinuation`(design §3,群聊 `Speaker` 同位置同角色)。**退出协议(反搁浅)**:拿路由锁 → 队列空才注销 `session_active_request` 退出,非空继续循环。续轮上限常量 50。
- [x] 5. 取消/清理接线:`cancel_chat_inner`、`delete_session_inner`、`clear_session_messages` 处清队列;graceful shutdown 无需处理(内存态)。
- [x] 6. R8 IPC 三件 + cancel 返回值扩展(Tauri command + daemon 路由 1:1,design §7):`list_queued_messages` / `remove_queued_message` / `recall_queued_message`(返回原文);`cancel_chat` unit → `{cleared_queued_count}`;drain 窗口 not-found → Err;集成测试(撤销后不注入 / recall 返回原文 / not-found 错误 / cancel 返回清空计数)。

### PR2 排队消息注入路径

- [x] 7. 续轮注入实现:drained 消息经 init.rs 现有 persist 链落库(seq 游标顺延,D-D guard 语义核对——排队项未落过库,不应触发 guard;attachments 若有,核实落盘时序从 turn 启动提前到入队时的最简做法)。
- [x] 8. 注入形态:每条独立 `role:user` APPEND 到请求尾部(与 drive.rs 通知循环同构);确认 wire 层零改动(to_wire 连续 user 消息直通已有先例)。
- [x] 9. 后端集成测试(harness 用 `make_harness_with_git_repo` / fake provider 先例)。**覆盖实况(Round 2 如实化,原勾含未覆盖项)**:
  - ✅ 忙时入队 → 当前轮结束后自动续轮,2+ 条消息按序注入、逐条落库;
  - ⬜ 错误终止后滞留项 + 新发送 → 统一路径下 FIFO 全队一起注入(P1-1 顺序回归;统一数据路径构造性保证,断言留 PR4 live);
  - ✅ 每次续轮内层 run 前 `TurnContinuation` 事件到达(事件序断言)+ 内层 Delta 到达前端 sink(Round 2 补,P0 回归锁);
  - ✅ Stop(cancel)→ 队列清空,无续轮,cancel 返回清空计数;
  - ✅ 错误终止 → 队列保留、无续轮;⬜ 续轮上限触顶分支(live);
  - ⬜ 反搁浅退出协议断言(窗口竞态,live);
  - ⬜ group_chat 忙时第二请求取消替换(AC4;chat_inner 层未动 + 既有群聊测试锁行为,live 复验);
  - ⬜ 空闲路径(AC3;既有 harness 快照不改 = 测试侧成立,chat_inner 生产路径差异见 design §8 修正口径,live 复验);
  - ✅ 上限 20 拒绝(队列模块单测)。

### PR3 前端解锁 + 排队视图 + 续轮渲染

- [x] 10. 编辑器解锁:`ChatInput.vue` sendDisabled / readOnly compartment 判定改 `sending && isGroupChat`(chatInputCodeMirror.ts compartment effects);Esc 规则 = 经典流式中**编辑器为空才触发 Stop**,非空不触发(design §6,P2-5);群聊回归测试锁定现行为。
- [x] 11. `send()` 分支(chatSendActions.ts):移除经典 session 流式 early-return;处理忙时 `{queued:true, position}` 返回 → 本地占位标 queued 徽标;闲时 unit 返回不变;store 新增 per-session `queuedMessages` Map(chat store,state-management spec:不进 streamController)。
- [x] 12. **续轮渲染适配(P1-2)**:streamEvents 新增 `TurnContinuation` case —— 尾部排队 user 占位物化为普通气泡(去徽标)+ push 新 assistant 占位 + `sealActiveThinking`/`flushPendingTimelineText`(照 start 分支模式);**不改** `:66-67` 守卫、**不泛化** `:93` groupChat 门控(start 是 run 内 LLM 调用边界,泛化会拆散多工具轮);`httpTransport.invoke` 透传 chat 返回值(P2-1 transport 层一行)。
- [x] 13. R8 UI:排队气泡 hover 撤销(×)/修改(退回 composer)按钮 → 调 remove/recall IPC + 占位移除/回填;切 session 与页面加载时 `list_queued_messages` 水合视图;not-found toast「已开始处理」。
- [x] 14. toast 集:Stop/edit/resend/retry「已丢弃 N 条」(N = cancel 返回的 cleared_queued_count);@@ 前缀流式中拒绝 toast(P2-4);error Done 后「N 条排队消息保留,下次发送时注入」(本地队列计数)。
- [x] 15. vitest:解锁判定 / queued 分支 / **续轮 TurnContinuation 物化 + 新 assistant 占位 + delta 不再被 `:67` 丢弃(定点用例)** / 撤销与退回 / 水合 / 各 toast / Esc 空编辑器规则 / 群聊不变;`vue-tsc --noEmit` 零错。

### PR4 收尾验证

- [ ] 16.(live 冒烟待重编 daemon 后执行:`./scripts/daemon.sh stop && cargo build --release -p everlasting --bin everlasting-daemon && ./scripts/daemon.sh start`;curl REST 排队分支 + 手工连发场景) clippy + cargo fmt + pnpm build 全绿;CI 双 job 本地预演。
- [ ] 17. live 冒烟(AC2 cache 断点):重编 daemon → `scripts/turn-smoke.sh --turns 2` 对照双轮 cache 率不劣化(基线同 session 形态下取);手工场景:长 turn 中连发 3 条 → 续轮按序注入、逐条气泡呈现、单条撤销不再注入、Stop 清空 toast 正确;curl 打 `POST /api/v1/agent/chat` 验证 REST 排队分支(d4f G-3)。
- [ ] 18. 文档归档:ROADMAP F1 行标注部分落地(A 档)+ ARCHITECTURE 输入侧 gate 小节 + tool-contract/agent-loop spec 沉淀注入契约(TurnContinuation 事件语义)+ IMPLEMENTATION 决策日志。

## Round 2 修复(评审 review-glm 后,2026-08-25)

- [x] **P0 DriverSink 转发**:`emit_chat_event` 按 `forward` 返回值转发,Error 分支去自转发(防双发);单测 2 例(常规事件穿透序 / Error 恰一次)+ driver 集成测试补 Delta 到达断言(单轮 + 续轮两处)。
- [x] **P1 寻址改 uuid**:`ChatAcceptance::Queued` 加 `id`(wire additive);前端占位 `queued:{id,position}`;`revoke`/`recallToComposer` 按 id 直达;`dropQueuedPlaceholder` 删占位并重排位次(消除 position 漂移导致的撤错条/静默失效)。
- [x] **P1 回填**:ChatInput 改 watch `recallDraft`(当前 session 内 recall 立即回填,消除"切回才幽灵回填")。
- [x] **P2 水合可见性**:`hydrate` 返回 entries → ChatPanel 接 `materializeQueuedPlaceholders`(按 queued.id 去重物化占位,恢复刷新/LRU 驱逐/PWA 第二端后的可见性)。
- [x] **P3**:turn_continuation 补 `sealActiveThinking` + flush(design §3 ③ 防御兜底);驱动器退出 session slot 注销加 rid 守卫;design §8 AC3"逐字节"口径修正;#9 覆盖标注如实化。
- [ ] **遗留**:PR4 #16-18(live 冒烟含 curl REST 排队分支 + 文档归档)仍待执行 —— P0 类缺陷只有真机能兜底。

## 验证命令

```bash
# 后端(根目录)
PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test -p everlasting --lib
cargo clippy -p everlasting --all-targets
# 前端
cd app && pnpm test && pnpm build   # 含 vue-tsc
# live
./scripts/turn-smoke.sh --turns 2
```

## 风险文件与回滚点

| 文件 | 风险 | 回滚 |
|---|---|---|
| `agent/chat.rs`(chat_inner 分流 + spawn 改驱动器) | 核心链路,79 路由共用 | 整 PR 单 commit revert |
| `agent/chat_loop/init.rs`(persist 时序) | seq 撞号 / D-D guard 误伤 | persist 逻辑内聚在续轮分支,revert 不影响常规轮 |
| `stores/chatSendActions.ts` | 全部发送入口 | 开关 + early-return 恢复一行 |
| `utils/chatInputCodeMirror.ts` | compartment 改判读,群聊锁行为 | 判定条件还原即回滚 |

## task.py start 前检查

- [ ] prd/design/implement 三件套齐且相互引用一致
- [ ] 内联工作流(trellis-before-dev 加载上下文),JSONL 门不适用
- [ ] 用户已明确批准最终规划摘要(本轮尚未批准)
