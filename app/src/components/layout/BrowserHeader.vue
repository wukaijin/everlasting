<script setup lang="ts">
// BrowserHeader — browser-mode top bar (P2 browser-degrade fix, 2026-07-23).
//
// Mirrors the visual layout of `TitleBar.vue` (AppLogo + slot + flexible
// spacer) but strips everything that requires the Tauri runtime:
//   - NO `getCurrentWindow()` (throws in a plain browser — it's what
//     crashed the old `TitleBar` at setup time and hid the whole top
//     bar, including the project tab switcher).
//   - NO window-control buttons (min / max / close) — the browser
//     chrome owns those.
//   - NO `data-tauri-drag-region` — browsers don't let a page move
//     the window anyway.
//
// `AppHeader` picks between `TitleBar` (Tauri webview) and this
// component (plain browser) via `isTauriWebview()`. The shared slot
// content (ProjectTabs + HiddenProjectsMenu + PendingBadge) lives in
// `AppHeader` so it's not duplicated.
//
// The bottom 1px divider is owned by `AppHeader` (single source of
// truth), mirroring the TitleBar layout where AppHeader also owns it.

import AppLogo from "./AppLogo.vue";
</script>

<template>
  <!-- Same 40px row + flex layout as TitleBar, minus Tauri chrome. -->
  <div class="browser-header">
    <!-- AppLogo brand mark at the far left, matching TitleBar. -->
    <div class="browser-header__logo">
      <AppLogo :size="20" class="browser-header__logo-svg" />
    </div>

    <!-- Slot holds the shared top-bar content (project tabs, hidden
         projects menu, pending badge) — same contract as TitleBar's
         slot. Interactive children; no drag region in browser mode. -->
    <div class="browser-header__content">
      <slot />
    </div>

    <!-- Right-side flexible spacer so the slot content sits at the
         left (matches TitleBar's right-hand empty drag region, minus
         the window-control buttons that only exist in Tauri). -->
    <div class="browser-header__spacer" />
  </div>
</template>

<style scoped>
.browser-header {
  display: flex;
  align-items: stretch;
  height: 40px;
  background: var(--color-bg-surface);
  color: var(--color-text-secondary);
  font-family: var(--font-sans);
  flex-shrink: 0;
  box-sizing: border-box;
}

/* AppLogo wrapper — identical dimensions to TitleBar's so the slot
   content starts at the same x-offset in both modes (no layout shift
   when the same user switches between Tauri and browser). The mark
   carries its own brand colors (see AppLogo.vue). */
.browser-header__logo {
  flex-shrink: 0;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  padding-left: 8px;
  padding-right: 8px;
  box-sizing: border-box;
}

/* Slot region — same flex contract as TitleBar's `__content`. */
.browser-header__content {
  flex: 0 1 auto;
  min-width: 0;
  display: flex;
  align-items: stretch;
  height: 100%;
}

/* Right-side spacer fills the leftover width. */
.browser-header__spacer {
  flex: 1 1 0;
  min-width: 0;
}
</style>
