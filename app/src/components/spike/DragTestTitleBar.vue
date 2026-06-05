<script setup lang="ts">
// spike-004: WSLg drag region + min/max/close verification
//
// Minimal Tauri 2 test bar that exercises:
//   - data-tauri-drag-region HTML attribute (the only cross-platform drag API
//     that works on WebKitGTK; do NOT use -webkit-app-region)
//   - data-tauri-drag-region="false" on interactive children to opt out
//   - getCurrentWindow().minimize() / toggleMaximize() / close()
//
// This component is intentionally isolated so D1-D4 can replace it cleanly.
// Per the spike brief, App.vue is wired to render THIS component for now
// (replacing ChatWindow) so the human can manually test on WSLg.
import { getCurrentWindow } from "@tauri-apps/api/window";

const win = getCurrentWindow();

async function onMin() {
  try {
    await win.minimize();
  } catch (e) {
    console.error("[spike-004] minimize failed:", e);
  }
}

async function onMax() {
  try {
    await win.toggleMaximize();
  } catch (e) {
    console.error("[spike-004] toggleMaximize failed:", e);
  }
}

async function onClose() {
  try {
    await win.close();
  } catch (e) {
    console.error("[spike-004] close failed:", e);
  }
}
</script>

<template>
  <!--
    Whole bar is draggable. The text label sits inside the drag region
    so the user knows where to grab. Buttons override the region opt-out
    so click events go to the buttons, not the drag handler.
  -->
  <div class="spike-titlebar" data-tauri-drag-region>
    <span class="spike-titlebar__label" data-tauri-drag-region>
      WSLg drag test — drag this bar (left/middle/right)
    </span>

    <div class="spike-titlebar__controls" data-tauri-drag-region="false">
      <button
        class="spike-titlebar__btn"
        type="button"
        title="最小化 / Minimize"
        data-tauri-drag-region="false"
        @click="onMin"
      >
        ー
      </button>
      <button
        class="spike-titlebar__btn"
        type="button"
        title="最大化 / Toggle Maximize"
        data-tauri-drag-region="false"
        @click="onMax"
      >
        □
      </button>
      <button
        class="spike-titlebar__btn spike-titlebar__btn--close"
        type="button"
        title="关闭 / Close"
        data-tauri-drag-region="false"
        @click="onClose"
      >
        ✕
      </button>
    </div>
  </div>
</template>

<style scoped>
.spike-titlebar {
  display: flex;
  align-items: center;
  width: 100%;
  height: 40px;
  background: #1e2a5e; /* Prussian tint, visible against light bg */
  color: #e5e7eb;
  font-size: 13px;
  user-select: none;
  -webkit-user-select: none;
  flex-shrink: 0;
  box-sizing: border-box;
  border-bottom: 1px solid #131822;
}

.spike-titlebar__label {
  flex: 1;
  padding: 0 12px;
  font-family: "Noto Sans CJK SC", "JetBrains Mono", monospace;
}

.spike-titlebar__controls {
  display: flex;
  height: 100%;
  margin-left: auto;
}

.spike-titlebar__btn {
  width: 46px;
  height: 100%;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  background: transparent;
  border: none;
  color: #e5e7eb;
  font-size: 14px;
  cursor: default;
  transition: background 80ms;
}

.spike-titlebar__btn:hover {
  background: rgba(255, 255, 255, 0.08);
}

.spike-titlebar__btn--close:hover {
  background: #c42b1c;
  color: #fff;
}
</style>
