# Implement — 执行计划

> 三段 commit:PR1(WP1 事件全链)/ PR2(WP2 归因列 + 聚合 IPC)/ PR3(WP2 前端)。每段独立可回滚。

## PR1 — WP1:ChatEvent::TurnUsage 全链

- [x] `llm/types/event.rs`:新 `TurnUsage` 变体(docstring 注明 read-only / 非持久化 / worker 进 transcript 不冒泡)。
- [x] `chat_loop/drive.rs` Done 臂(`drive.rs:1322` 块):upsert 后 emit(emit 不进 DB 错误分支);主/worker 同点,worker 切片按既有 NULL 契约传 None。**注意双层门边界(评审 P2)**:`if let Some(t) = usage` + `!skip_persist || !run_key.is_empty()` 门内——usage=None 与 worker run_key='' 降级不发事件,测试断言按此写。
- [x] Rust 单测:harness 实跑一轮,断言事件字段 == upsert 值(AC1,仅正常完成轮);worker 路径事件只进 buffer sink 不进主 chat(AC2;**不是每个 worker 轮必有事件**,run_key='' 降级路径无);mock DB 失败时事件仍发(AC3,测试内让 upsert 挂——参考 trace.rs best-effort 测试模式,若难注入则降为代码审查点并记录)。
- [x] `types/turnTrace.ts`:`TurnUsageEvent` wire 类型 + applyEvent 联合扩员。
- [x] `traceStore.ts`:applyEvent `turn_usage` 分支,merge 语义 upsert(保留已有 compaction/loopHint/breadcrumb)。
- [x] `streamEvents.ts`:`case "turn_usage"`(budget_trim 防御模板,malformed drop)。
- [x] 前端测试:live 事件 upsert 后 TurnTrace 字段齐(AC1);非当前 session 事件被 gate(AC2);malformed drop。
- [x] `scripts/turn-smoke.sh` live:**扩展脚本支持 SSE 订阅断言**(评审 P4:现状 `/agent/chat` 响应 `>/dev/null` 只轮询 DB,断言不了事件)——加 `--assert-turn-usage` 开关:并发订阅 SSE 流捕获 `kind=turn_usage`,断言字段非空且与 DB 落值同值(同时覆盖 AC1 一致性 + AC8);订阅参考 `scripts/remote-e2e-smoke.mjs` 的 SSE 消费。**脚本既有列存在性检查清单(`turn-smoke.sh:286-305` 模式)为 provider_id 加一行**(PR2 落地后)。

## PR2 — WP2:归因列 + 聚合 IPC

- [x] `db/trace.rs` + `migrations/schema.rs:944` 绿地 CREATE + `migrations/schema_helpers.rs:290` 重建模板 + `db/trace.rs:851` 测试内 CREATE **三处同步**:`provider_id TEXT NULL` 加列(`add_turn_trace_column_if_missing` helper,`columns.rs:190` 先例);加列排在 `rebuild_turn_trace_with_run_id` **之后**(评审 P3 顺序守则;若必须在重建前,则重建拷贝列清单同步补列——以 `list_worker_turn_traces` 那次为模板)。
- [x] migration 回填:session join 近似 UPDATE(一次性,幂等——`WHERE provider_id IS NULL`;join 不到的行留 NULL 进 unknown 桶)。
- [x] **provider_id 穿参(评审 P1 修正,不走 LoopInit)**:
  - 主路径:`ResolvedChatProviderWrapper`(`chat.rs:770`)加 `provider_id` 字段(`lookup_provider_for_session` 内 ModelRow 在手,`chat.rs:650`)→ `chat_inner` → `run_chat_loop` 新参(仿 `worktree_override` 先例)→ `drive_turn`。
  - worker 路径:`resolve_worker_provider`(`resolve.rs:151`)返回值扩 provider_id(catalog 命中分支内 `get_model` 已 fetch,`resolve.rs:168`,一行取得;miss/inherit 分支 = None)→ `WorkerModel`(`dispatch/model.rs:45`)加字段 → `run_subagent` 穿入 worker `run_chat_loop`。
- [x] `upsert_turn_trace_token` 扩第 12 参 `provider_id: Option<&str>`;调用点同步(Done 臂;compaction/loop_hint 旁路写点不涉 provider,不扩)。
- [x] `db/usage.rs`(新):per-provider 累计 / 主-worker 拆分 / 小时分布 / top sessions 四查询(window_hours 绑参 modifier,不拼串;NULL `token_usage_json` 行 `json_extract` 返 NULL、SUM 忽略,天然安全)。
- [x] `commands/usage.rs`(新):`usage_window` + `set_quota_settings`(config 表 `quota_window_hours` 缺省 5 / `quota_limit_tokens` 可 NULL);**注册链 5 处 + 测试锚**:daemon routes + `post()`、`lib.rs` invoke、`commands/mod.rs` 白名单、`http.ts CMD_TO_DOMAIN`(POST)、`http.test.ts` 回归锚。
- [x] Rust 集成测试:造跨窗口边界 + worker run 行 + 双 provider fixture,断言聚合与手工 SQL 一致、窗口滚动正确(AC4);config 键读写(AC5);provider_id=NULL 行归 unknown 桶。

## PR3 — WP2 前端:QuotaChip + 弹层

- [x] `stores/quota.ts`(新):`useQuotaStore`(state + fetch + setSettings)。
- [x] `<QuotaChip>` + `<QuotaPopover>`(reka-ui Popover):provider 段 / 小时分布 CSS 柱 / top sessions(`openSessionInProject` 跳转)/ 配额设置行。
- [x] AppHeader 集成(trace toggle 旁)+ 移动端降级(<430px 收缩/入溢出区,弹层全屏,S6a 档先例)。
- [x] streamController `reloadAfterFinalize` 尾部:fire-and-forget `quotaStore.refresh()`。
- [x] 前端测试:未设 limit 只显示消耗 / 设了显示占比(AC5);top sessions 渲染(AC6);store 测试;`http.test.ts` CMD_TO_DOMAIN 锚含 `usage_window`。
- [x] `pnpm build` + 手动过 AppHeader 布局(chip 挤占回归)。

## 验证命令

```bash
# 后端(WSL 需 PKG_CONFIG_PATH,见 AGENTS.md)
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo clippy --lib   # 零新增(基线既有除外)
cargo fmt --check

# 前端
cd app && pnpm test && pnpm build          # build 含 vue-tsc

# live 冒烟(AC1 / AC8)
scripts/turn-smoke.sh
sqlite3 -readonly ~/.local/share/dev.everlasting.app/everlasting.db \
  "SELECT provider_id, COUNT(*) FROM turn_trace GROUP BY provider_id"   # 归因落值抽查
```

## 风险文件 / 回滚点

- `drive.rs` Done 臂:高密度注释区,改动只加 emit + 1 参,**不动既有门**(`if let Some(t) = usage` / `!skip_persist || !run_key.is_empty()` 双层门与切片 NULL 契约)。
- `upsert_turn_trace_token` 扩参:db/trace.rs 既有调用点(含 compaction/loop_hint 旁路写点)签名兼容检查。
- `schema_helpers.rs` + `schema.rs` + `db/trace.rs:851` 测试 CREATE **三处同步**(表约束加宽守则,spec)——加列走幂等 helper,不动 UNIQUE。
- 穿参链 `chat.rs` wrapper → `run_chat_loop` 新参 → `drive_turn`:机械扩参,`run_chat_loop` 已有 25+ 参先例(`worktree_override` 模式)。
- 回滚:PR1/PR2/PR3 各自 revert;PR2 的列留下无害(NULL 容忍)。

## start 前检查

- [x] prd.md 收敛版无 open question(placement 已拍板 AppHeader chip + 弹层)。
- [x] implement.jsonl / check.jsonl:本任务 inline 工作流,跳过 JSONL gate(Phase 2 走 trellis-before-dev)。
- [x] spec 前置阅读:`.trellis/spec/backend/token-usage-tracking.md` + `agent-loop-architecture/pattern-*` + `frontend/state-management.md`。

---

## 完成记录(2026-08-21)

- **PR1**:event 变体 + Done 臂 emit + 防御臂 + **sink 跳过**(实现期实证修正:不挡则 worker transcript 每轮多一条无人消费记录,`persists_subagent_run` 3→4 断言破);Rust 4 新测试(turn_usage_event×2 + sink×1 + dispatch 4 元组断言);前端 TurnUsageEvent 类型 + applyTurnUsage merge upsert + turn_usage 路由(防御 + context_input_tokens 归一化兜底),traceStore 3 新测试。
- **PR2**:provider_id 列(绿地 CREATE + 重建后 one-shot helper 含回填 + missing-table 守卫;**重建模板不加列**——评审 P3 的"三处同步"实证修正为两处,db/trace.rs:851 是老形状夹具本就不该加);upsert 12 参;穿参链 wrapper→run_chat_loop(第 32 参)→drive_turn + 群聊(parent 默认 + per-speaker resolve_provider 元组化)+ worker(resolve_worker_provider 四元组 + display 回填块同源取 provider_id);db/usage.rs 聚合(**实现期发现 `sessions.input_tokens_total` 为孤儿列**(snapshot 重构后无写点),session 累计改 turn_trace 全量聚合,PRD AC6/背景已同步);usage IPC 双命令 + /api/v1/usage 路由组 + 注册链 + http.test.ts 锚;聚合 2 集成测试。
- **PR3**:useQuotaStore + QuotaChip(手写 popover 模式,design 原写 reka-ui 已按 house spec 修正)+ AppHeader 挂载 + reloadAfterFinalize 尾部 fire-and-forget refresh + 4+4 测试。
- **turn-smoke**:`--assert-turn-usage` 开关(SSE 捕获 + 字段断言 + 事件↔DB 同值 + provider_id 抽查)+ provider_id 列存在性检查。
- **验证**:后端 1878 passed + 2 failed(**clean-tree stash 对照同挂** = 预存 load-flaky,归因干净:subagent_guard + plan_mode_write_denied,solo 均过);前端 1144 + build(vue-tsc)绿;clippy 零新增(lib 1 / tests 3 均预存);fmt clean;live 冒烟 `--assert-turn-usage --keep` 全过(1 event captured, seq-consistent, provider_id=0548f76e…)+ 真库 usage_window IPC 非零(per-provider/hourly/topSessions 含 projectId)。
- **运维注记**:daemon 启动必须由存活的 run_in_background 任务前台承载(`daemon.sh start`),detach 模式会被工具调用进程组 SIGTERM(journal 既有坑,本次复证)。

## 质检记录(trellis-check,2026-08-21)

- **改动面盘点**:50 改 + 10 新(后端 event/穿参/迁移/聚合/IPC + 前端 store/组件/类型 + turn-smoke + spec 沉淀)。
- **🔴 Step 5 跨层审查抓到 2 个真 bug(单测全绿拦不住的 HTTP 模式专属缺陷),均已修 + live 复验**:
  1. `set_quota_settings` 嵌套 `request:` 参数 + daemon 路由结构体 camelCase rename —— httpTransport 顶层 camel→snake 转换下字段整体静默 miss(HTTP 模式表现为"保存永远被重置为缺省")。修正:command 扁平标量参数 + 路由结构体 snake_case 不 rename + 前端扁平 args(`quota.ts:111`)+ http.test.ts 锚同步。铁律沉淀 `spec/frontend/transport-and-pwa-modes.md`(命令形状 + 违例示例)。
  2. 清额度分支裸写 `DELETE FROM config` —— 实表 `app_config`(live 500 "no such table: config" 实证)。修正:db/config.rs 新增 `delete_config_value`(schema 知识归 db 层);live 复验 clear 200 + limitTokens null 读回,set 6h/2M → 读回 roundtrip 亦过。
- **套件终态**:后端 1878 passed + 2 failed(仅 2 个已知 load-flaky,与本次改动无关,clean-tree 对照归因);前端 1144 passed + build(vue-tsc)绿;clippy 零新增;fmt clean。
- **live 冒烟**:`turn-smoke.sh --assert-turn-usage` 全过(SSE 事件字段完备、事件↔DB seq 同值、provider_id 落值归因正确);`/api/v1/usage` 真库非零响应(per-provider/hourly/topSessions 含 projectId)。
- **视觉**:`ui-review.sh --screenshots-only` 7 截图,VLM 复核桌面(chip ⚡5h+数字,无重叠错位)与移动 <430px(降级 ⚡+5h,无溢出)双过。
- **spec 沉淀**:`transport-and-pwa-modes.md`(命令形状铁律)、`token-usage-tracking.md`(孤儿列勘误:`sessions.input_tokens_total/output_tokens_total` 自 snapshot 重构后无写点,session 累计必须聚合 turn_trace)。
