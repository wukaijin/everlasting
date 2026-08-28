<script setup lang="ts">
// SettingsModal — 设置弹窗壳(2026-08-29 settings-shell 重构)。
//
// 结构(借鉴 VS Code 设置窗,非照抄):标题行 + 工具行(搜索输入 +
// 「全局 | 项目」scope 分段控件)+ 两栏主体(左侧分组导航,右侧
// 内容区)。分类元数据与搜索过滤在 ./registry(纯数据 + 纯函数);
// 每个分类渲染一个自包含内容组件(原 tab 组件原样复用)。
//
// scope 语义:全局 = daemon 级 app_config / 目录表设置;项目 = 真正
// 项目级的数据(项目指令文件、项目子代理定义),由壳内的项目选择器
// 指定目标项目,projectId 以 prop 下发给项目分类组件。
//
// 交互细节:
// - 搜索在当前 scope 内按 title / description / keywords 过滤导航;
//   当前 scope 无结果而另一 scope 有时,给一键切换提示。
// - 上次停留的分类经 localStorage(everlasting.settingsNav)记忆,
//   打开时恢复;失效 id 回退「通用」。
// - reka-ui DialogRoot/Portal/Overlay/Content 沿用原壳(overlay +
//   focus trap + Esc),类名保持 `settings-modal` 以吃进 style.css
//   的移动端全屏块与 44px 触摸目标块。

import { computed, ref, watch, type Component } from "vue";
import {
  DialogRoot,
  DialogPortal,
  DialogOverlay,
  DialogContent,
  DialogTitle,
  DialogClose,
  SelectRoot,
  SelectTrigger,
  SelectValue,
  SelectIcon,
  SelectPortal,
  SelectContent,
  SelectViewport,
  SelectItem,
  SelectItemText,
} from "reka-ui";
import Icon from "../Icon.vue";
import { useProjectsStore, type ProjectInfo } from "../../stores/projects";
import {
  DEFAULT_CATEGORY_ID,
  categoriesForScope,
  filterCategories,
  findCategory,
  groupCategories,
  type SettingsCategory,
  type SettingsScope,
} from "./registry";
import ProvidersTab from "./ProvidersTab.vue";
import ModelsTab from "./ModelsTab.vue";
import DefaultTab from "./DefaultTab.vue";
import MemoryTab from "./MemoryTab.vue";
import SubagentsTab from "./SubagentsTab.vue";
import RemoteTab from "./RemoteTab.vue";
import SearchTab from "./SearchTab.vue";
import ScheduledTasksTab from "./ScheduledTasksTab.vue";
import GeneralTab from "./GeneralTab.vue";
import ProjectMemoryTab from "./ProjectMemoryTab.vue";
import ProjectSubagentsTab from "./ProjectSubagentsTab.vue";

const open = defineModel<boolean>("open", { required: true });

const projectsStore = useProjectsStore();

// ---------------------------------------------------------------------------
// 分类 → 内容组件映射。registry 保持纯数据,组件(非序列化)留在这。
// ---------------------------------------------------------------------------
const CATEGORY_COMPONENTS: Record<string, Component> = {
  general: GeneralTab,
  providers: ProvidersTab,
  models: ModelsTab,
  default: DefaultTab,
  memory: MemoryTab,
  subagents: SubagentsTab,
  search: SearchTab,
  scheduled: ScheduledTasksTab,
  remote: RemoteTab,
  "project-memory": ProjectMemoryTab,
  "project-subagents": ProjectSubagentsTab,
};

// ---------------------------------------------------------------------------
// 导航状态:scope + 分类 + 搜索词。打开弹窗时恢复上次停留位置。
// ---------------------------------------------------------------------------
const NAV_STORAGE_KEY = "everlasting.settingsNav";

interface SavedNav {
  scope: SettingsScope;
  id: string;
}

function readSavedNav(): SavedNav | null {
  try {
    const raw = window.localStorage.getItem(NAV_STORAGE_KEY);
    if (!raw) return null;
    const parsed = JSON.parse(raw) as Partial<SavedNav>;
    if ((parsed.scope !== "global" && parsed.scope !== "project")) return null;
    if (typeof parsed.id !== "string" || findCategory(parsed.id)?.scope !== parsed.scope) {
      return null;
    }
    return { scope: parsed.scope, id: parsed.id };
  } catch {
    return null;
  }
}

function persistNav(): void {
  try {
    window.localStorage.setItem(
      NAV_STORAGE_KEY,
      JSON.stringify({ scope: scope.value, id: activeId.value }),
    );
  } catch {
    // localStorage 不可用(隐私模式等)——静默降级为不记忆。
  }
}

const scope = ref<SettingsScope>("global");
const activeId = ref<string>(DEFAULT_CATEGORY_ID);
const query = ref("");

watch(open, (isOpen) => {
  if (!isOpen) return;
  query.value = "";
  const saved = readSavedNav();
  if (saved) {
    scope.value = saved.scope;
    activeId.value = saved.id;
  } else {
    scope.value = "global";
    activeId.value = DEFAULT_CATEGORY_ID;
  }
  // 项目选择器:默认跟当前活跃项目,失效/缺失回退第一个可见项目。
  if (!visibleProjects.value.some((p) => p.id === selectedProjectId.value)) {
    selectedProjectId.value =
      projectsStore.currentProjectId ?? visibleProjects.value[0]?.id ?? null;
  }
  // 选择器依赖项目列表;AppShell 启动即拉,这里只兜底空列表。
  if (projectsStore.projects.length === 0) {
    projectsStore.loadProjects().catch(() => {
      /* 选择器为空即可见,项目 scope 内容自会显示空态 */
    });
  }
});

// ---------------------------------------------------------------------------
// 派生:过滤后的导航 / 分组视图 / 活动分类 / 跨 scope 提示。
// ---------------------------------------------------------------------------
const visibleCats = computed<ReadonlyArray<SettingsCategory>>(() =>
  filterCategories(query.value, scope.value),
);

const navGroups = computed(() => groupCategories(visibleCats.value));

const activeCategory = computed<SettingsCategory | undefined>(() => findCategory(activeId.value));

const otherScope = computed<SettingsScope>(() => (scope.value === "global" ? "project" : "global"));
const otherScopeMatches = computed(
  () => filterCategories(query.value, otherScope.value).length,
);

function selectCategory(cat: SettingsCategory): void {
  activeId.value = cat.id;
  query.value = "";
  persistNav();
}

function switchScope(next: SettingsScope): void {
  if (scope.value === next) return;
  scope.value = next;
  query.value = "";
  if (findCategory(activeId.value)?.scope !== next) {
    activeId.value = categoriesForScope(next)[0]?.id ?? DEFAULT_CATEGORY_ID;
  }
  persistNav();
}

// ---------------------------------------------------------------------------
// 项目 scope:项目选择器 + 下发给项目分类组件的 projectId。
// ---------------------------------------------------------------------------
const visibleProjects = computed<ProjectInfo[]>(() =>
  projectsStore.projects.filter((p) => !p.hidden),
);
const selectedProjectId = ref<string | null>(projectsStore.currentProjectId);

/** 项目列表晚于弹窗打开到达(AppShell 异步 loadProjects)时补默认值:
 *  当前选中失效/为空 → 跟当前活跃项目,再退第一个可见项目。 */
watch(visibleProjects, (list) => {
  if (!open.value || list.length === 0) return;
  if (!list.some((p) => p.id === selectedProjectId.value)) {
    selectedProjectId.value =
      projectsStore.currentProjectId ?? list[0]?.id ?? null;
  }
});

const selectedProject = computed<ProjectInfo | undefined>(() =>
  visibleProjects.value.find((p) => p.id === selectedProjectId.value),
);

function onSelectProject(value: string | string[] | undefined): void {
  const v = Array.isArray(value) ? value[0] : value;
  selectedProjectId.value = typeof v === "string" && v ? v : null;
}

const contentComponent = computed<Component | undefined>(() =>
  activeCategory.value ? CATEGORY_COMPONENTS[activeCategory.value.id] : undefined,
);

/** 只有项目分类接 projectId;全局分类组件无 props,避免多余 attr
 *  透传到根元素。 */
const contentProps = computed<Record<string, unknown>>(() =>
  activeCategory.value?.scope === "project" ? { projectId: selectedProjectId.value } : {},
);
</script>

<template>
  <DialogRoot v-model:open="open">
    <DialogPortal>
      <DialogOverlay class="settings-modal__overlay" />
      <DialogContent
        class="settings-modal"
        :aria-describedby="undefined"
        @pointerdown-outside="open = false"
      >
        <header class="settings-modal__header">
          <DialogTitle class="settings-modal__title">Settings</DialogTitle>
          <DialogClose as-child>
            <button type="button" class="settings-modal__close btn btn--icon btn--ghost" aria-label="Close">
              <Icon name="x" :size="14" />
              <!-- S6b: 移动端语义化关闭文案(桌面 display:none) -->
              <span class="settings-modal__close-label">Done</span>
            </button>
          </DialogClose>
        </header>

        <!-- 工具行:搜索 + scope 分段 -->
        <div class="settings-modal__toolbar">
          <div class="settings-modal__search">
            <Icon name="magnifying-glass" :size="14" />
            <input
              v-model="query"
              type="text"
              placeholder="搜索设置…"
              aria-label="搜索设置"
            />
          </div>
          <div class="settings-modal__scope" role="group" aria-label="设置范围">
            <button
              type="button"
              class="settings-modal__scope-btn"
              :class="{ 'settings-modal__scope-btn--active': scope === 'global' }"
              :aria-pressed="scope === 'global'"
              @click="switchScope('global')"
            >
              全局
            </button>
            <button
              type="button"
              class="settings-modal__scope-btn"
              :class="{ 'settings-modal__scope-btn--active': scope === 'project' }"
              :aria-pressed="scope === 'project'"
              @click="switchScope('project')"
            >
              项目
            </button>
          </div>
        </div>

        <!-- 主体:左导航 + 右内容 -->
        <div class="settings-modal__body">
          <nav class="settings-modal__nav" aria-label="设置分类">
            <template v-for="group in navGroups" :key="group.label ?? '__standalone__'">
              <div v-if="group.label" class="settings-modal__group-label">
                {{ group.label }}
              </div>
              <button
                v-for="cat in group.items"
                :key="cat.id"
                type="button"
                class="settings-modal__nav-item"
                :class="{
                  'settings-modal__nav-item--active': cat.id === activeId,
                }"
                :aria-current="cat.id === activeId ? 'page' : undefined"
                @click="selectCategory(cat)"
              >
                {{ cat.title }}
              </button>
            </template>
            <div v-if="visibleCats.length === 0" class="settings-modal__nav-empty">
              <p>没有匹配「{{ query }}」的设置</p>
              <p v-if="otherScopeMatches > 0">
                {{ otherScope === "project" ? "项目" : "全局" }}范围有
                {{ otherScopeMatches }} 个匹配
                <button type="button" class="settings-modal__nav-switch" @click="switchScope(otherScope)">
                  切换过去
                </button>
              </p>
            </div>
          </nav>

          <section class="settings-modal__content">
            <header
              v-if="activeCategory"
              class="settings-modal__content-header"
            >
              <div class="settings-modal__content-heading">
                <h2 class="settings-modal__content-title">{{ activeCategory.title }}</h2>
                <p class="settings-modal__content-desc">{{ activeCategory.description }}</p>
              </div>
              <SelectRoot
                v-if="scope === 'project'"
                :model-value="selectedProjectId ?? undefined"
                @update:model-value="onSelectProject"
              >
                <SelectTrigger class="settings-modal__picker" aria-label="选择项目">
                  <SelectValue>{{ selectedProject?.name ?? "选择项目" }}</SelectValue>
                  <SelectIcon class="settings-modal__picker-icon">
                    <Icon name="chevron-down" :size="12" />
                  </SelectIcon>
                </SelectTrigger>
                <SelectPortal>
                  <SelectContent
                    class="settings-modal__picker-content"
                    position="popper"
                    :side-offset="4"
                    align="end"
                  >
                    <SelectViewport class="settings-modal__picker-viewport">
                      <SelectItem
                        v-for="p in visibleProjects"
                        :key="p.id"
                        :value="p.id"
                        class="settings-modal__picker-option"
                      >
                        <SelectItemText>
                          <span class="settings-modal__picker-name">{{ p.name }}</span>
                          <span class="settings-modal__picker-path">{{ p.path }}</span>
                        </SelectItemText>
                      </SelectItem>
                    </SelectViewport>
                  </SelectContent>
                </SelectPortal>
              </SelectRoot>
            </header>

            <div class="settings-modal__content-body">
              <component :is="contentComponent" v-bind="contentProps" />
            </div>
          </section>
        </div>
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
  z-index: var(--z-modal-overlay);
}

.settings-modal {
  position: fixed;
  top: 50%;
  left: 50%;
  transform: translate(-50%, -50%);
  width: min(1000px, calc(100vw - 48px));
  height: min(700px, 85vh);
  background: var(--color-bg-surface);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-lg);
  display: flex;
  flex-direction: column;
  overflow: hidden;
  box-shadow: var(--shadow-xl);
  z-index: var(--z-modal);
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

/* --- 标题行 --- */

.settings-modal__header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 10px var(--space-4);
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

/* 按钮样式由全局 .btn 家族承载(close = ghost icon)。 */

.settings-modal__close-label {
  display: none;
}

/* --- 工具行:搜索 + scope 分段 --- */

.settings-modal__toolbar {
  display: flex;
  align-items: center;
  gap: var(--space-3);
  padding: var(--space-2) var(--space-4);
  border-bottom: 1px solid var(--color-bg-border);
  background: var(--color-bg-elevated);
  flex-shrink: 0;
}

.settings-modal__search {
  flex: 1;
  min-width: 0;
  display: flex;
  align-items: center;
  gap: var(--space-2);
  background: var(--color-bg-app);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-md);
  padding: 6px 10px;
  color: var(--color-text-muted);
  transition:
    border-color var(--duration-base) var(--ease-out),
    box-shadow var(--duration-base) var(--ease-out);
}

.settings-modal__search:focus-within {
  border-color: var(--color-accent);
  box-shadow: var(--shadow-ring);
}

.settings-modal__search input {
  flex: 1;
  min-width: 0;
  background: transparent;
  border: 0;
  outline: none;
  color: var(--color-text-primary);
  font-size: var(--text-base);
  font-family: inherit;
}

.settings-modal__search input::placeholder {
  color: var(--color-text-muted);
}

.settings-modal__scope {
  display: flex;
  flex-shrink: 0;
  background: var(--color-bg-app);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-md);
  padding: 2px;
}

.settings-modal__scope-btn {
  border: 0;
  background: transparent;
  color: var(--color-text-muted);
  font-size: var(--text-sm);
  font-family: inherit;
  padding: 4px 12px;
  border-radius: var(--radius-sm);
  cursor: pointer;
  transition:
    background var(--duration-fast) var(--ease-out),
    color var(--duration-fast) var(--ease-out);
}

.settings-modal__scope-btn:hover {
  color: var(--color-text-secondary);
}

.settings-modal__scope-btn--active {
  background: var(--color-accent-muted);
  color: var(--color-accent-text);
}

/* --- 主体两栏 --- */

.settings-modal__body {
  flex: 1;
  display: flex;
  min-height: 0;
}

.settings-modal__nav {
  width: 220px;
  flex-shrink: 0;
  display: flex;
  flex-direction: column;
  gap: 2px;
  padding: var(--space-3) var(--space-2);
  border-right: 1px solid var(--color-bg-border);
  background: var(--color-bg-app);
  overflow-y: auto;
}

.settings-modal__group-label {
  font-family: var(--font-mono);
  font-size: var(--text-xs);
  text-transform: uppercase;
  letter-spacing: 0.06em;
  color: var(--color-text-muted);
  padding: var(--space-2) var(--space-2) var(--space-1);
}

/* 组间留白:非首个组标签上抬一档(独立首项「通用」后紧跟组头)。 */
.settings-modal__group-label:not(:first-child) {
  margin-top: var(--space-2);
}

.settings-modal__nav-item {
  text-align: left;
  border: 0;
  background: transparent;
  color: var(--color-text-secondary);
  font-size: var(--text-sm);
  font-family: inherit;
  padding: 6px 10px;
  border-radius: var(--radius-md);
  cursor: pointer;
  transition:
    background var(--duration-fast) var(--ease-out),
    color var(--duration-fast) var(--ease-out);
}

.settings-modal__nav-item:hover {
  background: var(--color-bg-hover);
  color: var(--color-text-primary);
}

.settings-modal__nav-item--active {
  background: var(--color-bg-selected);
  color: var(--color-accent-text);
}

.settings-modal__nav-empty {
  padding: var(--space-2);
  font-size: var(--text-sm);
  color: var(--color-text-muted);
  line-height: var(--leading-relaxed);
}

.settings-modal__nav-empty p {
  margin: 0 0 var(--space-1);
}

.settings-modal__nav-switch {
  background: none;
  border: 0;
  padding: 0;
  color: var(--color-accent-text);
  font-size: var(--text-sm);
  font-family: inherit;
  cursor: pointer;
  text-decoration: underline;
}

/* --- 右侧内容区 --- */

.settings-modal__content {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  background: var(--color-bg-app);
}

.settings-modal__content-header {
  display: flex;
  align-items: flex-end;
  justify-content: space-between;
  gap: var(--space-3);
  padding: var(--space-4) var(--space-5) var(--space-3);
  border-bottom: 1px solid var(--color-bg-border);
  flex-shrink: 0;
}

.settings-modal__content-title {
  margin: 0;
  font-size: var(--text-lg);
  font-weight: var(--weight-semibold);
  color: var(--color-text-primary);
}

.settings-modal__content-desc {
  margin: var(--space-1) 0 0;
  font-size: var(--text-sm);
  color: var(--color-text-secondary);
}

.settings-modal__content-body {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  padding: var(--space-4) var(--space-5);
}

/* --- 项目选择器(reka Select;portal 子元素 :deep()) --- */

.settings-modal__picker {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  max-width: 280px;
  padding: 5px 10px;
  background: var(--color-bg-surface);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-sm);
  color: var(--color-text-primary);
  font-size: var(--text-sm);
  font-family: inherit;
  cursor: pointer;
  transition:
    border-color var(--duration-base) var(--ease-out),
    background var(--duration-base) var(--ease-out);
}

.settings-modal__picker:hover {
  border-color: var(--color-accent-muted);
}

.settings-modal__picker[data-state="open"] {
  border-color: var(--color-accent);
  box-shadow: var(--shadow-ring);
}

.settings-modal__picker-icon {
  color: var(--color-text-muted);
  display: inline-flex;
  align-items: center;
  flex-shrink: 0;
}

:deep(.settings-modal__picker-content) {
  /* 注意:不要写 position:fixed —— reka 的 popper 包裹层(自带 fixed)
     承载定位,内容元素若自行 fixed 会脱离包裹流,包裹层塌缩 0×0,
     floating-ui 按零宽算对齐/碰撞 → align="end" 失效、面板向右溢出
     视口(2026-08-29 实测;left=触发器右缘=1199)。 */
  background: var(--color-bg-surface);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-md);
  box-shadow: var(--shadow-md);
  /* 触发器只显示项目名(~100px),照搬 trigger-width 会把带 mono 路径的
     选项裁掉(2026-08-29 用户截图实证);最小宽取 max(触发器, 320px)
     让路径 + 省略号有空间。align="end" 让面板贴触发器右缘向左展开。 */
  min-width: max(var(--reka-select-trigger-width, 240px), 320px);
  max-height: var(--reka-select-content-available-height);
  z-index: var(--z-over-modal) !important;
  overflow: hidden;
}

:deep(.settings-modal__picker-viewport) {
  padding: 4px;
  max-height: var(--reka-select-content-available-height);
}

:deep(.settings-modal__picker-option) {
  display: flex;
  align-items: center;
  padding: 6px 10px;
  font-size: var(--text-sm);
  color: var(--color-text-primary);
  border-radius: var(--radius-sm);
  cursor: pointer;
  user-select: none;
  line-height: var(--leading-normal);
}

:deep(.settings-modal__picker-option[data-highlighted]) {
  background: var(--color-bg-elevated);
  color: var(--color-text-primary);
}

:deep(.settings-modal__picker-option[data-state="checked"]) {
  color: var(--color-accent-text);
}

:deep(.settings-modal__picker-name) {
  display: block;
  color: var(--color-text-primary);
}

:deep(.settings-modal__picker-path) {
  display: block;
  font-family: var(--font-mono);
  font-size: var(--text-2xs);
  color: var(--color-text-muted);
  max-width: 100%;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

/* --- S6b 移动端适配(<768px,320-430px) ---
 * 容器全屏化 + 44px 触摸目标由 style.css 全局块覆盖(类名保持
 * `settings-modal` 自动吃进)。这里把左侧导航降级为横向滚动分类
 * chip 条(复用旧 tab strip 的 mask 渐变 + pill 高亮方案)。 */
@media (max-width: 767px) {
  /* 两栏 → 上下堆叠:body 保持 row 会把横向 chip 条当成一个占满
     整行、垂直拉伸的 flex 子项(2026-08-29 真机截图实证:chip 居中
     悬浮 + 内容区被压成 0 宽),必须显式翻成 column。 */
  .settings-modal__body {
    flex-direction: column;
  }

  /* 全屏化补丁:style.css 全局移动端块的 height/max-height 带
     var(--app-height) 但**无 !important**,会被本组件桌面块的
     `height: min(700px, 85vh)`(.settings-modal[data-v] 特异性更高)
     压住 → 移动端实际截在 700px(2026-08-29 截图实证)。同选择器
     源序靠后覆盖回全屏(旧 8-tab 版对 max-height 的同款处理,
     08-13-mobile-settings 真机迭代先例)。 */
  .settings-modal {
    height: var(--app-height);
    max-height: var(--app-height);
  }

  .settings-modal__toolbar {
    flex-wrap: wrap;
  }

  /* 搜索优先占满一行;scope 分段落到第二行右侧(空间够则同行)。 */
  .settings-modal__search {
    flex-basis: 100%;
    order: 1;
  }

  .settings-modal__scope {
    order: 0;
    margin-left: auto;
  }

  .settings-modal__nav {
    flex-direction: row;
    align-items: center;
    width: 100%;
    flex: none;
    gap: var(--space-1);
    padding: var(--space-2);
    border-right: 0;
    border-bottom: 1px solid var(--color-bg-border);
    overflow-x: auto;
    overflow-y: hidden;
    scrollbar-width: none; /* 隐藏滚动条(仍可触控滑) */
    -webkit-mask-image: linear-gradient(to right, transparent 0%, #000 8%, #000 92%, transparent 100%);
    mask-image: linear-gradient(to right, transparent 0%, #000 8%, #000 92%, transparent 100%);
  }

  .settings-modal__group-label {
    display: none;
  }

  .settings-modal__nav-item {
    flex-shrink: 0;
    white-space: nowrap;
  }

  /* 窄屏 pill 高亮(对比度优于背景 tint,旧 tab strip 同款)。 */
  .settings-modal__nav-item--active {
    background: var(--color-accent-muted);
    color: var(--color-accent-text);
  }

  .settings-modal__nav-empty {
    flex-shrink: 0;
    white-space: nowrap;
  }

  .settings-modal__content-header {
    flex-direction: column;
    align-items: stretch;
    gap: var(--space-2);
  }

  .settings-modal__picker {
    max-width: none;
  }

  .settings-modal__content-body {
    padding: var(--space-3) var(--space-4);
  }

  /* 真机迭代(2026-08-13 遗产):全局块 height: var(--app-height)
     已接管,无额外覆盖需求。 */
}
</style>
