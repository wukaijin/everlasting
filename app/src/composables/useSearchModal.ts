// useSearchModal — D2 (08-17-cross-session-search) global search
// modal open state. Module-level singleton composable (same shape
// as useMobileNav): AppShell owns the `<SearchModal>` mount + the
// Cmd/Ctrl+K keybinding, AppHeader owns the mobile-only entry
// button, and anything else can `openSearch()` without prop
// drilling. No Pinia store — the modal's query/results state is
// component-local and dies with the dialog; only the open flag
// needs cross-component sharing.
//
// D2②+ (08-17-search-history-card): `open()` gained an optional
// prefill. The SearchHistoryCard CTA uses it to reopen the modal
// pre-armed with the agent tool call's query (and, when the call
// was project-scoped, its project filter) so "查看全部" lands on a
// full, fresh result list instead of an empty input. The prefill
// is consumed by SearchModal's open watcher (query prefilled +
// immediate search, debounce skipped — programmatic opens have no
// IME window) and cleared on consumption; `open()` with no args
// keeps the original blank-open behavior.

import { ref } from "vue";

const searchModalOpen = ref(false);

/** One-shot open payload — consumed (and cleared) by SearchModal. */
export interface SearchModalPrefill {
  query: string;
  /** Initial project filter (`null` = all projects). */
  projectId?: string | null;
}

const pendingPrefill = ref<SearchModalPrefill | null>(null);

/** One-shot consume for the modal's open watcher: returns and
 *  clears the pending prefill (a prefill left set would leak into
 *  the next blank open). */
function consumePrefill(): SearchModalPrefill | null {
  const p = pendingPrefill.value;
  pendingPrefill.value = null;
  return p;
}

export function useSearchModal() {
  function open(prefill?: SearchModalPrefill): void {
    pendingPrefill.value = prefill ?? null;
    searchModalOpen.value = true;
  }
  function close(): void {
    searchModalOpen.value = false;
  }
  return { searchModalOpen, pendingPrefill, open, close, consumePrefill };
}
