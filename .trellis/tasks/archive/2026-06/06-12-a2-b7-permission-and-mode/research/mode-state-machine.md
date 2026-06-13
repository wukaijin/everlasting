# Research: Mode State Machine for Coding Agents

- **Query**: 调研 coding agent (Claude Code / Aider / OpenHands / Cursor / Continue.dev) 的 mode 状态机设计 — 持久化粒度、模式切换规则、与 tool 白名单的交互、UX 入口模式
- **Scope**: mixed (internal code patterns + external references)
- **Date**: 2026-06-12

## Internal Findings

### 1. Existing session-bound per-row pattern (template for `Mode`)

**Repository already has a near-perfect template:** per-session `model_id` override.

| Aspect | Existing implementation | Mapping to `Mode` |
|---|---|---|
| DB column | `sessions.model_id TEXT` (nullable soft FK to `models.id`) | `sessions.mode TEXT` (nullable, default = 'chat') |
| Migration | `add_session_column_if_missing(pool, "model_id", "TEXT")` in `db/migrations.rs:290` | Mirror with `add_session_column_if_missing(pool, "mode", "TEXT")` |
| Index | `CREATE INDEX IF NOT EXISTS idx_sessions_model_id ON sessions(model_id)` (`db/migrations.rs:293-294`) | Optional — mode is low-cardinality (5 values), index usually unnecessary |
| Backfill | `seed` function backfills `model_id` on legacy rows from `default_model_id` (`db/tests.rs:775`) | Mode backfill trivial — `UPDATE sessions SET mode = 'chat' WHERE mode IS NULL` |
| Model | `SessionRow.model_id: Option<String>` (`db/models.rs:197`, `db/sessions.rs:88,128`) | `SessionRow.mode: Option<String>` (or `Mode` enum serialized as string) |
| Update fn | `update_session_model_id(pool, session_id, model_id)` (`db/sessions.rs:304-329`) | `update_session_mode(pool, session_id, mode)` — identical pattern |
| Frontend model | `SessionSummary.model_id: string \| null` (`app/src/stores/chat.ts:180`, `app/src/stores/streamController.ts:235`) | `SessionSummary.mode: 'chat' \| 'plan' \| 'review' \| 'background' \| 'yolo' \| null` |
| Frontend UI | `ModelSelect.vue` (worktree-style popover) | New `ModeSelect.vue` — same popover pattern |
| Tauri command | `update_session_model_id(sessionId, modelId)` (Tauri command) | `set_session_mode(sessionId, mode)` — same shape |
| Fallback resolution | `currentModelId` falls back to `modelsStore.defaultModelId` when `s.model_id` is null (`ModelSelect.vue:44-51`) | `currentMode` falls back to `'chat'` (the default) when `s.mode` is null — no global override needed for v1 |
| Lock during streaming | `:disabled="isStreaming"` (`ModelSelect.vue:155`) | `:disabled="isStreaming"` — same constraint, mid-stream mode change is unsafe (see Q2 below) |

**Key files reviewed:**
- `/usr/local/code/github/everlasting/app/src-tauri/src/db/sessions.rs` (lines 25-76 create, 280-329 update_model_id)
- `/usr/local/code/github/everlasting/app/src-tauri/src/db/migrations.rs` (lines 285-294 migration pattern)
- `/usr/local/code/github/everlasting/app/src-tauri/src/db/models.rs:197` (SessionRow comment)
- `/usr/local/code/github/everlasting/app/src/stores/chat.ts:180` (TS model type)
- `/usr/local/code/github/everlasting/app/src/stores/streamController.ts:220-235` (stream state per session)
- `/usr/local/code/github/everlasting/app/src/components/chat/ModelSelect.vue` (full popover template)
- `/usr/local/code/github/everlasting/docs/ARCHITECTURE.md:332-354` (8a Mode check + ⑨ permission check)

### 2. Per-session vs global vs per-project — what our codebase already commits to

- **`model_id` chose Option A (per-session binding) with global default fallback** — and this is the right template. Reasons documented in the existing code:
  - "frontend's per-session model dropdown (StatusBar) so the user can switch models without changing the global default" (`db/sessions.rs:295-298`)
  - Soft FK with `NULL` = "use global default" (`db/sessions.rs:311-315`)
- **Mode is even more session-scoped than model**: you don't want a "Plan mode" leaking from a review session into a coding session. Per-session binding is the strict-safe default.
- **No per-project binding exists** for any session attribute — adding a third granularity (per-project) would be net-new and unjustified.

### 3. Existing UX surface candidates (mirrored from `model_id`)

| Location | Existing pattern | Pros for Mode | Cons for Mode |
|---|---|---|---|
| `app/src/components/chat/ModelSelect.vue` (chat input area) | Popover anchored to bottom of chat panel | Mode is intrinsically per-session — colocated with model picker is natural | Crowded if also used for model + worktree + project |
| `app/src/components/settings/SettingsModal.vue` (LLM Tab) | Centralized catalog management | LLM Tab is the natural home (per prd.md requirement #B7 line 1) | Modal is for config, not ephemeral state — Mode is more ephemeral than model |
| `app/src/components/layout/AppHeader.vue` (top bar) | Streaming badge via `streamController.streamingProjectIds` (line 38) | Visible at a glance, always-on indicator | No precedent for state-changing controls in header |
| Slash command (new surface) | Not implemented — but `/model`, `/mode`, `/permissions` already listed in BACKLOG §1.3 line 134 | Matches Claude Code convention | Discoverability lower than UI control |

The PRD already states the tension: "B7 UI 入口: Settings modal LLM Tab 加 Mode 选择器(per-session override), 或顶栏 quick switcher(全局临时)— 倾向前者,跟 model_id 模式一致" (prd.md line 45).

## External Findings

### Q1. Persistence granularity — what's the norm

| Product | Persistence | Rationale |
|---|---|---|
| **Claude Code** | **Per-session / per-conversation**, with global default at user level. `/mode` persists for the conversation, and there's a settings.json `defaultMode` for new sessions. | Mode is a per-conversation constraint, not a global preference — the same user can have a "plan" session and a "yolo" session open at once. |
| **Cursor** | **Per-workspace** — bound to the editor workspace (which is roughly per-project), with a dropdown in the chat input that defaults to the workspace's last value. | Cursor treats mode as a workspace-level safety stance ("are we trusting this codebase?") rather than a per-message toggle. |
| **Aider** | **Per-invocation** (CLI flag `--no-auto-commits`, `--auto-lint`, etc.) — not really a "mode" concept, more a set of independent flags. | Aider leans on command-line composition rather than a state machine. |
| **Continue.dev** | **Per-tab** (a tab ≈ a session); top-bar picker with "Chat", "Edit", "Agent", "Plan" modes. | Maps to a UI tab, which is effectively a session. |
| **OpenHands** | **Per-conversation**, with a `Runtime` config that includes a security `ConfirmationPolicy` (`ALWAYS`, `NEVER`, `RISKY_ONLY`). | Confirmation policy is per-conversation; runtime/security stance is the same shape as our permission system. |
| **GitHub Copilot Workspace** | **Per-task** (one task ≈ one mode instance); no explicit user-set mode, but each task has a permission profile. | Mode is implicit in the task framing. |

**Verdict for Everlasting:** **Option A (per-session binding)** matches the dominant norm (Claude Code / Continue.dev / OpenHands all use per-conversation). It also has the cleanest implementation in our codebase because `model_id` already paved the path.

**Why not Option B (global)?** Mode is a *safety stance*, not a preference. A user who wants to "review a PR diff" in one tab should not be forced to re-set mode after closing the tab. Global would make the user repeat the action on every session open.

**Why not Option C (per-project)?** Cursor uses this, but Cursor's unit of work is a workspace, not a chat. In Everlasting, projects are the parent of sessions, and the natural unit for "I'm thinking vs I'm doing" is the session. Per-project would be a coarser grain that surprises users ("why is this session in Plan mode? I never set that.").

### Q2. Mode transition rules — when can mode change?

| Product | Mid-stream switch? | Why? |
|---|---|---|
| **Claude Code** | **No** — `/mode` only takes effect on the *next* turn. If you type `/mode plan` mid-stream, the current stream finishes in whatever mode it started; the next request runs in plan. | Mode changes the system prompt; mid-stream switch would mean LLM is "thinking with a different brain" mid-response. |
| **Cursor** | **No** — dropdown is disabled while a request is in flight. | Same reason; also avoids race in the chat input. |
| **Continue.dev** | **No** — picker is disabled during streaming. | Same reason. |
| **Aider** | N/A — mode is per-CLI-invocation. | — |
| **OpenHands** | **No** — `ConfirmationPolicy` is read at runtime start of each action; changes apply to subsequent actions, not the in-flight one. | Mode is a per-action attribute, not per-token. |

**Verdict for Everlasting:**
- **Block mid-stream mode change** (mirror `ModelSelect.vue:155` `:disabled="isStreaming"`)
- **Apply on next turn boundary** — this is the natural place because the next turn re-constructs the system prompt and tool list.
- **Implementation note:** the `chat` command in `app/src-tauri/src/agent/chat.rs` builds the outbound messages + tools at the start of each turn. Reading `session.mode` at the top of each turn and applying it to ⑧a (Mode check) and ⑨ (permission check) gives us "next turn" semantics for free — no need to abort an in-flight stream.

**Special case — Background mode:** switching *to* Background mid-stream is arguably safe (it just changes notification semantics, not behavior). Switching *from* Background to Chat mid-stream is the risky one. The safest rule is the same — disable during streaming for all transitions, treat Background as a notification concern, not a behavioral concern.

### Q3. Mode × tool whitelist interaction

| Product | How is the LLM told? | Tool list strategy |
|---|---|---|
| **Claude Code `/plan`** | (a) System prompt says "you are in plan mode — you cannot make edits, you can only analyze and propose"; (b) `Edit` / `Write` / `Bash` tools are **removed** from the schema entirely; (c) `Read` / `Grep` / `Glob` remain. | Remove write tools + add an implicit "no-op" reminder. LLM is told via the system prompt *and* the tool absence. |
| **Cursor "Plan"** | (a) System prompt is rewritten with planning instructions; (b) `edit` / `apply` tools are not registered; (c) `read` / `search` remain. | Same approach. |
| **Continue.dev "Plan"** | (a) Mode system prompt is prepended; (b) tool list is filtered to read-only. | Same. |
| **OpenHands `ConfirmationPolicy: NEVER`** | Tools stay registered; the runtime always approves them. The LLM is not told about the policy — the runtime handles it. | Different model — no prompt change, runtime blanket-approves. |
| **Aider `--read-only`** | Edit tools are removed; the LLM can only read. | Pure tool-list filtering. |

**Two competing designs:**

**Design A — Tool-list filtering only (Aider / OpenHands):**
- `Plan` mode → tool list is empty (or only `read_file` etc.)
- `Yolo` mode → tool list is full, runtime skips permission gate
- **Pro:** LLM self-discovers constraints (it tries `write_file`, gets a "tool not available" error, learns).
- **Con:** LLM may still call `write_file` and waste a turn on a tool_use that's structurally forbidden. The first attempt is wasted; for expensive models this matters.

**Design B — System prompt + tool-list filtering (Claude Code / Cursor / Continue.dev):**
- `Plan` mode → system prompt says "you are in plan mode, do not attempt to make edits; you can only analyze and propose a plan"
- `Plan` mode → tool list filters out write tools
- **Pro:** LLM never attempts the forbidden action. Better turn economy.
- **Con:** Two sources of truth (prompt + tool list) can drift.

**Verdict for Everlasting:** **Adopt Design B** (Claude Code's approach). Reasons specific to our codebase:
1. Our `agent/chat.rs:971` has the architecture for ⑧a Mode check that explicitly says "拒绝所有 tool_use,改返回 text '我不能执行,只能分析'" (`docs/ARCHITECTURE.md:338`). This is a *runtime intercept*, not a tool-list absence. Implementing it via tool-list filtering would lose this behavior.
2. The ⑨ 关 permission check is already designed as runtime decision logic (5 gates: whitelist / schema / path / confirm / danger — `docs/ARCHITECTURE.md:359-371`). Removing tools from the schema would short-circuit ⑨, not just ⑧a. We want ⑨ to run for safety-relevant decisions even in plan mode (e.g., "this read_file path is outside the worktree" — still wants a check).
3. The system prompt is built per turn in our pipeline (5b in ARCHITECTURE.md §2.2). Adding a per-mode prefix to the prompt is one line.
4. **Hybrid recommendation:** in ⑧a intercept *and* in tool list, the LLM gets a coherent experience:
   - Tool list: filter out tools that are 100% forbidden in this mode (e.g., `write_file` in Plan)
   - System prompt: explain the mode and what the LLM should/shouldn't do
   - ⑧a runtime: catch any tool_use that slips through (LLMs sometimes ignore instructions) and return a polite "this mode doesn't allow X" tool_result

**Specific mode-by-mode interaction:**

| Mode | Tool list filter | System prompt | ⑧a runtime |
|---|---|---|---|
| **Chat** | full | none (or default) | full ⑨ |
| **Plan** | `write_file` / `edit_file` / `shell` removed | "you are in plan mode, propose a plan, do not execute" | any `tool_use` → text-only response "I cannot execute in plan mode" |
| **Review** | `write_file` / `edit_file` / `shell` removed | "you are in review mode, read-only analysis" | same as Plan, but allow read tools |
| **Background** | full (same as Chat) | none (or "this is a background run, be concise") | full ⑨ (same as Chat) |
| **Yolo** | full | "you are in yolo mode, no confirmations will be asked" | full ⑨ minus "ask user" gate |

### Q4. UX surface for mode switching

| Product | Surface | Discoverability | Safety |
|---|---|---|---|
| **Claude Code** | Slash command `/mode plan\|chat\|review\|auto` (also in `.claude/settings.json` `defaultMode`) | Medium — requires knowing slash commands. Once known, fast. | Safe — explicit text command, no accidental click. |
| **Cursor** | Dropdown in chat input footer (next to model picker) | High — always visible. | Medium — single click can switch safety stance, but the dropdown needs a confirm for risky modes. |
| **Continue.dev** | Top-bar picker (in the editor chrome) | High — persistent, glanceable. | Medium — single click. |
| **OpenHands** | Settings modal + runtime CLI flag | Low — buried in settings. | High — explicit navigation required. |
| **Aider** | CLI flag at invocation | High for power users, low for casual. | High — explicit per-invocation. |

**Three viable options for Everlasting:**

**Option 1 — Settings modal LLM Tab (per-session override dropdown)**
- Mirrors `ModelSelect.vue` exactly — same popover, same `update_session_*` IPC, same per-session binding.
- **Discoverability:** Medium (user has to open settings to find it).
- **Safety:** High — explicit navigation; not in the fast-path.
- **Match for our codebase:** **Highest** — direct template from model_id work.
- **Risk:** Mode is a frequently-toggled control (you switch into Plan, do a plan, switch back to Chat to execute). A modal hop is friction.

**Option 2 — Top-bar / ChatInput quick switcher (per-session override)**
- New component `ModeSelect.vue` placed adjacent to `ModelSelect.vue` in `ChatInput.vue` (or in the top bar via `AppHeader.vue`).
- **Discoverability:** High — always visible at the chat input footer (where users are about to send a message).
- **Safety:** Medium — single click can change mode. Mitigate by:
  - `:disabled` during streaming (same as model)
  - For Yolo specifically, show a confirmation modal: "Yolo skips all confirmations. Are you sure?"
- **Match for our codebase:** High — the trigger is the same popover as `ModelSelect.vue`; only the IPC and store property change.
- **Risk:** Crowded chat input footer (already has worktree + model + project). But Mode is a *safety* control that should be glanceable, so this is actually a feature.

**Option 3 — Slash command (per-session)**
- `/mode plan|chat|review|background|yolo` and `/mode` (show current).
- **Discoverability:** Medium-low — user has to know the command. Add to `/help`.
- **Safety:** High — explicit text, not a misclick.
- **Match for our codebase:** Medium — slash command infrastructure is mentioned in BACKLOG §1.3 line 134 but not yet implemented. Would be additive work.
- **Best use case:** power users; complements the UI by giving a keyboard fast path.

**Verdict for Everlasting (recommended combo):**
- **Primary: Option 2 (ChatInput quick switcher)** — discoverability matches safety-importance. Mode is something you want to *see* before each message, not something you set once and forget.
- **Complement: Option 1 (Settings modal)** for users who want a quieter UX or are reviewing their session defaults.
- **Defer: Option 3 (slash command)** — slash command infrastructure is a separate piece of work; not a blocker for A2+B7.

## Recommendation for our repo

### Persistence — Option A (per-session binding)

**Implementation template:** copy the `model_id` pattern verbatim.

1. **Migration** (`db/migrations.rs:285-294` template):
   ```rust
   add_session_column_if_missing(pool, "mode", "TEXT").await?;
   // Backfill: any existing row defaults to 'chat'
   sqlx::query("UPDATE sessions SET mode = 'chat' WHERE mode IS NULL")
       .execute(pool).await?;
   ```
2. **SessionRow** (`db/models.rs:197` template): add `mode: Option<String>`. Wire into `create_session` and `update_session_*`.
3. **Update fn** (`db/sessions.rs:304-329` template): `update_session_mode(pool, session_id, mode)`.
4. **TS type** (`app/src/stores/chat.ts:180` template): add `mode: SessionMode | null` to `SessionSummary`.
5. **Stream controller** (`app/src/stores/streamController.ts:220-235` template): add `mode: SessionMode | null` to the session state struct.
6. **Tauri command** (mirror `update_session_model_id`): `set_session_mode(sessionId, mode)`.
7. **Resolution at runtime:** in the `chat` command's pre-turn setup (where `resolve_chat_provider` is called for model), read `session.mode` and apply it to ⑧a + ⑨ in the same turn.

**Why this is the right call:** Per-session matches Claude Code / Continue.dev / OpenHands norms; per-session is already the architectural choice for `model_id` (lowest-friction template); per-session is the safest grain for a *safety* attribute (you don't accidentally inherit Yolo from a previous session).

### UI placement — 3 options (ranked)

1. **ChatInput quick switcher** (`ModeSelect.vue` next to `ModelSelect.vue`) — recommended primary. Discoverability matches the safety-criticality of mode. Same popover code; `:disabled` during streaming; Yolo gets a confirmation modal.
2. **Settings modal LLM Tab** — secondary location, for users who prefer quieter UX or want to review defaults. Mirrors where `model_id` could have lived (it didn't, but the catalog *management* lives there).
3. **Slash command `/mode`** — defer until slash command infrastructure lands (BACKLOG §1.3). Not a blocker for A2+B7.

## Caveats / Open

- **Mode in audit log:** PRD says ⑨ 关 audit is C4's job (prd.md line 96). A2 only emits the hook. Mode changes themselves should be in the audit log — the question is "is the mode-change event part of A2 or C4?" Recommend: A2 emits `mode:change` events on the audit stream; C4 persists them.
- **"background:" prefix in `streamController`:** PRD §4.2 line 487 says "Background mode emits `background:` prefix, frontend doesn't prompt strongly". The current `streamController` already has a notion of "background" — verify the existing implementation aligns with the proposed Mode=Background semantics before doing a separate pass.
- **Yolo default-off safety:** Claude Code's `bypassPermissions` (a.k.a. Yolo) requires an interactive "yes I understand" + a settings.json opt-in. A2+B7 should mirror this — never auto-enter Yolo.
- **No external research done on the OSS implementer view** (would benefit from a follow-up spike to read `claude-code` source, `openhands/runtime`, `aider/repomap` for the actual code patterns). The current recommendations are based on documented user-facing behavior.
- **Per-project Mode is not in any major product** — recommendation to skip Option C is well-supported but worth re-confirming if a user explicitly asks for "this project is always Plan mode".

## Related Specs

- `/usr/local/code/github/everlasting/.trellis/spec/backend/llm-contract.md` — needs Mode field added to chat request/response shape
- `/usr/local/code/github/everlasting/.trellis/spec/backend/tool-contract.md` — needs ⑨ 关 5-gate decision contract documented
- `/usr/local/code/github/everlasting/.trellis/spec/frontend/state-management.md` — needs `mode` added to `SessionSummary` and the stream controller's per-session state
- `/usr/local/code/github/everlasting/.trellis/spec/frontend/popover-pattern.md` — direct template for `ModeSelect.vue`
- `/usr/local/code/github/everlasting/docs/ARCHITECTURE.md:332-371` — ⑧a Mode check + ⑨ permission check design
- `/usr/local/code/github/everlasting/docs/BACKLOG.md:480-535` — §4.2 Mode table + §4.3 orchestration stub
- `/usr/local/code/github/everlasting/.trellis/tasks/06-12-a2-b7-permission-and-mode/prd.md` — the task spec this research feeds into
