<script setup lang="ts">
// DefaultTab — radio group to select the default model. Models are
// displayed grouped by provider. Selecting a model immediately calls
// `modelsStore.setDefault(modelId)` (PRD D2: "选中立即生效").
//
// R1 polish: replaced the hand-rolled `.default-tab__radio` div with
// reka-ui `RadioGroupRoot` / `RadioGroupItem` / `RadioGroupIndicator`.
// The selectable row layout (group-header / name / model-name / tag)
// stays the same — the radio is the click target. The custom div was
// functional but it wasn't keyboard-navigable or screen-reader
// labelled; reka-ui's RadioGroup gives us all of that for free.

import { RadioGroupRoot, RadioGroupItem, RadioGroupIndicator } from "reka-ui";
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

        <RadioGroupRoot
          :model-value="modelsStore.defaultModelId ?? ''"
          class="default-tab__radiogroup"
          @update:model-value="(v) => v && selectDefault(String(v))"
        >
          <label
            v-for="m in group.models"
            :key="m.id"
            class="default-tab__option"
            :class="{
              'default-tab__option--selected': m.id === modelsStore.defaultModelId,
            }"
          >
            <RadioGroupItem :value="m.id" class="default-tab__radio">
              <RadioGroupIndicator class="default-tab__radio-indicator" />
            </RadioGroupItem>
            <div class="default-tab__option-info">
              <span class="default-tab__option-name">{{ m.displayName }}</span>
              <span class="default-tab__option-id">{{ m.modelName }}</span>
              <span v-if="m.supportsThinking" class="default-tab__tag">thinking</span>
            </div>
          </label>
        </RadioGroupRoot>
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
  font-size: var(--text-md);
  font-weight: var(--weight-semibold);
  color: var(--color-text-primary);
}

.default-tab__current {
  font-size: var(--text-sm);
  color: var(--color-text-secondary);
}

.default-tab__current strong {
  color: var(--color-accent-text);
}

.default-tab__empty {
  padding: 24px;
  text-align: center;
  color: var(--color-text-muted);
  font-size: var(--text-base);
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
  font-size: var(--text-xs);
  font-weight: var(--weight-medium);
  text-transform: uppercase;
  letter-spacing: 0.04em;
}

.default-tab__radiogroup {
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.default-tab__option {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 8px 12px;
  background: var(--color-bg-elevated);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-md);
  cursor: pointer;
  transition: border-color var(--duration-base) var(--ease-out), background var(--duration-base) var(--ease-out);
}

.default-tab__option:hover {
  border-color: var(--color-accent-muted);
  background: var(--color-bg-border);
}

.default-tab__option--selected {
  border-color: var(--color-accent);
  background: var(--color-accent-muted);
}

/* reka-ui RadioGroupItem renders a <button> by default; we style
   the button to match the previous hand-rolled 14px circle. The
   inner `RadioGroupIndicator` is the filled dot. */
.default-tab__radio {
  width: 14px;
  height: 14px;
  border-radius: 50%;
  border: 2px solid var(--color-bg-border-strong);
  background: transparent;
  flex-shrink: 0;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  padding: 0;
  cursor: pointer;
  transition: border-color var(--duration-base) var(--ease-out);
}

.default-tab__radio:hover {
  border-color: var(--color-accent-muted);
}

.default-tab__radio[data-state="checked"] {
  border-color: var(--color-accent);
}

.default-tab__radio-indicator {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--color-accent);
  display: block;
}

.default-tab__option-info {
  display: flex;
  align-items: center;
  gap: 8px;
  min-width: 0;
  flex: 1;
}

.default-tab__option-name {
  font-size: var(--text-base);
  font-weight: var(--weight-medium);
  color: var(--color-text-primary);
}

.default-tab__option-id {
  font-size: var(--text-xs);
  color: var(--color-text-muted);
  font-family: var(--font-mono);
}

.default-tab__tag {
  font-size: var(--text-2xs);
  padding: 1px 6px;
  border-radius: 3px;
  background: var(--color-accent-muted);
  color: var(--color-accent-text);
  font-family: var(--font-mono);
  flex-shrink: 0;
}

/* --- S6b 移动端适配(08-13-mobile-settings, 320-430px) ---
 * 桌面样式块零改动,以下全部放 @media (max-width: 767px) 内。 */
@media (max-width: 767px) {
  /* B9:模型条目 name/id/tag 分层不挤,允许换行 */
  .default-tab__option-info {
    flex-wrap: wrap;
    row-gap: 2px;
  }
  .default-tab__option-name {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    max-width: 100%;
  }

  /* B10:分组标题 `CARLOS-API-XXX` 对比度提升(muted → secondary + 放大) */
  .default-tab__group-header {
    font-size: var(--text-sm);
    color: var(--color-text-secondary);
  }

  /* 真机迭代(2026-08-13 第二轮):选中行(name+id+thinking)换行后第二行
     id 与 badge 视觉重复感强 → 缩小 id 字号 + 弱化颜色,与 name 拉开
     层次;允许 name 在一行占满(选中项有 inset accent bar 标识可辨)。 */
  .default-tab__option-name {
    font-size: var(--text-base);
    flex-basis: 100%;
  }
  .default-tab__option-id {
    font-size: var(--text-2xs);
  }

  /* B11:选中态左侧 accent 条,窄屏一眼可辨(桌面圆点+边框保留,叠加 inset 条) */
  .default-tab__option--selected {
    border-color: var(--color-accent);
    box-shadow: inset 3px 0 0 var(--color-accent);
  }

  /* 真机迭代(2026-08-13 第二轮):全局 44px 规则(.settings-modal button,
     含 min-height:44px 和 min-width:44px)把 radio (RadioGroupItem 默认
     button,桌面 14px 圆点)撑成 44px 大圆点。上一轮只覆盖了 width/height,
     min-width:44px 仍是胜者 → 必须同时覆盖 min-*。触摸目标仍由整行
     option(label)承担,radio 只是视觉指示,不需要 44px。 */
  .default-tab__radio {
    width: 14px;
    height: 14px;
    min-width: 14px;
    min-height: 14px;
  }
}
</style>
