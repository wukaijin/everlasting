<script setup lang="ts">
// DirBrowserModal — browser-mode 目录浏览模态框("浏览文件夹")。
// Tauri 原生选目录对话框在 httpTransport 下不可用(pick_project_dir
// 无 daemon route),此前 web 模式降级为一条内联手输路径框;本模态框
// 提供点击点选目录的交互(数据源 `POST /api/v1/projects/browse_dir`),
// 同时保留路径输入 + 前往,覆盖原手动输入场景。
//
// 交互契约(对照 GUI 原生 picker,projects store 注释):
//   - 单击目录行 → 进入该目录;".." 行 / 「上一步」 → 上一级。
//   - 路径输入 + 「前往」 → 直接跳转(支持 ~ 展开,后端负责)。
//   - 「显示隐藏目录」 toggle → 以 showHidden 重取当前目录。
//   - 「选择此目录」 → store.addProjectByPath(当前目录) —— 与原生
//     picker 成功路径相同的注册尾巴(去重 / unhide / create),模态框
//     由 store 关闭。
//
// 组合方式镜像 MemoryModal(reka-ui DialogRoot/Portal/Overlay/
// Content),挂在 AppShell(store 状态驱动,ProjectTabs「+」与
// EmptyProjectState「添加项目」的 browser degrade 都会翻开它)。

import { ref, watch } from "vue";
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

/** 导航到指定目录。失败时横幅报错并保留上一次列表(行内重试),
 *  「选择此目录」/「上一步」随之禁用,与原生对话框「路径无效就停在
 *  原地」的体感一致。 */
async function navigate(path: string): Promise<void> {
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

function onUp(): void {
  if (parentPath.value) void navigate(parentPath.value);
}

function onToggleHidden(): void {
  showHidden.value = !showHidden.value;
  if (currentPath.value) void navigate(currentPath.value);
}

function onSelect(): void {
  if (!currentPath.value || busy.value) return;
  void store.addProjectByPath(currentPath.value);
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

        <div class="dir-browser__list">
          <div v-if="error" class="dir-browser__error" role="alert">
            <Icon name="warn" :size="14" />
            <span>{{ error }}</span>
          </div>
          <button
            v-if="parentPath"
            type="button"
            class="dir-browser__row dir-browser__row--up"
            :disabled="busy"
            title="上一级目录"
            @click="onUp"
          >
            <Icon name="folder" :size="15" icon-class="dir-browser__row-icon" />
            <span class="dir-browser__row-name">..</span>
          </button>
          <button
            v-for="e in entries"
            :key="e.path"
            type="button"
            class="dir-browser__row"
            :disabled="busy"
            :title="e.path"
            @click="navigate(e.path)"
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
            @click="onUp"
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
