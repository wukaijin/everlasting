<!-- Moved from permission-layer.md 2026-09-19 (doc-split) -->

### 4. ⑨ 关 5-Tier Decision Order

PR1's `agent::permissions::check` runs a 5-tier evaluation
in this exact order (SOT, matches Claude Code's
`deny > ask > mode > allow` rule from `permissions.md`):

```
Tier 1. Hooks           — pre-call interface (MVP: no-op)
       │ 命中 hook override? → 用 hook 决定(本期不实现)
       ↓
Tier 2. Deny rules      — hard kill list (dangerous::is_kill_listed)
       │ 命中 → 直接 Decision::Deny { critical: true, reason: ... }
       │ Yolo 模式也走 — 静默拒绝,不弹窗
       │ → Tier 6 写 audit (kind="tool_denied" 或 "tool_denied_yolo")
       ↓
Tier 3. Ask rules       — session_tool_permissions + emit + await
       │ 查 session_tool_permissions:
       │   有 "始终允许" 记录 → 跳过弹窗, 直接 Allow
       │   无 → emit("permission:ask", { rid, tool_name, tool_input, risk, reason? })
       │       等前端 permission_response (120s 超时 → 自动 Deny)
       │ 收到响应:
       │   allow_once  → 放行(不写表)
       │   allow_always → 放行 + INSERT INTO session_tool_permissions
       │   deny        → Deny { reason: "user denied" }
       │   timeout     → Deny { reason: "permission timed out after 120s, treat as denied" }
       │                + audit kind="permission_timeout"
       ↓
Tier 4. Mode check      — Plan 拦截 (3 档化 2026-06-13: Review 移除)
       │ Plan 模式 + tool ∈ {write_file, edit_file, shell}
       │ → Deny { reason: "I cannot execute X in Plan mode" }
       ↓
Tier 5. Allow rules     — 默认 allow-all (MVP 阶段)
       ↓
Tier 6. Audit hook      — 每个决策路径写 session_audit_events
       │ kind: tool_allowed / tool_denied / tool_permission_ask /
       │       permission_granted / permission_timeout / request_cancelled
       ↓
   → execute_tool(若 Allow) / 构造 is_error tool_result(若 Deny)
```

**关键行为**:
- **Deny 优先于 Ask**:`rm -rf /` 在 Yolo 模式下也是静默拒绝
  (Tier 2 在 Tier 3 之前)
- **Tier 3 拒绝 ≠ Cancel 整轮**:Deny 只跳该 tool_use,LLM 收到
  `is_error: true` 可自决;CancellationToken (C1) 才是整轮终止
- **超时 vs 主动 deny** 在 audit log 区分:`reason` 字段不同
  ("user denied" vs "permission timed out after 120s, treat as denied")
- **2026-09-03(task `09-03-ask-no-timeout`)**:全局开关 `ask_no_timeout`
  开 → Tier 3 的 120s 超时臂禁用(timeout sleep 换 `pending()`),
  审批无限挂起等用户;audit 仍区分路径,但 `permission_timeout` /
  `worker_ask_timed_out` 不再产生。开关缺省关,行为与本节一致。

#### 4.1. Re-grill update 2026-06-13: 5-tier 重排 + path-based 决策

> Supersedes 4 节上述旧设计。Source of truth:
> `.trellis/tasks/06-13-a2-b7-regrill-path-based/prd.md` §1。

旧设计 ⑨ 关 Tier 3 "总是弹窗" 在 Edit 模式下读 README
都要弹,反直觉;Tier 4 Mode check 在 Ask 之后让 Plan
模式下"用户点始终允许,然后被 Mode 拒"成为坏交互。
re-grill 锁定 10 决策,把决策层重构:

**新 Tier 顺序**:

```
Tier 1. Hooks           (MVP no-op)
Tier 2. Deny rules      (硬 kill list,shell 10 个 regex,Yolo 走 — 静默拒)
Tier 3. Mode check      (Plan 拦截 write/edit/shell,text 错,不发 modal)
Tier 4. Path / Prefix / External policy
       ├─ Path 工具:is_within_root → 查 session_tool_permissions
       │   (match_kind='path') → hit Allow / miss silent(in) / miss ask(out)
       ├─ Shell:classify_prefix → Allow(whitelist) / Ask(asklist + 未知)
       └─ Web Fetch:查 match_kind='tool' for 'web_fetch' → hit Allow / miss ask
       (Yolo:整段 bypass,直接 Allow;Tier 2 仍 hard wall)
Tier 5. Allow rules     (default allow-all)
Tier 6. Audit           (写 session_audit_events)
```

**跟旧设计 diff**:

| 改动 | 旧 (PR1) | 新 (re-grill) |
|---|---|---|
| Tier 顺序 | Hooks → Deny → Ask → Mode → Allow → Audit | Hooks → Deny → **Mode → Path/Prefix** → Allow → Audit |
| 弹窗判定 | risk 等级 + 总是弹 | **path-based**:仓库内 silent,仓库外 ask |
| Mode check 时机 | Tier 4(在 Ask 之后) | **Tier 3(在 Ask 之前)** — 消除 Plan + 始终允许坏交互 |
| "始终允许"持久化 | 只 `tool` | **3 种 match_kind: tool + path-glob + prefix** |
| shell 策略 | 总是 Tier 3 | **白名单/asklist/未知 三档**(prefix 解析) |
| Yolo × 仓库外 | 走 Tier 3 modal | **silent**(Yolo bypass Tier 4) |
| Tier 2 kill list | 9 regex → 10 regex | **不变**(RULE-B-004 加 find -delete/-exec 后 10 条) |
| `PermissionAskPayload` | rid + tool + input + risk + reason | + **`path: Option<String>`** (新, `skip_serializing_if`) |
| `Risk` 字段 | 4 档 | 不变(4 档,UI 视觉) |

详细 ⑨ 关 contract 见本文 §4(5-Tier);`shell_trust::classify_prefix` 的
whitelist / asklist 完整表见 `app/src-tauri/src/agent/permissions/shell_trust.rs` 模块文档。

#### 4.2. Tier 1 Hooks 实际实现路径 — P3 工具执行前召回(2026-06-29, 06-29-am-p3-tool-recall)

> **P3 收口前** Tier 1 Hooks 一直是 no-op(MVP 留口);P3 落地时**没有**改
> `permissions::check()` 内部(保持 5-tier 拦截链纯净),而是把"工具执行前
> 召回 pitfall"挂到了 `chat_loop.rs` 的"check → execute_tool"seam 上。

- **函数**:`agent::permissions::recall_pitfall_footnote(pool, tool_name, tool_input) -> Result<Option<String>, sqlx::Error>`
- **调用点**:`chat_loop.rs` parallel-batch L2 path(line ~1792) + serial path(line ~2361)
  - 时机:`permissions::check()` 返回 `Allow` **之后**、`execute_tool()` **之前**
  - 不走 `check()` 内部 → 5-tier 拦截链顺序未被打乱;P3 recall 是旁路
- **行为**:
  - 调 `db::memories::find_pitfalls_by_trigger(pool, tool_name, command, path)`(P1 产出)
  - 过滤 `status == 'active'`(`verified` 留 P5 软拦截,本阶段不消费)
  - 命中后构造 `⚠️ Memory: 此前在本项目执行类似操作时踩过坑 —\n• [title] content\n...` 注脚文本
  - `bump_hit_count` 走 `tokio::spawn` fire-and-forget,不阻塞 recall 步骤
  - 注脚 prepend 到 `tool_result.content` **在 envelope wrap 之前** — `tool_use_id` 配对与 `is_error` 语义不变
- **降级**:`Err(sqlx::Error)` → `tracing::warn!` + 返回 `None`,工具照常执行。**Recall failure 永不阻断工具执行**(PRD hard rule)
- **Decision 语义**:`check()` 仍返回 `Decision::Allow/Ask/Deny`,recall 仅产出 `Option<String>` 注脚;不参与决策链

**为什么挂在 seam 而非 `check()` 内部**:
1. **5-tier 纯净性**:`check()` 是决策层,recall 是"信息注入",职责不同
2. **P5 扩展性**:P5 的 verified 软拦截需要返回结构化 `Decision`,可直接进 `check()` Tier 1;P3 的 active 注脚是旁路 hint,放 seam 简化 P5 落地
3. **可测性**:`recall_pitfall_footnote` 是纯函数(pool + 字符串入参 → `Result<Option<String>>`),`tests_check.rs` 直接单测无需 mock 决策链

**Tests**(6 个,在 `permissions/tests_check.rs`):
- `recall_pitfall_footnote_active_hit_returns_text`
- `recall_pitfall_footnote_unrelated_tool_returns_none`
- `recall_pitfall_footnote_verified_hit_returns_none_for_p3`(verified 是 P5 范围,P3 严格排除)
- `recall_pitfall_footnote_candidate_hit_returns_none`(candidate 是 P2 范围,P3 严格排除)
- `recall_pitfall_footnote_command_pattern_mismatch_returns_none`
- `recall_pitfall_footnote_empty_db_returns_none`

#### 4.3. grant 入口 kind↔类别校验 + prefix 读侧消费(RULE-PERM-002 闭合,2026-08-27)

> 来源:任务 `.trellis/tasks/archive/2026-08/08-27-rule-smoke-perm-cleanup/`。

**契约**:Tier 4 消费矩阵决定 `(tool, match_kind)` 只有唯一合法组合 —— 写侧
grant 入口(`grant_tool_permission_inner`,IPC + daemon route 共用)按
`classify_tool` 拒绝一切其它组合(`ErrorCategory::InvalidRequest`),杜绝
"入库成功但永不生效"的死授权行:

| classify_tool | 工具 | 唯一合法 match_kind | 消费方(Tier 4) |
|---|---|---|---|
| `Path` | read_file / write_file / edit_file / list_dir / grep / glob | `path`(带 glob) | `check_path_grant`(只查 kind='path' 行) |
| `Shell` | shell / run_background_shell | `prefix`(带首 token) | `check_prefix_grant` |
| `WebFetch` / `GitMutation` / `Other` | web_fetch / merge_worker / discard_worker / 未知 | `tool`(value NULL) | `has_tool_permission` 族 |

- 校验函数 `commands::permissions::validate_grant_match_kind` **必须复用
  `classify_tool`**(经 `agent::permissions::check` re-export),不许出现第二份
  分类逻辑;矩阵与写侧自动挑 kind 的 `match_value_for_allow_always` 逐类一致
  (该函数产出的组合天然过校验,AllowAlways 路径零影响)。
- **拒绝而非转译**:tool→prefix 无从推导前缀,空 match_value 前缀 = 全放行,
  是提权不是兼容。默认 `match_kind=None → "tool"` 的回退**先解析再校验**
  —— `grant(shell, None)` 落在债项原始场景上,必须被拒。
- **prefix 读侧消费 `tool_name IN ('shell','run_background_shell')`**:
  AllowAlways(ask.rs)用原始 tool_name 直写 DB,在 `run_background_shell`
  上点击会写 `(run_background_shell, prefix, <token>)` 行;读侧若硬编码
  `tool_name='shell'` 该行永不命中(用户"始终允许"不粘轮)。读侧放宽同时
  救活两条写路径与存量行;worker 的 `RunGrantCache`(内存,raw tool_name
  精确相等)不经 DB,不在此契约内。

**Tests**:`commands/permissions.rs` tests 模块(`grant_kind_validation_*` 4 条:
全矩阵合法组合 / shell+默认 tool 拒绝 / path+prefix 拒绝 / 文案含唯一合法 kind)
+ `permissions/tests_check.rs`
(`tier4_prefix_grant_under_run_background_shell_short_circuits`:run_background_shell
名下 prefix 行命中短路 Allow)。

