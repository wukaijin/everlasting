<!-- Moved from popover-pattern.md 2026-09-19 (doc-split) -->

## Tauri Webview Gotcha: `window.confirm()` / `window.alert()` / `window.prompt()`

> **Never call `window.confirm()` / `window.alert()` /
> `window.prompt()` from this app's frontend code.** The Tauri
> webview does NOT reliably display native browser dialogs —
> the call often silently no-ops. Discovered during the 2026-06-11
> 体验优化 PR (commit `0140502`): clicking "delete" on a
> non-empty session would invoke `window.confirm()`, the dialog
> never appeared, and the click was lost. The fix was to
> replace it with the in-app `ConfirmDialog` component (see
> the "Confirmation Dialog Pattern" section above).

**Symptom**: the click handler runs, but the user sees nothing
happen. No dialog. No error. The action that should follow
the user's "OK" never executes.

**Why it happens**: Tauri uses a webview (WebKit on macOS,
WebView2 on Windows) for its frontend. These webviews
**block synchronous native dialogs** in their default config
to avoid pausing the main thread of the renderer. Tauri's
own dialog plugin (`@tauri-apps/plugin-dialog`) wraps the
native dialogs asynchronously — the synchronous
`window.confirm()` from a regular Vue event handler is not
wired up to it.

**Fix**: use the in-app `ConfirmDialog` component (or
`<Teleport>`-based modals for richer dialogs). The component
is just DOM rendered in the same webview — no native dialog
involved.

**Migration checklist for existing code**:

- [x] `SessionList.vue` `onDelete` + `contextDelete` — migrated
  to `ConfirmDialog` in 0140502.
- [ ] `DeleteWorktreeConfirm` — still uses its own hand-rolled
  modal, not migrated. Defer until a third call site exists.
- [ ] `DeleteModelConfirm` — same as above.
- [ ] (Search the codebase for any remaining
  `confirm(` / `alert(` / `prompt(` from Vue event handlers
  before shipping a release.)

**Note**: this gotcha is for the **synchronous** native dialog
APIs from the renderer JS context. The async Tauri dialog
plugin (`@tauri-apps/plugin-dialog`'s `ask()` / `message()`)
works correctly but is overkill for a Vue component — the
in-app `ConfirmDialog` is the right tool here.

---
