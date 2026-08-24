# Implement: turn 流式持久化 + 崩溃恢复

> 执行顺序按依赖排列;每个 WP 独立可验证、可回滚。WP1→WP2→WP3 串行,WP3(R3 前端)待范围确认。

## WP1 存储层(schema + db 函数 + 恢复 pass + 单测)

- [ ] `db/migrations/schema.rs`:`add_messages_column_if_missing(pool, "status", "TEXT")` + 部分索引 `idx_messages_status`(对齐既有列添加的位置/注释体例)
- [ ] `db/models.rs` `MessageRow` 加 `status: Option<String>`;`load_session` / 各 SELECT 核对列清单(**widen 先例教训:表约束/列清单改动禁 SELECT \* 拷贝,显式列清单**)
- [ ] `db/sessions/messages.rs`:
  - [ ] 提取 `persist_turn` 的内容派生私有 helper(JSON/text/has_* 计算)
  - [ ] `upsert_in_progress_turn` / `finalize_turn_persist`(ON CONFLICT DO UPDATE,见 design §3.1)/ `delete_in_progress_turn`
  - [ ] `recover_interrupted_messages(pool) -> RecoveryReport`(Step A + Step B + touch_session,design §3.3)
  - [ ] `INTERRUPTED_MARKER` 常量放 `agent/helpers.rs`(挨着 CANCELLED/ERROR marker)
- [ ] `state.rs:311` 后接恢复 pass(best-effort 壳 + 日志形态对齐 reap)
- [ ] 单测(新 `db/messages_checkpoint_tests.rs` 或并入既有 messages_tests):design §7 单元清单全项;**file-backed 建池**
- 验证:`cargo test -p everlasting --lib "db::" `+ `cargo test -p everlasting --lib "recover"`(全绿)
- 回滚点:纯新增,git revert 单 commit 即净

## WP2 写点接线(drive.rs + 集成锁)

- [ ] `TurnCheckpoint` 结构 + `CHECKPOINT_INTERVAL`(drive.rs)
- [ ] stream ready 后占位写(design §2.1-1;`!skip_persist` 门)
- [ ] Delta/ThinkingDelta 臂时间门检查点(只读克隆快照,design §2.2)
- [ ] drive.rs:1567 assistant 落库点改 `finalize_turn_persist`;`assistant_blocks.is_empty()` 分支补 `delete_in_progress_turn`
- [ ] 集成测试(tests_agent_loop):
  - [ ] 正常多 turn → 无 in_progress 残留 + 既有内容断言不动(error_persist 加 status=NULL 断言)
  - [ ] cancel mid-stream → 检查点被终态覆盖(cancel marker 仍在)
  - [ ] AC2 模拟崩溃:MockProvider 流中 return 后不走收尾路径的等效构造(或直接对 harness 池手工写 in_progress 行再跑 recover,分解为 AC2=DB 层已覆盖 + AC3/AC4 集成)
  - [ ] AC4:孤儿尾行 → recover → 后续请求上下文含合成 tool_result(不 400)
- [ ] worker 回归:`cargo test -p everlasting --lib "subagent"`(skip_persist 路零改动验证)
- 验证:`cargo test -p everlasting --lib`(全量,对照基线 flaky 名单)+ fmt + clippy 零新增
- 回滚点:WP2 依赖 WP1;revert WP2 commit 后 WP1 函数成 dead code(可 `#[allow(dead_code)]` 过渡或一并 revert)

## WP3 前端自愈(R3,待范围确认)

- [ ] `app/src/transport/http.ts` / `streamController.ts`:注册 `stream-resync` 监听
- [ ] 处理:streaming 中的 activeRequests → 本地中断终结(不弹 error toast)→ 当前 session `ensureLoaded(force)` 重拉
- [ ] TS interface `status?: string | null`(snake_case 镜像)
- [ ] vitest:fake 事件 → 占位清空 + load_session 调用断言;无 activeRequests 时 no-op
- 验证:`cd app && pnpm test` + `pnpm build`(vue-tsc)
- 回滚点:独立 commit,纯前端

## 收尾门(全任务)

- [ ] **最终全量 check**(2.2 last-iteration full-scope):`cargo test -p everlasting --lib` + `cargo test -p everlasting-remote` + `cd app && pnpm test` + vue-tsc + fmt + clippy 对照基线
- [ ] live 冒烟:`scripts/turn-smoke.sh`(需要 daemon;**必须根 target/release 二进制,单命令内联起停** —— 08-24 键盘/按钮任务两次实证的 harness 坑)
- [ ] 崩溃真演(手动证据,可选但推荐):起 daemon → turn-smoke 流式中 `kill -9` → 重启 → sqlite3 查该 seq 行含检查点内容 + status 流转 → 记入 evidence
- [ ] spec 回写(3.3):agent-loop-architecture(检查点写点契约)、database-guidelines(恢复 pass Scenario / upsert 边界)、llm-contract(孤儿修复与 pair atomicity)
- [ ] DEBT.md:删 RULE-PERSIST-001 条目(闭合)
- [ ] DEBUG_DB.md:messages.status 列 + 恢复查询示例(文档回归,对齐 08-20 模式)

## 基线与已知坑

- 基线 flaky:`tests_subagent` 2 例(08-24 备份任务 stash 对照确认,失败名先行记录)
- WSL cargo 需 `PKG_CONFIG_PATH`(AGENTS.md);测试多线程默认,勿加 `--test-threads=1`
- sqlx :memory: 池 VACUUM 静默 no-op 坑(backup.rs 实证)→ 恢复测试 file-backed
- daemon 在本 harness 内联起停,setsid 也不逃(08-24 双任务实证)
