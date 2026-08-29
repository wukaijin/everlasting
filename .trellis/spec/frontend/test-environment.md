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
