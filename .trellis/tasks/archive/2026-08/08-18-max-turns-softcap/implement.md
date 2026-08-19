# Implement — MAX_TURNS 软卡化

> 前置阅读:prd.md(决议表)、design.md(§2 契约 / §5 测试设计)。
> 排序原则:后端数据面(variant/audit)→ 循环重构 → force 穿参 → 前端 → 测试 → 验证。

## Checklist

### PR1 后端数据面(无行为变化)

- [x] `question_store.rs`:`InteractionKind::TurnLimitSoftcap`("turn_limit_softcap")
  + `PendingInteraction::TurnLimitSoftcap(ToolQuestionPayload)`(+ `kind()`
  match 臂;`as_str` 的 dead_code 属性同现有变体处理)。
- [x] `permissions/audit.rs`:`AuditKind::TurnLimitSoftcap` + `as_str` +
  `record_turn_limit_softcap_audit(db, session_id, turn, budget, action, seq)`
  (best-effort,warn+swallow,镜像 record_loop_intervention_audit)。
- [x] `cargo test -p everlasting --lib "question_store\|audit"`(现有单测不破)。

### PR2 循环重构 + 软卡询问(chat_loop.rs)

- [x] 提取 `emit_max_turns_terminal(...)`(现 1055-1084 的 persist + Done 体);
  worker break 落点改调它,行为字节级等价。
- [x] `for turn in 1..=turn_limit` → `loop` + `turns_budget` / `turn` /
  `force_compaction` / `softcap_terminal_emitted`;per-turn 体原封搬入;
  撞线判定 `turn > softcap_boundary()`(env `EVERLASTING_SOFTCAP_TURN_BOUNDARY`
  缺省回退 turns_budget)。
- [x] `SoftcapOutcome` + `ask_turn_limit_softcap(...)`:payload 条件构建
  (compaction_on 三支/两支)、`asked` audit、四臂 biased select
  (cancel / sleep(timeout) / rx)、`AlreadyPending` → warn + 停止。
- [x] 常量 `TURN_LIMIT_GRANT = MAX_TURNS`;`softcap_ask_timeout()`
  (env `EVERLASTING_SOFTCAP_TIMEOUT_MS`,缺省 600s)。
- [x] worker 臂:`effective_is_worker` → break(不进询问);循环后
  `record_worker_messages` 保留;`softcap_terminal_emitted` 时跳过
  `emit_max_turns_terminal` 但不跳 worker 捕获。

### PR3 force 压缩(drive.rs)

- [x] `drive_turn` 增参 `force_compaction: bool`;gate 条件加
  `force_compaction ||`(design §2.2,仅绕 token 线);chat_loop 在
  drive_turn 返回后置 false(一次性)。
- [x] `attempt_summary_compaction` 增参 `trigger_label: &'static str`;
  metadata `"trigger"` 改用参数(auto 调用点传 `"auto"`)。
- [x] `cargo test -p everlasting --lib "tests_agent_loop::"`(现有测试绿;
  预期 basic.rs:822 需同步改造——见 PR5)。

### PR4 前端

- [x] `streamEvents.ts`:前缀 `turn_limit_softcap_` → kind 标签。
- [x] `questionCards.types.ts`:kind 联合 + payload 类型。
- [x] `ChatPanel.vue`:`turnLimitSoftcap` computed + 浮动卡模板块
  (镜像 loopIntervention;settled removePending)。
- [x] `utils/audit.ts`:kind 联合 + 筛选 label「轮数软卡」+ 渲染 case。
- [x] `cd app && pnpm test`(现有前端测试绿;另加 streamController
  turn_limit_softcap kind routing 镜像用例,vue-tsc --noEmit 干净)。

### PR5 测试(softcap.rs + basic.rs 改造)

- [x] 新建 `tests_agent_loop/softcap.rs`(mod.rs / agent::mod 列表注册),
  七个用例见 design §5(continue / stop / timeout / cancel / compact-force /
  gate-off 两选项 / resolver kind 分流复验)。
- [x] 改造 `agent_loop_max_turns_emits_done_marker`(basic.rs:822):resolver
  按 `get_payload().kind` 分流——loop_intervention 答「继续」、
  turn_limit_softcap 答「停止」;断言不变(恰好一次 Done{max_turns}、
  send_count=MAX_TURNS)。
- [x] 前端如涉及 kind tagging 的既有测试(如有)同步。

### PR6 全量验证 + spec

- [x] `cd app/src-tauri && PKG_CONFIG_PATH=... cargo test --lib`(全量,禁 --test-threads=1)。
  2026-08-19 实测:1836/1838,61s。2 个失败为 main 既有并行 flaky
  (`agent_loop_dispatch_subagent_guard_does_not_evict_parent_session_active` /
  `plan_mode_write_denied`)——stash 基线复现同样失败、隔离单跑均过,
  与本任务无关。群聊软卡误入修复(chat_loop.rs `group_chat_state.is_some()`
  break 门 + `softcap_group_chat_breaks_without_ask` 回归用例)后,全量
  从 30 分钟(10min 超时挂起)回到 61s。
- [x] `cd app && pnpm test`(1108/1108,74 文件)。
- [x] live 冒烟:daemon 起服 + `EVERLASTING_SOFTCAP_TURN_BOUNDARY=2` 实跑
  `scripts/turn-smoke.sh` 链路,确认真实弹卡与三分支(手动 QA 记录到任务目录)。
  2026-08-19 PASS:见 `research/live-smoke-plan.md` 顶部结果(turn3 撞线 →
  kind/tool_use_id/三选项正确 → HTTP resolve 停止 → audit asked→stopped
  落库 → 尾部干净 → session 清理 → daemon 恢复无 env 状态)。
- [x] spec 更新(Phase 3.3):`agent-loop-architecture/` 新增
  `pattern-turn-limit-softcap.md` + index 挂链(软卡结构、超时臂、force
  穿参、audit action 集);`audit.ts` 前端 label 对应。

## 验证命令速查

```bash
# 后端定向
cargo test -p everlasting --lib "tests_agent_loop::softcap::" -- --nocapture 2>/dev/null || \
(cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" \
  cargo test --lib "tests_agent_loop::softcap::")
# 后端全量(最后跑)
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib
# 前端
cd app && pnpm test
```

## 风险与回滚点

- **chat_loop.rs 循环重构**是最大风险面:per-turn 体必须原封搬入(仅循环骨架
  变化);PR2 单独成 commit,出错整 commit revert。
- drive_turn 签名加参:机械性,编译器兜底。
- basic.rs:822 改造后若挂起,优先怀疑 resolver 未分流新 kind
  (timeout 10min 兜底会让测试显得"极慢"而非死锁——看到长时间挂起先查 env 泄漏)。
- 回滚:PR1-PR5 各自独立 commit,revert 对应层即可;无 schema/config/协议迁移。
