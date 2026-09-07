// useDiscussionLibrary — GCE M4b (09-07-gce-m4b-discussion-search)
// discussion-library modal open state. Module-level singleton
// composable (same shape as useSearchModal / useMobileNav): AppShell
// owns the `<DiscussionLibraryModal>` mount, and anything else (the
// Sidebar 讨论库 entry today) can `openDiscussionLibrary()` without
// prop drilling. No Pinia store — the modal's query/results/filter
// state is component-local and dies with the dialog; only the open
// flag needs cross-component sharing.

import { ref } from "vue";

const discussionLibraryOpen = ref(false);

export function useDiscussionLibrary() {
  function open(): void {
    discussionLibraryOpen.value = true;
  }
  function close(): void {
    discussionLibraryOpen.value = false;
  }
  return { discussionLibraryOpen, open, close };
}
