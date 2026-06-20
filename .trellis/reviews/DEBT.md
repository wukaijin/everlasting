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

> **本文件仅记录当前 open 债项**。已 closed 条目不在此保留;通过 git log 或 §Re-evaluation Log 追溯。

## P1 — 重要(正确性 + 资源) [1 items]

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


## P2 — 中等(健壮性 + 债务) [3 items]

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


## P3 — 轻微(文档/一致性) [5 items]

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

## 优先级分布

| Level | Count | 说明 |
|---|---|---|
| P0 | 0 | 全部 closed(详见 §Re-evaluation Log + git log) |
| P1 | 1 | 正确性 + 资源,影响功能或可靠性 |
| P2 | 3 | 健壮性 + 债务,中长期清理 |
| P3 | 5 | 文档 + 一致性,可延后 |
| **Total** | **9** | 当前 open items |

---

## Feature Follow-ups (FT) — 已全部 closed

> 全部 6 项 FT-F-001 / FT-F-002 / FT-F-003 / FT-F-004 / FT-F-005 已 closed (2026-06-20 ~ 2026-06-21)。详见 git log 与最近 commit(`9b685c8` / `6bb5060` / `272fbe9` / `3bf2b99` / `9e41594` / `586d4a5`)。

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
| 2026-06-16 | RULE-D-003 | open | **closed** | MAX_DATA_BYTES=1MiB cap(超限 drop 余下 data)+ strip_prefix(data:) 去空格容忍无空格版;+4 测试,502 pass | §收尾路径建议 |
| 2026-06-17 | RULE-A-010 | open | **closed** | D3 PR3 实施时收口,方式 = spec 偏离声明(非实现二次取消语义):`docs/ARCHITECTURE.md §2.5.1` 加 "已知偏离" 注释 + `docs/IMPLEMENTATION.md §4` 2026-06-17 D3 ADR 完整说明偏离理由 + 未来实现路径(tool 取消分支 + cancel check 之间加 N 状态机)。原 "实现二次取消语义" 选项不在本批次做(MVP 简化决策:tool 取消窗口短 + 二次取消 UX friction + 单用户误点概率低) | `.trellis/tasks/06-17-d3-message-edit-resend` |
| 2026-06-17 | RULE-A-007 | open | **closed** | error arm 对称 cancel 路径 persist partial turn。设计决策 A/B/C:**A** = `ERROR_MARKER`(`"[生成出错中断]"`)text 追加,对称 `CANCELLED_MARKER` 既有模式(否决 metadata 双表达);**B** = persist 失败 log-only(对称 cancel tool_result persist 失败,否决 `emit_persist_failure`——error 路径已 pre-emit Error,再 emit 第二个 terminal Error 冲突);**C** = persist 成功后 emit TurnComplete(seq + latency 指向 partial turn),Error + TurnComplete 并存不冲突。新增 `ERROR_MARKER` const 在 `agent/helpers.rs`;改 `chat_loop.rs`(RULE-A-006 闭环后单一权威,不动 chat.rs)。5 新测试覆盖 partial text/empty/thinking+tool_use/persist-fail-log-only/TurnComplete;567 tests 全 pass,0 warning。spec 同步:`agent-loop-architecture.md` 加 "Turn-boundary persist symmetry" pattern + 测试表 5 行;`error-handling.md` 加 "Agent Loop Error Paths" 段 + persist 失败处理矩阵 | `.trellis/tasks/06-17-a-007-error-arm-partial-text` |
| 2026-06-18 | RULE-D-005 | open | **closed** | openai_caps() 从 config.reasoning_effort 派生 caps(替代硬编码 true);gpt-4o 无 thinking_effort → caps.supports_reasoning_effort=false → strip 丢弃历史 Reasoning 块。未直接调 from_model_row(send 签名不带 model_row,config.reasoning_effort 等价);+2 测试 | `.trellis/tasks/06-18-p2-reasoning-caps-estimator-dedup` |
| 2026-06-18 | RULE-D-004 | open | **closed** | 删 WireRequest.reasoning_effort 死字段(OpenAI-specific 不属 wire 层;真参数走 config)+ docstring + 初始化 + 9 处测试构造 | 同上 task |
| 2026-06-18 | RULE-A-008 | open | **closed** | 抽 push_message_tokens helper,estimate_messages_tokens 与 _iter 共用;case_1~7 回归通过 | 同上 task |

| 2026-06-19 | RULE-A-012 | open | **closed** | 双根因合并 single RULE。**A** provider reqwest `.timeout(60s)`(总 deadline,reqwest 文档明示不适合 SSE)改 `.read_timeout(60s)`(per-chunk,resets per SSE event),`anthropic.rs:209-211` + `openai.rs:424-426` 同步改,保留 `.connect_timeout(10s)`;**D** `chat_loop.rs:657` `Err(err)` 静默包装补 `tracing::warn!(request_id, turn, category=?err.category(), error=%err, "chat: LLM stream errored")`(`LlmErrorCategory` 只有 Debug 没有 Display,故 `?` 走 Debug,产出五类 variant name 同 Display 行为)。incident `mz8s3hqwx6rmqjswgte` / `messages.seq=37`(seq=36→37 间隔 60.403s 实锤);fix commit `05037ac`,cargo check + 6 个 agent_loop_error_* 集成测试全 pass(622 总数,0 warning)。Out of scope:抬总超时到 600s(LiteLLM 风格)——否决,`read_timeout=60s` 已 cover 慢代理,真 60s 无 chunk 是代理死了该报错;per-provider timeout 列(`providers` / `models` 表加列)——否决本次做,DB schema 改动有迁移成本,等真有多 provider 用户被掐再上。spec 沉淀:`.trellis/spec/backend/error-handling.md` §RULE-A-012 + `docs/IMPLEMENTATION.md §4` 2026-06-19 ADR | `.trellis/tasks/2026-06/06-19-fix-llm-streaming-timeout-and-tracing` |

| 2026-06-19 | RULE-E-013 | open | **closed** | system prompt 工具清单:删除硬编码枚举改通用表述(比原"动态生成"更治本,PRD D2);`build_system_prompt_no_hardcoded_tool_list` 回归保护;随 behavior_prompt 同 task 落地 | `.trellis/tasks/06-19-system-prompt` |

| 2026-06-20 | RULE-A-014 | open | **closed** | B6 PR2b 收口。`is_worker: Option<bool>` 加为 `run_chat_loop` 第 21 参;worker 嵌套调用 `chat_loop.rs:2155` 传 `Some(true)`,run_chat_loop 内部构造 `PermissionContext` 读 `is_worker.unwrap_or(false)`;PR1b 的 dead-code `_worker_permission_ctx` 块删除;production `chat.rs:249` + 33 个 `agent_loop_*` 集成测试调用点更新 `Some(false)`。端到端测试 `agent_loop_dispatch_subagent_general_purpose_plan_mode_write_denied`(`/tmp/everlasting_worker_escape.txt` 路径 + Edit mode + general-purpose + write_file,`tokio::time::timeout(15s)` 包裹)验证:worker Tier 4 ask_path 收到 `is_worker=true` → 立即 `Decision::Deny` 无 oneshot 等待无挂起,tool_result `is_error=true` + deny 原因,1 行 `tool_denied` audit 落地,0 行 `tool_permission_ask`(ask 路径 collapse 验证)。PR2a 修 RULE-A-015 拆出 2 处 `skip_persist` gate,精确 gate 数 = 16(原 spec 18 处,Phase 3 spec commit 同步 `agent-loop-architecture.md` + `tool-contract.md`)。cargo test --lib 726 pass(PR2a 725 + PR2b 1),0 新 warning(对比 PR2a 4 pre-existing background_shell + 1 pre-existing `permission_ctx` unused)。P2 22→21,Total 47→46 | `.trellis/tasks/06-20-b6-pr2-subagent-persistence` |
| 2026-06-20 | RULE-A-016 | open | **closed** | B6 PR3a 顺手修。`permissions::ask_path` worker 分支(原 line 1002-1009 `record_audit(ToolDenied)`)删除 + 改 emit `PermissionAskPayload` via sink → `SubagentBufferSink::emit_permission_ask` 写 transcript `PermissionAsk` entry(PR3 drawer 可见)。`audit_not_polluted_by_worker` 测试断言不变(delta == 2,researcher silent allow 本就不写 audit);`agent_loop_dispatch_subagent_general_purpose_plan_mode_write_denied` 测试断言反转:parent `tool_denied` count 0(原 1)+ transcript `PermissionAsk` count 1(原 0)+ audit delta ≤ 2(原 ≤ 3)。cargo test --lib 732 pass(PR2b 726 + PR3a 6 = 2 新 db tests + 4 新 PR2 hotfix subagent tests;agent_loop_* tests 数量未变只更新断言)。0 新 warning(对比 PR2b 4 pre-existing)。P2 21→20,Total 46→45 | `.trellis/tasks/06-20-b6-pr3-frontend-expand` |

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
| **B2 / B3**(输入/触发层;**D2 已于 2026-06-17 降档到第三档**,见 [IMPLEMENTATION §4 2026-06-17](../docs/IMPLEMENTATION.md)) | 无直接耦合 | 零负担推进,不顺手不修 |
| **D3**(消息编辑/重发) | 会重走 turn 边界 + message 持久化 → 自然碰到 **A-007**(error 路径 partial text 丢失)、**A-010**(二次取消语义) | 做 D3 时是修这俩的天然窗口 |
| **B6 Subagent**(第三档,harness 学习价值最高) | worker agent 独立 context/token 预算 → **A-008**(estimator 两版重复)、**D-004/D-005**(capabilities 派生错误会污染 subagent 上下文) | **进 B6 前先抽 A-008 helper + 修 D-004/D-005** |

### 已失效债务清理(本次评估发现)

`RULE-C-001`(2026-06-15)Resolution Notes:watcher 已**整文件删除**改 read-through mtime fence。以下 2 条 finding 引用的 `watcher.rs` 已不存在,本次标 wontfix:

- **RULE-C-007**(`watcher.rs:331-339` 路径表 fallback)→ wontfix
- **RULE-C-009**(WSL/9p inotify 可靠性)→ wontfix(无 watcher 可失效)

### 建议执行节奏

1. ✅ **B-004 + E-009 + D-003 全部完成**(2026-06-16,502 tests pass;执行节奏第 1 条三项收口)。
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

**最后更新**: 2026-06-21 by carlos — Session 54:**FT-F-002 closed**(`3bf2b99`)— SubagentDrawer 1.5s miss 后 inline 提示(原 toast fallback):grill 前提校准 —— retry polling(B6 PR3b)已是 race 吸收层,FT-F-003(unmount guard)不影响 miss 频率,1.5s miss=真实故障(worker 没启动/IPC 挂/ID 漂移)。收窄 drop toast/ToastService/session banner → 最小 inline(`workerMissed` 三态 default/waiting/missed + warn icon"worker 未响应,点此重试"+ `--color-tool-shell` warning tint + 复用卡片 @click 重试 + per-card)。miss 路径 `workerMissed=true` 在 FT-F-003 unmount guard 之后(不写 unmounted ref)。290 pass,vue-tsc 0 error。**同 Session 54 前序:FT-F-004 closed**(`9e41594`,UX polish bundle C1+C2+C3)。**FT-F family 全部 closed**(001/002/003/004/005)。
**下个 review**: REVIEW-XXX-2026-XX-XX(待定)