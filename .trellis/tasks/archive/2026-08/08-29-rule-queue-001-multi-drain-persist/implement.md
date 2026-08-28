# Implement — RULE-QUEUE-001 多 drain 丢消息根治

## WP1 后端修复(单 PR)

- [x] `chat_loop/suite.rs`:`ChatLoopRequest.origin: Option<TaskOrigin>` →
  `drained: Vec<QueuedMessage>`;字段 doc 写不变量(非空 ⇒ 尾条 = messages 尾条
  user;非驱动器路径恒空)。
- [x] init.rs:`task_origin` 改派生 `request.drained.last()`;尾条 persist 块之前
  插入非尾条 persist 循环(design §4:split_last / role 防御 / skip_persist /
  RULE-A-003 失败语义 / 信封 gate 与形状 / seq 自增)。
- [x] 构造点收口 5 处:`chat.rs:733`(经典)、`group_chat_loop.rs:346/540`、
  `subagent/dispatch/drive.rs:134`、`tests_common.rs:399` → `drained: Vec::new()`。
- [x] 驱动器 chat.rs:move `drained` 进请求(extend 之后),更新 :1119 附近
  「persist 只写尾条对齐」注释为根治后口径。
- [x] 测试 `tests_message_queue.rs`:改写钉行为测试(design §6 第一条)+
  新增 3 条 multi-drain 全 manual 对照(三行全落 / seq 有序 / metadata 全 None)。
- 验证:`cargo test -p everlasting --lib` 全量(2076 passed)+ `cargo clippy -p
  everlasting --lib -- -D warnings` + `cargo fmt` 全绿。

## WP2 spec 收口 + 销账

- [x] `.trellis/spec/backend/agent-loop-architecture/pattern-message-queue-driver.md`:
  驱动器 loop 伪代码 persist 分工注记 + Tests 节补两个新用例名。
- [x] `.trellis/spec/backend/scheduled-tasks.md` §origin 载体链:链路第 3 环改为
  `ChatLoopRequest.drained`;删除「多 drain 时只有尾条 origin 生效」条款;§6
  测试清单同步改写后用例名。
- [x] `.trellis/spec/backend/agent-loop-architecture/signature-run-chat-loop.md`:
  ChatLoopRequest 字段表补 `drained`(注明替代原 `origin` 尾条字段)。
- [x] `.trellis/reviews/DEBT.md`:删 §RULE-QUEUE-001 条目(P2 区归零 + 闭合注记)。

## 验证命令速查

```bash
cargo test -p everlasting --lib          # 全量(~2000+,需 PKG_CONFIG_PATH)
cargo clippy -p everlasting --lib -- -D warnings
cargo fmt --all -- --check
```

## 回滚点

单 commit;无 schema / wire / 迁移面,revert 即回滚。
