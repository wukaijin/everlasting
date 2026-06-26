<script setup lang="ts">
// HiddenProjectsMenu — AppHeader 入口，展示已关闭(hidden=1)项目并提供
// "重新打开" 操作。
//
// 背景(RULE-FrontProj-001):关闭项目后，UI 唯一可见的"重新打开"入口是
// EmptyProjectState(只在 currentProjectId === null 时挂载);多项目用户隐藏
// 单个后，主 UI(ChatPanel)完全无入口。本组件补这条路径——在 AppHeader 的
// `+` 按钮前显示一个 archive 图标按钮 + count badge,点击展开 popover 列
// 出 hidden projects + per-row 重新打开。
//
// 设计取舍:
//   - 用 reka-ui DropdownMenu(项目已用，见 MessageActionsMenu.vue),拿到
//     keyboard arrow / Esc / focus-return a11y。
//   - 触发按钮仅在 hiddenProjects.length > 0 时显示，0 hidden 不渲染
//     (避免噪声)。
//   - 复用 projects.ts 既有 unhideProject(id),内部已 load + focus,
//     无新 store 逻辑。
//   - popover 内容样式借鉴 EmptyProjectState hidden-projects section
//     (compact two-column name+path + 重新打开按钮)保持视觉一致。
//
// **Portal + scoped 坑**:
// reka-ui `DropdownMenuContent` 内部 `<Teleport to="body">`,Vue
// `<style scoped>` 编译时会带 `data-v-xxx` 选择器后缀，portal 子节点
// 没有该属性 → 样式静默不生效(裸文本，无背景)。本组件触发按钮在
// 组件 template 内 → scoped 正常;**popover 内容必须用 `:deep()`** 穿透。
// (spec: `.trellis/spec/frontend/reka-ui-usage.md` §"Gotcha: <style
// scoped> does NOT apply to portal children")。
//
// **事件绑定策略**(用户反馈):
// 事件只绑到内层 "重新打开" 按钮，**不**绑到 `DropdownMenuItem` 整行。
// 鼠标点行不触发 unhide,只点按钮才触发(避免误触)。
//
// Out of scope:hidden projects 的批量操作、archive 主题、永久删除(项目
// 删除是 V2 路线图项，PROPOSAL 明确 out of scope)。

import { onMounted } from "vue";
import {
  DropdownMenuRoot,
  DropdownMenuTrigger,
  DropdownMenuPortal,
  DropdownMenuContent,
} from "reka-ui";
import { useProjectsStore } from "../stores/projects";
import Icon from "./Icon.vue";

const projectsStore = useProjectsStore();

onMounted(async () => {
  // Best-effort: populate the badge count on app start so the user
  // sees the entry even before they hide something in this session.
  // Matches EmptyProjectState's lazy-load behavior (Fix 2).
  try {
    await projectsStore.loadHiddenProjects();
  } catch {
    // Silent — the menu will still render as 0 hidden.
  }
});

async function onUnhide(id: string): Promise<void> {
  await projectsStore.unhideProject(id);
}
</script>

<template>
  <DropdownMenuRoot v-if="projectsStore.hiddenProjects.length > 0">
    <DropdownMenuTrigger as-child>
      <button
        type="button"
        class="hidden-menu__trigger"
        :title="`${projectsStore.hiddenProjects.length} 个已隐藏项目`"
        :aria-label="`已隐藏项目 (${projectsStore.hiddenProjects.length})`"
        data-testid="hidden-projects-trigger"
      >
        <Icon name="archive" :size="14" icon-class="hidden-menu__icon" />
        <span class="hidden-menu__count" data-testid="hidden-projects-count">
          {{ projectsStore.hiddenProjects.length }}
        </span>
      </button>
    </DropdownMenuTrigger>
    <DropdownMenuPortal>
      <DropdownMenuContent
        class="hidden-menu__content"
        :side-offset="6"
        align="end"
      >
        <div class="hidden-menu__header">
          已隐藏的项目
        </div>
        <!-- Each row is a layout-only <div> (NOT a DropdownMenuItem):
             we deliberately bind @click to the inner "重新打开" button
             only, so clicking the name/path area does NOT trigger
             unhide. The button is the sole action affordance. -->
        <div
          v-for="p in projectsStore.hiddenProjects"
          :key="p.id"
          class="hidden-menu__row"
          data-testid="hidden-projects-row"
        >
          <div class="hidden-menu__meta">
            <div class="hidden-menu__name-row">
              <span class="hidden-menu__name" :title="p.name">{{ p.name }}</span>
            </div>
            <div class="hidden-menu__path" :title="p.path">{{ p.path }}</div>
          </div>
          <button
            type="button"
            class="hidden-menu__action"
            :aria-label="`重新打开 ${p.name}`"
            data-testid="hidden-projects-action"
            @click="void onUnhide(p.id)"
          >
            重新打开
          </button>
        </div>
      </DropdownMenuContent>
    </DropdownMenuPortal>
  </DropdownMenuRoot>
</template>

<style scoped>
/* Trigger button — in template, NOT teleported, so scoped is fine. */
.hidden-menu__trigger {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  height: 100%;
  padding: 0 10px;
  background: transparent;
  border: none;
  border-left: 1px solid var(--color-bg-border);
  cursor: pointer;
  color: var(--color-text-secondary);
  transition: background var(--duration-fast) var(--ease-out), color var(--duration-fast) var(--ease-out);
  font-family: inherit;
  position: relative;
}

.hidden-menu__trigger:hover {
  background: var(--color-accent-muted);
  color: var(--color-accent);
}

.hidden-menu__icon {
  line-height: 1;
}

.hidden-menu__count {
  font-size: var(--text-xs);
  font-variant-numeric: tabular-nums;
  background: var(--color-accent);
  color: var(--color-text-on-accent);
  border-radius: 999px;
  padding: 1px 6px;
  min-width: 18px;
  text-align: center;
  font-weight: var(--weight-semibold);
  line-height: 1.4;
}

.hidden-menu__trigger:hover .hidden-menu__count {
  background: var(--color-accent-hover);
}

/* === Popover content (teleported to body) ===
   MUST use :deep() — the portal mounts outside this component's
   template tree, so a scoped selector with `data-v-xxx` suffix
   would silently fail to match. See reka-ui-usage.md §Gotcha. */

:deep(.hidden-menu__content) {
  min-width: 320px;
  max-width: 420px;
  max-height: 60vh;
  overflow-y: auto;
  padding: 4px;
  background: var(--color-bg-elevated);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-lg);
  box-shadow: 0 8px 24px rgba(0, 0, 0, 0.35);
  /* reka-ui 2.9.9 default opening animation; ~var(--duration-fast) var(--ease-out). */
  animation: hidden-menu-content-enter var(--duration-fast) var(--ease-out);
}

@keyframes hidden-menu-content-enter {
  from {
    opacity: 0;
    transform: translateY(-2px);
  }
  to {
    opacity: 1;
    transform: translateY(0);
  }
}

:deep(.hidden-menu__header) {
  font-size: var(--text-xs);
  font-weight: var(--weight-semibold);
  color: var(--color-text-muted);
  text-transform: uppercase;
  letter-spacing: 0.05em;
  padding: 8px 10px 6px;
}

/* Row is a layout-only container; no hover/cursor affordance so
   users don't expect the row itself to be clickable. */
:deep(.hidden-menu__row) {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 8px 10px;
  border-radius: var(--radius-md);
  font-size: var(--text-base);
}

:deep(.hidden-menu__meta) {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 2px;
}

:deep(.hidden-menu__name-row) {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
}

:deep(.hidden-menu__name) {
  color: var(--color-text-primary);
  font-weight: var(--weight-medium);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

:deep(.hidden-menu__path) {
  font-size: var(--text-xs);
  color: var(--color-text-muted);
  font-family: var(--font-mono);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  direction: rtl; /* keep the path tail visible when truncated */
  text-align: left;
}

:deep(.hidden-menu__action) {
  flex-shrink: 0;
  padding: 4px 10px;
  background: var(--color-bg-surface);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-sm);
  color: var(--color-accent);
  font-size: var(--text-sm);
  cursor: pointer;
  transition: background var(--duration-fast) var(--ease-out), border-color var(--duration-fast) var(--ease-out);
  font-family: inherit;
}

:deep(.hidden-menu__action:hover) {
  background: var(--color-accent-muted);
  border-color: var(--color-accent);
}
</style>