// useSearchModal — D2 (08-17-cross-session-search) global search
// modal open state. Module-level singleton composable (same shape
// as useMobileNav): AppShell owns the `<SearchModal>` mount + the
// Cmd/Ctrl+K keybinding, AppHeader owns the mobile-only entry
// button, and anything else can `openSearch()` without prop
// drilling. No Pinia store — the modal's query/results state is
// component-local and dies with the dialog; only the open flag
// needs cross-component sharing.

import { ref } from "vue";

const searchModalOpen = ref(false);

export function useSearchModal() {
  function open(): void {
    searchModalOpen.value = true;
  }
  function close(): void {
    searchModalOpen.value = false;
  }
  return { searchModalOpen, open, close };
}
