<script setup lang="ts">
// DirBrowserModal — 「添加项目」的目录浏览模态框("浏览文件夹")。
// 2026-09-03 起为全模式统一入口(桌面 / 浏览器 / sidecar / remote;
// 原 native 选目录对话框链已整链下线)。数据源
// `POST /api/v1/projects/browse_dir`,同时保留路径输入 + 前往的
// 直达场景。
//
// 交互契约(projects store「添加项目」流):
//   - 单击目录行 → 进入该目录;".." 行 / 「上一步」 → 上一级。
//   - 路径输入 + 「前往」 → 直接跳转(支持 ~ 展开,后端负责)。
//   - 「显示隐藏目录」 toggle → 以 showHidden 重取当前目录。
//   - 「选择此目录」 → store.addProjectByPath(当前目录),注册尾巴
//     为去重 / unhide / create + focus(RULE-FrontProj-001),模态框
//     由 store 关闭。
//   - 键盘导航(roving tabindex):方向键在「.. + 目录行」间移动焦点
//     (钳边不环绕),Enter 走 button 原生激活;列表发起的导航完成后
//     焦点复位到新列表首行,非列表发起(路径直达 / 前往 / 上一步按钮)
//     不抢焦点。Esc 关窗沿用 reka-ui Dialog 自带行为。
//
// 组合方式镜像 MemoryModal(reka-ui DialogRoot/Portal/Overlay/
// Content),挂在 AppShell(store 状态驱动,ProjectTabs「+」与
// EmptyProjectState「添加项目」都汇到这里)。

import { nextTick, ref, watch } from "vue";
import {
  DialogRoot,
  DialogPortal,
  DialogOverlay,
  DialogContent,
  DialogTitle,
  DialogClose,
} from "reka-ui";

import { transport } from "../transport";
import { useProjectsStore } from "../stores/projects";
import { extractErrorMessage } from "../utils/useErrorBus";
import Icon from "./Icon.vue";

const open = defineModel<boolean>("open", { required: true });

/** `commands::projects::BrowseDirEntry`(serde 默认 snake_case 字段
 *  名与 TS 侧一致)。 */
interface BrowseDirEntry {
  name: string;
  path: string;
}

/** `commands::projects::BrowseDirPayload`。 */
interface BrowseDirPayload {
  path: string;
  parent: string | null;
  entries: BrowseDirEntry[];
}

const store = useProjectsStore();

// 浏览器状态:当前目录(canonical)、父目录、子目录列表、输入框
// 草稿。输入框草稿与 currentPath 分开 —— 「前往」失败时保留用户
// 输入以便修改,不回跳旧路径。
const currentPath = ref("");
const parentPath = ref<string | null>(null);
const entries = ref<BrowseDirEntry[]>([]);
const pathDraft = ref("");
const showHidden = ref(false);
const busy = ref(false);
const error = ref<string | null>(null);

// 键盘导航(roving tabindex):行集合 = 「..」(有父目录时,index 0)
// + entries。恰一行 tabindex=0(activeIndex 锚定),其余 -1。
const activeIndex = ref(0);
const listEl = ref<HTMLElement | null>(null);

/** 导航到指定目录。失败时横幅报错并保留上一次列表(行内重试),
 *  「选择此目录」/「上一步」随之禁用,与原生对话框「路径无效就停在
 *  原地」的体感一致。
 *
 *  `fromList` = 导航由列表发起(行点击 / 行上 Enter):完成后把焦点
 *  复位到新列表首行(键盘用户连续走目录不丢焦点)。非列表发起
 *  (路径直达 / 前往 / 上一步按钮 / 隐藏开关)不抢焦点。 */
async function navigate(
  path: string,
  opts: { fromList?: boolean } = {},
): Promise<void> {
  if (busy.value) return;
  busy.value = true;
  error.value = null;
  try {
    const payload = await transport.invoke<BrowseDirPayload>("browse_dir", {
      path,
      showHidden: showHidden.value,
    });
    currentPath.value = payload.path;
    parentPath.value = payload.parent;
    entries.value = payload.entries;
    pathDraft.value = payload.path;
  } catch (e) {
    error.value = extractErrorMessage(e);
  } finally {
    busy.value = false;
    if (opts.fromList) {
      // Rows are :disabled while busy — focus AFTER busy clears (and
      // one tick for the DOM to settle). On failure the previous
      // list is kept, so this re-anchors the first row for retry.
      activeIndex.value = 0;
      await nextTick();
      focusActiveRow();
    }
  }
}

// 每次打开都从 home 目录冷启动(与系统文件对话框落 home 的惯例
// 一致;get_home_dir 失败退化到 /)。隐藏目录开关不跨打开保留 ——
// 常规添加项目场景不需要记住。
watch(open, async (v) => {
  if (!v) return;
  error.value = null;
  entries.value = [];
  currentPath.value = "";
  parentPath.value = null;
  pathDraft.value = "";
  showHidden.value = false;
  activeIndex.value = 0;
  let home = "/";
  try {
    home = (await transport.invoke<string | null>("get_home_dir", {})) ?? "/";
  } catch {
    // home 拿不到就从根开始,浏览功能不受阻。
  }
  await navigate(home);
});

function onGo(): void {
  const p = pathDraft.value.trim();
  if (p) void navigate(p);
}

/** 「上一步」/ ".." 行共用。`fromList` 标记是否由列表行发起(.. 行
 *  传 true → 导航后焦点复位;底部按钮不抢焦点)。 */
function onUp(fromList = false): void {
  if (parentPath.value) void navigate(parentPath.value, { fromList });
}

function onToggleHidden(): void {
  showHidden.value = !showHidden.value;
  if (currentPath.value) void navigate(currentPath.value);
}

function onSelect(): void {
  if (!currentPath.value || busy.value) return;
  void store.addProjectByPath(currentPath.value);
}

// --- 键盘导航(roving tabindex)-----------------------------------------
// ArrowDown/ArrowUp 移动 DOM 焦点(钳边,不环绕);Enter 走 button
// 原生激活(无 JS handler)。keydown 挂在列表容器上 —— 路径输入框在
// 容器外,其方向键(移动光标)不会进到这里。

/** 可聚焦行总数:「..」(有父目录时)+ entries。 */
function rowCount(): number {
  return (parentPath.value ? 1 : 0) + entries.value.length;
}

/** entry `i` 的绝对行索引 → roving tabindex 值。 */
function entryTabIndex(i: number): 0 | -1 {
  return (parentPath.value ? 1 : 0) + i === activeIndex.value ? 0 : -1;
}

/** 焦点落到 activeIndex 行(钳到可用范围;busy 行 disabled 不落焦)。 */
function focusActiveRow(): void {
  const root = listEl.value;
  if (!root) return;
  const rows = Array.from(
    root.querySelectorAll<HTMLElement>(
      "button.dir-browser__row:not(:disabled)",
    ),
  );
  if (rows.length === 0) return;
  const idx = Math.min(activeIndex.value, rows.length - 1);
  activeIndex.value = idx;
  rows[idx].focus();
}

function onListKeydown(e: KeyboardEvent): void {
  if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
  const count = rowCount();
  if (count === 0) return;
  e.preventDefault();
  const current = Math.min(activeIndex.value, count - 1);
  const next =
    e.key === "ArrowDown"
      ? Math.min(current + 1, count - 1)
      : Math.max(current - 1, 0);
  activeIndex.value = next;
  focusActiveRow();
}
</script>

<template>
  <DialogRoot v-model:open="open">
    <DialogPortal>
      <DialogOverlay class="dir-browser__overlay" />
      <DialogContent
        class="dir-browser"
        :aria-describedby="undefined"
        @pointerdown-outside="open = false"
      >
        <header class="dir-browser__header">
          <DialogTitle class="dir-browser__title">浏览文件夹</DialogTitle>
          <DialogClose as-child>
            <button
              type="button"
              class="dir-browser__close btn btn--icon btn--ghost"
              aria-label="关闭"
            >
              <Icon name="x" :size="14" />
            </button>
          </DialogClose>
        </header>

        <div class="dir-browser__controls">
          <input
            v-model="pathDraft"
            class="dir-browser__path"
            type="text"
            spellcheck="false"
            placeholder="/absolute/path/to/project"
            :disabled="busy"
            @keydown.enter.prevent="onGo"
          />
          <button
            type="button"
            class="dir-browser__go btn btn--ghost"
            :disabled="busy || !pathDraft.trim()"
            @click="onGo"
          >
            前往
          </button>
          <button
            type="button"
            class="dir-browser__hidden btn btn--ghost"
            :class="{ 'dir-browser__hidden--on': showHidden }"
            :aria-pressed="showHidden"
            title="切换是否显示以 . 开头的目录"
            @click="onToggleHidden"
          >
            <Icon :name="showHidden ? 'eye-slash' : 'eye'" :size="13" />
            显示隐藏目录
          </button>
        </div>

        <div
          ref="listEl"
          class="dir-browser__list"
          @keydown="onListKeydown"
        >
          <div v-if="error" class="dir-browser__error" role="alert">
            <Icon name="warn" :size="14" />
            <span>{{ error }}</span>
          </div>
          <button
            v-if="parentPath"
            type="button"
            class="dir-browser__row dir-browser__row--up"
            :tabindex="activeIndex === 0 ? 0 : -1"
            :disabled="busy"
            title="上一级目录"
            @click="onUp(true)"
          >
            <Icon name="folder" :size="15" icon-class="dir-browser__row-icon" />
            <span class="dir-browser__row-name">..</span>
          </button>
          <button
            v-for="(e, i) in entries"
            :key="e.path"
            type="button"
            class="dir-browser__row"
            :tabindex="entryTabIndex(i)"
            :disabled="busy"
            :title="e.path"
            @click="navigate(e.path, { fromList: true })"
          >
            <Icon name="folder" :size="15" icon-class="dir-browser__row-icon" />
            <span class="dir-browser__row-name">{{ e.name }}</span>
          </button>
          <div
            v-if="!busy && !error && entries.length === 0 && currentPath"
            class="dir-browser__empty"
          >
            空目录
          </div>
          <div v-if="busy" class="dir-browser__loading">
            <span class="app-spinner app-spinner--xs" />
          </div>
        </div>

        <footer class="dir-browser__footer">
          <button
            type="button"
            class="dir-browser__back btn btn--ghost"
            :disabled="busy || !parentPath || !!error"
            @click="onUp(false)"
          >
            上一步
          </button>
          <button
            type="button"
            class="dir-browser__choose btn btn--primary"
            :disabled="busy || !!error || !currentPath"
            :title="currentPath"
            @click="onSelect"
          >
            选择此目录
          </button>
        </footer>
      </DialogContent>
    </DialogPortal>
  </DialogRoot>
</template>

<style scoped>
/*
 * reka-ui DialogPortal teleports to <body>; Vue 3.5 scoped CSS keeps
 * data-v attrs on teleported children (SettingsModal / MemoryModal
 * precedent — see reka-ui-usage.md gotcha). Non-:deep() style here.
 */

.dir-browser__overlay {
  position: fixed;
  inset: 0;
  background: color-mix(in srgb, var(--color-bg-app) 70%, transparent);
  backdrop-filter: blur(4px);
  z-index: var(--z-modal-overlay);
}

.dir-browser {
  position: fixed;
  top: 50%;
  left: 50%;
  transform: translate(-50%, -50%);
  width: 560px;
  max-width: calc(100vw - 40px);
  max-height: 80vh;
  background: var(--color-bg-surface);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-lg);
  display: flex;
  flex-direction: column;
  overflow: hidden;
  box-shadow: var(--shadow-lg);
  z-index: var(--z-modal);
  outline: none;
  animation: dir-browser-zoom var(--duration-modal-in) var(--ease-modal-in) both;
}

.dir-browser[data-state="closed"] {
  animation: dir-browser-zoom-out var(--duration-modal-out) var(--ease-accelerate) forwards;
}

@keyframes dir-browser-zoom {
  from {
    opacity: 0;
    transform: translate(-50%, -50%) scale(0.97);
  }
  to {
    opacity: 1;
    transform: translate(-50%, -50%) scale(1);
  }
}

@keyframes dir-browser-zoom-out {
  from {
    opacity: 1;
    transform: translate(-50%, -50%) scale(1);
  }
  to {
    opacity: 0;
    transform: translate(-50%, -50%) scale(0.97);
  }
}

.dir-browser__header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: var(--space-3) var(--space-4);
  border-bottom: 1px solid var(--color-bg-border);
  flex-shrink: 0;
}

.dir-browser__title {
  font-size: var(--text-lg);
  font-weight: var(--weight-semibold);
  color: var(--color-text-primary);
}

.dir-browser__close {
  width: 24px;
  height: 24px;
}

.dir-browser__controls {
  display: flex;
  gap: var(--space-2);
  padding: var(--space-3) var(--space-4);
  flex-shrink: 0;
}

.dir-browser__path {
  flex: 1;
  min-width: 0;
  height: 30px;
  padding: 0 var(--space-2);
  background: var(--color-bg-elevated);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-md);
  color: var(--color-text-primary);
  font-family: var(--font-mono);
  font-size: var(--text-base);
}

.dir-browser__path:focus {
  outline: none;
  border-color: var(--color-accent);
  box-shadow: 0 0 0 3px color-mix(in srgb, var(--color-accent) 20%, transparent);
}

.dir-browser__path:disabled {
  opacity: 0.6;
}

.dir-browser__go,
.dir-browser__hidden {
  flex-shrink: 0;
  height: 30px;
  padding: 0 var(--space-3);
  font-size: var(--text-sm);
}

/* toggle 激活态:accent-muted 底 + accent-text 字(填充 500/文字 400
   规则 —— accent 当字用 400 档 token)。 */
.dir-browser__hidden--on {
  background: var(--color-accent-muted);
  color: var(--color-accent-text);
  border-color: var(--color-accent-muted);
}

.dir-browser__list {
  flex: 1;
  min-height: 200px;
  max-height: 46vh;
  overflow-y: auto;
  margin: 0 var(--space-4);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-md);
  background: var(--color-bg-app);
  padding: var(--space-1);
}

.dir-browser__row {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  width: 100%;
  padding: 0 var(--space-2);
  height: 32px;
  border: none;
  border-radius: var(--radius-sm);
  background: transparent;
  color: var(--color-text-primary);
  font-size: var(--text-base);
  text-align: left;
  cursor: pointer;
  transition: background var(--duration-fast) var(--ease-out);
}

.dir-browser__row:hover:not(:disabled) {
  background: var(--color-bg-hover);
}

.dir-browser__row:disabled {
  opacity: 0.6;
  cursor: default;
}

.dir-browser__row--up .dir-browser__row-name {
  color: var(--color-text-secondary);
}

.dir-browser__row-icon {
  flex-shrink: 0;
  color: var(--color-accent-text);
}

.dir-browser__row-name {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.dir-browser__error {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  padding: var(--space-2);
  color: var(--color-tool-error-text);
  font-size: var(--text-sm);
  word-break: break-all;
}

.dir-browser__empty {
  padding: var(--space-4);
  text-align: center;
  color: var(--color-text-muted);
  font-size: var(--text-sm);
}

.dir-browser__loading {
  display: flex;
  justify-content: center;
  padding: var(--space-3);
}

.dir-browser__footer {
  display: flex;
  justify-content: flex-end;
  gap: var(--space-2);
  padding: var(--space-3) var(--space-4) var(--space-4);
  flex-shrink: 0;
}
</style>
