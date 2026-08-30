# Browser Interaction Regression (Playwright)

> The browser layer for regressions vitest + jsdom structurally cannot cover:
> real keyboard/pointer trusted input, cross-component scroll/focus linkage,
> modals stacked over modals. Lives in `app/e2e/*.spec.ts`, driven by
> `@playwright/test` against the vite dev server. Established by
> `.trellis/tasks/08-30-rule-test-001-browser-pipeline` (PRD + design are the
> authoritative decision records); this file captures the operating rules.
> Run instructions live in `docs/HACKING-wsl.md`; fixture API + default mock
> table + testid registry live in `app/e2e/README.md`.

---

## 1. Which layer guards what (ask in this order)

| Layer | Guards | Examples |
|---|---|---|
| vitest (jsdom, `src/**/*.test.ts`) | Default layer: component props/events, store state machines, pure functions, transport units | ~1466 tests |
| **browser spec (`app/e2e/*.spec.ts`)** | Categories jsdom has *provably failed* at: real keyboard/pointer trusted events, cross-component scroll + store linkage, outside-click / modal stacking, contenteditable default browser behavior | 3 pilot specs: CH5-1 Shift+Enter, CH8-2 question-card scroll, CH7-4 revoke confirm |
| Rust `--lib` | Backend logic | ~2000 tests |
| Rust `--test e2e` | Wire contracts (routes / serialization / SSE semantics) — see spec `daemon-server.md` | RULE-TEST-003 baseline |
| `turn-smoke.sh` | One real-LLM turn through the live daemon | after agent-loop changes |
| `ui-review.sh` | Visual appearance (VLM review) — **not** interaction; static screenshots can't see hit areas, hover, animation, state inside collapsed groups (AGENTS.md method note) | after style changes |

**Decision order for a new interaction regression**: (1) Can a pure function or
store-level test express it? → vitest. (2) Is it about the *shape* of a request
or event payload? → Rust `--test e2e` (the wire owner). (3) Does it need a
*trusted* keyboard/pointer event, real scrolling, or stacked overlays to
behave? → browser spec. (4) Is it "does it look right"? → ui-review, never a
browser spec. (5) Does it need a real LLM? → turn-smoke, and it is by
definition not CI-deterministic.

---

## 2. Harness contract (`app/e2e/fixtures.ts`)

| Fixture | Semantics |
|---|---|
| `mockCmd(domain, cmd, payload)` | Registers `POST /api/v1/{domain}/{cmd}` → `payload` verbatim. Registry key is `{domain}/{cmd}`; later registration wins over the built-in boot defaults. |
| `mockHealth(health)` | Overrides `GET /api/v1/health` (default `{daemonId, daemonVersion:"0.1.0", apiVersions:["v1"]}`). |
| `stream.emit(name, payload)` | Fake EventSource dispatch — constructs a real `MessageEvent` per named listener. |
| `reqs` / `waitForCmd(domain, cmd)` | Every intercepted `/api/v1/**` request (including misses and preflights) is recorded *before* dispatch; `waitForCmd` polls the record. Request-body assertions go through `reqs`/`waitForCmd`, never through page spies. |
| `boot(path = "/")` | `goto` + wait for app mount (editor host attached). Route + initScript are installed in the `world` fixture setup, i.e. always before any `goto`. |

**Boot order is structural, not advisory**: the health handshake runs in
`main.ts` *before* `app.mount`, and the SSE listeners create the EventSource on
first `listen` (`transport/http.ts` lazy-singleton). Routes and the
`addInitScript` EventSource replacement must therefore exist before the first
navigation — the `world` auto-fixture guarantees this; don't register routes
after `boot()`.

**The catch-all dispatcher with fail-loud is mandatory**: vite dev proxies
`/api → localhost:7456` (`vite.config.ts` `server.proxy`). An unmocked request
that slips through would silently reach a real local daemon and mask the gap.
The dispatcher answers registry misses with **500** (body
`{kind:"e2e-mock-miss", message}`, still recorded in `reqs`). Verified in PR1:
a direct `fetch` to an unregistered cmd returns 500 and lands in `reqs`.

**Boot defaults**: `fixtures.ts` pre-registers the minimum boot + send surface
(17 cmds — `config.load`, projects, scheduled tasks, session CRUD, queue
hydrate, pending reconcile, trace load, usage window, `agent/chat`). If a new
spec's flow fires a cmd that isn't there, **register it in the spec**, or the
fail-loud 500 will surface it. When a cmd turns out to fire on the *default*
boot path itself, add it to `bootDefaults()` — that happened twice in PR1
(`projects/list_hidden_projects`, `usage/usage_window`): stores catch invoke
errors, so nothing else would have exposed them.

---

## 3. Wire facts (constraint — do not "fix" by intuition)

- **No envelope**. Success responses are daemon `Json<T>` passed through
  verbatim; empty body parses as `null` (`transport/http.ts` invoke tail).
  `mockCmd` payload is the `Json<T>` body itself; `null` payload is JSON null.
  Errors are `TransportError(status, {kind?, message?, request_id?})`.
- **Request top-level keys are camelCase → snake_case** (`transformArgsTopLevel`,
  `transport/http.ts:270-289`): `requestId` → `request_id`. Registry matching
  and body assertions use snake_case at the top level; nested values pass
  through untouched.
- **Response casing is per-struct and often the opposite direction**: e.g.
  `list_session_tool_permissions` rows are **camelCase** (Rust
  `PermissionGrantRow` `rename_all = "camelCase"`), while
  `ToolQuestionPayload` events are **snake_case** (no rename). Read the
  consuming store's generic for each cmd — see spec
  `transport-and-pwa-modes.md` for the casing rules.
- **`GET /api/v1/health` is a mount hard gate** (`transport/health.ts`):
  `apiVersions` must contain `"v1"` or `main.ts` renders the fatal overlay and
  never mounts. The body must carry `daemonId` and must **not** carry
  `remoteId` — that field flips the router into remote context and redirects
  to `/pairing` (spec `transport-and-pwa-modes.md`, two-signals section).

---

## 4. Checklist for a new browser spec

1. **Deterministic or it doesn't enter CI** (PRD D2: blocking gate). Route-mock
   driven; no daemon, no LLM, no network, no DB seeds. If a case is inherently
   timing-dependent, mark it local-only instead of relying on retries
   (`retries: CI ? 1 : 0` — the local zero exists so flake isn't papered over).
2. **Seed through production loaders, not store internals**: answer the load
   IPCs (`list_sessions`, `load_session`, `list_session_tool_permissions`, …)
   with production-shaped rows. Components with onMounted loads re-read the
   backend and overwrite direct seeds (same rule as spec `test-environment.md`
   §6).
3. **Drive streams via `stream.emit`** with exact channel names and payload
   shapes read from the consumer (`streamEvents.ts` / `streamController.ts`):
   `chat-event`, `tool:call`, `tool:result`, `tool:question`,
   `mode:change:request`, `task:state:transition:request`, `subagent:event`,
   `permission:ask`, `projects:refreshed`, `stream-resync`.
4. **Wait on behavior signals, never sleeps**: `expect.poll` over scroll
   deltas / class flips, `waitForSelector` / web-first `expect` on stable
   selectors, `waitForCmd` for network. A fixed `waitForTimeout` is allowed
   only as a *negative*-assertion window ("nothing must have happened by
   now"), e.g. the Shift+Enter no-send check.
5. **Selectors: stable scoped class hooks first** (ui-review.sh SELECTORS
   precedent), `data-testid` only when no hook exists — and register it in
   `app/e2e/README.md` (registry table at the bottom). `app/src` production
   diff must stay ≈ 0 beyond registered testids.
6. **Update the docs in the same change**: new default-surface cmd →
   `bootDefaults()` + `app/e2e/README.md` table; new trap → the section below.

---

## 5. Known traps (each one actually bit us)

### 5.1 CodeMirror "is the editor empty" is not `.cm-line` textContent

**Symptom**: asserting the cleared editor via line text fails — the line
contains "问点什么，或输入 / 调出命令…". **Cause**: the placeholder is a
`.cm-placeholder` *widget* rendered inside the first `.cm-line`; a real line
break adds a `.cm-line` div, but emptiness is only observable via the
placeholder. **Fix**: count `.cm-line` for line breaks (soft wraps don't split
lines); assert `:assert` visibility of `.cm-placeholder` for "cleared".
Measured in `e2e/chat-input-keys.spec.ts` (CH5-1). Also note: the chat input's
keymap has **no `Shift+Enter` binding** (`utils/chatInputCodeMirror.ts` — it
handles `Enter` / ArrowUp / ArrowDown / Tab / Escape only), so Shift+Enter
falls through to contenteditable default behavior, which is exactly why it
needs real CDP keys (docs/BUGLIST.md §4 CH5-1).

### 5.2 The authoritative pull wipes your seed mid-action

**Symptom**: pending interaction emitted via `stream.emit` disappears before
the code under test reads it. **Cause**: `send()` runs
`controller.ensureLoaded` → `get_pending_interaction`, and a `null` answer
removes the pending as "stale" (cache contract, spec `test-environment.md`
§8). **Fix**: `mockCmd("question", "get_pending_interaction",
{kind:"question", payload})` agreeing with the emitted seed. Measured in
`e2e/question-card-scroll.spec.ts` (CH8-2b toast).

### 5.3 Streaming state without a frontend send: cross-client adoption

**Symptom/technique**: to make `isCurrentSessionStreaming` true without
driving the whole send path, emit `chat-event` with `{request_id, session_id,
kind:"start"}` — `adoptForeignRequest` (`streamEvents.ts`, 2026-08-27
cross-client claim) registers the request for that session and pushes a
streaming placeholder. **Boundaries**: without `session_id` (old wire) the
event is dropped; a rid in `completedRequests` is not re-adopted. DOM signal:
`.chat-input__row--streaming`. Measured in
`e2e/question-card-scroll.spec.ts`.

### 5.4 The fake EventSource deliberately has no reconnection

The injected `window.EventSource` (`fixtures.ts` init script) implements
`addEventListener` / `removeEventListener` / `close` + `readyState` only.
Native auto-reconnect + `Last-Event-ID` replay + `stream-resync` semantics are
an excluded test surface — they are covered by Rust `--test e2e` and spec
`backend/daemon-server.md`. Implementing reconnect here would mask real
regressions. Named events with no listener are dropped, mirroring the native
behavior; `http.ts` only uses named listeners.

### 5.5 History seeding goes through `load_session`, not `list_messages`

There is no `list_messages` command; the authoritative history read is
`sessions/load_session` returning the `LoadedSession` wire
(`streamRehydrate.ts`: `{session, messages: LoadedMessage[]}`, per-message
`seq`/`content` blocks/`text`, latency fields nullable). `rehydrateMessages`
also merges user-row tool_results into the preceding assistant and splices
orphan-repair rows — seed content blocks accordingly if your assertions touch
tool cards. Measured in `e2e/question-card-scroll.spec.ts`.

### 5.6 Environment facts (WSL / dev server)

- Playwright webServer runs `pnpm dev --port 1422`: `webServer.port` only
  declares what to *wait for* — vite's hardcoded `port: 1420, strictPort`
  (`vite.config.ts`) is overridden by the CLI flag. 1422 avoids the user's dev
  server (1420) and the daemon (7456).
- Dev mode `daemonBase()` is `http://localhost:7456` (http.ts DEV probe), so
  page→mock requests are cross-origin from :1422. Fulfilled responses carry
  `Access-Control-Allow-Origin: *` and the dispatcher answers OPTIONS;
  measured under Chromium interception no preflights are actually emitted,
  the OPTIONS branch stays as a safety net.
- Local run needs no `playwright install-deps` on this machine (the
  ui-review scratch playwright-core already proved the system libs); see
  `docs/HACKING-wsl.md` for the full local instructions.
