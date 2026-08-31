# implement:提示词头部易变注入下沉

前置:阅读 `research/evidence-cache-head-volatility.md` 与 `prd.md`;spec 见 implement.jsonl。

## 执行清单(按序)

- [ ] 1. D1 breadcrumb 下沉:`inject.rs::append_workflow_breadcrumb` / `append_delegation_template` 注入目标从 `first_mut()` 改为 `last_mut()`(user-role Blocks guard 保留);`drive.rs:939` 调用点不变。
- [ ] 2. 同步测试:改断言 messages[0] → 最后一条消息;新增 wire 级回归:构造三轮序列(含一次状态迁移),断言第 2、3 轮请求的 `messages[0..1]` + system 字节相同。
- [ ] 3. D3 head_sha 出 system:移入尾部状态 Text 块(与 breadcrumb 相邻);更新 RULE-A-005 相关注释为新不变量;`tests_prompts.rs` 相应断言更新。
- [ ] 4. D2 instruction 冻结:init 读到的四块缓存进 session 上下文,同 session 复用;编译期 kill-switch;测试:模拟同 session 第二次请求 + 磁盘文件已改 → wire 头部仍用首次内容。
- [ ] 5. D5 日志:usage 到达时 cache_read==0 && input>50k → WARN。
- [ ] 6. D4 调查(可后置独立 commit):定位 tools=0 调用方;turn-smoke 验证;结论写回本文件与 research/。
- [ ] 7. 验证:`cargo test -p everlasting --lib`(PKL 环境变量见 AGENTS.md);`scripts/turn-smoke.sh` 实跑;群聊/worker/workflow 既有测试不回归。

## 验证命令

```bash
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib
./scripts/turn-smoke.sh
```

## 回滚点

每个编号步骤独立 commit;D2 有编译期开关。
