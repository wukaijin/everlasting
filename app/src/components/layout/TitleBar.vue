<script setup lang="ts">
// TitleBar — custom title bar (D4).
//
// Cross-platform behavior (per research/tauri-titlebar-patterns.md §2.1):
//   - macOS: Tauri keeps the native traffic lights at (14, 14). We reserve
//     80px on the left for them and draw no buttons (the system already
//     provides min/max/close via the red lights).
//   - Windows / Linux / WSLg: `decorations: false` in tauri.conf.json
//     removes the system frame, so we self-draw min/max/close on the
//     right (3 × 46px = 138px wide).
//
// Drag region:
//   - Root <div> is a drag region. The 80px macOS spacer, the AppLogo
//     (with `data-tauri-drag-region="false"` to opt out of drag), and
//     the right-hand empty area are part of it (so the user can grab
//     the title bar there).
//   - The <slot/> is wrapped in `data-tauri-drag-region="false"` so
//     project tabs (interactive children) don't get hijacked by drag.
//   - Window control buttons explicitly opt out too, defensively.
//
// `decorations: false` is the official Tauri 2 "fully custom title bar"
// mode and is verified working on WSLg in spike-004. macOS keeps its
// traffic lights via the `titleBarStyle: "Overlay"` config field (which
// takes precedence on macOS).
//
// D6 polish: the AppLogo SVG monogram is rendered at the FAR LEFT of
// the bar (before the macOS spacer, before the slot). It opts out of
// the drag region so it's clickable in the future. Window control
// buttons now use heroicons instead of the old ー/□/❐/✕ typography.
//
// Maximize behavior (bug-fix v4): the previous v3 logic divided
// `monitor.size` by `monitor.scaleFactor` to convert to logical
// pixels before calling `setSize(new LogicalSize(...))`. On WSLg
// that produced a 1920×1080 ceiling regardless of the host's
// actual 4K display, because WSLg's virtual monitor reports a
// physical size of 1920×1080 with scaleFactor=1.0 — so
// `monitor.size / scaleFactor` == `monitor.size`. The fix is to
// use `PhysicalSize` directly (skip the scaleFactor math) and
// read the size from the *window's* current monitor (not the
// global `currentMonitor()`), since the window may live on a
// different display than the primary. macOS keeps
// `toggleMaximize()` so the red lights + native fullscreen
// semantics still work.

import { onBeforeUnmount, onMounted, ref } from "vue";
import {
  getCurrentWindow,
  currentMonitor,
  LogicalSize,
  PhysicalSize,
  PhysicalPosition,
} from "@tauri-apps/api/window";
import { platform, type Platform } from "@tauri-apps/plugin-os";
import AppLogo from "./AppLogo.vue";
import Icon from "../Icon.vue";

const os = ref<Platform | null>(null);
const isMaximized = ref(false);
const isMac = ref(false);

let unlistenResize: (() => void) | null = null;
const win = getCurrentWindow();

/** Default "restored" size — matches `tauri.conf.json` window defaults. */
const DEFAULT_W = 1440;
const DEFAULT_H = 900;

/** Re-sync `isMaximized` with reality. Called on mount + on every
 *  resize. Behaviour differs per platform — see comment block above. */
async function syncMaximizedState() {
  try {
    if (isMac.value) {
      isMaximized.value = await win.isMaximized();
      return;
    }
    // Win / Linux: a window is "maximized" (in our sense) iff its
    // outer size matches the *window's* current monitor's full
    // physical size, within a 4px tolerance for AA / DPI rounding.
    // `win.outerSize()` and `monitor.size` are both in physical
    // pixels per Tauri 2 docs, so they compare directly. The
    // standalone `currentMonitor()` from `@tauri-apps/api/window`
    // resolves to the monitor the current webview is on (per
    // Tauri 2 docs), which is what we want — using
    // `win.currentMonitor()` would require a WebviewWindow method
    // that isn't exposed in this Tauri version.
    const monitor = await monitorAtWindow();
    if (!monitor) {
      isMaximized.value = await win.isMaximized();
      return;
    }
    const winSize = await win.outerSize();
    isMaximized.value =
      Math.abs(winSize.width - monitor.size.width) < 4 &&
      Math.abs(winSize.height - monitor.size.height) < 4;
  } catch (e) {
    console.error("[TitleBar] syncMaximizedState failed", e);
  }
}

/** Pick the monitor the Tauri window is currently on. The
 *  user can drag the window between any of the system's
 *  monitors (e.g. RDP virtual display + host primary in an RDP
 *  session, or just multi-monitor on a desktop), and clicking
 *  maximize should fill whichever monitor the window is on
 *  right now. `currentMonitor()` is exactly that — it returns
 *  the monitor that contains the current webview. */
async function monitorAtWindow(): Promise<Awaited<ReturnType<typeof currentMonitor>>> {
  return await currentMonitor();
}

onMounted(async () => {
  try {
    const p = await platform();
    os.value = p;
    isMac.value = p === "macos";
  } catch (e) {
    // Fallback: treat as non-macOS (show all 3 window control buttons).
    // This is a sensible default — on Win/Linux it's correct; on macOS
    // the user would just see duplicate min/max/close alongside the
    // native red lights, which is ugly but not broken.
    console.error("[TitleBar] platform() failed; assuming non-macOS", e);
    isMac.value = false;
  }
  await syncMaximizedState();
  try {
    unlistenResize = await win.onResized(() => {
      void syncMaximizedState();
    });
  } catch (e) {
    console.error("[TitleBar] onResized wiring failed", e);
  }
});

onBeforeUnmount(() => {
  if (unlistenResize) {
    unlistenResize();
    unlistenResize = null;
  }
});

async function onMinimize() {
  try {
    await win.minimize();
  } catch (e) {
    console.error("[TitleBar] minimize failed", e);
  }
}

async function onToggleMaximize() {
  try {
    if (isMac.value) {
      // macOS: defer to the OS so the native fullscreen / maximize
      // animation + red-light semantics are preserved.
      await win.toggleMaximize();
      return;
    }
    // Win / Linux / WSLg: toggle between "fill the monitor the
    // window is currently on" and the default 1440×900 (centered
    // on that monitor). This is what users expect from a maximize
    // button when the OS work area is unacceptably small.
    //
    // TODO(bug-position): in some RDP / multi-monitor setups
    // (RDP virtual display + host primary, both reported as
    // monitors), the window ends up growing rightward instead of
    // moving to the host primary's top-left after setSize. The
    // order is setPosition-then-setSize (Tauri's auto-clamp on
    // setSize overrode the position otherwise), but the issue
    // persists. Likely candidate: `setFullscreen(true)` as a
    // deliberate "maximize" trade-off. See prd.md "Progress so
    // far" + journal-1.md (2026-06-07).
    if (isMaximized.value) {
      // Restore: LogicalSize matches the tauri.conf.json default.
      // Center on the monitor using PhysicalPosition to keep the
      // math consistent with the maximize case below.
      await win.setSize(new LogicalSize(DEFAULT_W, DEFAULT_H));
      const monitor = await monitorAtWindow();
      if (monitor) {
        const factor = monitor.scaleFactor;
        const winW = DEFAULT_W * factor;
        const winH = DEFAULT_H * factor;
        const mW = monitor.size.width;
        const mH = monitor.size.height;
        const x = monitor.position.x + Math.max(0, (mW - winW) / 2);
        const y = monitor.position.y + Math.max(0, (mH - winH) / 2);
        await win.setPosition(new PhysicalPosition(x, y));
      }
      isMaximized.value = false;
    } else {
      // Maximize: fill the entire physical monitor the window is
      // on. `monitorAtWindow` reads `currentMonitor()` so the
      // user can drag the window to a different display first,
      // then click maximize to fill that display.
      const monitor = await monitorAtWindow();
      if (!monitor) {
        // No monitor info (very unusual) — fall back to OS maximize.
        await win.toggleMaximize();
        return;
      }
      try {
        // Order matters: setPosition first, then setSize. If we
        // resize first, Tauri may auto-reposition the window to
        // keep it on-screen, and the subsequent setPosition can
        // be overridden by a window manager clamp.
        await win.setPosition(
          new PhysicalPosition(monitor.position.x, monitor.position.y),
        );
        await win.setSize(
          new PhysicalSize(monitor.size.width, monitor.size.height),
        );
      } catch (e) {
        console.error("[TitleBar] setSize/Position failed, falling back to toggleMaximize", e);
        await win.toggleMaximize();
        return;
      }
      isMaximized.value = true;
    }
  } catch (e) {
    console.error("[TitleBar] toggleMaximize failed", e);
  }
}

async function onClose() {
  try {
    await win.close();
  } catch (e) {
    console.error("[TitleBar] close failed", e);
  }
}
</script>

<template>
  <!--
    Root drag region. The slot content (ProjectTabs), the AppLogo,
    and the window controls opt out below, so only the empty
    padding areas are draggable.
  -->
  <div
    :class="['titlebar', { 'titlebar--mac': isMac }]"
    data-tauri-drag-region
  >
    <!--
      App logo (D6): a small monogram at the far left. It opts
      out of the drag region so future click handlers don't have
      to fight the parent. Wrapped in a fixed-width cell so the
      slot content never shifts when this element changes size.
    -->
    <div
      class="titlebar__logo"
      data-tauri-drag-region="false"
    >
      <AppLogo :size="20" class="titlebar__logo-svg" />
    </div>

    <!--
      macOS: reserve 80px for the native traffic lights at (14, 14).
      This empty padded area is part of the drag region so the user can
      still grab the bar to the left of where the slot content starts.
    -->
    <div
      v-if="isMac"
      class="titlebar__mac-spacer"
      data-tauri-drag-region
    />

    <!--
      Slot holds the project tab bar (and any other left-side chrome
      the parent wants to render). We opt out of the drag region here
      so tab clicks and horizontal scroll inside ProjectTabs still work
      even though the root is a drag region.
    -->
    <div class="titlebar__content" data-tauri-drag-region="false">
      <slot />
    </div>

    <!--
      Right-side empty drag region. On Win/Linux this is the area
      between the tabs and the window control buttons (and beyond
      the buttons if the user widens the window past the controls).
      On macOS, since there are no window control buttons, this is
      the entire right half of the bar.
    -->
    <div class="titlebar__spacer" data-tauri-drag-region />

    <!--
      Window controls. macOS uses the system traffic lights (top-left),
      so we suppress these. On Win/Linux/WSLg we self-draw.
    -->
    <div
      v-if="!isMac"
      class="titlebar__controls"
      data-tauri-drag-region="false"
    >
      <button
        class="titlebar__btn"
        type="button"
        title="最小化 / Minimize"
        aria-label="Minimize"
        data-tauri-drag-region="false"
        @click="onMinimize"
      >
        <Icon name="minus" :size="14" />
      </button>
      <button
        class="titlebar__btn"
        type="button"
        :title="isMaximized ? '还原 / Restore' : '最大化 / Maximize'"
        :aria-label="isMaximized ? 'Restore' : 'Maximize'"
        data-tauri-drag-region="false"
        @click="onToggleMaximize"
      >
        <Icon :name="isMaximized ? 'restore' : 'maximize'" :size="14" />
      </button>
      <button
        class="titlebar__btn titlebar__btn--close"
        type="button"
        title="关闭 / Close"
        aria-label="Close"
        data-tauri-drag-region="false"
        @click="onClose"
      >
        <Icon name="x" :size="14" />
      </button>
    </div>
  </div>
</template>

<style scoped>
.titlebar {
  display: flex;
  align-items: stretch;
  height: 40px;
  background: var(--color-bg-surface);
  color: var(--color-text-secondary);
  font-family: var(--font-sans);
  user-select: none;
  -webkit-user-select: none;
  flex-shrink: 0;
  box-sizing: border-box;
  border-bottom: 1px solid var(--color-bg-border);
}

/* macOS leaves a slim sliver of breathing room between the top edge
   and the traffic lights; the rest of the bar is normal surface. */
.titlebar--mac {
  padding-left: 0; /* the 80px spacer below handles traffic-light clearance */
}

/* AppLogo wrapper: fixed-width cell at the far left, with a small
   left padding so the monogram doesn't touch the window edge and
   a right margin so it doesn't crowd the slot content (project
   tabs). The SVG itself uses `currentColor` so we paint it in the
   accent hue here. */
.titlebar__logo {
  flex-shrink: 0;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 32px;
  padding-left: 8px;
  padding-right: 12px;
  box-sizing: border-box;
}

.titlebar__logo-svg {
  color: var(--color-accent);
}

.titlebar__mac-spacer {
  width: 80px; /* 14 left + 14 (red light) + 14 + 14 + 14 + 14 = 80 — covers all 3 red lights at (14, 14) */
  flex-shrink: 0;
  height: 100%;
}

/* The middle (slot) is the only interactive region; it expands only
   to its content but can shrink (with min-width: 0) to let the right
   spacer take the leftover room. */
.titlebar__content {
  flex: 0 1 auto;
  min-width: 0;
  display: flex;
  align-items: stretch;
  height: 100%;
}

/* Right-side empty area: drag region. Grows to fill any leftover
   space on the row so the user can grab the bar there. */
.titlebar__spacer {
  flex: 1 1 0;
  min-width: 0;
}

.titlebar__controls {
  display: flex;
  height: 100%;
  flex-shrink: 0;
}

.titlebar__btn {
  width: 46px; /* Windows 11 standard */
  height: 100%;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  background: transparent;
  border: none;
  color: var(--color-text-secondary);
  line-height: 1;
  cursor: default;
  font-family: inherit;
  padding: 0;
  transition: background 0.1s, color 0.1s;
}

.titlebar__btn:hover {
  background: var(--color-bg-elevated);
  color: var(--color-text-primary);
}

.titlebar__btn:active {
  background: var(--color-bg-border);
}

.titlebar__btn--close:hover {
  background: var(--color-tool-error);
  color: #ffffff;
}
</style>
