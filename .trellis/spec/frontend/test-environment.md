# Test Environment Gotchas (vitest + jsdom)

> Frontend unit tests run in vitest + jsdom on plain Node — NOT the Tauri
> webview. Each entry below is a trap that actually bit us (with the fix
> pattern), so the same hour isn't spent twice. Canonical mock setups live
> in the referenced test files; read those before inventing a new shape.

---

## 1. fake-timers cycles permanently kill jsdom's rAF clock

**Symptom**: `requestAnimationFrame` callbacks never fire — not while timers
are faked (expected), but *also after* `vi.useRealTimers()` restores them.
A `new Promise(r => requestAnimationFrame(r))` inside a component hangs
forever and silently drops the continuation (BUGLIST CH12-1b: the search
modal's scroll-to-message test failed with zero scroll calls; a diagnostic
probe confirmed rAF dead for the rest of the file once any earlier test had
run a `useFakeTimers()` / `useRealTimers()` cycle).

**Fix pattern** — never await rAF bare in production code; race it against
a short macrotask so test environments (and background tabs, which stall
rAF in real browsers too) degrade gracefully:

```ts
function nextPaint(): Promise<void> {
  return Promise.race([
    new Promise<void>((resolve) => requestAnimationFrame(() => resolve())),
    new Promise<void>((resolve) => setTimeout(resolve, 60)),
  ]);
}
```

**Test-side**: don't drive the wait with fake timers; use real timers and a
`setTimeout` sleep slightly longer than the fallback. See
`components/search/SearchModal.test.ts` (CH12-1b test).

## 2. jsdom lacks `Element.scrollIntoView` / `Element.animate`

jsdom implements neither. Components that call them on reveal/locate need a
stub — file-local convention is a `beforeEach`:

```ts
beforeEach(() => {
  Element.prototype.scrollIntoView = vi.fn();
});
```

(`SearchModal.test.ts:32`; `el.animate?.()` optional-call is the other half
of the pattern — code should treat both as maybe-missing.)

## 3. Raw HTML (v-html) can't carry Vue listeners — delegate

`utils/markdown.ts` output goes through `v-html`, so any interactive chrome
emitted into it (e.g. the fenced-code-block copy button) must be driven by
**event delegation on the container**, not `@click` bindings:

- markdown pipeline emits stable hooks: `data-code-block` on the wrapper,
  `data-code-copy` on the button;
- the rendering component attaches ONE delegated click handler
  (`composables/useCodeBlockCopy.ts`) that `closest()`-finds the button and
  reads the sibling `<pre><code>` text;
- button label feedback ("复制" → "已复制") mutates `textContent` directly —
  the node is not Vue-managed.

## 4. Transport mock pattern (canonical)

Stores talk to the backend only through the `../transport` barrel. Tests
mock the barrel and drive everything through one `invokeMock`:

```ts
const invokeMock = vi.fn();
vi.mock("../transport", () => ({
  transport: { invoke: (...args: unknown[]) => invokeMock(...args), listen: async () => () => {} },
}));
```

Full examples: `stores/projects.test.ts` (also re-stubs
`../transport/http`'s `TransportError` class), `stores/createNewSession.test.ts`
(ctx-boundary mock for the session-actions factory), `components/search/SearchModal.test.ts`
(component + store together; note its `attachTo: document.body` +
`document.body` queries because reka-ui teleports to body).

## 5. VTU stubs `transition-group` by default — opt out when the component grabs the real `<ul>`

**Symptom**: `wrapper.get("ul.messages")` (or any `document.querySelector` for the
TransitionGroup's rendered tag) fails; the rendered HTML shows
`<transition-group-stub class="messages">` instead of `<ul class="messages">`.

**Cause**: @vue/test-utils v2 stubs `Transition`/`TransitionGroup` by default.
Components that reach the real rendered element through the group's
component-instance `$el` (MessageList's `setListEl` function ref) get the stub,
whose `$el` is not the styled tag — scroll logic silently no-ops.

**Fix**: opt the group out per mount — `global.stubs: { "transition-group": false }`.
(`MessageList.test.ts` is the worked example.)

## 6. Component `onMounted` store reload overwrites direct store seeds

**Symptom**: test seeds `store.someList = [...]` before mounting; the component
renders as if the list were empty (`<!--v-if-->` root).

**Cause**: components with a best-effort `onMounted` load (`HiddenProjectsMenu`
→ `loadHiddenProjects()`) re-read the backend on mount and **overwrite** the
seeded state with the mock's (empty) answer.

**Fix**: don't seed the store directly — make `invokeMock` answer the load IPC
with the initial list (production-shaped seeding). Same pattern applies to any
`watch(open)` load-on-transition component (PermissionGrantsModal: mount closed,
`setProps({ open: true })` to fire the load watcher).

## 7. rAF settle loops race test interactions — wait out the quiet window

**Symptom**: test sets `el.scrollTop = 100`, a frame later it is back at
`scrollHeight`; assertions flake by timing.

**Cause**: mount-time settle loops (`MessageList.stickToBottomUntilStable`:
pin-to-bottom every rAF until scrollHeight is quiet for 150ms or 1s deadline)
keep re-pinning scroll position AFTER the test has scrolled.

**Fix**: after mount, sleep past the quiet window before interacting
(`await new Promise(r => setTimeout(r, 300))` — real timers, NOT fake ones;
see gotcha 1). `MessageList.test.ts` `mountList` helper.

## 8. Authoritative pull wipes test-seeded state mid-action — mock the pull

**Symptom**: store state seeded before a store action; inside the action the
state is gone (e.g. pending interaction seeded, then `send` runs and
`getPending` returns undefined).

**Cause**: actions call an authoritative re-read mid-flight
(`send` → `controller.ensureLoaded` → `get_pending_interaction`; a null answer
removes the seeded pending as "stale" per the cache contract).

**Fix**: the mock must agree with the seed — answer the pull IPC with the
seeded entry. And do it AFTER the setup helper has drained its watchers:
swapping `invokeMock.mockImplementation` before setup adds an extra microtask
hop to `list_sessions`, pushing `onProjectChange`'s
`currentSessionId = null` tail past the drain and into the test
(`chatSendActions.test.ts` CH8-2b describe, worked example).
