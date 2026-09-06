# implement.md — 群聊 P0 打断最小语义

## 执行清单(有序)

### A. 后端:控制注册表 + 注入通道

- [x] A1 `state.rs`:`AppState.group_chat_controls` 字段(`Arc<Mutex<HashMap<String,
      GroupChatControl>>>`),锁纪律注释(最后获取);`state.rs` 两处 AppState 形态
      (GUI/sidecar 共用构造点)同步。
- [x] A2 `db/sessions/`:`insert_user_inject(pool, session_id, text, seq)`——
      user 行 + `metadata.kind="user_inject"` + text 列带 `[用户插入] ` 前缀;
      注释引 compaction seq 纪律(仅限无活跃游标时调用 = 编排器轮头独占)。
- [x] A3 `chat.rs` 'routing 临界区:群聊 busy 分支(§2.1)——push 缓冲 +
      `ChatAcceptance::Injected`;防御分支(busy 但注册表缺条目)warn + legacy。
      spawn 侧:群聊分支注册 controls 条目(同 cancellations 时序)。
- [x] A4 `chat.rs`:`ChatAcceptance::Injected` 变体;daemon `routes/agent.rs`
      wire 序列化 + Tauri command 返回路径(五处接线清点:command 注册 /
      all_command_names / daemon route / 前端 controller 类型)。

### B. 后端:编排器(轮头消费 + 收束状态机)

- [x] B1 `group_chat_loop.rs`:签名加 `controls: GroupChatControl`(或句柄);
      Drop guard 保证全退出路径清理注册表条目(cancel/error/normal 全覆盖)。
- [x] B2 轮头:drain `pending_injects` → `insert_user_inject` 逐条落库
      (MAX(seq)+1,此时无活跃游标);退出路径 flush 残余(best-effort + warn)。
- [x] B3 preempt:轮头检测 `preempt_requested` → 收束轮(prompt 追加段:
      立即 end_discussion 总结、勿 nominate)→ 命中 = `STOP_REASON_PREEMPTED`
      + summary;未命中/报错重试 1 次 → 强制立断兜底(stop_reason 仍 preempted,
      summary 缺);收束轮 ERROR 不进 GC5 熔断计数。
- [x] B4 `group_chat_prompts.rs`:moderator prompt 注入段(`[用户插入]`
      语义指引)+ 收束轮指令段;`tests_group_chat_prompts` 补纯函数断言。

### C. 后端:preempt 命令

- [x] C1 `commands/`(新文件或 group_chat 相关文件):`preempt_group_chat(
      session_id)`——注册表查不到报「无进行中的讨论」;查到置位返回 Ok。
- [x] C2 五处接线:Tauri command 注册 / all_command_names / daemon route
      (仿 cancel.rs 先例)/ capabilities 如需 / DAEMON-API §4 文档
      (stop_reason 补 `preempted` + inject 语义 + Injected acceptance)。

### D. 前端

- [x] D1 `chatSendActions.ts`:去掉群聊 busy 先-`await cancel()` 分支(D9-Q4
      语义退役,注释改指向本任务);busy 注入路径直接 send。
- [x] D2 `Injected` acceptance 处理:消息即时渲染(乐观入列,同 Queued 先例)、
      无流占位、busy 态持续。
- [x] D3 stop_reason=`preempted` 终态处理:finalize 白名单 + notice 文案
      (streamController / chat store done handler,同 error 位)。

### E. 测试(机制层 MockProvider,C3.1 先例)

- [x] E1 注入可见性:busy 场注入 → 下一 moderator 轮 prompt 含 `[用户插入]`
      文本(断言 moderator 视图),讨论不终止(busy 持续 / stop_reason 不变)。
- [x] E2 seq 纪律:participant burst 进行中注入 → 在途轮 persist 不被破坏、
      注入行落在 burst 后(回归锚:无 UNIQUE 冲突)。
- [x] E3 preempt 收束:置位 → 在途轮跑完 → 收束轮 end_discussion →
      stop_reason=preempted + discussion_summary 有值。
- [x] E4 preempt 兜底:收束轮两次不 end → 强制立断,stop_reason=preempted,
      summary 缺。
- [x] E5 可区分性:preempted / cancelled / group_chat_end 三态落库值互异。
- [x] E6 生命周期:cancel/error/正常终路径注册表条目必清(防二跑残留);
      复用 session 二跑 controls 重新注册。
- [x] E7 前端:chatSendActions busy 群聊发送不再调 cancel(mock controller
      断言零 cancel 调用)+ Injected 渲染;经典聊排队路径回归不动。
- [x] E8 兼容:经典聊 3a 路径既有测试全绿(零改动验证)。

### F. 文档 + spec 沉淀

- [x] F1 DAEMON-API:§4 stop_reason 表补 `preempted`;注入/busy 语义 +
      `Injected` acceptance;`preempt_group_chat` 端点。
- [x] F2 `.trellis/spec/backend/`:schema 决议(user_inject 双轨标记 + 编排器
      独占落库纪律 + controls 注册表生命周期)。
- [x] F3 ROADMAP / GROUP-CHAT-API-ROADMAP §6:M3 前置 P0 完成记账;BUGLIST
      无新条目(本任务是能力新增非缺陷)。

## 验证命令

```bash
# 机制层(全量 lib)
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib
# 定点冒烟(单次调用内过滤)
cargo test --lib "tests_group_chat::"        # 编排器家族
cargo test -p everlasting --lib "tests_group_chat_prompts::"
# Lint / 前端
cargo clippy -p everlasting --all-targets -- -D warnings
cd app && pnpm test && pnpm vue-tsc
# 行为层(prompt 结构变更,R5)
scripts/turn-smoke.sh
# 可选 live(烧 token,一场群聊 + 中途注入/preempt 各一次;机制层全绿后再考虑)
node scripts/group-chat-run.mjs run --preset review --topic "..."
```

## 风险文件 / 回滚点

| 文件 | 风险 | 缓解 |
|---|---|---|
| `agent/chat.rs` 路由临界区 | 锁纪律破坏 / 经典聊回归 | 分支仅群聊进入;E8;review 盯锁序 |
| `agent/group_chat_loop.rs` | 生命周期清理遗漏(cancel/error 路径) | Drop guard(结构化,不靠手工);E6 |
| `stores/chatSendActions.ts` | D9-Q4 退役影响既有 e2e | e2e 群聊用例盘点;跑 pnpm test:e2e |
| seq 撞主键(E2 场景) | 直插被否决后理论消除,但 flush-on-exit 仍走 MAX+1 | flush 仅在无活跃游标处调用,注释锁定 |

回滚:任务为独立 commit 族,revert 即净;无 DB 迁移、无 config 变更。

## start 前检查

- [x] prd.md 收敛(决策内嵌,无未决 open question)
- [x] design.md / implement.md 齐备
- [ ] 用户审批规划工件(阻塞项)
- [x] `task.py start`(审批后)
