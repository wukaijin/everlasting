<script setup lang="ts">
// SettingsModal — full-viewport overlay with 6 tabs (Providers, Models,
// Default, Memory, Subagents, Remote). Uses reka-ui DialogRoot/DialogContent
// for overlay + focus trap and TabsRoot/TabsList/TabsTrigger/TabsContent for
// the tab switcher. Receives v-model:open from the parent (Sidebar footer
// button).

import { DialogRoot, DialogPortal, DialogOverlay, DialogContent, DialogTitle, DialogClose } from "reka-ui";
import { TabsRoot, TabsList, TabsTrigger, TabsContent } from "reka-ui";
import Icon from "../Icon.vue";
import ProvidersTab from "./ProvidersTab.vue";
import ModelsTab from "./ModelsTab.vue";
import DefaultTab from "./DefaultTab.vue";
import MemoryTab from "./MemoryTab.vue";
import SubagentsTab from "./SubagentsTab.vue";
import RemoteTab from "./RemoteTab.vue";

const open = defineModel<boolean>("open", { required: true });
</script>

<template>
  <DialogRoot v-model:open="open">
    <DialogPortal>
      <DialogOverlay class="settings-modal__overlay" />
      <DialogContent class="settings-modal" @pointerdown-outside="open = false">
        <header class="settings-modal__header">
          <DialogTitle class="settings-modal__title">Settings</DialogTitle>
          <DialogClose as-child>
            <button type="button" class="settings-modal__close" aria-label="Close">
              <Icon name="x" :size="14" />
              <!-- S6b: 移动端语义化关闭文案(桌面 display:none) -->
              <span class="settings-modal__close-label">Done</span>
            </button>
          </DialogClose>
        </header>

        <TabsRoot default-value="providers" class="settings-modal__body">
          <TabsList class="settings-modal__tabs">
            <TabsTrigger value="providers" class="settings-modal__tab">
              Providers
            </TabsTrigger>
            <TabsTrigger value="models" class="settings-modal__tab">
              Models
            </TabsTrigger>
            <TabsTrigger value="default" class="settings-modal__tab">
              Default
            </TabsTrigger>
            <TabsTrigger value="memory" class="settings-modal__tab">
              Memory
            </TabsTrigger>
            <TabsTrigger value="subagents" class="settings-modal__tab">
              Subagents
            </TabsTrigger>
            <TabsTrigger value="remote" class="settings-modal__tab">
              Remote
            </TabsTrigger>
          </TabsList>

          <TabsContent value="providers" class="settings-modal__content">
            <ProvidersTab />
          </TabsContent>
          <TabsContent value="models" class="settings-modal__content">
            <ModelsTab />
          </TabsContent>
          <TabsContent value="default" class="settings-modal__content">
            <DefaultTab />
          </TabsContent>
          <TabsContent value="memory" class="settings-modal__content">
            <MemoryTab />
          </TabsContent>
          <TabsContent value="subagents" class="settings-modal__content">
            <SubagentsTab />
          </TabsContent>
          <TabsContent value="remote" class="settings-modal__content">
            <RemoteTab />
          </TabsContent>
        </TabsRoot>
      </DialogContent>
    </DialogPortal>
  </DialogRoot>
</template>

<style scoped>
.settings-modal__overlay {
  position: fixed;
  inset: 0;
  background: color-mix(in srgb, var(--color-bg-app) 70%, transparent);
  backdrop-filter: blur(4px);
  z-index: 2000;
}

.settings-modal {
  position: fixed;
  top: 50%;
  left: 50%;
  transform: translate(-50%, -50%);
  width: 640px;
  max-width: calc(100vw - 40px);
  max-height: 80vh;
  background: var(--color-bg-surface);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-lg);
  display: flex;
  flex-direction: column;
  overflow: hidden;
  box-shadow: var(--shadow-xl);
  z-index: 2001;
  /* reka-ui DialogContent sets outline on focus; suppress for our design */
  outline: none;
  animation: settings-modal-zoom var(--duration-modal-in) var(--ease-modal-in) both;
}

.settings-modal[data-state="closed"] {
  animation: settings-modal-zoom-out var(--duration-modal-out) var(--ease-accelerate) forwards;
}

@keyframes settings-modal-zoom {
  from { opacity: 0; transform: translate(-50%, -50%) scale(0.1); }
  to   { opacity: 1; transform: translate(-50%, -50%) scale(1); }
}

@keyframes settings-modal-zoom-out {
  from { opacity: 1; transform: translate(-50%, -50%) scale(1); }
  to   { opacity: 0; transform: translate(-50%, -50%) scale(0.1); }
}

.settings-modal__header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 10px 16px;
  border-bottom: 1px solid var(--color-bg-border);
  background: var(--color-bg-elevated);
  flex-shrink: 0;
}

.settings-modal__title {
  margin: 0;
  font-size: var(--text-base);
  font-weight: var(--weight-semibold);
  color: var(--color-text-primary);
}

.settings-modal__close {
  background: transparent;
  border: 0;
  color: var(--color-text-muted);
  cursor: pointer;
  padding: 4px;
  border-radius: var(--radius-sm);
  display: inline-flex;
  align-items: center;
  justify-content: center;
}

.settings-modal__close:hover {
  background: var(--color-bg-border);
  color: var(--color-text-primary);
}

.settings-modal__body {
  flex: 1;
  display: flex;
  flex-direction: column;
  min-height: 0;
}

.settings-modal__tabs {
  display: flex;
  gap: 0;
  padding: 0 16px;
  border-bottom: 1px solid var(--color-bg-border);
  background: var(--color-bg-elevated);
  flex-shrink: 0;
}

.settings-modal__tab {
  padding: 8px 16px;
  font-size: var(--text-sm);
  font-weight: var(--weight-medium);
  color: var(--color-text-muted);
  background: transparent;
  border: 0;
  border-bottom: 2px solid transparent;
  cursor: pointer;
  transition: color var(--duration-base) var(--ease-out), border-color var(--duration-base) var(--ease-out);
}

.settings-modal__tab:hover {
  color: var(--color-text-secondary);
}

.settings-modal__tab[data-state="active"] {
  color: var(--color-text-primary);
  border-bottom-color: var(--color-accent);
}

.settings-modal__content {
  flex: 1;
  overflow-y: auto;
  padding: 16px;
  background: var(--color-bg-app);
}

/* --- S6b 关闭按钮文字 label(08-13-mobile-settings) ---
 * 桌面隐藏;移动端 icon + "Done" 并排(见下方移动端块)。 */
.settings-modal__close-label {
  display: none;
}

/* --- S6b 移动端适配(08-13-mobile-settings, 320-430px) ---
 * 桌面样式块零改动,以下全部放 @media (max-width: 767px) 内。
 * 容器全屏化 + 44px 触摸目标已由 style.css 全局块覆盖,不重复。 */
@media (max-width: 767px) {
  /* B1/B2/B3:tab 行横向可滚动 + 边缘渐变提示 + 当前 tab 背景 pill 高亮。
   * 6 个 tab 一字排开在窄屏必然超宽,overflow-x 让它左右可滑;
   * mask-image 双写 -webkit- + 标准(webkit/blink 兼容,见 design §3 风险表),
   * 右侧 88%→100% 淡出暗示还有内容可滑。
   *
   * 真机迭代第二轮(2026-08-13):用户看不到 "Subage..." 被截 → 补左侧
   * 0%→8% 渐变(已向左滑过),并把右侧渐变收紧到 92%→100%,露出更多
   * 完整 tab 字。 */
  .settings-modal__tabs {
    overflow-x: auto;
    scrollbar-width: none; /* 隐藏滚动条(仍可触控滑) */
    -webkit-mask-image: linear-gradient(to right, transparent 0%, #000 8%, #000 92%, transparent 100%);
    mask-image: linear-gradient(to right, transparent 0%, #000 8%, #000 92%, transparent 100%);
  }

  /* 当前 tab 不被压缩成一团,文字不折行;下划线高亮让位给背景 pill */
  .settings-modal__tab {
    flex-shrink: 0;
    white-space: nowrap;
    padding: 8px 10px;
    border-bottom-width: 0;
  }

  /* B3:当前 tab 背景 pill(替代下划线),对比度明显 */
  .settings-modal__tab[data-state="active"] {
    background: var(--color-accent-muted);
    color: var(--color-accent-text);
    border-radius: var(--radius-md);
  }

  /* B4:关闭按钮 "Done" 语义化 —— icon 与文字并排,44px 目标已由全局块保证 */
  .settings-modal__close {
    gap: 4px;
  }
  .settings-modal__close-label {
    display: inline;
  }

  /* 真机迭代(2026-08-13):scoped 的 max-height:80vh(桌面块)特异性高于
     style.css 全局块的 max-height:var(--app-height)(无 !important)——
     移动端实际被截在 80vh。这里同选择器源序靠后覆盖为 100vh(全屏)。
     height 保持全局 var(--app-height)(100dvh)。 */
  .settings-modal {
    max-height: 100vh;
  }
}
</style>
