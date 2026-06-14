# Everlasting Agent Loop 全盘架构审视

> **审视日期**: 2026-06-14
> **审视范围**: Agent Loop 全链路 + 辅助功能（Permission/Mode、Memory、Provider/Model/SSE、Worktree/Git/Tools/Boundary）
> **审视类型**: 代码审计（post-implementation full audit）
> **审视基线**: commit `a4fb302`（2026-06-14），Rust 28324 行 / 5 子系统
> **审视方法**: 5 路并行深挖（每路读完整源码 + 单测 + spec 对齐）+ 跨子系统综合 + P0 级断言人工二次核验
> **对照基准**: `REVIEW-sse-agent-loop-2026-06-12` / `REVIEW-a2-b7-permission-mode-plan-2026-06-13` / `REVIEW-b5-memory-grill-2026-06-10` 三份历史 review

---

## 0. 总体评价

**综合评分: ★★★★ (4/5) — 工程素质与架构分层属同类自研 harness 的上乘水准,但 LLM agent 安全面有 3 个可利用的 P0 拉低整体。**

| 子系统 | 评分 | 一句话 |
|---|---|---|
| Agent Loop 核心 | ★★★★ | cancel 安全 / RAII / catalog 复用是教科书级;但 C3 压缩 tail pair 边界有自承认的 orphan bug + persist 静默失败 |
| Permission + Mode | ★★★★½ | 前期 review 的 P0 BLOCKER 已解决且实现更优;5 道顺序统一、11 类审计、biased cancel;清理路径隐性化是主要扣分 |
| Memory | ★★★★ | cache_control 链路全程打通 + 选了技术最对的方案 B;watcher debounce race + 缺真实 cache 命中实测 |
| Provider/Model/SSE | ★★★★½ | trait 抽象 + Wire 中间层 + 自研 SSE 是工程质量最高的子系统;API key 明文 + o1 max_tokens 协议错误 + 历史 review 债务 0 落地 |
| Worktree/Git/Tools | ★★★★ | boundary 双层防御 + ReadGuard 三维指纹 + worktree self-heal 扎实;但 shell env 泄漏 + 不 kill 进程组 + web_fetch SSRF 绕过 3 个 P0 集中于此 |

**核心结论**:

1. **架构层面健康**。分层清晰（IPC 命令 → agent loop → provider/tool）、自研不依赖 SDK、cancel / RAII / catalog / boundary 等关键不变式都有单测锁定。对"对标 Claude Code 能力"的目标,地基是稳的。

2. **3 个安全 P0 是当前最大短板**,且全部集中在 Worktree/Git/Tools 子系统的 LLM 执行面:
   - `shell` 子进程继承父进程全部环境变量(含 `ANTHROPIC_API_KEY`)——LLM 一句 `env` 即可窃取(`shell.rs:237`)
   - `shell` 不 kill 进程组——`sleep 60 &` / 管道 / `nohup` 产生的孤儿进程在 cancel/timeout 后继续跑(`shell.rs:79-99`)
   - `web_fetch` redirect 不重做 IP 校验——`attacker.com → 169.254.169.254` 可绕 SSRF 打云 metadata(`web_fetch.rs:385`)
   这 3 个都是 LLM agent 的经典攻击面,修复成本各 5-15 行,优先级最高。

3. **历史 review 债务积压**。`REVIEW-sse-agent-loop` 的 4 条改进建议(data_buf cap / persist 失败 emit / GLM max_tokens 误分类 / mock 集成测试)**0 条完全落地**,被两路深挖分别复述。

4. **测试不对称**。前端 `streamController.test.ts`(54KB)/`permissions.test.ts`/`chatMode.test.ts` 自测密集;后端 Agent Loop **无 mock HTTP server 驱动的集成测试**,turn 循环边界只能靠手动验证。

---

## 1. 全局架构图

```
┌─────────────────────────────────────────────────────────────────────┐
│ 前端 (Vue 3 + Pinia)                                                  │
│  ChatWindow.vue → chat.ts(send) ──invoke("chat")──┐                 │
│   ├ streamController.ts  ← chat-event/tool:call/tool:result          │
│   ├ permissions.ts       ← permission:ask → permission_response      │
│   └ (120s timer 双保险)                                              │
└─────────────────────────────────────────────────────────────────────┘
                                                  │ IPC
┌─────────────────────────────────────────────────────────────────────┐
│ Rust 后端                                                            │
│                                                                      │
│  commands/sessions.rs::chat (IPC shim, 317 行)                       │
│   ├ pre-flight: lookup_provider_for_session(catalog.get)  [子系统D]  │
│   ├ 注册 CancellationToken + session→rid                             │
│   └ tauri::spawn ──┐                                                 │
│                    ▼                                                 │
│  agent/chat.rs (1372 行) ─── Agent Loop 主体                         │
│   ├ CancellationGuard RAII (state.rs:336)                            │
│   ├ boundary 校验 ×2 (projects/boundary.rs)              [子系统E]   │
│   ├ build_system_prompt + mode_prefix (system_prompt.rs)             │
│   ├ Memory build_instructions_blocks (memory/loader.rs)   [子系统C]  │
│   │   └ cache_control: ephemeral 注入 banner block                   │
│   ├ for turn in 1..=MAX_TURNS(50):                                   │
│   │   ├ compact_messages (agent/context.rs) ── C3 压缩    [子系统A]  │
│   │   ├ filter_tools_for_mode (permissions/mod.rs:1232)   [子系统B]  │
│   │   ├ provider.send(system, msgs, tools) ──→ [子系统D]             │
│   │   │   └ SSE stream (llm/sse.rs 状态机) → ChatEvent               │
│   │   │      └ tokio::select! biased; cancel vs stream.next()        │
│   │   ├ flush thinking, build assistant_blocks                       │
│   │   ├ persist_turn (DB) + emit TurnComplete                        │
│   │   └ for each tool_use:                                           │
│   │       ├ permissions::check 5 道 (mod.rs:413)          [子系统B]  │
│   │       │   └ Tier3 ask: permission:ask IPC ← permissions.ts       │
│   │       ├ execute_tool (tools/mod.rs:189)              [子系统E]   │
│   │       │   └ read_file/write_file/edit_file/shell/grep/glob/...   │
│   │       │       └ ReadGuard 三维指纹 (read_guard.rs)               │
│   │       │       └ assert_within_root (boundary.rs)                 │
│   │       │       └ worktree cwd (git/worktree.rs)                   │
│   │       ├ record_tool_executed_audit (11 类 AuditKind)             │
│   │       └ tool_result 回填 → 再请求                                │
│   └ MAX_TURNS 兜底 emit Done("max_turns")                            │
│                                                                      │
│  state.rs::AppState                                                  │
│   ├ db: SqlitePool                                                   │
│   ├ catalog: Arc<RwLock<ProviderCatalog>>  ← rebuild on CRUD [子系统D]│
│   ├ cancellations + session_active_request                           │
│   ├ read_guard: ReadGuard (进程内,不持久)                            │
│   ├ memory_cache: Arc<MemoryCache> ← watcher invalidate   [子系统C]  │
│   └ permission_asks: PermissionStore (oneshot map)        [子系统B]  │
└─────────────────────────────────────────────────────────────────────┘
```

---

## 2. 子系统逐一审视

### 2.1 Agent Loop 核心 — ★★★★ (4/5)

**主体**: `agent/chat.rs`(1372) + `context.rs`(876, C3 压缩) + `helpers.rs` + `system_prompt.rs` + `thinking.rs` + `provider.rs` + `tools/mod.rs`(execute_tool 分发)

#### 做对的事 ✅

1. **CancellationGuard RAII 覆盖所有退出路径**(`state.rs:336-361`,`chat.rs:196-201`)。正常/error/cancel/max_turns 四条路径 Drop 都清理两个 Map;单测 `cancellation_guard_removes_entry_on_drop`(`tests.rs:145-179`)锁定。避免"新加 early-return 忘了 remove"经典泄漏。
2. **三层 cancel 覆盖**(`chat.rs:562-567` stream 侧;`:1114-1116` tool 后;`:934-978`+`:1159-1199` 两条清理路径)。`biased;` 保证 cancel arm 优先(`chat.rs:563`)。
3. **catalog lookup 取代每轮 build_provider**(`state.rs:55`,`chat.rs:1286-1298`)。`Arc<dyn Provider>` spawn 前解析一次,50 轮复用。
4. **C3 压缩与 MAX_TURNS 解耦**(`agent/mod.rs:42-48`)。20→50 后 MAX_TURNS 仅兜底 pathological loops,真正 overflow 防护在 `compact_messages`(`context.rs:160`)。
5. **thinking 块 flush 时机正确**(`thinking.rs:27-38`),每个 Delta/ToolCall 前 + turn 结尾各 flush,signature 不孤立。
6. **tool_result 永远成对(2013 orphan 修复)**(`chat.rs:934-963`,`helpers.rs:78-100`)。cancel 后若已有 tool_use,生成 synthetic `is_error:true` tool_result。
7. **TurnComplete emit 在 persist 之后**(`chat.rs:886-928`),前端 reload 必读完整数据,无 race。

#### 不足点

| 级别 | 问题 | 证据 | 影响 |
|---|---|---|---|
| **P0-1** | C3 `group_droppable_turns` tail pair 边界可能产生 **orphan tool_result** | `context.rs:820-825` 注释自承"Under heavy pressure this leaves an orphan tool_result";`context.rs:334-381` 当 `assistant(tool_use)` 紧邻 protected tail 时被当 singleton drop,而 tail `user(tool_result)` 保留 | 极压场景撞 Anthropic 400 "tool_result without matching tool_use",整 chat 崩。**已二次核验:`context.rs:28` "orphaned" 注释 + `:341` `while i < tail_index` 边界确认** |
| **P0-2** | `compact_messages` 全部 middle 丢完仍超 target 时**静默不丢** | `context.rs:221-233` greedy drop 循环无"still over after compaction"错误返回;`:235-242` 仅 `dropped_count==0` 原样返回 | 单条超大 tool_result(shell dump/read_file)单独构成 tail 时,超窗仍发给 LLM 撞 `prompt is too long` |
| **P1-1** | `persist_turn` 失败静默,**DB 与 in-memory 永久分叉** | `chat.rs:439-447`(user)/`:875-886`(assistant)/`:1205-1216`(tool_result) 全部 `tracing::error!` 后继续 | 磁盘满/DB 锁竞争失败时,消息"发了回了"但下次打开 session 空白 |
| **P1-2** | `record_tool_executed_audit` 在 cancel 检查**之后**写,audit 撒谎 | `chat.rs:1094-1110` audit 在 `:1114-1116` cancel 检查之前 | cancel 短路的 tool 仍记一行 `tool_executed`,审计误导 |
| **P1-3** | `head_sha` spawn 前查一次,50 轮中不刷新 | `chat.rs:362` 一次性;`:528` 每轮 clone 同一 system_prompt | agent 在 turn 3 commit 后,turn 4 system_prompt 的 HEAD SHA 与 `git log` 不一致,LLM 认知漂移 |
| **P1-4** | error 路径 partial text 丢失 | `chat.rs:741-756` Error arm 不 persist 已累积 text | SSE 流中途 error 时已渲染的 delta,reload 后从 DB 读不到 |
| **P2-1** | error 路径无 persist partial(与 cancel 路径 `:796-805` 不对称) | 同上 | — |
| **P2-2** | `messages.clone()` 每轮 2 次(50 轮 × 80KB ≈ 8MB churn) | `chat.rs:461`+`:529` | 性能,可改 `Arc<Vec>` |
| **P2-3** | `estimate_messages_tokens` 与 `_iter` 版大段重复 | `context.rs:85-133` vs `:275-317` | 新增 ContentBlock 变体易漏算 |
| **P3** | `let _ = &base_prompt;`(`:432`)/`let _ = turn_send_at;`(`:512`) 警告抑制死代码;`ChatEvent::ToolResult` 变体从不构造(`types.rs:357`) | — | 代码噪音 |

**spec 偏离**: ARCHITECTURE.md §2.5.1 规定"取消不立即终止,把'取消'作为 tool_result 回传给 LLM 一次自我收敛机会;二次取消才真终止"。当前实现(`chat.rs:934-978`)单次 cancel 即 emit Done("cancelled") 终止,**未实现二次取消语义**——MVP 简化,但与 spec 不符。

**测试缺口**: 全单测级(helpers/system_prompt/thinking 累积/cancel token),**无 mock HTTP server 驱动的完整 turn 集成测试**。turn 边界(cancel 在 turn 2 tool 执行中 / max_turns 在 turn 50 / C3 在 turn 30 触发)只能手动验证。

---

### 2.2 Permission + Mode — ★★★★½ (4.5/5)

**主体**: `agent/permissions/mod.rs`(1644) + `dangerous.rs`(207) + `shell_trust.rs`(732) + `db/permissions.rs` + `commands/permissions.rs`

#### 做对的事 ✅

1. **⑨ 关顺序已统一,且比前期 review 推荐更优**。前期 review §1 推荐 `Hooks→Deny→Ask→Mode→Allow`,实际实现 `Hooks→Deny→Mode→Path/Prefix/External(含Ask)→Allow→Audit`(`mod.rs:422-644`)。**Mode 提到 Ask 之前**(`mod.rs:404-412` docstring 说明消除"用户点始终允许后被 Mode 拒"的坏交互),Ask 分散到 Tier 4 各分支,逻辑更清晰。**已二次核验**。
2. **Deny 优先于 Yolo,硬墙成立**(`mod.rs:430` 在 `:495` Yolo bypass 前)。Yolo 下 `rm -rf /` 仍静默拒绝(`:432` audit `ToolDeniedYolo`,`critical:true`)。
3. **审计覆盖度极高**: 11 类 AuditKind(代码实际 11 个,docstring 写"10"是 debt)**全部有触发点**,每条决策路径都写 audit(包括所有 Allow 路径)。
4. **oneshot future 清理三路齐全**: 用户响应(`resolve_ask` `map.remove`,`mod.rs:314`)+ 超时 120s(`ask_path` `:957`)+ cancel(`biased select!` `:947`)。
5. **root check 实现正确**(`commands/permissions.rs:52-60`): `unsafe libc::geteuid()`,**时机在 `set_session_mode` 进 Yolo 时**(`:104`)而非启动时,修复了前期 review §3.3。无 nix crate 依赖。
6. **Per-Mode Tool List 过滤已落地**(`filter_tools_for_mode` `mod.rs:1232`): Plan 模式从 tool list **物理移除** write_file/edit_file/shell,由 `chat.rs:526` 每 turn 调用。LLM 在 Plan 下根本看不到这三个工具——比仅 runtime 拦截更省 turn。

#### 不足点

| 级别 | 问题 | 证据 | 影响 |
|---|---|---|---|
| **P1-1** | `delete_session` **不直接**清理 `permission_asks`,靠隐性 cancel 链 | `commands/sessions.rs:126` 只调 `cancel_inflight_for_session`,未调 `cancel_session_asks`(`mod.rs:330`,标 `#[allow(dead_code)]`) | 实际不泄漏(biased select! 间接清理),但隐性依赖,`cancel_session_asks` 死代码误导维护者 |
| **P1-2** | `cancel_session_asks` 是 `map.clear()` 全清,session_id 参数被忽略 | `mod.rs:330-341` `_session_id: &str` 下划线前缀,body 直接 `clear()` | **latent bug**:一旦未来接到 `delete_session`,会误清其他 session 的 pending ask。当前安全只因未被调用 |
| **P1-3** | 超时分支未通过 `resolve_ask`(cancel 分支调了),清理路径不一致 | `mod.rs:955-969` 直接 `map.remove` 后 return | 实际无 race(双向安全),但可读性差 |
| **P2-1** | `sqlite_glob_match` 的 `?` 分支有 dead code | `mod.rs:766-783` 内层 `if tbytes[ti] == b'/'` 永远 true(外层已判),`return false` 必达 | 功能正确,代码冗余 |
| **P2-2** | 危险命令检测有**真实绕过路径** | `dangerous.rs:81-108` regex 无 `(?i)` 大小写敏感;`find / -delete` 走 `READ_ONLY_WHITELIST`(`shell_trust.rs:108`)**直接 Allow**;长选项 `--recursive`/子 shell `bash -c`/env 展开 `$VAR` 不匹配 | `find / -delete` 是漏网之鱼;其余有 Tier 4 Ask 兜底缓解 |
| **P2-3** | shell trust 结构降级 false positive | `shell_trust.rs:365` `cmd.contains('|')` 把 `grep "a|b"`(正则管道符)误降级 Ask | UX 打折,安全侧正确(fail-safe) |
| **P3-1** | AuditKind docstring 写"10 variants"实际 11(`ToolExecuted` C4 新增未更新 doc) | `mod.rs:140` vs `:152-179` | 文档 debt |
| **P3-2** | Background Mode 仍空壳 | `types.rs:193` `#[allow(dead_code)]`,`mode_system_prefix`(`:1214`)占位字符串 | UI 已移除,enum 保留预留,可接受 |

#### 与前期 review 一致性

| 前期 review 问题 | 原级别 | 现状 |
|---|---|---|
| §1 ⑨关顺序不一致 | P0 | ✅ **已解决**,且实现更优 |
| §1.4 Yolo 下 deny 行为 | P0 | ✅ 静默拒绝 |
| §2 Per-Mode Tool List 过滤 | P1 | ✅ **已解决** |
| §3.2 IPC 超时/去重 | P2 | ✅ 已解决(120s + `map.remove`) |
| §3.2 Session 删除僵尸 future | P2 | ⚠️ **间接解决**(cancel 链,无显式调用) |
| §3.3 root check 时机/libc | P2 | ✅ 已解决 |
| §4.3 SQLite foreign_keys | P3 | ✅ `migrations.rs:46` PRAGMA ON + CASCADE |

---

### 2.3 Memory / 指令文件 — ★★★★ (4/5)

**主体**: `memory/{mod,loader,file,watcher,tokens,types,tests}.rs` + `commands/memory.rs`

#### 做对的事 ✅

1. **cache_control 链路全程打通,选了技术最对的方案 B**。`ContentBlock::Text{cache_control}`(`types.rs:73-83`)→ `chat_message_to_wire_messages` 检测 `has_cacheable` 走 `WireMessage::UserBlocks` **保持块边界不 concat**(`wire.rs:281-333`)→ `WireBlock::Text{cache_control}`→ Anthropic serde `{"type":"ephemeral"}`。OpenAI 侧 drop。这是 FINDINGS 文档的方案 B(非最省事的 A/C)。
2. **每请求注入一次,靠 messages 历史 + cache_control 跨轮复用**。`build_instructions_blocks` 在 `for turn` 之外调用(`chat.rs:388-390`),后续每轮 `provider.send(..., messages.clone(), ...)` 带同一 pair → Anthropic 第 2..MAX_TURNS 轮命中 cache(5-min TTL)。
3. **watcher 用 `Weak<MemoryCache>` 不延长 AppState 生命周期**(`watcher.rs:80-83`,`state.rs:193`)。`upgrade()` 失败即 return,Drop 语义干净。
4. **每轮读穿 cache(read-through)**(`loader.rs:186-203`)。watcher 失效只置 slot 为 None,下次 read 自然 miss 重读。chat 中途不打断(grill Q5 决议)。
5. **大小校验防注入爆炸**: 单文件 cap 100 KiB(`mod.rs:60`),非 UTF-8 直接拒绝(`file.rs:164-186`,注释明确不 lossy convert 污染 CJK prompt)。
6. **isolated failure tolerance**: 单文件故障返回 `Error` 不影响其他 3 个(`mod.rs:16-22`)。
7. **IPC 安全边界**: `read_memory_content`/`open_memory_in_editor` 用 `all_paths` 白名单匹配(`commands/memory.rs:79-92,144-156`)。

#### 不足点

| 级别 | 问题 | 证据 | 影响 |
|---|---|---|---|
| **P1-1** | watcher debounce 1s 窗口内 race(已自查承认) | `watcher.rs:179-219` 收 notify → 标 pending → 等 1s debounce → 才 invalidate;`loader.rs:206-214` read-through 无 fence | 编辑器保存 → 立即下一条消息**概率性读到旧指令**。"立即生效"是概率性非确定性 |
| **P1-2** | 新建 project / 新建 memory 文件**不自动 watch** | `state.rs:178-197` 仅启动时收集一次 `list_projects`;运行时新增 project 目录不 watch(`watcher.rs:75-79` 注释承认) | 新建 project 后写其 CLAUDE.md,memory cache 不失效(直到重启)。与 PRD 文案一致但 UX 暗坑 |
| **P1-3** | token 估算不反映 cache 折扣(语义偏差) | `context.rs:85-133` estimator 无 cache 概念;head pair(PROTECTED_HEAD=2)走 cache 实际计费 0.1×,但 estimator 按全价算 | compact 比"实际成本"更激进,提前压缩掉还有空间的对话 |
| **P2-1** | `MemoryWatcher` 不在 AppState 持有,仅启动即弃 | `state.rs:192-197` 成功路径返回值被丢弃 | 靠 notify 内部 thread 生命周期间接维持,结构不健壮;也阻碍 P1-2 的 `add_watch` 扩展 |
| **P2-2** | user_dir 路径与 Claude Code 实际路径不一致 | `file.rs:58-66` 用 `~/.config/everlasting/CLAUDE.md`;Claude Code 用户级在 `~/.claude/CLAUDE.md` | 用户从 Claude Code 切过来**用户层指令不共享**(项目层共享成立) |
| **P2-3** | 4 文件总大小无 cap | `mod.rs:54-60` 仅单文件 cap | 4 × 100 KiB ≈ 100K token,占 200K 窗口一半,挤压对话空间 |
| **P2-4** | watcher 路径表 fallback 按 `file_name()` 匹配可能误触发 | `watcher.rs:331-339` 精确匹配失败后按文件名 | 跨 project 的 CLAUDE.md 写入可能误失效其他 project/user cache |
| **P3-1** | grill Q4 "AGENTS.md 物理顺序前置"未严格执行 | `loader.rs:321` 仍按 CLAUDE→AGENTS 顺序;优先级仅靠 `<primary>`/`<reference>` wrapper 标签 | 软提示 vs 硬提示,标签可能已足够 |
| **P3-2** | WSL/9p/drvfs 下 inotify 可靠性未验证 | — | `/mnt/c/...` 路径 watcher 可能收不到事件 |

#### 与 grill review 一致性(9 题决议)

7 题 ✅ 落地,2 题 ⚠️(Q4 顺序部分 / Q8 "~40 行"被 FINDINGS 提前纠正实际 150-200 行),0 题 ❌。**grill 决议精神全部兑现**。

**关键缺口**: FINDINGS §四 设计的 cache 命中实测(curl 验证 `cache_creation_input_tokens > 0` / `cache_read_input_tokens > 0)**无执行记录**。代码静态正确,但 Anthropic 是否真命中 cache 无实测数据。

---

### 2.4 Provider / Model / SSE — ★★★★½ (4.5/5)

**主体**: `llm/{mod,sse,error,types}.rs` + `llm/provider/{mod,wire,anthropic,openai}.rs` + `db/{providers,config,models,types}.rs` + `commands/{providers,config}.rs`

测试核验: `cargo test --lib llm` → **105 passed; 0 failed**。

#### 做对的事 ✅

1. **trait 抽象干净,object-safe**(`provider/mod.rs:66-99`)。`send` 返回 `Pin<Box<dyn Stream + Send + 'static>>`,两实现 `assert_send_sync` 编译期锁定。不用 `async_trait`(单方法 trait overkill)。
2. **Wire 中间层真正隔离两协议差异**(`wire.rs:114-244`)。tool_result 从 Anthropic `role:user` 内 lift 成独立 `WireMessage::Tool`,OpenAI 侧直产 `role:tool`(`openai.rs:210-219`),反向重组回 tool_result block(`wire.rs:636-650`)。
3. **`strip_unsupported` 单点决策矩阵**(`wire.rs:455-527`)。Anthropic→Anthropic 也跑 strip(caps 全 true→no-op),保证"唯一 strip 规则点"不变式。
4. **自研 SSE 取舍合理**(`sse.rs` 68 行三字段状态机,无 eventsource-stream 依赖)。
5. **catalog 失败降级全面**(`state.rs:264-319`)。DB 错误→空 catalog;单行失败→skip;chat 还有 catalog miss→DB slow path 三层 fall back。
6. **热重载写锁时长最小化**(`state.rs:242-246`)。`build_provider_catalog`(DB I/O)在 `write().await` **之前**,写锁只覆盖指针替换一行。进行中 chat 已 clone `Arc<dyn Provider>`,不受 rebuild 影响。
7. **thinking signature round-trip 1:1 锁定**(`wire.rs:1149-1231`)。`Thinking{thinking,signature}`→wire→回 `Thinking` 不退化成两块,避免 Anthropic 400。

#### 不足点

| 级别 | 问题 | 证据 | 影响 |
|---|---|---|---|
| **P1-1** | **API key 明文存储** | `db/migrations.rs:240` `api_key TEXT NOT NULL DEFAULT ''`;`commands/providers.rs:38-42` 原样写;`db/providers.rs:62-82` 原样读返回 IPC | DB 文件泄露=全部 provider key 泄露。`app_data_dir` 权限 0700 非绝对边界 |
| **P1-2** | OpenAI `max_tokens` 对 o1+ 模型**协议错误** | `openai.rs:243-248` 硬编码 `max_tokens`;o1/o3/o4-mini 要求 `max_completion_tokens`,发 `max_tokens` 会 400 | 用户配置 o1 model 后所有 chat 400 |
| **P2-1** | SSE parser 不容忍 `data:` 无空格 + `data_buf` 无上限(复述 REVIEW-sse P2) | `sse.rs:43,45` 精确前缀带尾随空格;`:13` `data_buf` 无 cap | 主流 provider 不触发;第三方代理发无空格版本被静默 drop;恶意上游 GB 级 data OOM |
| **P2-2** | `WireRequest.reasoning_effort` 是 dead field | `wire.rs:133` `#[allow(dead_code)]`,注释说"OpenAI reads it"但实际 OpenAI 读 `config.reasoning_effort`(`openai.rs:266`) | 架构误导,未来 PR 以为已接好 |
| **P2-3** | OpenAI `supports_reasoning_effort` caps hardcode true | `openai.rs:370-374`;`WireCapabilities::from_model_row`(`wire.rs:97-110`)已实现正确派生却没被调用 | gpt-4o(无 reasoning)model 错误保留 Reasoning 块,污染上下文 |
| **P3-1** | OpenAI 多 tool_call `index` 缺失默认 0 | `openai.rs:593-597` `unwrap_or(0)` | 两个无 index tool_call 都映射 index 0,后者覆盖前者。官方 API 总带 index,第三方兼容层风险 |
| **P3-2** | `parse_anthropic_usage` 全零判 None 假设 | `anthropic.rs:617-627` | 极低,真实响应 input 永远 >0 |

#### 与 REVIEW-sse §8 一致性(4 条改进建议)

| 建议 | 当前状态 |
|---|---|
| P2 SSE data_buf 加 1 MiB 上限 | ❌ 未实施 |
| P3 persist_turn 失败 emit Error | ❌ 未实施(子系统 A P1-1 升级重述) |
| P4 GLM max_tokens 500/400 误分类加 keyword | ❌ 未实施(`error.rs:129-136` keyword 无 `max_tokens`) |
| P5 mock HTTP server 集成测试 | ⚠️ 部分(有 live test,无 mock) |

**4 条 0 条完全落地,1 条部分**——明确债务 backlog。

---

### 2.5 Worktree / Git / Tools / Boundary — ★★★★ (4/5)

**主体**: `git/{mod,worktree,diff,error}.rs` + `commands/worktree.rs` + `projects/{mod,boundary,store,detector,types}.rs` + `tools/{mod,read_file,write_file,edit_file,grep,glob,list_dir,shell,web_fetch,read_guard}.rs`

#### 做对的事 ✅

1. **boundary 单点校验 + 双层防御**。`assert_within_root`(`boundary.rs:28-53`)双 canonicalize + component-wise `starts_with` 作 source of truth;7 edge case 全测覆盖(含 symlink-outside / broken symlink)。`is_within_root`(lexical,容忍不存在)只在 permission 层辅助,文档明确不替换工具层。
2. **worktree ↔ cancel 联动**。detach/delete 在命令 entry 调 `cancel_inflight_for_session`(`commands/worktree.rs:119-124,197-202`),先停 in-flight LLM 再动磁盘。
3. **ReadGuard 防 LLM 盲改**: 三 gate(`edit_file.rs:127-134`),fingerprint = mtime+size+head_hash 三维(`read_guard.rs:38-50`),写后 invalidate 强制重读。
4. **shell cancel/timeout/正常完成三向 race**(`shell.rs:263-306`,`biased` 让 cancel 永远先 poll),timeout/cancel 用 flag 区分,marker 不同。
5. **edit_file fuzzy match UX 对齐 claude-code**: 0 匹配 Jaccard hint,N>1 列行号 cap 20。
6. **diff 生成双源 + 兜底**: libgit2 不含 untracked 用 `statuses()` 补(`diff.rs:175-261`),line_stats 用 shell-out `git --numstat` 权威源 + libgit2 fallback(`diff.rs:123-148`)。
7. **worktree 元数据/分支名解耦**: `WorktreeAddOptions::reference` + `name=session_id`(无斜杠)避免 `git worktree prune` 误删(`worktree.rs:240-270` 完整踩坑记录)。

#### 不足点

| 级别 | 问题 | 证据 | 影响 |
|---|---|---|---|
| **P0-1** | `web_fetch` redirect **不做** per-redirect IP 校验(SSRF 绕过) | `web_fetch.rs:385` `Policy::limited(MAX_REDIRECTS)`;docstring `:17` 写"each redirect target"但 §5 security notes 又写"not implemented"——**spec 自相矛盾**,实现走弱路线 | `attacker.com/redirect → 169.254.169.254` 绕 SSRF 打 AWS metadata 泄漏 IAM 临时凭证。**已二次核验** |
| **P0-2** | `shell` 不 kill 进程组 → 子进程孤儿 | `shell.rs:79-99` `child.kill()` 只 kill 直接子;`shell.rs:237` `Command::new("sh")` **无** `process_group(N)` | `sleep 60 &`/管道/`nohup` 产生的孤儿进程 cancel/timeout 后继续跑。**已二次核验: grep `process_group` 仅 0 命中** |
| **P0-3** | `shell` 子进程继承父进程**全部环境变量**(含 ANTHROPIC_API_KEY) | `shell.rs:237` `Command::new("sh")` **无** `env_clear()` | LLM 一句 `env`/`printenv` 即窃取 API key,配合 shell `curl -X POST` 外传。LLM agent 经典提权面。**已二次核验: grep `env_clear` 0 命中** |
| **P1-1** | `glob` 用 sync `std::fs::read_dir` **阻塞 tokio runtime** | `glob.rs:205-226` `walk_dir` 被 async fn 直接调(`:115`);其他 tool 都用 `tokio::fs` | 大 repo(Chromium/Linux kernel)glob 卡死 worker,拖累同 runtime 并发 session |
| **P1-2** | worktree destroy 在 cancel 尚未生效窗口内删目录 | `commands/worktree.rs:237-260` destroy 紧接 `cancel_inflight_for_session`(`helpers.rs:198-205` 只 `token.cancel()` 不 await 退出) | 窄窗口内 agent loop 下一次写 ENOENT/panic/残留 fingerprint 指向已删文件 |
| **P1-3** | ReadGuard 进程内不持久,重启失效 | `read_guard.rs:17-21` 明示"Lifetime is the process" | 跨重启续聊每个 edit 多一轮 read(功能 safe,UX 退化) |
| **P1-4** | `edit_file` `find_similar_lines` 对大文件单行爆 | `edit_file.rs:277-306` 0 匹配时对每行 `split_whitespace` + `HashSet<char>` Jaccard | minified bundle 单行 1MB 字符的 HashSet 是灾难,CPU/内存爆 |
| **P2-1** | `read_file` UTF-8 切片 panic 风险(中文/emoji 文件 ≥50KB) | `read_file.rs:222-225` 按字节切片 `&content[..head_end]`;`diff.rs:298-302` 已修但 read_file **没同步** | 多字节字符边界 panic,**同 repo 内不一致** |
| **P2-2** | shell spillover 文件不日常清理 | `shell.rs:391-404` `cleanup_outputs_dir` 仅 `delete_session` 调 | 长跑 session 累积 30KB+ 输出文件,磁盘膨胀 |
| **P2-3** | worktree create self-heal 强制 `remove_dir_all` orphan 目录 | `worktree.rs:216-227`;注释 `:137-141` 自承"silent auto-cleanup would be a footgun"但接着删 | 罕见但灾难性,用户手动放的文件被无声删除 |
| **P3-1** | `git::worktree::data_dir` 走 env 而非 Tauri path | `worktree.rs:40-56` | Windows/macOS 部署后 worktree 路径异常(/tmp fallback 是 world-writable + 重启清空) |

---

## 3. 跨子系统主题分析

> 这是单看任何一路深挖都看不到的视角——五个子系统的发现拼在一起,暴露出几个**系统性主题**。

### 3.1 安全面:3 个 P0 全集中在 LLM 执行面

| P0 | 子系统 | 根因 | 利用难度 |
|---|---|---|---|
| shell env 泄漏 API key | E | `Command::new("sh")` 无 `env_clear` | **极低**:LLM 主动 `env` 即可 |
| shell 不 kill 进程组 | E | 无 `process_group(0)` | 被动:cancel/timeout 后孤儿累积 |
| web_fetch SSRF redirect 绕过 | E | `Policy::limited` 不重做 IP check | 中:需诱导 LLM 访问 attacker URL |

**为什么这是系统性问题而非孤立 bug**: 这三个 P0 共享同一设计前提——"LLM 执行的 shell/web 操作运行在与 agent 主进程**同等信任**的上下文里"。env 全继承、进程组共享、redirect 信任,都是把"LLM 可能被投毒(instructions/网页/npm 包)"这个威胁模型低估了。**Permission 系统(子系统 B)的 5 道检查防的是'tool 该不该执行',防不了'tool 执行起来后内部窃密'**。

修复方向是给 LLM 执行上下文**降权隔离**:
- shell: `env_clear()` + 白名单注入(PATH/HOME/LANG/TERM),排除 `*_API_KEY`/`*_TOKEN`;`process_group(0)` + kill PGID
- web_fetch: 自定义 `Policy::custom`,每 3xx 重做 `lookup_host` + `is_blocked`

三者各 5-30 行,是本次审视**优先级最高**的行动项。

### 3.2 数据完整性:C3 orphan + persist 静默失败 + worktree 竞态

三个分散的数据完整性风险,共同模式是**"失败路径被吞掉或边界没守住"**:
- A-P0-1: C3 压缩的配对保护在 tail 边界失效(算法自己注释承认)
- A-P0-2: compact 全丢完仍超窗时静默继续(无降级返回)
- A-P1-1: persist_turn 失败只 log,DB 与内存永久分叉
- E-P1-2: worktree destroy 不等 cancel 生效就删目录

**共同修复模式**: 给这些"吞错"路径补**显式错误传播 + 安全失败**(compact 返回 Result / persist 失败 emit Error / destroy 等 agent drained)。

### 3.3 取消语义:横跨 4 个子系统的隐性依赖链

cancel 的正确性依赖一条**跨子系统的因果链**:

```
permission:ask oneshot 清理(子系统B P1-1)
  ← 依赖 biased select! 听 cancel token
    ← 依赖 CancellationGuard RAII(子系统A)
      ← 依赖 delete_session/detach 调 cancel_inflight_for_session(子系统E)
        ← 依赖 session_active_request Map 一致性(子系统A state.rs)
```

链上任一环节改动都可能重新引入泄漏。**当前 `cancel_session_asks`(B)是死代码,安全性完全靠这条隐性链**。建议把隐性依赖显式化(delete_session 显式调清理 + 注释标注因果),否则未来重构极易破。

另外,**spec §2.5.1 的"二次取消才真终止"语义未实现**(A)— 单次 cancel 即终止。这是设计层面的偏离,影响 LLM 自我收敛能力。

### 3.4 债务积压:历史 review 建议 0 落地

`REVIEW-sse-agent-loop` §8 的 4 条改进(data_buf cap / persist 失败 emit / GLM max_tokens 误分类 / mock 集成测试)被**两路深挖(A 和 D)分别独立复述**,说明这些债务是真实的、可观测的、长期悬挂的。建议建立明确的"review 跟进 backlog",每条标注 owner + 优先级,避免下次 review 再次复述。

### 3.5 测试不对称:前端重测,后端 Agent Loop 无集成测

| 层 | 测试状态 |
|---|---|
| 前端 streamController | `streamController.test.ts` **54KB**,SSE 消费密集自测 |
| 前端 permissions/mode | `permissions.test.ts` / `chatMode.test.ts` 存在 |
| 后端 llm | `cargo test --lib llm` **105 passed** |
| 后端 boundary/read_guard/edit | 各 6-16 单测,edge case 覆盖好 |
| **后端 Agent Loop** | **仅单测级,无 mock HTTP server 驱动的完整 turn 集成测试** |

Agent Loop 是风险最高的子系统(turn 边界 cancel / max_turns / C3 触发 / orphan 配对),却测试最薄。补一个 `MockProvider` 实现 `Provider` trait 返回预设 `Stream<ChatEvent>`,跑完整 chat 命令断言 messages + DB rows,是最高 ROI 的测试投入。

### 3.6 跨子系统冗余/不一致

| 不一致 | 证据 | 修复 |
|---|---|---|
| UTF-8 字节切片:diff.rs 已修,read_file 没同步 | `diff.rs:298-302` vs `read_file.rs:222-225` | 抽公共 `floor_char_boundary` helper |
| AuditKind 数量:docstring "10" 实际 11 | `permissions/mod.rs:140` vs `:152-179` | 更新 docstring |
| token 估算:memory/context 共用 cl100k_base 但 head pair 重复计入未折扣 | `context.rs:43,132` | 文档化或加 cache 折扣 |
| reasoning_effort:wire 定义 dead field,caps hardcode | D-P2-2/P2-3 | 接通 `from_model_row` 或删字段 |

---

## 4. 与历史 review 的一致性对照

| 历史 review | 核心发现 | 当前状态 |
|---|---|---|
| REVIEW-sse-agent-loop (★★★★½) | §8 四条改进(data_buf cap/persist emit/GLM 误分类/mock 测试) | **0 完全落地,1 部分** |
| REVIEW-a2-b7-permission-plan | ⑨关顺序 P0 BLOCKER + Per-Mode tool list P1 | ✅ **全部解决且更优** |
| REVIEW-b5-memory-grill | 9 题决议 | 7 ✅ / 2 ⚠️(Q4 顺序 / Q8 行数估算) |
| FINDINGS-b5-cache-wire | 方案 B/C 选择 + cache 实测 | 选了 B ✅;**实测无记录** ❌ |

---

## 5. 优先级行动清单(全局汇总)

### P0 — 必须尽快修复(安全 + 数据完整性)

- [ ] **shell `env_clear()` + 白名单注入**(E-P0-3,`shell.rs:237`,~10 行)—— 排除 `*_API_KEY`/`*_TOKEN`,防 LLM 窃取密钥
- [ ] **shell `process_group(0)` + kill PGID**(E-P0-2,`shell.rs:79-99`,~15 行)—— 防孤儿进程
- [ ] **web_fetch 自定义 redirect policy 重做 IP check**(E-P0-1,`web_fetch.rs:385`,~30 行)—— 防 SSRF 打 metadata;同步修 spec 内部矛盾
- [ ] **C3 `group_droppable_turns` tail pair orphan**(A-P0-1,`context.rs:334-381`)—— 把 tail-adjacent assistant(tool_use) 纳入隐式保护,补触发该路径的单测
- [ ] **compact_messages 超窗降级返回**(A-P0-2,`context.rs:160-260`)—— 全丢完仍超 target 时返回 Result/emit Error,而非静默继续

### P1 — 重要(正确性 + 资源)

- [ ] **persist_turn 失败 emit Error**(A-P1-1)— 兑现 REVIEW-sse P3,防 DB/内存分叉
- [ ] **glob 改 `spawn_blocking`**(E-P1-1,`glob.rs:115`)— 防阻塞 runtime
- [ ] **worktree destroy 等 agent drained**(E-P1-2)— `cancel_inflight_for_session` 返回退出信号,destroy await
- [ ] **API key 加密存储**(D-P1-1)— `keyring` crate 或应用层对称加密
- [ ] **OpenAI `max_completion_tokens` for o1+**(D-P1-2,`openai.rs:243`)— `is_o1_family` 分支
- [ ] **record_tool_executed_audit 提到 cancel 检查之前**(A-P1-2,`chat.rs:1094-1116`)
- [ ] **head_sha 每 N 轮刷新**(A-P1-3,`chat.rs:362`)— 或每次 tool 执行后
- [ ] **delete_session 显式清理 permission_asks**(B-P1-1/P1-2)— 先把 `cancel_session_asks` 改成按 session 过滤,再接入
- [ ] **memory watcher 新建 project 自动 watch**(C-P1-2)— `MemoryWatcher` 提升到 AppState

### P2 — 中等(健壮性 + 债务)

- [ ] **兑现 REVIEW-sse data_buf 1MiB cap + GLM max_tokens 误分类 keyword**(D-P2-1)— 清债务 backlog
- [ ] **补 Agent Loop mock 集成测试**(A 测试缺口)— MockProvider 跑完整 turn
- [ ] **memory cache 命中实测**(C)— curl 验证 `cache_creation/read_input_tokens > 0`
- [ ] **read_file UTF-8 切片同步 diff.rs 修法**(E-P2-1)
- [ ] **危险命令检测加 `find -delete`/`(?i)`/长选项**(B-P2-2)
- [ ] **edit_file 大文件行级 cap**(E-P1-4)
- [ ] **shell spillover LRU 清理**(E-P2-2)
- [ ] **`WireRequest.reasoning_effort` 接通或删字段 + OpenAI caps `from_model_row`**(D-P2-2/P2-3)
- [ ] **4 文件总 token cap**(C-P2-3)

### P3 — 轻微(文档/一致性)

- [ ] AuditKind docstring "10"→"11"(B-P3-1)
- [ ] `MemoryWatcher` 注释过时(watcher.rs:21-24)(C-P3-5)
- [ ] user_dir 路径与 Claude Code `~/.claude/` 对齐决策(C-P2-2)
- [ ] `git::worktree::data_dir` 改 Tauri path(E-P3-1)
- [ ] grill Q4 AGENTS.md 物理顺序前置(C-P3-1)
- [ ] worktree self-heal 非空目录拒创建(E-P2-3)

---

## 6. 前后端衔接视角

| 衔接点 | 前端 | 后端 | 状态 |
|---|---|---|---|
| SSE 消费 | `streamController.ts`(1537 行)+ 54KB 测试 | `llm/sse.rs` + `chat.rs` emit | ✅ 前端重测,后端 emit 稳定 |
| permission:ask | `permissions.ts`(298 行)+ 120s timer 双保险 | `mod.rs:944-970` ASK_TIMEOUT=120s | ✅ 前后端超时对齐(都是 120s) |
| cancel | Stop 按钮 → `cancel_chat` | CancellationGuard RAII + biased select! | ✅ 链路完整 |
| provider 热重载 | `config.ts`/`providers.ts`/`models.ts` | `rebuild_catalog`(写锁最小化) | ✅ 进行中 chat 不受影响 |
| memory 热更新 | `memory.ts`(8.9KB) | watcher invalidate | ⚠️ 1s debounce race(C-P1-1) |
| mode 切换 | `chatMode.test.ts` | `set_session_mode` + DB 持久化 | ✅ |
| worktree | ProjectTabs 等 | `commands/worktree.rs` + cancel 联动 | ⚠️ destroy 竞态(E-P1-2) |

**总体**: 前后端衔接设计成熟,事件契约(chat-event/tool:call/tool:result/permission:ask)清晰,前端有对应测试。主要风险在后端执行面(shell/web_fetch 的 P0)和取消竞态窗口,这些前端无法感知也无法缓解,必须后端修。

---

## 7. 结论

**Everlasting 的 Agent Loop 架构是健康的**,分层清晰、自研不依赖 SDK、关键不变式(cancel/RAII/catalog/boundary/配对保护)都有单测锁定。对"对标 Claude Code 能力"的目标,地基稳固——chat/编辑代码/运行命令的主体能力已经闭环,多 Provider 抽象、Memory prompt caching、5 道权限 + 3 档 Mode 都是同类自研 harness 中的上乘实现。

**但当前有三个明确的、可立即修复的短板拖在"可对外发布"的门槛上**:

1. **LLM 执行面的 3 个安全 P0**(shell env 泄漏 / 不 kill 进程组 / web_fetch SSRF 绕过)——修复成本各 5-30 行,是发布前必须闭合的。
2. **C3 压缩的 tail pair orphan + compact 超窗静默**(2 个 P0)——会在长对话/大 tool_result 场景撞 Anthropic 400,影响核心体验。
3. **历史 review 债务积压 + Agent Loop 无集成测试**——是回归风险的长期来源。

建议的修复节奏:**先 P0(1-2 个 PR 闭合安全面 + C3)→ 再 P1(persist/竞态/API key)→ 最后 P2 债务清理 + 集成测试补齐**。完成 P0 后,整体评分可从 ★4 升到 ★4.5;补齐集成测试 + 二次取消语义后可冲 ★5。

---

## 附录: 审视覆盖的关键文件

| 子系统 | 主体文件 | 行数 |
|---|---|---|
| Agent Loop | `agent/chat.rs` `agent/context.rs` `agent/{helpers,system_prompt,thinking,provider,tests}.rs` | 1372+876+~900 |
| Permission/Mode | `agent/permissions/{mod,dangerous,shell_trust}.rs` `db/permissions.rs` `commands/permissions.rs` | 1644+207+732+266+343 |
| Memory | `memory/{mod,loader,file,watcher,tokens,types,tests}.rs` `commands/memory.rs` | ~2400 |
| Provider/SSE | `llm/{mod,sse,error,types}.rs` `llm/provider/{mod,wire,anthropic,openai}.rs` `db/{providers,config,models,types}.rs` `commands/{providers,config}.rs` | ~5500 |
| Worktree/Tools | `git/{worktree,diff,error,mod}.rs` `commands/worktree.rs` `projects/{boundary,store,detector,types,mod}.rs` `tools/{mod,read_file,write_file,edit_file,grep,glob,list_dir,shell,web_fetch,read_guard}.rs` | ~7800 |

> 本审视所有 P0 级断言(shell env_clear/process_group、web_fetch redirect、C3 orphan)均已通过 grep / 行号二次核验。file:line 引用基于 commit `a4fb302`,后续代码演进请以当前代码为准。
