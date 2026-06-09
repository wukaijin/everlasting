<script setup lang="ts">
// StatusBar — bottom-of-content bar. Left: gear icon to open Settings.
// Right: model dropdown grouped by provider. When no model is available,
// shows "(未选择模型)" gray text that opens Settings on click.
//
// Per spike-003 the bar is 11px mono, surface background, and
// runs flush against the right column's bottom edge (no top
// separator so it visually merges with the input region above).

import { computed, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { useConfigStore } from "../../stores/config";
import { useModelsStore } from "../../stores/models";
import { useChatStore } from "../../stores/chat";
import SettingsModal from "../settings/SettingsModal.vue";
import Icon from "../Icon.vue";

const config = useConfigStore();
const modelsStore = useModelsStore();
const chatStore = useChatStore();

const settingsOpen = ref(false);

/** The currently active session, if any. */
const currentSession = computed(() => {
  const id = chatStore.currentSessionId;
  if (!id) return null;
  return chatStore.sessions.find((s) => s.id === id) ?? null;
});

/** The model id for the current session: per-session override first,
 *  then global default. `null` when no model is set at all. */
const currentModelId = computed<string | null>(() => {
  if (currentSession.value?.model_id) {
    return currentSession.value.model_id;
  }
  return modelsStore.defaultModelId ?? null;
});

/** True when at least one model exists in the catalog. */
const hasModels = computed<boolean>(() => modelsStore.models.length > 0);

/** True when the default model's provider has a non-empty api_key. */
const isConfigured = computed<boolean>(() => config.configured);

/** True when the current session is streaming — disables the dropdown. */
const isStreaming = computed<boolean>(() => chatStore.isCurrentSessionStreaming);

/** Handle model selection change from the dropdown. Immediately
 *  persists the per-session model override via IPC. */
async function onModelChange(event: Event) {
  const select = event.target as HTMLSelectElement;
  const modelId = select.value;
  const sid = chatStore.currentSessionId;
  if (!sid || !modelId) return;
  try {
    await invoke("update_session_model_id", {
      sessionId: sid,
      modelId: modelId,
    });
    // Update the local session summary so the dropdown reflects
    // the change without a full sessions reload.
    const summary = chatStore.sessions.find((s) => s.id === sid);
    if (summary) {
      (summary as { model_id: string | null }).model_id = modelId;
    }
  } catch (e) {
    console.error("Failed to update session model:", e);
  }
}

/** Click on the "(未选择模型)" text opens Settings. */
function onNoModelClick() {
  settingsOpen.value = true;
}

/** Click on the gear icon toggles Settings modal. */
function onGearClick() {
  settingsOpen.value = !settingsOpen.value;
}
</script>

<template>
  <div
    v-if="config.loaded"
    :class="['status-bar', { 'status-bar--warn': !isConfigured }]"
  >
    <!-- Left: gear icon -->
    <button
      type="button"
      class="status-bar__gear"
      title="Settings"
      :aria-label="'Open Settings'"
      @click="onGearClick"
    >
      <Icon name="cog" :size="12" />
    </button>

    <span class="status-bar__dot" />

    <!-- Right: model dropdown or empty-state text -->
    <div class="status-bar__right">
      <template v-if="hasModels">
        <select
          class="status-bar__select"
          :value="currentModelId ?? ''"
          :disabled="isStreaming"
          :title="isStreaming ? 'Streaming 中,无法切换模型' : '切换模型'"
          @change="onModelChange"
        >
          <option v-if="!currentModelId" disabled value="">
            (选择模型)
          </option>
          <optgroup
            v-for="group in modelsStore.modelsGroupedByProvider"
            :key="group.provider.id"
            :label="group.provider.displayName"
          >
            <option
              v-for="m in group.models"
              :key="m.id"
              :value="m.id"
            >
              {{ m.displayName }}
            </option>
          </optgroup>
        </select>
      </template>
      <template v-else>
        <span
          class="status-bar__empty"
          title="请在 Settings 中添加模型"
          @click="onNoModelClick"
        >
          (未选择模型)
        </span>
      </template>

      <span v-if="!isConfigured && hasModels" class="status-bar__hint">
        API Key 未设置
      </span>
    </div>

    <SettingsModal v-model:open="settingsOpen" />
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

.status-bar__gear {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  background: transparent;
  border: 0;
  color: var(--color-text-muted);
  cursor: pointer;
  padding: 2px;
  border-radius: 3px;
  font: inherit;
}

.status-bar__gear:hover {
  background: var(--color-bg-elevated);
  color: var(--color-text-primary);
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

.status-bar__right {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-left: auto;
}

.status-bar__select {
  font-family: var(--font-mono);
  font-size: 11px;
  font-weight: 500;
  color: var(--color-text-primary);
  background: var(--color-bg-elevated);
  border: 1px solid var(--color-bg-border);
  border-radius: 4px;
  padding: 2px 6px;
  cursor: pointer;
  max-width: 220px;
  overflow: hidden;
  text-overflow: ellipsis;
  appearance: auto;
}

.status-bar__select:hover:not(:disabled) {
  border-color: var(--color-accent);
}

.status-bar__select:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}

.status-bar__empty {
  color: var(--color-text-muted);
  cursor: pointer;
  font-style: italic;
}

.status-bar__empty:hover {
  color: var(--color-text-secondary);
  text-decoration: underline;
}

.status-bar__hint {
  color: var(--color-tool-shell);
  font-weight: 500;
}
</style>
