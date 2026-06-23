<script setup lang="ts">
// SubagentDrawerErrorCard — PR6 R25 error card (the ❌ block below
// DrawerPromptCard when `status === "error"`).
//
// Split out of SubagentDrawer (2026-06-23, PRD
// `06-23-06-23-split-subagent-drawer`) so the main drawer can drop
// to ~900 lines. Pure presentation — receives the resolved error
// message as a prop and renders the chrome (icon + title + message
// body). The 4-level fallback chain (transcriptJson → finalText →
// summary → canned "(no error text captured)") stays in the main
// drawer's `errorMessage` computed; this component just renders
// whatever it's given.
//
// The visual language is a deliberate cousin of `PermissionModal
// --critical` / `YoloConfirmModal` — red 3px left border using
// `--color-tool-error`, per the design-tokens "extreme risk"
// convention (see `design-tokens.md` Border Tokens exception). The
// header banner (FT-F-005) shows an 80-char summary line; this card
// shows the full error message verbatim, mono font, scrollable
// above 240px max-height.

import Icon from "../Icon.vue";

defineProps<{
  /** Resolved error message from the main drawer's `errorMessage`
   *  computed (4-level fallback). Never empty when this card is
   *  mounted (the main drawer v-ifs on `errorMessage !== null`). */
  errorMessage: string;
}>();
</script>

<template>
  <div class="subagent-drawer__error-card" role="alert">
    <div class="subagent-drawer__error-header">
      <span class="subagent-drawer__error-icon">
        <Icon name="shield-x" :size="14" />
      </span>
      <span class="subagent-drawer__error-title">Worker error</span>
    </div>
    <p class="subagent-drawer__error-message">{{ errorMessage }}</p>
  </div>
</template>

<style scoped>
/* PR6 R25: error card chrome. Red 3px left border (--color-tool-error)
   per design-tokens convention; matches PermissionModal --critical /
   YoloConfirmModal visual language for "extreme risk" surfaces. */
.subagent-drawer__error-card {
  background: var(--color-bg-surface);
  border: 1px solid var(--color-bg-border);
  border-left: 3px solid var(--color-tool-error);
  border-radius: 6px;
  padding: 8px 12px;
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.subagent-drawer__error-header {
  display: flex;
  align-items: center;
  gap: 6px;
  font-family: var(--font-sans);
  font-size: 12px;
  font-weight: 600;
  color: var(--color-tool-error);
}

.subagent-drawer__error-icon {
  display: inline-flex;
  align-items: center;
  flex-shrink: 0;
}

.subagent-drawer__error-title {
  color: var(--color-tool-error);
}

.subagent-drawer__error-message {
  margin: 0;
  font-family: var(--font-mono);
  font-size: 11px;
  line-height: 1.5;
  color: var(--color-text-primary);
  word-break: break-word;
  white-space: pre-wrap;
  max-height: 240px;
  overflow-y: auto;
}
</style>
