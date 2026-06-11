<script setup lang="ts">
// MemoryPreview — top-level panel for the B5 memory preview UI.
//
// Used in two contexts:
//   1. Settings page "Memory" tab — `kind="user"` filter (the
//      settings page is project-agnostic, so it shows the 2 User
//      layers only).
//   2. ProjectTabs "Memory" dropdown — `kind="project"` filter
//      for the currently-active project (shows the 2 Project
//      layers for that project).
//
// The panel auto-loads on mount and on `kind` / `projectId`
// changes. Layers are loaded via the shared `useMemoryStore`
// (so the project tab and the settings tab share the same
// in-memory cache — switching tabs doesn't re-fetch).
//
// Failure policy: if `read_memory_layers` fails (e.g. backend
// down), the panel renders "Memory 暂不可用" and does not
// throw. The Rust side already returns a structured error;
// the store catches it and exposes it via `store.error`.

import { computed, onMounted, ref, watch } from "vue";

import { useMemoryStore, type MemoryKind } from "../../stores/memory";
import { useProjectsStore } from "../../stores/projects";
import MemoryLayerItem from "./MemoryLayerItem.vue";
import Icon from "../Icon.vue";

const props = withDefaults(
  defineProps<{
    /** Which layer to show. "user" for the Settings page, "project"
     *  for the ProjectTabs dropdown. "all" (default for tests) shows
     *  every loaded layer. */
    kind?: MemoryKind | "all";
    /** Project id for the Project layer; ignored for "user". When
     *  `null` (e.g. Settings page), the project is read from
     *  `useProjectsStore().currentProjectId` — the project layers
     *  are scoped to the active project. */
    projectId?: string | null;
  }>(),
  { kind: "all", projectId: null },
);

const store = useMemoryStore();
const projectsStore = useProjectsStore();

// Which project id to actually query. The ProjectTabs dropdown
// always passes its own `projectId`; the Settings page omits it
// and falls back to the active project (so opening Settings from
// project A's dropdown still shows project A's CLAUDE.md /
// AGENTS.md).
const effectiveProjectId = computed<string | null>(() => {
  if (props.projectId) return props.projectId;
  return projectsStore.currentProjectId;
});

const visibleLayers = computed(() => {
  if (props.kind === "all") return store.layers;
  return store.layersOfKind(props.kind);
});

// Count of loaded vs missing layers (for the header chip).
const loadedCount = computed<number>(
  () => visibleLayers.value.filter((l) => l.status.kind === "loaded").length,
);
const missingCount = computed<number>(
  () => visibleLayers.value.filter((l) => l.status.kind === "missing").length,
);
const errorCount = computed<number>(
  () => visibleLayers.value.filter((l) => l.status.kind === "error").length,
);

// Loading state for the initial fetch. Subsequent refreshes flip
// `store.loading` but the layers stay visible (with a small
// "刷新中" hint).
const initialLoading = ref<boolean>(false);

// Initial load. Triggered on mount and whenever the effective
// project id changes (project switch in the ProjectTabs dropdown).
async function load() {
  const pid = effectiveProjectId.value;
  if (!pid) {
    // No project to query — show the empty state, not an error.
    // (The Settings page calls us with no project id only when
    // there's no active project at all.)
    return;
  }
  if (store.layers.length === 0) {
    initialLoading.value = true;
  }
  try {
    await store.loadForProject(pid);
  } finally {
    initialLoading.value = false;
  }
}

onMounted(() => {
  void load();
});

watch(
  () => effectiveProjectId.value,
  () => {
    void load();
  },
);

async function onRefresh() {
  await store.refresh();
}

async function onOpenEditor(path: string) {
  try {
    await store.openInEditor(path);
  } catch (e) {
    // The Rust side already returns a structured error string
    // (e.g. "open_memory_in_editor: project '...' not found").
    // We surface it via the same `error` ref the fetch path uses
    // so the panel renders a single error banner.
    store.error = String(e);
  }
}

// Header title. Slightly different wording per entry point.
const headerTitle = computed<string>(() => {
  if (props.kind === "user") return "用户指令文件";
  if (props.kind === "project") return "项目指令文件";
  return "指令文件";
});

const headerHint = computed<string>(() => {
  if (props.kind === "user") {
    return "~/.config/everlasting/CLAUDE.md + AGENTS.md(全局,所有项目可见)";
  }
  if (props.kind === "project") {
    return "项目根目录下的 CLAUDE.md + AGENTS.md(仅本项目可见)";
  }
  return "用户 + 项目,共 4 个指令文件";
});
</script>

<template>
  <div class="memory-preview">
    <header class="memory-preview__header">
      <div class="memory-preview__header-left">
        <h3 class="memory-preview__title">{{ headerTitle }}</h3>
        <p class="memory-preview__hint">{{ headerHint }}</p>
      </div>
      <div class="memory-preview__header-right">
        <span v-if="visibleLayers.length > 0" class="memory-preview__chip">
          <span class="memory-preview__chip-loaded">{{ loadedCount }} loaded</span>
          <span v-if="missingCount > 0" class="memory-preview__chip-missing">
            · {{ missingCount }} missing
          </span>
          <span v-if="errorCount > 0" class="memory-preview__chip-error">
            · {{ errorCount }} error
          </span>
        </span>
        <button
          class="memory-preview__refresh"
          type="button"
          :disabled="store.loading || !effectiveProjectId"
          @click="onRefresh"
        >
          <Icon name="refresh" :size="12" />
          <span>刷新</span>
        </button>
      </div>
    </header>

    <div v-if="store.error" class="memory-preview__error">
      <Icon name="warn" :size="14" />
      <span>指令文件暂不可用:{{ store.error }}</span>
    </div>

    <div
      v-if="!effectiveProjectId"
      class="memory-preview__empty"
    >
      <p>请先选择一个项目以查看指令文件。</p>
    </div>

    <div
      v-else-if="initialLoading && visibleLayers.length === 0"
      class="memory-preview__loading"
    >
      加载指令文件中…
    </div>

    <div
      v-else-if="visibleLayers.length === 0"
      class="memory-preview__empty"
    >
      <p>该层下没有指令文件。</p>
    </div>

    <div v-else class="memory-preview__list">
      <MemoryLayerItem
        v-for="layer in visibleLayers"
        :key="layer.path"
        :layer="layer"
        @open-editor="onOpenEditor"
      />
    </div>

    <footer class="memory-preview__footer">
      <p>
        指令文件每 <strong>1 秒</strong> 自动监听变更;
        新建文件需重启 session 生效。
        详细规范见
        <code>docs/IMPLEMENTATION.md</code> §4(B5 决策)。
      </p>
    </footer>
  </div>
</template>

<style scoped>
.memory-preview {
  display: flex;
  flex-direction: column;
  gap: 12px;
  width: 100%;
}

.memory-preview__header {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 12px;
  padding-bottom: 8px;
  border-bottom: 1px solid var(--color-bg-border);
}

.memory-preview__header-left {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
}

.memory-preview__title {
  margin: 0;
  font-size: 14px;
  font-weight: 600;
  color: var(--color-text-primary);
}

.memory-preview__hint {
  margin: 0;
  font-size: 11px;
  color: var(--color-text-muted);
  font-family: var(--font-mono);
}

.memory-preview__header-right {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-shrink: 0;
}

.memory-preview__chip {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  font-size: 11px;
  font-family: var(--font-mono);
  color: var(--color-text-muted);
}

.memory-preview__chip-loaded {
  color: #4ade80;
}

.memory-preview__chip-missing {
  color: var(--color-text-muted);
}

.memory-preview__chip-error {
  color: #fbbf24;
}

.memory-preview__refresh {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 4px 8px;
  background: var(--color-bg-elevated);
  border: 1px solid var(--color-bg-border);
  border-radius: 4px;
  color: var(--color-text-primary);
  font-size: 11px;
  font-family: inherit;
  cursor: pointer;
  transition: border-color 0.15s, background 0.15s;
}

.memory-preview__refresh:hover:not(:disabled) {
  background: var(--color-accent-muted);
  border-color: var(--color-accent);
}

.memory-preview__refresh:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}

.memory-preview__error {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 12px;
  background: color-mix(in srgb, var(--color-tool-error) 10%, transparent);
  border: 1px solid color-mix(in srgb, var(--color-tool-error) 40%, transparent);
  border-radius: 6px;
  color: var(--color-tool-error);
  font-size: 12px;
}

.memory-preview__empty,
.memory-preview__loading {
  padding: 24px;
  text-align: center;
  color: var(--color-text-muted);
  font-size: 13px;
}

.memory-preview__list {
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.memory-preview__footer {
  margin-top: 4px;
  padding-top: 8px;
  border-top: 1px solid var(--color-bg-border);
  color: var(--color-text-muted);
  font-size: 11px;
}

.memory-preview__footer p {
  margin: 0;
  line-height: 1.5;
}

.memory-preview__footer code {
  background: var(--color-bg-elevated);
  border: 1px solid var(--color-bg-border);
  border-radius: 3px;
  padding: 0 4px;
  font-family: var(--font-mono);
  font-size: 10px;
}
</style>
