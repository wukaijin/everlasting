## Scenario: `use_ui` `button` primitive + `apply_ui_diff` IPC (B9+ D3+D4, 2026-07-13)

> **Source of truth**: implementation lives in
> - `app/src-tauri/src/tools/use_ui.rs` (B9+ D3 schema/validator)
> - `app/src-tauri/src/diff_apply.rs` (hand-written unified-diff parser/applier)
> - `app/src-tauri/src/commands/ui.rs` (`apply_ui_diff` IPC handler)
> - `app/src-tauri/src/agent/permissions/audit.rs` (`AuditKind::UiDiffApplied`)
> - Frontend `app/src/components/chat/primitives/DiffPrimitive.vue` /
>   `ButtonPrimitive.vue` / `app/src/utils/uiDiffApply.ts`
>
> **何时读本文**:涉及 `use_ui` 新 `button` primitive / `apply_ui_diff`
> IPC / `AuditKind::UiDiffApplied` / 手写 hunk apply 时。

### 1. Scope / Trigger

B9 (07-02) shipped `use_ui` as silent-allow display-only with
`diff` + `code_block` primitives. Parent PRD deferred D3 (independent
button + action allowlist) and D4 (diff apply) — B9+ (`07-13-b9plus-
generative-ui-followup`) ships both. The motivating gap: LLM could
**propose** changes via diff cards but had **no way to let the user
commit them**. D3 adds a generic button primitive for human-in-the-loop
intent signaling; D4 wires the apply path.

**Why code-spec depth — mandatory**: D3 + D4 cross three security
boundaries (LLM tool layer / user-triggered IPC / project boundary /
audit log). The "apply diff" action is the project's first **user-
click-driven write path** that lives outside the LLM tool permission
chain — getting the contract wrong opens up arbitrary file writes.

### 2. Signatures

#### `use_ui` `button` type (D3)

Schema addition in `definition().input_schema`:

```jsonc
"type": {
  "type": "string",
  "enum": ["diff", "code_block", "button"],   // ← "button" added
  ...
},
"action": {
  "type": "string",
  "enum": ["apply_diff", "copy", "dismiss"],  // ← new (button only)
  "description": "(button only) ..."
},
"label": {
  "type": "string",
  "description": "(button only) Optional override for the button label."
}
```

`execute` validation:
- `button` type must carry `action ∈ KNOWN_BUTTON_ACTIONS` (defined in
  `tools::use_ui.rs`); missing / unknown → `is_error: true` with the
  offending index + value surfaced.
- `action = "apply_diff"` additionally requires
  `payload.diff_text` (string, non-empty after trim). Same validation
  pattern as `edit_file`'s `old_string` non-empty check.
- `use_ui` still **does not execute** any action — it only validates
  the shape and returns `"已渲染 N 个 primitive"`. The click-time
  dispatch is in the frontend `<ButtonPrimitive>` renderer.

#### `apply_ui_diff` IPC (D4, user-triggered — NOT a tool)

```rust
#[tauri::command]
pub async fn apply_ui_diff(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    diff_text: String,
) -> Result<ApplyUiDiffResult, String>;
```

Wire shape (`ApplyUiDiffResult`):

```ts
type ApplyUiDiffResult =
  | { ok: true; files: Array<{ path: string; added: number; removed: number }> }
  | { ok: false; kind: "boundary" | "parse" | "conflict" | "io" | "empty"; error: string };
```

**Not registered as a tool** — `apply_ui_diff` lives in
`commands::ui`, NOT in `builtin_tools()`. Therefore:
- `filter_tools_for_mode` doesn't see it (plan mode users can still
  apply proposed diffs).
- `risk_for_tool` doesn't apply (it's not a tool).
- `PermissionStore` is bypassed (user click IS the authorization).

#### Audit row (`AuditKind::UiDiffApplied`)

```rust
AuditKind::UiDiffApplied => "ui_diff_applied"
```

Payload (`record_ui_diff_applied_audit`):
```jsonc
{
  "files": [{ "path": "...", "added": N, "removed": M }, ...],  // ≤ 32 entries
  "total_files": N
}
```

Failure paths (`boundary` / `parse` / `conflict` / `io` / `empty`)
do NOT write the audit row — `apply_ui_diff` only audits **after all
files written successfully**. Frontend shows inline error keyed by
`kind`.

### 3. Contracts

#### Apply pipeline (D4)

1. **Empty `diff_text`** → `{ok: false, kind: "empty"}`. Fast-fail.
2. **`parse_unified_diff`** (`diff_apply.rs`) — `parse` failure
   → `{ok: false, kind: "parse"}`.
3. **Resolve write target** = `session.worktree_path` ??
   `session.current_cwd`. No worktree ≠ reject (per
   `edit_file:369` chat_loop convention).
4. **For each FilePatch** (all-or-nothing across the multi-file
   apply):
   - Resolve absolute path (relative anchors on write root).
   - `assert_within_root(write_root, requested)` — `boundary`
     failure → `{ok: false, kind: "boundary"}`.
   - `tokio::fs::read_to_string` — read failure →
     `{ok: false, kind: "io"}`.
   - `apply_to_file(patch, &current)` — context mismatch /
     past-EOF → `{ok: false, kind: "conflict"}`. **No writes
     happen at all** on any patch failure.
5. **All patches applied in memory** → write each file. Single
   write failure → `{ok: false, kind: "io"}`; partial writes have
   already landed (MVP doesn't rollback; the error surfaces so
   the user can investigate).
6. **`record_ui_diff_applied_audit`** (best-effort, like other
   audit helpers — failure logged at `warn!`, never propagated).
7. **Success response** with `files` array.

#### Hand-written `diff_apply.rs` (D-Q5 zero-dep)

Pure functions, no I/O:

```rust
pub fn parse_unified_diff(text: &str) -> Result<Vec<FilePatch>, ParseError>;
pub fn apply_to_file(patch: &FilePatch, current: &str)
    -> Result<(String /* new content */, FileStats /* {added, removed} */), ApplyError>;
```

**Algorithm**: for each hunk, in order, walk `@@ -oldStart,oldLines
+newStart,newLines @@`; `oldStart` is 1-indexed in the **original**
file (not shifted by prior hunks). Track `cumulative_offset =
Σ (newLines - oldLines)` over applied hunks; in the modified buffer
the hunk starts at `oldStart - 1 + cumulative_offset`. Verify
Context+Remove lines match the buffer at that position; mismatch →
`ApplyError::Conflict` (fail-fast, no partial application).

**Load-bearing parser invariant**: `parse_unified_diff` MUST emit ONE
`FilePatch` per file path even when a diff has multiple `@@` hunks for
that file (`push_or_merge_hunk` appends by cleaned path). This is the
precondition for `cumulative_offset` to compose — splitting same-path
hunks into N `FilePatch`es would make the IPC's per-`FilePatch`
read-original → apply → write loop silently drop earlier hunks
(last-write-wins on the shared path). Regression guard:
`parse_then_apply_multi_hunk_same_file` (B9+ P0 fix, 2026-07-13).

**MVP 能力边界** (see `diff_apply.rs` module docstring):
- ✅ Standard unified diff, multi-file, multi-hunk
- ✅ `@@` line-number + context-line verification
- ✅ Conflict fail-fast (no partial writes)
- ❌ Binary diff / new empty file / rename / mode change
- ❌ LLM-style headerless `+/-` fragments (DiffPrimitive
  apply button disabled; ButtonPrimitive's `apply_diff`
  payload has same gate via `parse_unified_diff` returning
  `ParseError::MissingHeader` or `IncompleteHeader`)

#### Frontend `<DiffPrimitive>` / `<ButtonPrimitive>` action dispatch

Both renderers consume the same `applyUiDiff` IPC
(`utils/uiDiffApply.ts`):

| Renderer | Action        | Frontend handler                                  |
|----------|---------------|---------------------------------------------------|
| `<DiffPrimitive>` (Apply button) | implicit — uses `primitive.diff_text` | `applyUiDiff(sid, primitive.diff_text)` |
| `<ButtonPrimitive>` `apply_diff` | `applyUiDiff(sid, primitive.payload.diff_text)` | |
| `<ButtonPrimitive>` `copy` | `navigator.clipboard.writeText(primitive.payload.text)` + toast |
| `<ButtonPrimitive>` `dismiss` | local-only hide (`v-if="state !== 'done'"`) |

Success UX (apply_diff): toast「已应用 N 个文件」+ card marked
「已应用」+ buttons disabled.
Failure UX: inline error keyed by `kind`
(`APPLY_UI_DIFF_ERROR_TEXT` table in `uiDiffApply.ts`):

| kind | Inline text |
|------|-------------|
| `boundary` | "路径越界 — diff 中包含项目外的文件路径" |
| `parse` | "diff 格式无法应用 — 需要带 ---/+++ 路径头的标准 unified diff" |
| `conflict` | "文件已变 — diff 的上下文行与当前文件不匹配,请重新生成 diff" |
| `io` | "文件读写失败 — 请检查文件权限和磁盘空间" |
| `empty` | "diff 内容为空" |

#### Raw fallback gate (D-Q8)

`<DiffPrimitive>` exposes `hasUnifiedHeaders` predicate
(`/^--- /m.test(text) && /^\+\+\+ /m.test(text)`). Raw fallback
fragments (no path headers) → Apply button `disabled` + tooltip.
Backend `parse_unified_diff` is the second-line gate (defense-in-
depth — returns `kind="parse"` if the apply is somehow attempted).

### 4. Validation & Error Matrix

| Condition | `apply_ui_diff` outcome |
|-----------|-------------------------|
| `diff_text` empty/whitespace | `{ok: false, kind: "empty"}` |
| No `---`/`+++` headers (LLM fragment) | `{ok: false, kind: "parse"}` |
| Malformed `@@` hunk header | `{ok: false, kind: "parse"}` |
| Path outside `write_root` (`assert_within_root` fail) | `{ok: false, kind: "boundary"}`, no writes |
| File read failure (permission / ENOENT) | `{ok: false, kind: "io"}`, no writes |
| Any hunk context mismatch | `{ok: false, kind: "conflict"}`, no writes |
| Hunk past EOF | `{ok: false, kind: "conflict"}`, no writes |
| All patches apply successfully, write fails | `{ok: false, kind: "io"}`; partial writes already landed |
| All writes succeed | `{ok: true, files}`, `AuditKind::UiDiffApplied` row |
| `audit.write` fails | log `warn!`, still return `{ok: true}` (audit is best-effort) |

| `use_ui` button validation | Result |
|-----------------------------|--------|
| Missing `action` | `is_error: true`, message names the bad index |
| `action` not in enum | `is_error: true`, message lists the bad action |
| `action = apply_diff`, missing `payload.diff_text` | `is_error: true` |
| `action = apply_diff`, empty `payload.diff_text` | `is_error: true` |

### 5. Good / Base / Bad Cases

**Good (apply_diff happy path)**: session with worktree `/proj` →
LLM emits `use_ui({primitives: [{type: "diff", diff_text: "--- a/foo.rs\n+++ b/foo.rs\n@@ -1 +1 @@\n-old\n+new\n"}]})` → user clicks Apply → IPC parses → `assert_within_root("/proj", "/proj/foo.rs")` ok → reads `/proj/foo.rs` (contains `old\n`) → `apply_to_file` matches context → write `new\n` → audit `ui_diff_applied {files: [{path: "/proj/foo.rs", added: 1, removed: 1}]}` → toast「已应用 1 个文件」+ card marked「已应用」.

**Base (multi-file apply)**: 3-file diff; first 2 apply successfully, third has conflict → all 3 fail (no writes anywhere); user sees `kind="conflict"` inline error.

**Good (button copy)**: `use_ui({primitives: [{type: "button", action: "copy", payload: {text: "snippet"}}]})` → user clicks 复制 → `navigator.clipboard.writeText("snippet")` → toast「已复制到剪贴板」+ card hides.

**Bad (LLM tries `run_command` button)**: `use_ui({primitives: [{type: "button", action: "run_command", payload: {command: "rm -rf /"}}]})` → `execute` rejects with `action='run_command' 不在支持列表 ["apply_diff", "copy", "dismiss"]`. Pre-MVP gating (D-Q3: command-class actions deferred, not yet in enum).

**Bad (apply_diff path outside root)**: diff with `--- a/../etc/passwd` → `assert_within_root` rejects → `kind="boundary"`, no write, audit not landed.

**Bad (LLM emits headerless fragment)**: `-old\n+new` → DiffPrimitive's `hasUnifiedHeaders` is false → Apply button `disabled`. If somehow invoked (defense-in-depth), backend returns `kind="parse"`.

### 6. Tests Required

**Backend (`diff_apply::tests` — 24 cases)**:
- Parse: standard unified / multi-file / multi-hunk / short header
  (`@@ -5 +5 @@`) / `\ No newline at end of file` marker / empty
  context line / empty input / missing headers (LLM fragment) /
  invalid hunk header / incomplete header
- Apply: simple replacement / pure addition / pure deletion /
  multi-hunk with offset (no real shift) / multi-hunk with real
  shift (+2 line offset) / trailing newline preserved / no
  trailing newline preserved / context mismatch (Conflict) / past
  EOF (Conflict)
- End-to-end: parse + apply round-trip

**Backend (`commands::ui::tests` — 4 cases)**:
- `parse_error_kind_label_is_parse` — string-literal compile guard
  for the 5 IPC kind labels
- `success_result_omits_kind_and_error` — verify the
  `skip_serializing_if = "Option::is_none"` field attributes
- `error_result_carries_kind_and_error` — failure variant
- `parse_error_variants_match_handler_mapping` — `parse_unified_diff`
  empty / no-headers produce the same `kind="parse"` outcome

**Backend (`tools::use_ui::tests` — 20 cases, +7 B9+ D3)**:
- `execute_button_apply_diff_happy` / `_copy_happy` / `_dismiss_happy`
- `execute_button_missing_action_rejected`
- `execute_button_unknown_action_rejected`
- `execute_button_apply_diff_missing_payload_rejected`
- `execute_button_apply_diff_empty_diff_rejected`

**Frontend (`DiffPrimitive.test.ts` — 17 B9+ D4 cases)**:
- IPC invoke spy with `(sessionId, diffText)`
- Success: toast + 「已应用」 tag + buttons hidden
- Failure: inline error keyed by `kind`
- Raw fallback: Apply `disabled` + tooltip
- No active session: Apply `disabled` + tooltip
- Unexpected IPC error → `kind="io"`
- Reject: card hides, no IPC

**Frontend (`ButtonPrimitive.test.ts` — 11 cases)**:
- Default labels per action (apply_diff → "应用", etc.)
- Custom `label` override
- `apply_diff` invokes `apply_ui_diff` with `(sessionId, payload.diff_text)`
- `copy` invokes `clipboard.writeText` with `payload.text`
- `dismiss` hides card locally (no IPC, no toast)
- Disabled state when no active session (apply_diff only)
- Defensive: unknown action degrades to no-op click

### 7. Wrong vs Correct — apply path permission boundary

#### Wrong: `apply_ui_diff` registered as a tool

```rust
// BAD — registering as a tool puts it in builtin_tools(), so
// filter_tools_for_mode removes it in plan mode. The whole
// point of the IPC is to be USER-clickable in plan mode
// (plan constrains the LLM, not the user).
pub fn definition() -> ToolDef {
    ToolDef { name: "apply_ui_diff", ... }
}
```

**Why it's wrong**: `filter_tools_for_mode` drops `apply_ui_diff`
in plan mode → LLM can't even surface the button to the user →
plan mode has no "propose diff → user clicks Apply" path. D-Q1
explicitly rejected this; the IPC is `commands::ui::apply_ui_diff`,
NOT a tool.

#### Correct: separate `commands::ui::apply_ui_diff` IPC

```rust
// GOOD — lives outside the tool registry. Plan mode users can
// still apply diffs the LLM proposes (via DiffPrimitive /
// ButtonPrimitive action="apply_diff"). User click IS the
// authorization; no Tier / PermissionStore consult.
#[tauri::command]
pub async fn apply_ui_diff(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    diff_text: String,
) -> Result<ApplyUiDiffResult, String> { ... }
```

Sibling to `merge_worker_run` (L3b PR3, 2026-06-27) which is also
a user-triggered IPC with `assert_within_root` + audit, no Tier /
PermissionStore.

### 8. Design Decisions

#### Decision: `apply_ui_diff` is user-triggered, NOT a tool (D-Q1)

**Context**: B9+ D4 needs to write files to disk from a `<DiffPrimitive>`
click. Two options: (a) make it an LLM tool the user authorizes via
Tier 4 ask; (b) make it a user-triggered IPC that bypasses Tier.

**Decision**: (b). The user clicking Apply IS the authorization —
mirroring `merge_worker_run` (L3b PR3, 2026-06-27). This gives
plan-mode users a write path (plan constrains the LLM but not the
user), avoids spurious Tier 4 asks (the user already clicked), and
keeps the IPC outside `filter_tools_for_mode` (which would otherwise
strip it in plan mode — the failure mode this decision explicitly
rejects).

**Why not (a)**: Tier 4 ask for every apply click is UX friction
(no one reads a modal for a change they already saw previewed in
DiffView). Plus plan-mode LLM could never surface the apply button
if `apply_ui_diff` were a tool.

#### Decision: predefined action enum (D-Q2a/b)

**Context**: button primitive needs an action model. Three options:
(a) `action = 引用既有 LLM tool 名`(e.g. `action: "edit_file"`,
`action: "shell"`); (b) `action = 自由字符串`,前端按 string 派发;
(c) **预定义枚举**(`{"apply_diff", "copy", "dismiss"}` + 白名单)。

**Decision**: (c). (a) contradicts D-Q1 (would route user click
through LLM tool chain); (b) explodes the security surface
(arbitrary frontmatter action names reaching IPC).

**Why not free-form payload**: 个人工具杠杆不足,开放 payload 等于
"any string → any handler"的安全陷阱。`copy` / `dismiss` 是纯前端
零副作用,`apply_diff` 是已知结构(`payload.diff_text` 单一字段)
— 闭集足够。`run_command` 等命令类 action 与 shell tool 重复触发
路径,安全面陡增,**D-Q3 推到 follow-up**(产品决策非架构缺口)。

#### Decision: hand-written diff parser (D-Q5)

**Context**: unified diff apply needs a parser + applier. Three
options: (a) `diffy` crate; (b) hand-written, ~250 LOC;
(c) frontend `parsePatch` (jsdiff) → backend gets structured patch
(no parser needed).

**Decision**: (b). TECH §1.4 "零新增依赖"硬约束;算法小,24 单测
覆盖;项目自研调性一致(对照 `read_file::cat_n_format`、`llm::sse`、
`resource_loader` 手写 frontmatter parser)。Hand-rolled 还能跨测试
平台 zero-config。

**Why not (c)**: 后端 apply 仍要写行号匹配 + context 校验,
契约变大(IPC wire 变成"结构化 patch")反而更复杂。b 是契约最
小的方案(diff_text 就是 unified diff,后端纯函数即可)。

#### Decision: fail-fast across multi-file apply (D-Q6)

**Context**: 多文件 diff apply,任一 hunk 失败怎么办?
Three options: (a) 部分写 + 记录失败的(前端报告 "N/M 文件已应用");
(b) **整失败不部分写**;
(c) `git apply` 风格:试到第一个失败就停,前面已写的保留。

**Decision**: (b)。整失败让 LLM 可重试 / 用户可重新审视 diff /
避免"半应用"状态误导 UX。`tokio::fs::write` 已经原子(单文件级),
所以先收集全部 `(path, new_content)` tuples,最后才批量写 — 全成功
才 audit,任一失败整体 abort。

**Why not (a)**: 部分应用状态 = "文件 X 改了但文件 Y 没改" —
LLM 看到 tool_result `kind="io"` 会困惑"哪个文件失败了",
且磁盘状态跟 LLM 脑内 model 不一致(下次 read 会看到 N-1 个新
文件)。整失败 = 干净回滚语义。

**MVP 不做的边界**:文件创建(`oldLines=0` 暗示新增文件)、rename、
mode change、二进制 diff。这些扩 `parse_unified_diff` 时再加。

#### Decision: only success audits (D-Q6 cont.)

**Context**: apply 失败时写不写 audit?

**Decision**: 不写。`record_ui_diff_applied_audit` 仅在所有
patch 成功 + 全部 `tokio::fs::write` 成功后才调。失败路径由
前端 inline error 反馈 + console.error 兜底。

**Why**: 审计语义 = "应用 diff 行为发生了"。空失败 = 行为未发生
= 不该落审计。前端 console.error 已经记录足够上下文;若未来需要
失败可观测性,加单独的 `AuditKind::UiDiffFailed` 变体,**不**
复用 `UiDiffApplied`(成功 vs 失败是不同语义,审计查询不应混)。
