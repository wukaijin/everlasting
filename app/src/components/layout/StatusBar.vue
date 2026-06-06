<script setup lang="ts">
// StatusBar — bottom-of-content bar showing the LLM model and base
// URL. When the API key is missing (`!config.configured`) we flip
// the dot to amber and surface a hint per the original behavior.
//
// Per spike-003 the bar is 11px mono, surface background, and
// runs flush against the right column's bottom edge (no top
// separator so it visually merges with the input region above).

import { useConfigStore } from "../../stores/config";

const config = useConfigStore();
</script>

<template>
  <div
    v-if="config.loaded"
    :class="['status-bar', { 'status-bar--warn': !config.configured }]"
  >
    <span class="status-bar__dot" />
    <span class="status-bar__model">{{ config.model || "(no model)" }}</span>
    <span class="status-bar__sep">·</span>
    <span class="status-bar__url">{{ config.baseUrl || "(no base_url)" }}</span>
    <span v-if="!config.configured" class="status-bar__hint">
      ANTHROPIC_API_KEY 未设置
    </span>
  </div>
</template>

<style scoped>
.status-bar {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 6px 16px;
  background: var(--color-bg-surface);
  font-size: 11px;
  color: var(--color-text-muted);
  font-family: var(--font-mono);
  flex-shrink: 0;
}

.status-bar--warn {
  background: var(--color-bg-elevated);
  color: var(--color-tool-shell);
}

.status-bar__dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--color-tool-write);
  flex-shrink: 0;
}

.status-bar--warn .status-bar__dot {
  background: var(--color-tool-shell);
}

.status-bar__model {
  font-weight: 500;
  color: var(--color-text-primary);
}

.status-bar__sep {
  color: var(--color-text-muted);
}

.status-bar__url {
  color: var(--color-text-secondary);
}

.status-bar__hint {
  margin-left: auto;
  color: var(--color-tool-shell);
  font-weight: 500;
}
</style>
