# Review Backlog — 跨 review 债务整合

> **目的**: 集中追踪所有 review(审计 / SPEC 对照 / 历史 review)的 finding,避免下次 audit 重新独立复述
>
> **基线审计**: `docs/_reviews/REVIEW-agent-loop-full-audit-2026-06-14.md`(commit `a4fb302`)
>
> **创建**: 2026-06-14(由 `.trellis/tasks/06-14-review-debt-consolidation` 启动)

---

## 新增 finding 流程

> **重要**: 任何新 audit / review / spec 对照,**第一步必须 diff 本文件**。

### 添加新 finding

```markdown
### RULE-{Subsystem}-{Seq}

- **Level**: P0 | P1 | P2 | P3
- **Subsystem**: Agent Loop | Permission | Memory | Provider | Tools | Cross
- **File**: `path/to/file.rs:LINE`
- **Description**: 一句话描述问题
- **Fix**: 修复方向(行数估算)
- **Status**: open | in_progress | closed
- **Owner**: carlos | 待分配
- **Related Task**: `.trellis/tasks/XX-YY-name` 或 null
- **Discovered In**: `docs/_reviews/REVIEW-XXX.md`
- **Closed At**: commit hash 或 null
- **Related PR**: #N 或 null
```

### 流程规则

1. **不重新展开已记录 finding**: 新 audit 中遇到已记录的 RULE-X-XXX,**只标一行** `// See DEBT.md §RULE-X-XXX`,不重新描述 file:line 和影响
2. **闭合时填 commit**: PR merge 后必须更新 `Closed At` 和 `Related PR`
3. **优先级重审**: 每次 audit 可重新评估,但需在本文件 `Re-evaluation Log` 段记录理由
4. **ID 一旦分配不变**: 即使 finding 后续证明不是问题,ID 保留但 Status 标 `wontfix`

### 复述检测

如果新 audit 复述了某条 finding 但未引用 DEBT.md:
- **轻度**: review 本身不扣分,但应在结论段标注"漏查 DEBT.md"
- **重度**: 如果是 P0/P1 漏查,review 应被打回修订

---

## 优先级分布

| Level | Count | 说明 |
|---|---|---|
| P0 | 5 | 安全 + 数据完整性,必须尽快修复 |
| P1 | 12 | 正确性 + 资源,影响功能或可靠性 |
| P2 | 20 | 健壮性 + 债务,中长期清理 |
| P3 | 8 | 文档 + 一致性,可延后 |
| **Total** | **45** | 含历史 review 合并 |

---

## P0 — 必须尽快修复(安全 + 数据完整性)

### RULE-A-001 — C3 tail pair orphan

- **Level**: P0
- **Subsystem**: Agent Loop
- **File**: `app/src-tauri/src/agent/context.rs:334-381`
- **Description**: `group_droppable_turns` tail pair 边界可能产生 orphan tool_result,代码注释自承 "Under heavy pressure this leaves an orphan tool_result";当 assistant(tool_use) 紧邻 protected tail 时被当 singleton drop,而 tail user(tool_result) 保留
- **Impact**: 极压场景撞 Anthropic 400 "tool_result without matching tool_use",整 chat 崩
- **Fix**: 把 tail-adjacent assistant(tool_use) 纳入隐式保护,补触发该路径的单测
- **Status**: **closed (2026-06-14)** — `group_droppable_turns` orphan 分支改 skip(隐式保护 tool_use-bearing assistant,不再 singleton drop),context.rs:433-459;3 新单测 + 1 改名
- **Owner**: carlos
- **Related Task**: 待开 `06-14-p0-c3-tail-pair-orphan`
- **Discovered In**: REVIEW-agent-loop-full-audit-2026-06-14 §2.1

### RULE-A-002 — compact_messages 超窗静默不丢

- **Level**: P0
- **Subsystem**: Agent Loop
- **File**: `app/src-tauri/src/agent/context.rs:160-260`
- **Description**: `compact_messages` 全部 middle 丢完仍超 target 时**静默不丢**,greedy drop 循环无"still over after compaction"错误返回
- **Impact**: 单条超大 tool_result(shell dump/read_file)单独构成 tail 时,超窗仍发给 LLM 撞 `prompt is too long`
- **Fix**: 全丢完仍超 target 时返回 Result/emit Error,而非静默继续
- **Status**: **closed (2026-06-14)** — `CompactResult` 加 `degradation: DegradationKind`,全 droppable 丢完仍超窗返回 `StillOver { tokens_after, target }`;chat.rs + chat_loop.rs(副本同步)`match degradation` → emit `ChatEvent::Error { InvalidRequest }` + 早返回,不静默发超窗 prompt
- **Owner**: carlos
- **Related Task**: 待开 `06-14-p0-c3-tail-pair-orphan`(同 PR)
- **Discovered In**: REVIEW-agent-loop-full-audit-2026-06-14 §2.1

### RULE-E-001 — shell 子进程继承父进程全部环境变量

- **Level**: P0
- **Subsystem**: Tools
- **File**: `app/src-tauri/src/tools/shell.rs:237`
- **Description**: `Command::new("sh")` **无** `env_clear()`,继承含 `ANTHROPIC_API_KEY` 的全部 env
- **Impact**: LLM 一句 `env`/`printenv` 即窃取 API key,配合 shell `curl -X POST` 外传。LLM agent 经典提权面。Permission Tier 4 ask 防"该不该执行",防不了"执行后内部窃密"
- **Fix**: `Command::new("sh").env_clear()` + 白名单注入(PATH/HOME/LANG/TERM/LC_*),排除 `*_API_KEY`/`*_TOKEN`(~10 行)
- **Status**: closed
- **Owner**: carlos
- **Related Task**: `.trellis/tasks/06-14-p0-shell-env-clear`
- **Closed At**: `2abd7a2`
- **Related PR**: (待创建)
- **Discovered In**: REVIEW-agent-loop-full-audit-2026-06-14 §2.5 + §3.1

### RULE-E-002 — shell 不 kill 进程组 → 孤儿进程

- **Level**: P0
- **Subsystem**: Tools
- **File**: `app/src-tauri/src/tools/shell.rs:79-99`
- **Description**: `child.kill()` 只 kill 直接子;`Command::new("sh")` **无** `process_group(N)`
- **Impact**: `sleep 60 &`/管道/`nohup` 产生的孤儿进程 cancel/timeout 后继续跑,资源累积泄漏
- **Fix**: `process_group(0)` + kill PGID(传 `-PID` 给 kill,Unix;Windows 用 `creation_flags`)(~15 行)
- **Status**: closed
- **Owner**: carlos
- **Related Task**: `.trellis/tasks/06-14-p0-shell-process-group`
- **Closed At**: `29e2ea8`
- **Related PR**: (待创建)
- **Discovered In**: REVIEW-agent-loop-full-audit-2026-06-14 §2.5 + §3.1

### RULE-E-003 — web_fetch redirect 不重做 IP 校验 → SSRF 绕过

- **Level**: P0
- **Subsystem**: Tools
- **File**: `app/src-tauri/src/tools/web_fetch.rs:385`
- **Description**: `Policy::limited(MAX_REDIRECTS)` 不重做 host resolution + IP check;docstring 自相矛盾(`:17` 写 "each redirect target" vs §5 "not implemented")
- **Impact**: `attacker.com → 169.254.169.254` 绕 SSRF 打 AWS metadata 泄漏 IAM 临时凭证
- **Fix**: 自定义 `Policy::custom` 每 3xx 重做 `lookup_host` + `is_blocked`(~30 行);同步修 spec 内部矛盾
- **Status**: closed
- **Owner**: carlos
- **Related Task**: `.trellis/tasks/06-14-p0-web-fetch-redirect-ssrf`
- **Closed At**: `4b46bc6`
- **Related PR**: (待创建)
- **Discovered In**: REVIEW-agent-loop-full-audit-2026-06-14 §2.5 + §3.1
- **Resolution Notes**: 实施 `redirect::Policy::custom` callback + 同步 `resolve_and_check_sync` (硬编码 `allow_private=false` 防止测试 bypass 漏到生产 redirect 路径),加 `WebFetchError::RedirectBlocked { from, to }` 变体。`web_fetch.rs:17` docstring 同步更新引用 `build_redirect_policy` 与 `resolve_and_check_sync`,DRIFT-002 矛盾已闭合。31 个 web_fetch 测试 + 469 后端测试全 pass。

---

## P1 — 重要(正确性 + 资源)

### RULE-A-003 — persist_turn 失败静默,DB 与内存永久分叉

- **Level**: P1
- **Subsystem**: Agent Loop
- **File**: `app/src-tauri/src/agent/chat_loop.rs`(迁移后;旧 `chat.rs:439-447/875-886/1205-1216` 行号已失效,见 RULE-A-006)
- **Description**: `persist_turn` 失败只 `tracing::error!` 后继续,无 Error 事件 emit
- **Impact**: 磁盘满/DB 锁竞争失败时,消息"发了回了"但下次打开 session 空白
- **Fix**: 失败时 emit Error 事件(兑现 REVIEW-sse P3)
- **Status**: **closed (2026-06-15)** — 5 处 `persist_turn` 失败路径分类处理:正常路径 3 处(初始 user `:263` / assistant turn `:513` / tool_result `:723`)→ `emit_persist_failure`(emit `ChatEvent::Error{Server}` 中文文案)+ `return`;cancel 路径 2 处(`:544`/`:687`)→ 保持 `tracing::error!`-only(避免与 cancelled `Done` 双终止事件冲突)。helper `emit_persist_failure` 在 `chat_loop.rs` 末尾。决策 = emit Error + 终止(对齐 RULE-A-002 StillOver),category 复用 `Server`(已验证前端不基于 category 分支,零前端改动)。集成测试 `agent_loop_persist_failure_emits_error`(`BEFORE INSERT ON messages` trigger 拦截,断言 call_count==0 + 1 Error + Server + 文案)。
- **Owner**: carlos
- **Related Task**: `.trellis/tasks/06-15-p1-persist-emit-error-and-audit-cancel-order`
- **Closed At**: `d8ee7d9`
- **Discovered In**: REVIEW-agent-loop-full-audit-2026-06-14 §2.1 + REVIEW-sse-agent-loop §8 P3(2026-06-12 提出,0 落地 → 2026-06-15 兑现)

### RULE-A-004 — record_tool_executed_audit 在 cancel 检查之前 → audit 撒谎

- **Level**: P1
- **Subsystem**: Agent Loop
- **File**: `app/src-tauri/src/agent/chat_loop.rs:643`(迁移后;旧 `chat.rs:1094-1116` 行号已失效)
- **Description**: audit 在 cancel 检查之前调用,cancel 短路的 tool 仍记一行 `tool_executed`
- **Impact**: 审计完整性破坏,追责和回放分析误导
- **Fix**: audit 提到 cancel 检查之后
- **Status**: **closed (2026-06-15)** — `record_tool_executed_audit` 块从 cancel 检查**前**移到**后**,用 `if token.is_cancelled() { cancelled = true; } else if audit {...}` 串联 —— cancelled 的 tool 不落 audit。两检查背靠背无 `.await`,token 状态一致。集成测试 `agent_loop_cancel_skips_audit_for_cancelled_tool`(turn 1 `list_dir` tool_use + cancel task `yield_now` gate call_count>=1,断言 `session_audit_events` 无 `tool_executed` 行)。
- **Owner**: carlos
- **Related Task**: `.trellis/tasks/06-15-p1-persist-emit-error-and-audit-cancel-order`
- **Closed At**: `d8ee7d9`
- **Discovered In**: REVIEW-agent-loop-full-audit-2026-06-14 §2.1

### RULE-A-006 — Agent Loop 集成测试缺口(MockProvider)

- **Level**: P1 (**前移到 PR5**)
- **Subsystem**: Agent Loop
- **File**: (新建测试模块)
- **Description**: Agent Loop turn 边界(cancel / max_turns / C3 触发 / orphan 配对)只能手动验证,无 mock HTTP server 驱动的完整 turn 集成测试
- **Impact**: P0 修复(RULE-E-001/002/003 + RULE-A-001/002)无回归保护,等于盲修
- **Fix**: `MockProvider` 实现 trait 返回预设 `Stream<ChatEvent>`,跑完整 chat 命令断言 messages + DB rows + events
- **Status**: **closed (2026-06-15)** — agent loop body 已统一,production `chat.rs` spawn 闭包 → `chat_loop::run_chat_loop` 调用,`#[allow(dead_code)]` 已去除,9 个 `agent_loop_*` 集成测试现覆盖 production 真实路径(原本只覆盖测试副本)。全套 484 tests pass
- **Owner**: carlos
- **Related Task**: ✅ `06-15-unify-chat-loop-dispatch` 闭环迁移
- **Discovered In**: REVIEW-agent-loop-full-audit-2026-06-14 §3.5 + REVIEW-sse-agent-loop §8 P5(2026-06-12 提出,0 落地)

> **Closure Note (2026-06-15)**: `.trellis/tasks/06-15-unify-chat-loop-dispatch` 已实施完成,`chat.rs` 缩减为薄 pre-flight 包装,agent loop body 全部路由到 `chat_loop::run_chat_loop`(接 `AppHandleSink`),副本彻底消除,drift hazard 闭合。
>
> - **真闭环**:`chat` Tauri 命令 spawn 后只调一次 `run_chat_loop(...)`,production 与 test 共享同一函数。改 agent loop body = 改 1 处,9 个 `agent_loop_*` 测试即测真实路径。
> - **覆盖范围**:RULE-A-003(persist 失败)/ RULE-A-004(audit 时序)/ RULE-E-005(worktree destroy)改的是 `run_chat_loop`,测试即生效(原本改 `chat.rs` 副本不同步则失效)。
> - **回归风险**:run_chat_loop 与原闭包 7 维度对等(C3 / MAX_TURNS / send / cancel / permission / audit / persist)由 06-14-p1 check 逐项核查;484 tests 全 pass,`cargo check` 0 warning,emit 序列与原闭包 1:1 对应(已迁移完毕)。

### RULE-B-001 — delete_session 不直接清理 permission_asks

- **Level**: P1
- **Subsystem**: Permission
- **File**: `app/src-tauri/src/commands/sessions.rs:126`
- **Description**: delete_session 只调 `cancel_inflight_for_session`,未调 `cancel_session_asks`(`mod.rs:330`,标 `#[allow(dead_code)]`)
- **Impact**: 实际不泄漏(biased select! 间接清理),但隐性依赖,`cancel_session_asks` 死代码误导维护者
- **Fix**: 先改 `cancel_session_asks` 按 session_id 过滤(RULE-B-002),再接入 delete_session
- **Status**: **closed (2026-06-16)** — `delete_session` 在 `await_inflight_exit` 后接入 `cancel_session_asks`(`commands/sessions.rs`);显式清理移除对 biased select! 的隐性依赖
- **Owner**: carlos
- **Related Task**: .trellis/tasks/06-16-p1-permission-asks-cleanup
- **Closed At**: `3b16528`
- **Discovered In**: REVIEW-agent-loop-full-audit-2026-06-14 §2.2

### RULE-B-002 — cancel_session_asks 是 map.clear() 全清(latent bug)

- **Level**: P1
- **Subsystem**: Permission
- **File**: `app/src-tauri/src/agent/permissions/mod.rs:330-341`
- **Description**: `_session_id: &str` 下划线前缀,body 直接 `clear()`,session_id 参数被忽略
- **Impact**: 当前安全只因未被调用。一旦未来接到 delete_session(RULE-B-001),会误清其他 session 的 pending ask
- **Fix**: 改函数签名接受并过滤,先修接口再接调用
- **Status**: **closed (2026-06-16)** — `PermissionStore` value 改 `PendingAsk{session_id,tx}`(rid key 不变,resolve 端 IPC 只传 rid 故 session 落 value);`cancel_session_asks` 用 `map.retain` 按 session 过滤(去 `#[allow(dead_code)]`);`register_ask` 加 session_id 参;删 `cancel_pending_asks` dead wrapper;+跨 session 隔离单测(cancel A 不动 B)
- **Owner**: carlos
- **Related Task**: .trellis/tasks/06-16-p1-permission-asks-cleanup
- **Closed At**: `3b16528`
- **Discovered In**: REVIEW-agent-loop-full-audit-2026-06-14 §2.2 + §3.3

### RULE-C-001 — memory watcher debounce 1s 窗口内 race

- **Level**: P1 (**C-P1-1 升级**)
- **Subsystem**: Memory
- **File**: `app/src-tauri/src/memory/watcher.rs:179-219` + `loader.rs:206-214`(watcher.rs 已删)
- **Description**: notify → 标 pending → 等 1s debounce → invalidate;read-through 无 fence
- **Impact**: 编辑器保存后立即下一条消息**概率性读到旧指令**。PRD 承诺"立即生效"是概率性非确定性,产品诚信问题
- **Fix**: 加 read-through fence 或 invalidate 时标记,read 检测 fence
- **Status**: **closed (2026-06-15)**
- **Owner**: carlos
- **Related Task**: `.trellis/tasks/06-15-p1-memory-watcher-appstate`
- **Closed At**: `759607c`
- **Discovered In**: REVIEW-agent-loop-full-audit-2026-06-14 §2.3 + §6
- **Resolution Notes**: brainstorm 核实发现 watcher **疑似完全失效**(`start_watcher` 返回值在 `state.rs:219` 被丢弃 → `RecommendedWatcher` drop → inotify handle 关闭 → debounce loop 因 tx drop 退出;严重性 >> 原"概率性 race",实为确定性读旧)。grill 决策 **W 方案**:砍 notify watcher,改 read-through **mtime fence**——`MemoryCache` slot 加 `CachedLayer { layer, mtime }`,`read_or_load_*` 每次 `tokio::fs::metadata` stat 比较 mtime,不符则 reload。read 路径成为 freshness 权威,无 debounce 窗口、无 watcher 依赖;C-002/C-004 自动满足。`watcher.rs` 整文件删 + `invalidate_*` API 删 + `notify` 依赖移除 + 前端 `memory:reloaded` dead listener 清理。4 个 fence 测试(change/hit/appear/vanish),489 tests pass。

### RULE-C-002 — 新建 project / memory 文件不自动 watch

- **Level**: P1
- **Subsystem**: Memory
- **File**: `app/src-tauri/src/state.rs:178-197`(project_paths 收集块已删)
- **Description**: 仅启动时收集一次 `list_projects`;运行时新增 project 目录不 watch(`watcher.rs:75-79` 注释承认)
- **Impact**: 新建 project 后写其 CLAUDE.md,memory cache 不失效(直到重启)
- **Fix**: `MemoryWatcher` 提升到 AppState,加 `add_watch(project_id)` API
- **Status**: **closed (2026-06-15)** — 自动满足:watcher 删除后 memory freshness 走 mtime fence;新 project 首次 `load_for_session` 即 stat 其文件,无需 watch/add_watch API。见 RULE-C-001 Resolution Notes。
- **Owner**: carlos
- **Related Task**: `.trellis/tasks/06-15-p1-memory-watcher-appstate`
- **Closed At**: `759607c`
- **Discovered In**: REVIEW-agent-loop-full-audit-2026-06-14 §2.3

### RULE-D-001 — API key 明文存储

- **Level**: P1
- **Subsystem**: Provider
- **File**: `app/src-tauri/src/db/migrations.rs:240` + `commands/providers.rs:38-42` + `db/providers.rs:62-82`
- **Description**: `api_key TEXT NOT NULL DEFAULT ''` 原样写读返回 IPC;`app_data_dir` 权限 0700 非绝对边界
- **Impact**: DB 文件泄露=全部 provider key 泄露
- **Fix**: keyring crate(macOS Keychain / Windows Credential Vault / Linux Secret Service)或应用层对称加密
- **Status**: open
- **Owner**: carlos
- **Related Task**: 待开 `06-14-p1-api-key-encryption`
- **Discovered In**: REVIEW-agent-loop-full-audit-2026-06-14 §2.4

### RULE-D-002 — OpenAI max_tokens 对 o1+ 协议错误

- **Level**: P1
- **Subsystem**: Provider
- **File**: `app/src-tauri/src/llm/provider/openai.rs:243-248`
- **Description**: 硬编码 `max_tokens`;o1/o3/o4-mini 要求 `max_completion_tokens`,发 `max_tokens` 会 400
- **Impact**: 用户配置 o1 model 后所有 chat 400
- **Fix**: `is_o1_family` 分支改用 `max_completion_tokens`(~10 行)
- **Status**: **closed (2026-06-16)** — `is_o1_family`(o1/o3/o4 前缀,case-insensitive) 分支:o1 family 用 `max_completion_tokens` 否则 `max_tokens`;+3 单测(matches/rejects/body 构造)+1 回归断言
- **Owner**: carlos
- **Related Task**: .trellis/tasks/06-16-p1-openai-o1-glob-spawn-blocking（与 RULE-E-004 合并实施）
- **Closed At**: `361336e`
- **Discovered In**: REVIEW-agent-loop-full-audit-2026-06-14 §2.4

### RULE-E-004 — glob 用 sync std::fs::read_dir 阻塞 tokio runtime

- **Level**: P1
- **Subsystem**: Tools
- **File**: `app/src-tauri/src/tools/glob.rs:115/205-226`
- **Description**: `walk_dir` 被 async fn 直接调,其他 tool 都用 `tokio::fs`
- **Impact**: 大 repo(Chromium/Linux kernel)glob 卡死 worker,拖累同 runtime 并发 session
- **Fix**: `spawn_blocking` 包裹 walk_dir
- **Status**: **closed (2026-06-16)** — walk + glob match + mtime collect 整体包 `tokio::task::spawn_blocking`(返回 `(Vec<Match>, usize)`),sort/输出留 async 侧;GlobMatcher Send 验证通过,7 个 glob 单测行为不变
- **Owner**: carlos
- **Related Task**: .trellis/tasks/06-16-p1-openai-o1-glob-spawn-blocking（与 RULE-D-002 合并实施）
- **Closed At**: `361336e`
- **Discovered In**: REVIEW-agent-loop-full-audit-2026-06-14 §2.5

### RULE-E-005 — worktree destroy 不等 cancel 生效就删目录

- **Level**: P1
- **Subsystem**: Tools
- **File**: `app/src-tauri/src/commands/worktree.rs` + `commands/sessions.rs` + `agent/helpers.rs` + `agent/chat.rs` + `state.rs`
- **Description**: destroy 紧接 `cancel_inflight_for_session`(`helpers.rs` 只 `token.cancel()` 不 await 退出)
- **Impact**: 窄窗口内 agent loop 下一次写 ENOENT/panic/残留 fingerprint 指向已删文件
- **Fix**: `cancel_inflight_for_session` 返回退出信号,destroy await
- **Status**: **closed (2026-06-15)** — `cancel_inflight_for_session` 加 `inflight_exits` 参数,取消 token 后 take 出 `oneshot::Receiver`(单消费者)返回;新增 `await_inflight_exit(rx, label)` helper(10s 防御性 timeout,超时 log warn + 仍进行)。`delete_worktree` / `detach_worktree` / `delete_session` 三处在 destructive 工作前 await。chat.rs spawn 闭包 `run_chat_loop().await` 后 `done_tx.send(())` + 清 `inflight_exits` entry。`AppState.inflight_exits` 新字段 + `load` 初始化。**不动 `cancellations` map 值类型**(cancel_chat / run_chat_loop / CancellationGuard / TestHarness 全不碰,最小涟漪)。设计决策 = 独立 map + oneshot(规避 `tauri::async_runtime::JoinHandle` 跨 Mutex<HashMap> 存储/await 语义不确定);scope = 三者皆 await(用户确认,共享同一 helper 同一类 race)。4 个 cancel 单测(3 改造补第 4 参 + 1 新增 `cancel_inflight_returns_exit_signal_resolving_on_completion`,spawn+flag+sleep 模式证明"先 pending、send 后才 resolve")。487 tests pass。
- **Owner**: carlos
- **Related Task**: `.trellis/tasks/06-15-worktree-destroy-await-cancel-rule-e-005`
- **Closed At**: `16f373a`
- **Discovered In**: REVIEW-agent-loop-full-audit-2026-06-14 §2.5
- **Resolution Notes**: spec `backend/worktree-contract.md` 同步——cancel_inflight_for_session 签名(`Option<oneshot::Receiver<()>>`)、AppState.inflight_exits 字段、Ordering invariant 改写为 "cancel → **await 退出** → destructive → event"、cancel-hook 步骤补 take receiver + await。

### RULE-E-006 — git::worktree::data_dir 走 env 而非 Tauri path

- **Level**: P1 (**P3 升级**)
- **Subsystem**: Tools
- **File**: `app/src-tauri/src/git/worktree.rs:40-56`(旧路径;修复后函数已删)
- **Description**: `data_dir` 用 env 变量,fallback 到 `/tmp`(world-writable + 重启清空)
- **Impact**: Windows/macOS 部署后 worktree 路径异常;**`/tmp` 重启清空 = 用户工作数据丢失**
- **Fix**: 改用 Tauri `app_data_dir()`,跨平台一致
- **Status**: **closed (2026-06-15)** — worktree `data_dir()` 已删,改用 `state.app_data_dir`(与 SQLite DB 同根 `~/.local/share/dev.everlasting.app/`),`/tmp` fallback 消失
- **Owner**: carlos
- **Related Task**: .trellis/tasks/06-15-p1-worktree-data-dir-tauri
- **Closed At**: `d8ee7d9`
- **Discovered In**: REVIEW-agent-loop-full-audit-2026-06-14 §2.5(P3 评级偏低,meta-review 升级 P1)

---

## P2 — 中等(健壮性 + 债务)

### RULE-A-005 — head_sha spawn 前查一次 50 轮不刷新

- **Level**: P2 (**P1 降级**)
- **Subsystem**: Agent Loop
- **File**: `app/src-tauri/src/agent/chat.rs:362/528`
- **Description**: spawn 前一次性;每轮 clone 同一 system_prompt
- **Impact**: agent 在 turn 3 commit 后,turn 4 system_prompt 的 HEAD SHA 与 `git log` 不一致,LLM 认知漂移(原 P1,meta-review 降 P2)
- **Fix**: 每 N 轮或每次 tool 执行后刷新
- **Status**: open
- **Owner**: carlos
- **Discovered In**: REVIEW-agent-loop-full-audit-2026-06-14 §2.1

### RULE-A-007 — error 路径 partial text 丢失

- **Level**: P2
- **Subsystem**: Agent Loop
- **File**: `app/src-tauri/src/agent/chat.rs:741-756`
- **Description**: Error arm 不 persist 已累积 text
- **Impact**: SSE 流中途 error 时已渲染的 delta,reload 后从 DB 读不到(与 cancel 路径 `:796-805` 不对称)
- **Fix**: Error arm persist 已累积 text
- **Status**: open
- **Owner**: carlos
- **Discovered In**: REVIEW-agent-loop-full-audit-2026-06-14 §2.1

### RULE-A-008 — estimate_messages_tokens 与 _iter 版大段重复

- **Level**: P2
- **Subsystem**: Agent Loop
- **File**: `app/src-tauri/src/agent/context.rs:85-133` vs `:275-317`
- **Description**: 两函数大段重复,新增 ContentBlock 变体易漏算
- **Fix**: 抽公共 helper
- **Status**: open
- **Owner**: carlos
- **Discovered In**: REVIEW-agent-loop-full-audit-2026-06-14 §2.1

### RULE-A-009 — 死代码抑制噪音

- **Level**: P2
- **Subsystem**: Agent Loop
- **File**: `app/src-tauri/src/agent/chat.rs:432/512` + `types.rs:357`
- **Description**: `let _ = &base_prompt;` / `let _ = turn_send_at;` 警告抑制死代码;`ChatEvent::ToolResult` 变体从不构造
- **Fix**: 删除未用变量和构造路径
- **Status**: open
- **Owner**: carlos
- **Discovered In**: REVIEW-agent-loop-full-audit-2026-06-14 §2.1

### RULE-B-003 — sqlite_glob_match 的 ? 分支 dead code

- **Level**: P2
- **Subsystem**: Permission
- **File**: `app/src-tauri/src/agent/permissions/mod.rs:766-783`
- **Description**: 内层 `if tbytes[ti] == b'/'` 永远 true(外层已判),`return false` 必达
- **Fix**: 删除冗余分支
- **Status**: open
- **Owner**: carlos
- **Discovered In**: REVIEW-agent-loop-full-audit-2026-06-14 §2.2

### RULE-B-004 — 危险命令检测有真实绕过路径

- **Level**: P2
- **Subsystem**: Permission
- **File**: `app/src-tauri/src/agent/permissions/dangerous.rs:81-108` + `shell_trust.rs:108`
- **Description**: regex 无 `(?i)` 大小写敏感;`find / -delete` 走 `READ_ONLY_WHITELIST` **直接 Allow**;长选项 / 子 shell / env 展开不匹配
- **Impact**: `find / -delete` 是漏网之鱼;其余有 Tier 4 Ask 兜底缓解
- **Fix**: 加 `(?i)` + `find -delete` 黑名单 + 长选项 / 子 shell / env 展开检测
- **Status**: **closed (2026-06-16)** — DENY_PATTERNS 全部正则加 `(?i)`(堵 `RM -RF /`/`MKFS`/`DD IF=` 大小写绕过);新增 `find ... -delete` + `find ... -exec(dir)` 两条硬墙(closes 真实漏网:`find` 在 Tier 4 是 ReadOnly,kill list 是唯一拦截层)。**不动** shell_trust find=ReadOnly 分级(双层架构:Tier4 放行 find / Tier2 拦破坏性 action,注释 :105-108 现成立)。长选项 / 子shell / env 展开不在本批(DEBT 已标"Tier4 兜底",避免范围蔓延)。+3 测试(大小写绕过 / find 漏网 / benign find 放行),498 tests pass
- **Owner**: carlos
- **Related Task**: （无单独 task,见 commit `5f1cdd0`）
- **Closed At**: `5f1cdd0`
- **Discovered In**: REVIEW-agent-loop-full-audit-2026-06-14 §2.2

### RULE-B-005 — shell trust 结构降级 false positive

- **Level**: P2
- **Subsystem**: Permission
- **File**: `app/src-tauri/src/agent/permissions/shell_trust.rs:365`
- **Description**: `cmd.contains('|')` 把 `grep "a|b"`(正则管道符)误降级 Ask
- **Impact**: UX 打折,安全侧正确(fail-safe)
- **Fix**: 加引号 / 转义上下文检测
- **Status**: open
- **Owner**: carlos
- **Discovered In**: REVIEW-agent-loop-full-audit-2026-06-14 §2.2

### RULE-C-003 — token 估算不反映 cache 折扣

- **Level**: P3 (**P1 降级**)
- **Subsystem**: Memory
- **File**: `app/src-tauri/src/agent/context.rs:85-133`
- **Description**: estimator 无 cache 概念;head pair(PROTECTED_HEAD=2)走 cache 实际计费 0.1×,但 estimator 按全价算
- **Impact**: compact 比"实际成本"更激进,提前压缩掉还有空间的对话(原 P1,但过度压缩 ≠ 数据丢失,meta-review 降 P3)
- **Fix**: 文档化或加 head pair cache 折扣
- **Status**: open
- **Owner**: carlos
- **Discovered In**: REVIEW-agent-loop-full-audit-2026-06-14 §2.3

### RULE-C-004 — MemoryWatcher 不在 AppState 持有

- **Level**: P2
- **Subsystem**: Memory
- **File**: `app/src-tauri/src/state.rs:192-197`(start_watcher 调用已删)
- **Description**: 成功路径返回值被丢弃
- **Impact**: 靠 notify 内部 thread 生命周期间接维持,结构不健壮;阻碍 RULE-C-002 的 add_watch 扩展
- **Fix**: 提升到 AppState,便于后续 add_watch(随 RULE-C-001/C-002 同 PR)
- **Status**: **closed (2026-06-15)** — 自动满足:watcher 整体删除,不再有"返回值丢弃 → handle drop"问题(无 watcher 可丢弃)。原 grill D2"AppState 加字段"方案被 D3(砍 watcher 改 mtime 轮询)推翻——发现 watcher 疑似完全失效(C-001)后,mtime fence 让 watcher 全链路冗余,直接删除比重构持有更净。见 RULE-C-001 Resolution Notes。
- **Owner**: carlos
- **Related Task**: `.trellis/tasks/06-15-p1-memory-watcher-appstate`
- **Closed At**: `759607c`
- **Discovered In**: REVIEW-agent-loop-full-audit-2026-06-14 §2.3

### RULE-C-005 — user_dir 路径与 Claude Code 不一致

- **Level**: P2
- **Subsystem**: Memory
- **File**: `app/src-tauri/src/memory/file.rs:58-66`
- **Description**: `~/.config/everlasting/CLAUDE.md`;Claude Code 用户级在 `~/.claude/CLAUDE.md`
- **Impact**: 用户从 Claude Code 切过来**用户层指令不共享**(项目层共享成立)
- **Fix**: 产品层决策 — 当前路径 or 对齐 `~/.claude/`?取决于产品定位
- **Status**: open
- **Owner**: carlos
- **Discovered In**: REVIEW-agent-loop-full-audit-2026-06-14 §2.3

### RULE-C-006 — 4 文件总大小无 cap

- **Level**: P2
- **Subsystem**: Memory
- **File**: `app/src-tauri/src/memory/mod.rs:54-60`
- **Description**: 仅单文件 cap 100 KiB;4 × 100 KiB ≈ 100K token,占 200K 窗口一半,挤压对话空间
- **Fix**: 加 ~400KB 总 cap
- **Status**: open
- **Owner**: carlos
- **Discovered In**: REVIEW-agent-loop-full-audit-2026-06-14 §2.3

### RULE-C-007 — watcher 路径表 fallback 按 file_name() 可能误触发

- **Level**: P2
- **Subsystem**: Memory
- **File**: `app/src-tauri/src/memory/watcher.rs:331-339`
- **Description**: 精确匹配失败后按文件名
- **Impact**: 跨 project 的 CLAUDE.md 写入可能误失效其他 project/user cache
- **Fix**: 加 parent_dir 校验
- **Status**: wontfix(2026-06-16)— 引用的 `watcher.rs` 已随 RULE-C-001 删除(改 read-through mtime fence 取代),路径表 fallback 逻辑不复存在。见 §收尾路径建议。
- **Owner**: carlos
- **Discovered In**: REVIEW-agent-loop-full-audit-2026-06-14 §2.3

### RULE-D-003 — SSE parser data_buf 无上限 + 不容忍 data: 无空格

- **Level**: P2
- **Subsystem**: Provider
- **File**: `app/src-tauri/src/llm/sse.rs:13/43-45`
- **Description**: `data_buf` 无 cap(GB 级 data OOM);`data:` 精确前缀带尾随空格,无空格版本被静默 drop
- **Impact**: 主流 provider 不触发;第三方代理发无空格版本被静默 drop;恶意上游 GB 级 data OOM
- **Fix**: 加 1 MiB cap + 容忍无空格版本(REVIEW-sse §8 P2 复述,2026-06-12 提出,0 落地)
- **Status**: open
- **Owner**: carlos
- **Discovered In**: REVIEW-sse-agent-loop-2026-06-12 §8 + REVIEW-agent-loop-full-audit-2026-06-14 §2.4

### RULE-D-004 — WireRequest.reasoning_effort dead field

- **Level**: P2
- **Subsystem**: Provider
- **File**: `app/src-tauri/src/llm/provider/wire.rs:133`
- **Description**: `#[allow(dead_code)]`,注释说"OpenAI reads it"但实际 OpenAI 读 `config.reasoning_effort`(`openai.rs:266`)
- **Impact**: 架构误导,未来 PR 以为已接好
- **Fix**: 接通 `from_model_row` 或删字段
- **Status**: open
- **Owner**: carlos
- **Discovered In**: REVIEW-agent-loop-full-audit-2026-06-14 §2.4

### RULE-D-005 — OpenAI supports_reasoning_effort caps hardcode true

- **Level**: P2
- **Subsystem**: Provider
- **File**: `app/src-tauri/src/llm/provider/openai.rs:370-374`
- **Description**: hardcode true;`WireCapabilities::from_model_row`(`wire.rs:97-110`)已实现正确派生却没被调用
- **Impact**: gpt-4o(无 reasoning)model 错误保留 Reasoning 块,污染上下文
- **Fix**: 调 `from_model_row`
- **Status**: open
- **Owner**: carlos
- **Discovered In**: REVIEW-agent-loop-full-audit-2026-06-14 §2.4

### RULE-D-006 — GLM max_tokens 500/400 误分类加 keyword

- **Level**: P2
- **Subsystem**: Provider
- **File**: `app/src-tauri/src/llm/error.rs:129-136`
- **Description**: keyword 列表无 `max_tokens`
- **Impact**: GLM 报 500/400 时误分类为服务器错误而非用户参数错误
- **Fix**: keyword 列表加 `max_tokens`(REVIEW-sse §8 P4 复述,2026-06-12 提出,0 落地)
- **Status**: open
- **Owner**: carlos
- **Discovered In**: REVIEW-sse-agent-loop-2026-06-12 §8 + REVIEW-agent-loop-full-audit-2026-06-14 §2.4

### RULE-E-007 — ReadGuard 进程内不持久,重启失效

- **Level**: P2
- **Subsystem**: Tools
- **File**: `app/src-tauri/src/tools/read_guard.rs:17-21`
- **Description**: 明示"Lifetime is the process"
- **Impact**: 跨重启续聊每个 edit 多一轮 read(功能 safe,UX 退化)
- **Fix**: SQLite 持久化 fingerprint(可选)
- **Status**: open
- **Owner**: carlos
- **Discovered In**: REVIEW-agent-loop-full-audit-2026-06-14 §2.5

### RULE-E-008 — edit_file find_similar_lines 大文件单行爆

- **Level**: P2
- **Subsystem**: Tools
- **File**: `app/src-tauri/src/tools/edit_file.rs:277-306`
- **Description**: 0 匹配时对每行 `split_whitespace` + `HashSet<char>` Jaccard
- **Impact**: minified bundle 单行 1MB 字符的 HashSet 是灾难,CPU/内存爆
- **Fix**: 行级 cap + 提前返回
- **Status**: open
- **Owner**: carlos
- **Discovered In**: REVIEW-agent-loop-full-audit-2026-06-14 §2.5

### RULE-E-009 — read_file UTF-8 切片 panic 风险(中文/emoji ≥50KB)

- **Level**: P2
- **Subsystem**: Tools
- **File**: `app/src-tauri/src/tools/read_file.rs:222-225`
- **Description**: 按字节切片 `&content[..head_end]`;`diff.rs:298-302` 已修但 read_file **没同步**
- **Impact**: 多字节字符边界 panic,**同 repo 内不一致**
- **Fix**: 同步 diff.rs 的 `floor_char_boundary`
- **Status**: **closed (2026-06-16)** — 4 处字节切片(`truncate_full_output` head/tail + `truncate_output` numbered head/tail,行号因 offset/limit 重构漂移,原 :222-225)改用 std `str::floor_char_boundary`(head)/`ceil_char_boundary`(tail);对齐 git/diff.rs 字符边界逻辑(后者对 `&[u8]` 手写循环,read_file 是 `&str` 用 std helper 更地道)。+2 多字节不 panic 测试(72KB 单字 CJK full path + 119KB offset path),498 tests pass
- **Owner**: carlos
- **Related Task**: （无单独 task,见 commit `5f1cdd0`）
- **Closed At**: `5f1cdd0`
- **Discovered In**: REVIEW-agent-loop-full-audit-2026-06-14 §2.5 + §3.6

### RULE-E-010 — shell spillover 文件不日常清理

- **Level**: P2
- **Subsystem**: Tools
- **File**: `app/src-tauri/src/tools/shell.rs:391-404`
- **Description**: `cleanup_outputs_dir` 仅 `delete_session` 调
- **Impact**: 长跑 session 累积 30KB+ 输出文件,磁盘膨胀
- **Fix**: LRU 清理或定期 background task
- **Status**: open
- **Owner**: carlos
- **Discovered In**: REVIEW-agent-loop-full-audit-2026-06-14 §2.5

### RULE-E-011 — worktree create self-heal 强制 remove_dir_all orphan 目录

- **Level**: P2
- **Subsystem**: Tools
- **File**: `app/src-tauri/src/git/worktree.rs:216-227`
- **Description**: 注释自承"silent auto-cleanup would be a footgun"但接着删
- **Impact**: 罕见但灾难性,用户手动放的文件被无声删除
- **Fix**: 非空目录拒创建,返回错误让用户决定
- **Status**: open
- **Owner**: carlos
- **Discovered In**: REVIEW-agent-loop-full-audit-2026-06-14 §2.5

---

## P3 — 轻微(文档/一致性)

### RULE-A-010 — spec §2.5.1 二次取消语义未实现

- **Level**: P3
- **Subsystem**: Agent Loop
- **File**: `docs/ARCHITECTURE.md §2.5.1` vs `app/src-tauri/src/agent/chat.rs:934-978`
- **Description**: spec 要求"取消不立即终止,把'取消'作为 tool_result 回传给 LLM 一次自我收敛机会;二次取消才真终止";当前实现单次 cancel 即 emit Done("cancelled") 终止
- **Impact**: MVP 简化,但与 spec 不符,影响 LLM 自我收敛能力
- **Fix**: 实现二次取消语义,或更新 spec 标"已偏离"
- **Status**: open
- **Owner**: carlos
- **Discovered In**: REVIEW-agent-loop-full-audit-2026-06-14 §2.1 + `.trellis/reviews/SPEC-DRIFT.md`

### RULE-A-011 — A-P2-1 messages.clone Arc<Vec> 过度优化

- **Level**: P3 (**标记 wontfix**)
- **Subsystem**: Agent Loop
- **File**: `app/src-tauri/src/agent/chat.rs:461/529`
- **Description**: 50 轮 × 80KB ≈ 8MB churn,提议改 `Arc<Vec>`
- **Impact**: 8MB 在现代机器上不是性能问题,反而 `Arc<Vec>` 破坏借用边界、增加 atomic 开销、可变性表达不清晰
- **Decision**: **不做**,meta-review 评估为过度优化
- **Status**: wontfix
- **Owner**: carlos
- **Discovered In**: REVIEW-agent-loop-full-audit-2026-06-14 §2.1

### RULE-B-006 — AuditKind docstring "10"→"11"

- **Level**: P3
- **Subsystem**: Permission
- **File**: `app/src-tauri/src/agent/permissions/mod.rs:140` vs `:152-179`
- **Description**: docstring 写"10 variants"实际 11(`ToolExecuted` C4 新增未更新 doc)
- **Fix**: 更新 docstring
- **Status**: open
- **Owner**: carlos
- **Discovered In**: REVIEW-agent-loop-full-audit-2026-06-14 §2.2

### RULE-B-007 — Background Mode 仍空壳

- **Level**: P3
- **Subsystem**: Permission
- **File**: `app/src-tauri/src/agent/permissions/types.rs:193` + `mod.rs:1214`
- **Description**: `#[allow(dead_code)]`,`mode_system_prefix` 占位字符串
- **Impact**: UI 已移除,enum 保留预留
- **Fix**: 路线图评估移除 or 保留
- **Status**: open
- **Owner**: carlos
- **Discovered In**: REVIEW-agent-loop-full-audit-2026-06-14 §2.2

### RULE-C-008 — grill Q4 "AGENTS.md 物理顺序前置"未严格执行

- **Level**: P3
- **Subsystem**: Memory
- **File**: `app/src-tauri/src/memory/loader.rs:321`
- **Description**: 仍按 CLAUDE→AGENTS 顺序;优先级仅靠 `<primary>`/`<reference>` wrapper 标签
- **Impact**: 软提示 vs 硬提示,标签可能已足够
- **Fix**: 决定硬前置 or 维持当前
- **Status**: open
- **Owner**: carlos
- **Discovered In**: REVIEW-b5-memory-grill-2026-06-10 + REVIEW-agent-loop-full-audit-2026-06-14 §2.3

### RULE-C-009 — WSL/9p/drvfs 下 inotify 可靠性未验证

- **Level**: P3
- **Subsystem**: Memory
- **File**: —
- **Description**: `/mnt/c/...` 路径 watcher 可能收不到事件
- **Impact**: WSL 用户 memory 缓存不更新
- **Fix**: 实测验证,失败则 fallback polling
- **Status**: wontfix(2026-06-16)— watcher 已随 RULE-C-001 删除,memory freshness 走 read-through mtime fence(stat 不依赖 inotify),WSL/9p 可靠性问题自动消解。见 §收尾路径建议。
- **Owner**: carlos
- **Discovered In**: REVIEW-agent-loop-full-audit-2026-06-14 §2.3

### RULE-D-007 — OpenAI 多 tool_call index 缺失默认 0

- **Level**: P3
- **Subsystem**: Provider
- **File**: `app/src-tauri/src/llm/provider/openai.rs:593-597`
- **Description**: `unwrap_or(0)`,两个无 index tool_call 都映射 index 0,后者覆盖前者
- **Impact**: 官方 API 总带 index,第三方兼容层风险
- **Fix**: index 缺失报错而非默认
- **Status**: open
- **Owner**: carlos
- **Discovered In**: REVIEW-agent-loop-full-audit-2026-06-14 §2.4

### RULE-D-008 — parse_anthropic_usage 全零判 None 假设

- **Level**: P3
- **Subsystem**: Provider
- **File**: `app/src-tauri/src/llm/provider/anthropic.rs:617-627`
- **Description**: 全零判 None
- **Impact**: 极低,真实响应 input 永远 >0
- **Fix**: 防御性编程,改 None if not_present
- **Status**: open
- **Owner**: carlos
- **Discovered In**: REVIEW-agent-loop-full-audit-2026-06-14 §2.4

---

## 历史 review 债务合并追踪

> 状态: 4 条建议中 3 条已合并入本文件 RULE-X-XXX(RULE-D-003 / RULE-A-003 / RULE-D-006),1 条升级到 RULE-A-006(集成测试,**前移**)

### REVIEW-sse-agent-loop-2026-06-12 §8(2026-06-12 提出)

| 原建议 | DEBT.md 对应 | 当前状态 |
|---|---|---|
| P2 SSE data_buf 加 1 MiB 上限 | RULE-D-003 (P2) | open |
| P3 persist_turn 失败 emit Error | RULE-A-003 (P1) | closed (2026-06-15) |
| P4 GLM max_tokens 500/400 误分类加 keyword | RULE-D-006 (P2) | open |
| P5 mock HTTP server 集成测试 | RULE-A-006 (P1,**PR5 前移**) | closed (2026-06-15) |

### REVIEW-b5-memory-grill-2026-06-10(9 题决议)

7 题 ✅ 落地,2 题 ⚠️:
- Q4 AGENTS.md 物理顺序前置 → RULE-C-008 (P3)
- Q8 "~40 行"被 FINDINGS 提前纠正实际 150-200 行 → 已纠错,无后续

---

## Re-evaluation Log

> 记录后续 audit 对优先级的重新评估(每次必须填日期 + 理由 + 引用)

| Date | RULE ID | Old Level | New Level | 理由 | 引用 |
|---|---|---|---|---|---|
| 2026-06-14 | RULE-A-005 | P1 | P2 | head_sha 漂移是 UX 而非数据丢失,降级 | meta-review §2.2 |
| 2026-06-14 | RULE-A-006 | P2 | P1(**前移**) | P0 修复无回归保护等于盲修 | meta-review §2.4 |
| 2026-06-14 | RULE-A-011 | P2 | P3(wontfix) | 8MB 不是性能问题,Arc<Vec> 损害借用边界 | meta-review §2.1 |
| 2026-06-14 | RULE-C-001 | P1 | P1(确认) | PRD 承诺"立即生效"是产品诚信问题 | meta-review §2.5 |
| 2026-06-14 | RULE-C-003 | P1 | P3 | 过度压缩 ≠ 数据丢失 | meta-review §2.2 |
| 2026-06-14 | RULE-E-006 | P3 | P1 | `/tmp` fallback 重启清空 = 工作数据丢失 | meta-review §2.3 |
| 2026-06-15 | RULE-E-006 | open | **closed** | worktree data_dir 从 env/home/tmp 改 Tauri app_data_dir,对齐 DB,消除 /tmp 数据丢失;`git::data_dir()` 函数 + re-export + 模块 docstring 全部删除,Grill decision #2 不变式保留(`catalog` 紧跟 `db`,`app_data_dir` 落在 data-plane group 内) | .trellis/tasks/06-15-p1-worktree-data-dir-tauri |
| 2026-06-15 | RULE-A-006 | partial | **closed** | production `chat.rs` → `run_chat_loop` 迁移完成,副本消除,9 个 agent_loop_* 测试现覆盖 production 真实路径 | `.trellis/tasks/06-15-unify-chat-loop-dispatch` |
| 2026-06-15 | RULE-A-003 | open | **closed** | 5 处 persist 失败分类处理:正常路径 3 处 emit `Error{Server}`+return(对齐 RULE-A-002 StillOver),cancel 路径 2 处 log-only(避免与 cancelled `Done` 双终止事件);前端不基于 category 分支故复用 `Server`,零前端改动。`emit_persist_failure` helper 集中文案;`agent_loop_persist_failure_emits_error` 测试(trigger 拦截 INSERT) | `.trellis/tasks/06-15-p1-persist-emit-error-and-audit-cancel-order` |
| 2026-06-15 | RULE-A-004 | open | **closed** | `record_tool_executed_audit` 块从 cancel 检查前移到后(`else if` 串联),cancelled 的 tool 不落 audit;两检查背靠背无 await,token 状态一致。`agent_loop_cancel_skips_audit_for_cancelled_tool` 测试 | 同上 task |
| 2026-06-15 | RULE-E-005 | open | **closed** | `cancel_inflight_for_session` 加 `inflight_exits` 参数返回 `oneshot::Receiver`(单消费者),新增 `await_inflight_exit`(10s timeout backstop);`delete_worktree`/`detach_worktree`/`delete_session` 三处 await;chat.rs spawn 闭包 `run_chat_loop` 后 `done_tx.send` + 清 entry。独立 map + oneshot(不动 cancellations 值类型,规避 Tauri JoinHandle 存储语义不确定)。4 cancel 单测(3 改造 + 1 新增)。spec Ordering invariant 改写。487 tests pass | `.trellis/tasks/06-15-worktree-destroy-await-cancel-rule-e-005` |
| 2026-06-15 | RULE-C-001 | open | **closed** | 砍 notify watcher 改 read-through mtime fence;brainstorm 核实发现 watcher 疑似完全失效(返回值丢弃→handle drop→确定性读旧,严重性>>原"概率性 race")。W 方案:slot 加 `CachedLayer{layer,mtime}`,read 每次 stat 比较,read 路径成 freshness 权威。watcher.rs/invalidate_*/notify 依赖/前端 dead listener 全清,4 fence 测试,489 pass | `.trellis/tasks/06-15-p1-memory-watcher-appstate` |
| 2026-06-15 | RULE-C-002 | open | **closed** | 自动满足:watcher 删后新 project 首 `load_for_session` 即 stat,无需 watch/add_watch | 同上 task |
| 2026-06-15 | RULE-C-004 | open | **closed** | 自动满足:watcher 删除,无 handle 可丢弃;D2"AppState 加字段"方案被 D3 砍 watcher 推翻 | 同上 task |
| 2026-06-16 | RULE-D-002 | open | **closed** | is_o1_family 前缀分支(o1/o3/o4),o1 family 用 max_completion_tokens;+3 单测。与 E-004 合并 task(两项均小修 active bug) | `.trellis/tasks/06-16-p1-openai-o1-glob-spawn-blocking` |
| 2026-06-16 | RULE-E-004 | open | **closed** | glob walk+match+collect 包 spawn_blocking(GlobMatcher Send 验证);与 D-002 合并 task | 同上 task |
| 2026-06-16 | RULE-C-007 | open | **wontfix** | 引用的 watcher.rs 已随 RULE-C-001 删除(mtime fence 取代),fallback 逻辑不存在 | §收尾路径建议 |
| 2026-06-16 | RULE-C-009 | open | **wontfix** | watcher 删除后 freshness 走 mtime stat,不依赖 inotify,WSL 可靠性问题消解 | §收尾路径建议 |
| 2026-06-16 | RULE-B-004 | open | **closed** | DENY_PATTERNS 全加 (?i) + 新增 find -delete/-exec 硬墙;不动 shell_trust 分级(双层架构);长选项/子shell/env 留 Tier4 兜底;+3 测试,498 pass | §收尾路径建议 |
| 2026-06-16 | RULE-E-009 | open | **closed** | 4 处字节切片改 floor/ceil_char_boundary(对齐 diff.rs);+2 多字节测试,498 pass | §收尾路径建议 |

---

## 子 task 编排建议

| PR 顺序 | Task 名 | 包含 RULE | 依赖 |
|---|---|---|---|
| PR1 | `06-14-p0-shell-env-clear` | RULE-E-001 | — |
| PR2 | `06-14-p0-shell-process-group` | RULE-E-002 | — |
| PR3 | `06-14-p0-web-fetch-redirect-ssrf` | RULE-E-003 | — |
| PR4 | `06-14-p0-c3-tail-pair-orphan` | RULE-A-001 + RULE-A-002 | — |
| **PR5** | **`06-14-p1-agent-loop-integration-tests`** | **RULE-A-006** | **必须在 P0 修复后立刻补,为后续 P1 提供回归保护** |
| **PR5b** | **`06-15-unify-chat-loop-dispatch`** | **RULE-A-006(闭环)** | **依赖 PR5 — production `chat.rs` → `run_chat_loop` 迁移,副本消除** |
| PR6+PR7 | `06-15-p1-persist-emit-error-and-audit-cancel-order` | RULE-A-003 + RULE-A-004(合并一个 task) | ✅ closed (2026-06-15) — PR5(RULE-A-006)解阻后合并实现 |
| PR8 | `06-16-p1-permission-asks-cleanup` | RULE-B-001 + RULE-B-002 | ✅ closed (2026-06-16) → `3b16528` — store value 加 session 绑定,delete_session 接入 |
| PR9 | `06-15-p1-memory-watcher-appstate` | RULE-C-001 + RULE-C-002 + RULE-C-004 | ✅ closed (2026-06-15) — W 方案:砍 watcher 改 mtime fence,C-002/C-004 自动满足 |
| PR10 | `06-14-p1-api-key-encryption` | RULE-D-001 | — |
| PR11+PR12 | `06-16-p1-openai-o1-glob-spawn-blocking` | RULE-D-002 + RULE-E-004(合并一个 task) | ✅ closed (2026-06-16) → `361336e` — 两项均小修 active bug,合并 PR |
| PR13 | `06-15-worktree-destroy-await-cancel-rule-e-005` | RULE-E-005 | ✅ closed (2026-06-15) — 依赖 PR5(RULE-A-006 已 closed,解阻) |
| PR14 | `06-15-p1-worktree-data-dir-tauri` | RULE-E-006 | — |
| PR-N+ | P2 各项子 task | RULE-*-P2 | — |
| PR-N+ | P3 各项子 task | RULE-*-P3 | — |

---

## 收尾路径建议(基于 ROADMAP 耦合,2026-06-16 评估)

> 维度:按"与接下来 ROADMAP 里程碑的耦合"给债务排收尾节奏。**不替代**上方"子 task 编排建议"(那是按 PR 依赖顺序),两者互补——编排建议看依赖,本段看功能契机。
>
> **现状判断**:P0 已清零(5/5 closed);P1 仅剩 `RULE-D-001`(API key 明文);**无任何债务阻塞 ROADMAP 第二档功能**。

### 三梯队

| 梯队 | RULE | 处置 |
|---|---|---|
| 🟢 可一直挂 | A-005 / A-008 / A-009 / B-003 / B-006 / C-003 / C-006 / D-004~D-008 / E-007 / E-010 / E-011 | 卫生债,不坏功能 |
| 🟡 看到顺手修 | **B-004**(`find / -delete` 漏网,P2 唯一偏安全)、**E-009**(read_file UTF-8 panic,同 repo diff.rs 已修纯不一致)、**D-003**(SSE data_buf 无上限,第三方代理可踩) | 独立便宜活,任意时点可挑 |
| 🔴 需决策 | **D-001**(API key 明文,P1) | 接受风险 vs 引入 keyring 依赖(Linux 走 Secret Service/D-Bus,WSL 体验存疑),建议先标"已知接受" |

### 按 ROADMAP 里程碑的收尾契机

| ROADMAP 里程碑 | 耦合债务 | 建议 |
|---|---|---|
| **B2 / B3 / D2**(输入/检索层) | 无直接耦合 | 零负担推进,不顺手不修 |
| **D3**(消息编辑/重发) | 会重走 turn 边界 + message 持久化 → 自然碰到 **A-007**(error 路径 partial text 丢失)、**A-010**(二次取消语义) | 做 D3 时是修这俩的天然窗口 |
| **B6 Subagent**(第三档,harness 学习价值最高) | worker agent 独立 context/token 预算 → **A-008**(estimator 两版重复)、**D-004/D-005**(capabilities 派生错误会污染 subagent 上下文) | **进 B6 前先抽 A-008 helper + 修 D-004/D-005** |

### 已失效债务清理(本次评估发现)

`RULE-C-001`(2026-06-15)Resolution Notes:watcher 已**整文件删除**改 read-through mtime fence。以下 2 条 finding 引用的 `watcher.rs` 已不存在,本次标 wontfix:

- **RULE-C-007**(`watcher.rs:331-339` 路径表 fallback)→ wontfix
- **RULE-C-009**(WSL/9p inotify 可靠性)→ wontfix(无 watcher 可失效)

### 建议执行节奏

1. ✅ **B-004 + E-009 已完成**(2026-06-16,498 tests pass);**D-003 待做**(SSE data_buf cap,下一批)。
2. 推 B2/B3/D2(零耦合功能)。
3. 做 D3 时顺手清 A-007 + A-010。
4. 进 B6 前抽 A-008 + 修 D-004/D-005。
5. D-001 待个人威胁模型决策(暂标已知接受)。

---

## 维护说明

- **每次 audit 必须 diff 本文件** 第一步
- **每次 PR merge 必须更新 Closed At + Related PR**
- **每条 finding 闭合后状态变更不可逆**(除非重新打开)
- **子 task 创建时在本文件 Related Task 字段填 task 路径**
- **下次 audit 模板**: 第一段写 "DEBT.md diff 结果",已记录 finding 仅引用,新 finding 按模板加入

---

**最后更新**: 2026-06-16 by carlos
**下个 review**: REVIEW-XXX-2026-XX-XX(待定)