# Research: Yolo / "skip all confirmations" mode safety design

- **Query**: Yolo mode safety patterns across coding agents — Claude Code
  `--dangerously-skip-permissions` / `bypassPermissions`, Cursor "Run Everything"
  / Auto-review, Aider `--yes-always`, OpenHands `NeverConfirm` policy — and a
  concrete recommendation for the everlasting project's Yolo mode.
- **Scope**: external (primary) + internal codebase mapping (secondary)
- **Date**: 2026-06-12
- **Owner**: research agent, task `06-12-a2-b7-permission-and-mode`

## Findings (synthesized, with confidence)

Confidence levels: **[H]** corroborated by official docs and changelogs;
**[M]** one source; **[L]** inferred or unverified.

### 1. How the four reference agents implement Yolo-equivalent modes

| Agent | Activation flag / UI | Confirm dialog? | Hard kill list / what stays blocked | LLM sees the flag? | Source |
|---|---|---|---|---|---|
| **Claude Code** `bypassPermissions` | `--dangerously-skip-permissions` / `--permission-mode bypassPermissions` / `--allow-dangerously-skip-permissions` (CLI); VS Code: extension "Allow dangerously skip permissions" toggle; Desktop: enabled in settings | **Cannot enter from inside a session** without one of those enabling flags; mode in `Shift+Tab` cycle is opt-in per session; `--allow-` adds the mode to the cycle without activating it | **Yes** — `rm -rf /` and `rm -rf ~` still prompt as a circuit breaker (after v2.1.126). On Linux/macOS refuses to start in this mode when running as root or under `sudo`. `permissions.deny` and explicit `ask` rules in settings still apply (including in `bypassPermissions`) | Implied via system-context (mode appears in status bar `⏵⏵ bypass permissions on`); not explicit injection into the prompt | [H] code.claude.com permission-modes.md, permissions.md, cli-reference.md, changelog |
| **Claude Code** `auto` (the recommended modern Yolo) | `--permission-mode auto` / `CLAUDE_CODE_ENABLE_AUTO_MODE=1` on 3P providers; admin opt-in for Team/Enterprise | First-time: opt-in prompt in `Shift+Tab` cycle, "No, don't ask again" removes it; one-time notice on new-session screen in VSCode | Classifier blocks `curl \| bash`, mass deletion, force-push, sending sensitive data to external endpoints, prod deploys/migrations, granting IAM/repo perms, modifying shared infra, irreversibly destroying pre-existing files, direct push to `main` | Not stated; classifier sees user messages + tool calls + CLAUDE.md content (not tool results) | [H] auto-mode-config.md, permission-modes.md |
| **Cursor** `Run Everything` | Setting toggle in **Cursor Settings > Agents > Run Mode** | None — opt-in via UI | Sandboxing still wraps every call by default; classifier is removed in this mode (no LLM check); net = pure pass-through. Enterprise admins can disable "Enable Run Everything" entirely | N/A — model doesn't see mode | [H] cursor.com agent-security.md, terminal.md |
| **Cursor** `Auto-review` (recommended default since 3.6) | Default-on since Cursor 3.6 | First-time onboarding for Auto-review; then silent | 3-step: allowlist → sandbox → LLM classifier; explicit "best-effort, can make mistakes" | N/A | [H] terminal.md |
| **Aider** `--yes-always` | CLI flag; env `AIDER_YES_ALWAYS` | None (auto-confirms) | Aider doesn't run shell itself; LLM **suggests** shell commands and the user pastes/runs them manually. `--yes-always` auto-confirms Aider's in-chat confirmations (commit messages, code edits) but doesn't bypass the user manually running the suggested shell. `git push` is still a user action | No explicit injection | [H] aider.chat docs/config/options.html, `aider_body.txt` excerpt: "Always say yes to every confirmation" |
| **OpenHands** `NeverConfirm` | `set_confirmation_policy(NeverConfirm())` (SDK) | None (auto-allows) | Pure policy-level Yolo. Risk is **inferred from the LLM's own `security_risk` field** in the tool-call args (`LOW`/`MEDIUM`/`HIGH`/`UNKNOWN`); with `NeverConfirm`, that field is ignored. `NoOpSecurityAnalyzer` is the analyzer-level "no analysis" option | The LLM is **required** to emit a `security_risk` value (schema-enforced param on every non-read-only tool). With `NeverConfirm`, the value is recorded on the `ActionEvent` for audit but doesn't gate execution | [H] docs.openhands.dev sdk/arch/security.md, sdk/guides/security.md |

### 2. Three-tier pattern: every Yolo implementation has the same shape

All four agents implement the same conceptual **3-tier safety stack**, with the Yolo flag toggling the second and third tiers independently:

```
┌──────────────────────────────────────────────────────────────┐
│ Tier 1 — STATIC GUARDRAILS (never bypassed)                 │
│   - Tool allowlist / hard-deny rules (e.g. "Bash(rm *)")     │
│   - Path policy (cannot escape project root)                │
│   - Privilege policy (refuse to run as root)                 │
├──────────────────────────────────────────────────────────────┤
│ Tier 2 — DYNAMIC CHECKS (skip-able per-mode)                │
│   - Per-action "is this risky?" classifier or heuristic      │
│   - Per-domain first-time permission                       │
├──────────────────────────────────────────────────────────────┤
│ Tier 3 — PROMPTS (skip-able in Yolo)                         │
│   - canUseTool / AskPermission / permission:ask IPC          │
│   - "allow once / always allow / deny" UI                    │
└──────────────────────────────────────────────────────────────┘
```

Claude Code's evaluation order from [`permissions.md`](https://code.claude.com/docs/en/permissions.md) is **Hooks → Deny rules → Ask rules → Permission mode → Allow rules → canUseTool callback**:

> "A hook that returns `allow` does not skip the deny and ask rules below; those are evaluated regardless of the hook result."
> "`bypassPermissions` approves everything that reaches this step."

This means **`deny` rules + `ask` rules are evaluated *before* the mode**, so they apply even in Yolo. This is the architectural lever we want.

### 3. Specific safeguards used by the 4 agents — what survives Yolo

| Safeguard | Claude Code | Cursor | Aider | OpenHands | What it does |
|---|---|---|---|---|---|
| **Refuse root/sudo** | Yes (refuses to start) | (n/a) | (n/a) | (n/a) | Hard kill: Yolo requires non-root user; check is skipped "inside a recognized sandbox" | [H] Claude permission-modes.md |
| **Hard-coded `rm -rf /` / `~` block** | Yes (still prompts) | (sandboxed) | (LLM-only) | (LLM risk field) | Single pattern; circuit breaker against model error | [H] Claude permission-modes.md |
| **Hard-deny rules in settings** | Yes — evaluated *before* mode | (allowlist) | n/a | (LLM risk field) | `disallowed_tools=["Bash(rm *)"]` blocks even in `bypassPermissions` | [H] Claude permissions.md |
| **Ask rules in settings** | Yes — evaluated *before* mode | (per-tool allowlist) | n/a | (threshold policy) | An explicit `ask` rule forces a prompt even in `bypassPermissions` | [H] Claude permissions.md |
| **Workspace/path policy** | Yes — `assert_within_root` equivalent; "Write access restriction" | Sandbox by default | (n/a — no shell) | Sandbox container | Cannot write outside `working_directory` + `additionalDirectories` | [H] Claude security.md |
| **Network restriction** | Sandbox or default-deny for fetch/curl | Sandbox default; per-domain allowlist in `sandbox.json` | (n/a) | Container | Network calls go through firewall; SSRF blocked (we have this via `web_fetch` already) | [H] Cursor terminal.md |
| **Protected paths** | `.git`, `.vscode`, `.claude`, `~/.bashrc`, `~/.zshrc`, etc. — never auto-approved *except in `bypassPermissions`* since v2.1.126 | `File-Deletion Protection`, `Dotfile Protection`, `External-File Protection` toggles | n/a | n/a | Claude Code's `bypassPermissions` (full Yolo) **does** allow writes to these; the other modes prompt | [H] Claude permission-modes.md |
| **Subagent inherits parent mode** | Yes — `bypassPermissions` cascades | (sandboxed) | n/a | n/a | Subagents cannot escape parent's Yolo | [H] Claude permissions.md |
| **Auto-classifier with classifier-level overrides** | Yes — `auto` mode (4 tiers: `hard_deny` / `soft_deny` / `allow` / explicit user intent) | Yes — `Auto-review` (allowlist → sandbox → LLM classifier) | n/a | Yes — `LLMSecurityAnalyzer` + `ConfirmRisky` | When the per-call classifier is too lossy, prefer this over no-checks-at-all | [H] Claude auto-mode-config.md, Cursor terminal.md |
| **Audit log** | Implicit via transcript (every tool call recorded) | Implicit | Git history (auto-commits) | Explicit `ActionEvent` with `security_risk` field | All Yolo actions must be replayable for retrospective | [H] all |
| **Session-bound** | Mode is per-session | Per-workspace | Per-launch | Per-conversation | No global "Yolo" toggle | [H] all |
| **Root required → refuse** | Yes | (n/a) | (n/a) | (n/a) | Don't let an attacker escalate to root + Yolo | [H] Claude permission-modes.md |
| **Repeated-block fallback** | 3 consecutive or 20 total denials in `auto` mode → falls back to prompting | n/a | n/a | n/a | Circuit breaker against classifier-loop | [H] Claude permission-modes.md |

### 4. UX of entering Yolo (the opt-in ceremony)

This matters a lot — UX friction is the cheapest safety control.

| Agent | Steps to enter Yolo | Visual treatment |
|---|---|---|
| **Claude Code** CLI `bypassPermissions` | Must pass `--dangerously-skip-permissions` at **launch** (cannot toggle in mid-session); entering the `Shift+Tab` cycle for `bypassPermissions` is itself an opt-in prompt (you must select "Yes" to add it to the cycle; "No, don't ask again" removes it). On Linux/macOS, refuses to start with root/sudo and prints a one-line error | Status bar `⏵⏵ bypass permissions on`; no special color, but the status line is a constant text strip |
| **Claude Code** CLI `auto` | First time you cycle to `auto`, opt-in prompt appears: "auto mode" + Accept/Reject. "No, don't ask again" suppresses future opt-in prompts. Enterprise admin can lock off via `permissions.disableAutoMode` | "Auto mode" status bar text |
| **Cursor** `Run Everything` | Setting toggle in **Cursor Settings > Agents > Run Mode** — must explicitly scroll/click to reach; enterprise admins can disable "Enable Run Everything" entirely (no UI at all for end users) | Mode label in run mode dropdown |
| **Aider** `--yes-always` | CLI flag at launch; no in-app confirm | n/a |
| **OpenHands** `NeverConfirm` | SDK constructor: `set_confirmation_policy(NeverConfirm())` — no UI; programmatic only | n/a |

**Pattern across all four**: opt-in is **explicit and at-launch** (Claude Code CLI, Aider), or **explicit and at-settings-change** (Cursor, VSCode). Nobody offers a one-click "Yolo now" button mid-session without an extra confirm. The cheapest UX is "type YOLO" or "Yes, I know what I'm doing" type confirmation.

### 5. Mapping to our repo

| Our existing asset | Maps to | Reuse for Yolo? |
|---|---|---|
| `projects::boundary::assert_within_root` (called in `chat.rs:279` and inside each tool) | Claude Code "Write access restriction" | **YES — keep, this is our Tier 1 hard guard** |
| `tracing::warn!` in tool execution | Claude Code transcript-level audit (we don't have a structured audit log yet) | **PARTIAL — need to elevate to a Yolo-specific tracing target with structured fields; C4 audit task will consume** |
| Session persistence (`db::Session` model) | Per-session mode binding | **YES — extend with `mode: Mode` field; `defaultMode` in user settings** |
| 4-file memory system (B5) | Claude Code "boundaries you state in conversation" + `autoMode` config | **YES — `user/project CLAUDE.md` can carry custom Yolo allow/deny rules; we already load these into the prompt, so a "deny list" in CLAUDE.md is already LLM-visible** |
| `web_fetch` SSRF blocklist | Tier 1 network policy | **YES — keep; web_fetch already refuses private IPs** |
| `tool-contract.md:463` mentions "Domain permissions gate (Claude Code-style first-time per host)" as out-of-scope | Tier 3 first-time permission UX | **OUT-OF-SCOPE — same as web_fetch, defer to A2 follow-up** |
| `ChatInput.vue` + `AppHeader.vue` are the UI entry points mentioned in the task | Mode switcher UI | **YES — Mode selector goes here** |

### 6. Concrete recommendation for everlasting's Yolo safety pattern (MVP)

Of the 6 candidate safeguards in the task, include these 4 in MVP:

| # | Safeguard | Why include | Why not include | Effort |
|---|---|---|---|---|
| 1 | **Hard kill list** — `rm -rf /`, `rm -rf ~`, `rm -rf $HOME`, plus a tiny list of catastrophic system commands (`mkfs`, `dd if=`, `chmod -R 777 /`, `:(){:\|:&};:`) | Cheap; mirrors Claude Code's circuit breaker; survives model error | — | **LOW** (static rule file in `tools/shell.rs` or `tools/mod.rs`) |
| 2 | **Audit log mandatory** — every tool call in Yolo mode emits `tracing::warn!` with `{session_id, tool, args, mode, decision}` to a dedicated `yolo_audit` target; C4 task consumes the hook | Cheap; required for retrospective; C4 already needs the hook anyway | — | **LOW** (tracing subscriber layer) |
| 3 | **Yolo is per-session only, not global** — Mode is bound to `Session.mode`; user can have a normal session open while a Yolo session runs | Cheap; matches Claude Code's "cannot enter from inside session" pattern; prevents accidental Yolo cascading across tabs | — | **LOW** (one DB column + one Tauri command) |
| 4 | **Refuse root/sudo at startup** — if `geteuid() == 0` and mode == Yolo, refuse with one-line error | Mirrors Claude Code; cheap; defense-in-depth | — | **LOW** (one `#[cfg(target_os = "linux")]` check) |
| ✗ | **Yolo only in dedicated worktree** | High value, high effort | Requires A2 ⑨ 关 + worktree integration that doesn't exist yet; defer to a follow-up after MVP ships | **MEDIUM** |
| ✗ | **Rate limit in Yolo** | Marginal value; can do later | Adds complexity for unclear benefit; user can already `Stop` (C1) | **MEDIUM** |
| ✗ | **Read-only Yolo (Cursor-style)** | Different product — not what the task asks for | Task PRD already has Plan/Review modes for read-only; Yolo is for "do everything" | n/a |

**Layer integration order** (mirrors Claude Code's evaluation order):

```
1. Tool static allowlist (always run; tools/mod.rs builtin_tools())
2. Hard kill list (new — apply to shell.rs only, matches against command prefix)
3. Path policy (existing — boundary::assert_within_root)
4. Schema validation (existing — tool_contract)
5. Mode check (new — ⑧a Plan/Review; Yolo skips ahead)
6. Permission policy (new — ⑨ check)
   - 6a. deny rules → block (overrides Yolo)
   - 6b. ask rules  → permission:ask IPC (overrides Yolo)
   - 6c. mode == Yolo → allow
   - 6d. first-time tool → permission:ask
   - 6e. else → allow
7. Audit hook (new — emit tracing::warn regardless of decision in Yolo mode)
```

### 7. UX recommendation for entering Yolo

**Two-click with destructive framing**, not one-click and not "type YOLO":

```
┌─ Mode ─────────────────────────────────────────┐
│  Current: Chat                                │
│                                                │
│  Switch to: ○ Chat   ○ Plan   ○ Review         │
│             ◉ Yolo   ○ Background              │
│                                                │
│  ⚠ Yolo mode: ALL permission prompts are      │
│  skipped. Destructive actions (rm -rf,         │
│  git push --force, dropping tables) run        │
│  without confirmation. You can abort with      │
│  Stop (Esc) at any time.                       │
│                                                │
│  A small set of catastrophic commands          │
│  (rm -rf /, mkfs, dd to disk) is hard-blocked. │
│                                                │
│  All Yolo actions are logged for review.       │
│                                                │
│  [Cancel]            [I understand, enable]    │
└────────────────────────────────────────────────┘
```

Why this shape:
- **One-click is wrong**: A misclick on a Mode dropdown shouldn't be Yolo. One-click = Chat ↔ Plan is fine; Yolo is destructive enough to need a confirm.
- **"Type YOLO" is wrong**: We have a 4-mode set; Yolo is one of them. Forcing a 4-letter typing ceremony is gratuitous friction for a *deliberate* mode change. Claude Code's own model is "opt-in once in `Shift+Tab` cycle" — we can use the same pattern: a "I understand, enable" button.
- **Modal is correct** (not inline): the modal forces the user to read the warning. Inline confirmations are easy to dismiss without reading.
- **No "Don't ask again" in MVP**: the cost of one extra click per Yolo entry is acceptable. After A2 ships we can add a per-session "trust this Yolo session" option that skips the modal within the session.

**Where it lives**: `ChatInput.vue` (or `AppHeader.vue`) gets a Mode badge. Click → popover with the 5 modes. Selecting "Yolo" → modal above. "I understand" → calls `set_mode` Tauri command → backend persists `Session.mode = Yolo` → next tool call goes through the new ⑨ 关 with `mode == Yolo` decision.

### 8. Concrete spec sections that need updates

When this research is consumed by `update-spec`:

- `app/src-tauri/src/commands/sessions.rs` — add `set_mode` Tauri command
- `app/src-tauri/src/db/models.rs` — add `mode: Mode` to `Session` model; add `default_mode: Mode` to a new or existing `AppConfig` row
- `app/src-tauri/src/agent/chat.rs:971` — wrap `execute_tool` with the new ⑨ 关 dispatch (right after `Mode` check at ⑧a, before `execute_tool`)
- `app/src-tauri/src/tools/mod.rs` — add a `HardKillList` matcher applied to `shell` tool input
- `app/src-tauri/src/llm/types.rs` — add `Mode { Chat | Plan | Review | Background | Yolo }` enum
- `app/src/components/chat/ChatInput.vue` (or new `ModeSwitcher.vue`) — Mode badge + popover + Yolo modal
- `.trellis/spec/backend/tool-contract.md` — document the ⑨ 关 evaluation order and Yolo semantics
- `.trellis/spec/backend/llm-contract.md` — document Mode checks in ⑧a
- `docs/ARCHITECTURE.md` §2.5 — fill in the audit hook spec (currently planned but unimplemented)

## Caveats / Not Found

- **No source for "type YOLO" or similar 4-letter typing pattern** in mainstream agents. Claude Code, Cursor, Aider, OpenHands all use modal/flag opt-in. The task's "type YOLO" option appears to be a folk pattern; the modal-button pattern is more standard.
- **No source for "rate limit in Yolo"** in any of the 4 agents. Cursor's Auto-review has a *denial-count fallback* (3 consecutive or 20 total → fall back to prompting) which is the inverse: a circuit breaker on the classifier, not a rate limit on user actions. We should not invent a rate limit.
- **No source for "Yolo only in dedicated worktree"** as a hard rule in any of the 4 agents. Claude Code's `bypassPermissions` is orthogonal to worktrees — you can have Yolo without a worktree, and you can have a worktree without Yolo. The worktree feature is for parallel-session isolation, not for Yolo gating. We can still implement this as an *additional* safeguard if we want, but it's not prior art.
- **Aider `--yes-always`** is technically not a "Yolo for shell" — Aider's model is LLM-suggests, user-pastes-shell. So `--yes-always` skips Aider's own confirmations (commit messages, apply file edits) but the user always has a chance to refuse a pasted shell command. This is structurally different from Claude Code's `bypassPermissions` (where Claude runs the shell directly). Our project is closer to Claude Code's model, so use Claude Code as the primary reference, not Aider.
- **Cursor's "Run Everything"** is the only mode that disables both the sandbox and the classifier. Other modes (Auto-review, Allowlist) keep one or both. This is the closest UX analog to "raw Yolo" but is enterprise-controllable (admins can disable the toggle entirely).
- **The Claude Code changelog** (entry above) mentions: "Fixed subagents in background sessions bypassing the worktree-isolation guard" — this is a bug, not a feature, but it illustrates that **Yolo-cascading-to-subagents is a real footgun** in real-world deployments. The permission-modes.md doc explicitly warns about it. Our `Mode` check at ⑧a must be inherited by subagents (which we don't have yet, but the rule is: if we ever add subagents, the parent's mode is sticky).

## External References (cited)

- Claude Code permission modes: https://code.claude.com/docs/en/permission-modes.md (fetched 2026-06-12, ~13.5 KB Markdown)
- Claude Code permissions: https://code.claude.com/docs/en/permissions.md (~38 KB) — evaluation order
- Claude Code auto-mode config: https://code.claude.com/docs/en/auto-mode-config.md (~13.5 KB) — 4-tier classifier rules
- Claude Code security: https://code.claude.com/docs/en/security.md (~10.7 KB)
- Claude Code CLI reference: https://docs.claude.com/en/docs/claude-code/cli-reference (extracted `--dangerously-skip-permissions`, `--permission-mode` modes)
- Claude Code changelog (2.1.83+): https://code.claude.com/docs/en/changelog.md — auto-mode history, v2.1.126 protected-paths change
- Claude Code worktrees: https://code.claude.com/docs/en/worktrees.md — `--worktree` flag (orthogonal to Yolo)
- Cursor agent security: https://cursor.com/docs/agent/security.md (4 KB) — Run Mode overview
- Cursor terminal tool: https://cursor.com/docs/agent/tools/terminal.md (18.5 KB) — Run Mode table, Auto-review flow
- Cursor llms.txt: https://cursor.com/llms.txt (index)
- Aider options reference: https://aider.chat/docs/config/options.html — `--yes-always` (Always say yes to every confirmation)
- Aider chat modes: https://aider.chat/docs/usage/modes.html — code/ask/architect/help (orthogonal to Yolo)
- OpenHands security arch: https://docs.openhands.dev/sdk/arch/security.md (14 KB) — 4 risk levels, 3 policies
- OpenHands security guide: https://docs.openhands.dev/sdk/guides/security.md (36 KB) — `AlwaysConfirm` / `NeverConfirm` / `ConfirmRisky` policies
- OpenHands llms.txt: https://docs.openhands.dev/llms.txt (index)

## Internal References (this repo)

- `app/src-tauri/src/agent/chat.rs:279` — existing `boundary::assert_within_root` call (Tier 1 guard)
- `app/src-tauri/src/agent/chat.rs:971` — `execute_tool(...)` call site, **where ⑨ 关 must wrap** (per PRD §"What I already know")
- `app/src-tauri/src/tools/web_fetch.rs` — existing Tier 1 SSRF block (precedent for hard kill list)
- `app/src-tauri/src/tools/mod.rs` — `builtin_tools()` + `execute_tool()` dispatch (where audit hook attaches)
- `app/src/components/chat/ChatInput.vue` + `app/src/components/layout/AppHeader.vue` — UI entry points for Mode switcher
- `.trellis/spec/backend/tool-contract.md:463` — prior art: "Domain permissions gate (Claude Code-style first-time per host)" out-of-scope for web_fetch
- `docs/ARCHITECTURE.md §2.5` — Mode + permission design (planned, not yet implemented); §2.5.8 — audit hook
- `docs/BACKLOG.md §4.2` — original 5-mode design (Chat/Plan/Review/Background/Yolo)
- `docs/IMPLEMENTATION.md §1` — self-built agent core rationale (don't reuse SDK permission machinery)
- PRD: `.trellis/tasks/06-12-a2-b7-permission-and-mode/prd.md` — assumption 4: "Yolo 默认关 + 进入 Yolo 二次确认 + Yolo 操作进审计 三件套" (corroborates our recommendation)
