<script setup lang="ts">
// DefaultTab — radio group to select the default model. Models are
// displayed grouped by provider. Selecting a model immediately calls
// `modelsStore.setDefault(modelId)` (PRD D2: "选中立即生效").

import { useModelsStore } from "../../stores/models";
import Icon from "../Icon.vue";

const modelsStore = useModelsStore();

function selectDefault(modelId: string) {
  modelsStore.setDefault(modelId);
}
</script>

<template>
  <div class="default-tab">
    <div class="default-tab__header">
      <h3 class="default-tab__heading">Default Model</h3>
      <span v-if="modelsStore.defaultModel" class="default-tab__current">
        Current: <strong>{{ modelsStore.defaultModel.displayName }}</strong>
      </span>
    </div>

    <div v-if="modelsStore.models.length === 0" class="default-tab__empty">
      No models available. Add models in the Models tab first.
    </div>

    <div v-else class="default-tab__groups">
      <div
        v-for="group in modelsStore.modelsGroupedByProvider"
        :key="group.provider.id"
        class="default-tab__group"
      >
        <div class="default-tab__group-header">
          <Icon name="server" :size="12" />
          {{ group.provider.displayName }}
        </div>

        <div
          v-for="m in group.models"
          :key="m.id"
          :class="[
            'default-tab__option',
            { 'default-tab__option--selected': m.id === modelsStore.defaultModelId },
          ]"
          @click="selectDefault(m.id)"
        >
          <span
            :class="[
              'default-tab__radio',
              { 'default-tab__radio--checked': m.id === modelsStore.defaultModelId },
            ]"
          />
          <div class="default-tab__option-info">
            <span class="default-tab__option-name">{{ m.displayName }}</span>
            <span class="default-tab__option-id">{{ m.modelName }}</span>
            <span v-if="m.supportsThinking" class="default-tab__tag">thinking</span>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.default-tab {
  display: flex;
  flex-direction: column;
  gap: 12px;
}

.default-tab__header {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
}

.default-tab__heading {
  margin: 0;
  font-size: 14px;
  font-weight: 600;
  color: var(--color-text-primary);
}

.default-tab__current {
  font-size: 12px;
  color: var(--color-text-secondary);
}

.default-tab__current strong {
  color: var(--color-accent);
}

.default-tab__empty {
  padding: 24px;
  text-align: center;
  color: var(--color-text-muted);
  font-size: 13px;
}

.default-tab__groups {
  display: flex;
  flex-direction: column;
  gap: 16px;
}

.default-tab__group {
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.default-tab__group-header {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 4px 0;
  color: var(--color-text-muted);
  font-size: 11px;
  font-weight: 500;
  text-transform: uppercase;
  letter-spacing: 0.04em;
}

.default-tab__option {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 8px 12px;
  background: var(--color-bg-elevated);
  border: 1px solid var(--color-bg-border);
  border-radius: 6px;
  cursor: pointer;
  transition: border-color 0.15s, background 0.15s;
}

.default-tab__option:hover {
  border-color: var(--color-accent-muted);
  background: var(--color-bg-border);
}

.default-tab__option--selected {
  border-color: var(--color-accent);
  background: var(--color-accent-muted);
}

.default-tab__radio {
  width: 14px;
  height: 14px;
  border-radius: 50%;
  border: 2px solid var(--color-bg-border-strong);
  flex-shrink: 0;
  position: relative;
}

.default-tab__radio--checked {
  border-color: var(--color-accent);
}

.default-tab__radio--checked::after {
  content: "";
  position: absolute;
  top: 2px;
  left: 2px;
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--color-accent);
}

.default-tab__option-info {
  display: flex;
  align-items: center;
  gap: 8px;
  min-width: 0;
  flex: 1;
}

.default-tab__option-name {
  font-size: 13px;
  font-weight: 500;
  color: var(--color-text-primary);
}

.default-tab__option-id {
  font-size: 11px;
  color: var(--color-text-muted);
  font-family: var(--font-mono);
}

.default-tab__tag {
  font-size: 10px;
  padding: 1px 6px;
  border-radius: 3px;
  background: var(--color-accent-muted);
  color: var(--color-accent);
  font-family: var(--font-mono);
  flex-shrink: 0;
}
</style>
