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
//
// P2 PR3: extends the panel with a "Runtime Memories" / 自主记忆
// section, sourcing from `store.runtimeMemories` (the P2 autonomous
// memory list visible to the current project). The section renders
// below the instruction-file section, with its own loading / error
// / empty states and a per-row delete affordance. The instruction-
// file section is unchanged.

import { computed, onMounted, ref, watch } from "vue";

import {
  useMemoryStore,
  type AutonomousMemory,
  type MemoryKind,
} from "../../stores/memory";
import { useProjectsStore } from "../../stores/projects";
import MemoryLayerItem from "./MemoryLayerItem.vue";
import Icon from "../Icon.vue";
import ConfirmDialog from "../common/ConfirmDialog.vue";

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
    return "~/.config/everlasting/CLAUDE.md + AGENTS.md(全局，所有项目可见)";
  }
  if (props.kind === "project") {
    return "项目根目录下的 CLAUDE.md + AGENTS.md(仅本项目可见)";
  }
  return "用户 + 项目，共 4 个指令文件";
});

// ---------------------------------------------------------------------
// P2 PR3: runtime-memories section
// ---------------------------------------------------------------------

// Per-row delete confirmation slot. `null` = no row pending. The
// ID is the SQLite auto-id (matches the v-for :key), not the
// UUID `memoryId` — see `store.deleteMemory` for the resolution.
const pendingDeleteId = ref<number | null>(null);

function onDeleteClick(id: number) {
  pendingDeleteId.value = id;
}

function onDeleteCancel() {
  pendingDeleteId.value = null;
}

async function onDeleteConfirm() {
  const id = pendingDeleteId.value;
  if (id === null) return;
  pendingDeleteId.value = null;
  await store.deleteMemory(id);
}

const pendingDeleteMemory = computed<AutonomousMemory | null>(() => {
  if (pendingDeleteId.value === null) return null;
  return (
    store.runtimeMemories.find((m) => m.id === pendingDeleteId.value) ?? null
  );
});

// P2 PR3: a content preview that's at most 80 chars (mirrors the
// 500-char MAX_CONTENT_LEN cap from the Rust write safety net — a
// full preview would push the row out of the list density). Trims
// trailing whitespace + adds an ellipsis when truncated.
function contentPreview(m: AutonomousMemory): string {
  const text = m.content.trim().replace(/\s+/g, " ");
  if (text.length <= 80) return text;
  return text.slice(0, 80) + "…";
}

// P2 PR3: parse the JSON-encoded tags field for the badge row.
// Falls back to a free-form string when the field isn't valid JSON
// (defensive — the DB column is plain TEXT).
function parseTags(json: string): string[] {
  try {
    const parsed = JSON.parse(json);
    if (Array.isArray(parsed)) {
      return parsed.filter((t): t is string => typeof t === "string");
    }
  } catch {
    // fall through
  }
  return [];
}

// P2 PR3: human-readable kind label. The DB stores lowercase
// strings; the UI uses the original B5 / spike-007 nomenclature.
const kindLabel: Record<string, string> = {
  pitfall: "踩坑",
  preference: "偏好",
  fact: "事实",
  decision: "决策",
};
function kindBadgeText(kind: string): string {
  return kindLabel[kind] ?? kind;
}

// P2 PR3: human-readable scope label.
function scopeBadgeText(scope: string): string {
  return scope === "user" ? "user" : "project";
}

// P2 PR3: human-readable status label.
const statusLabel: Record<string, string> = {
  candidate: "candidate",
  active: "active",
  verified: "verified",
  demoted: "demoted",
};
function statusBadgeText(status: string): string {
  return statusLabel[status] ?? status;
}

// P2 PR3: timestamp display — strip the RFC 3339 fractional +
// timezone to a compact YYYY-MM-DD HH:MM for the row meta line.
// The DB stores RFC 3339; we keep only the wall-clock digits.
function formatTimestamp(rfc3339: string): string {
  // RFC 3339 example: "2026-06-29T12:34:56.789+00:00" → "2026-06-29 12:34"
  const m = rfc3339.match(/^(\d{4}-\d{2}-\d{2})T(\d{2}:\d{2})/);
  return m ? `${m[1]} ${m[2]}` : rfc3339;
}
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

    <!-- P2 PR3: Runtime Memories section. New, additive — the
         instruction-file section above is untouched. -->
    <section class="memory-preview__runtime" aria-labelledby="runtime-memories-title">
      <header class="memory-preview__runtime-header">
        <div class="memory-preview__runtime-header-left">
          <h3 id="runtime-memories-title" class="memory-preview__runtime-title">
            自主记忆
          </h3>
          <p class="memory-preview__runtime-hint">
            agent 通过 remember tool 写入的跨 session 记忆(user + 当前 project)
          </p>
        </div>
        <div class="memory-preview__runtime-header-right">
          <span
            v-if="store.runtimeMemories.length > 0"
            class="memory-preview__runtime-count"
          >
            {{ store.runtimeMemories.length }} 条
          </span>
          <button
            class="memory-preview__refresh"
            type="button"
            :disabled="
              store.runtimeMemoriesLoading || !effectiveProjectId
            "
            @click="store.fetchMemories()"
          >
            <Icon name="refresh" :size="12" />
            <span>刷新</span>
          </button>
        </div>
      </header>

      <div
        v-if="store.runtimeMemoriesError"
        class="memory-preview__error"
      >
        <Icon name="warn" :size="14" />
        <span>自主记忆暂不可用:{{ store.runtimeMemoriesError }}</span>
      </div>

      <div
        v-else-if="!effectiveProjectId"
        class="memory-preview__empty"
      >
        <p>请先选择一个项目以查看自主记忆。</p>
      </div>

      <div
        v-else-if="
          store.runtimeMemoriesLoading && store.runtimeMemories.length === 0
        "
        class="memory-preview__loading"
      >
        加载自主记忆中…
      </div>

      <div
        v-else-if="store.runtimeMemories.length === 0"
        class="memory-preview__empty"
      >
        <p>该项目暂无自主记忆。agent 通过 remember tool 写入后会自动出现在这里。</p>
      </div>

      <ul v-else class="memory-preview__runtime-list">
        <li
          v-for="mem in store.runtimeMemories"
          :key="mem.id"
          class="runtime-memory"
        >
          <div class="runtime-memory__main">
            <div class="runtime-memory__head">
              <span class="runtime-memory__title">{{ mem.title }}</span>
              <span
                class="runtime-memory__badge"
                :class="`runtime-memory__badge--kind-${mem.kind}`"
              >
                {{ kindBadgeText(mem.kind) }}
              </span>
              <span
                class="runtime-memory__badge"
                :class="`runtime-memory__badge--scope-${mem.scope}`"
              >
                {{ scopeBadgeText(mem.scope) }}
              </span>
              <span
                class="runtime-memory__badge"
                :class="`runtime-memory__badge--status-${mem.status}`"
              >
                {{ statusBadgeText(mem.status) }}
              </span>
            </div>
            <p class="runtime-memory__content">{{ contentPreview(mem) }}</p>
            <div class="runtime-memory__meta">
              <span class="runtime-memory__timestamp">
                {{ formatTimestamp(mem.createdAt) }}
              </span>
              <span
                v-for="tag in parseTags(mem.tags)"
                :key="tag"
                class="runtime-memory__tag"
              >
                #{{ tag }}
              </span>
            </div>
          </div>
          <button
            type="button"
            class="runtime-memory__delete"
            aria-label="删除记忆"
            title="删除记忆"
            @click="onDeleteClick(mem.id)"
          >
            <Icon name="trash" :size="12" />
          </button>
        </li>
      </ul>
    </section>

    <footer class="memory-preview__footer">
      <p>
        指令文件每 <strong>1 秒</strong> 自动监听变更;
        新建文件需重启 session 生效。
        详细规范见
        <code>docs/IMPLEMENTATION.md</code> §4(B5 决策)。
      </p>
    </footer>

    <!-- P2 PR3: delete confirmation modal. Reuses the project-wide
         ConfirmDialog primitive (see app/src/components/common/ConfirmDialog.vue).
         Renders a focused danger modal — no silent deletes. -->
    <ConfirmDialog
      :open="pendingDeleteId !== null"
      title="删除自主记忆"
      variant="danger"
      confirm-text="删除"
      @cancel="onDeleteCancel"
      @confirm="onDeleteConfirm"
    >
      <p v-if="pendingDeleteMemory">
        确认删除「<strong>{{ pendingDeleteMemory.title }}</strong>」吗?
        此操作不可撤销。
      </p>
      <p v-else>确认删除该条记忆吗?此操作不可撤销。</p>
    </ConfirmDialog>
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
  font-size: var(--text-md);
  font-weight: var(--weight-semibold);
  color: var(--color-text-primary);
}

.memory-preview__hint {
  margin: 0;
  font-size: var(--text-xs);
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
  font-size: var(--text-xs);
  font-family: var(--font-mono);
  color: var(--color-text-muted);
}

.memory-preview__chip-loaded {
  color: var(--color-status-success);
}

.memory-preview__chip-missing {
  color: var(--color-text-muted);
}

.memory-preview__chip-error {
  color: var(--color-status-warn);
}

.memory-preview__refresh {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 4px 8px;
  background: var(--color-bg-elevated);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-sm);
  color: var(--color-text-primary);
  font-size: var(--text-xs);
  font-family: inherit;
  cursor: pointer;
  transition: border-color var(--duration-base) var(--ease-out), background var(--duration-base) var(--ease-out);
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
  border-radius: var(--radius-md);
  color: var(--color-tool-error);
  font-size: var(--text-sm);
}

.memory-preview__empty,
.memory-preview__loading {
  padding: 24px;
  text-align: center;
  color: var(--color-text-muted);
  font-size: var(--text-base);
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
  font-size: var(--text-xs);
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
  font-size: var(--text-2xs);
}

/* ---------------------------------------------------------------------
   P2 PR3: Runtime Memories section. Stylistically distinct from the
   instruction-file section (bordered container + 自主记忆 label) so
   the user can see at a glance which is which.
   --------------------------------------------------------------------- */

.memory-preview__runtime {
  display: flex;
  flex-direction: column;
  gap: 8px;
  margin-top: 4px;
  padding-top: 12px;
  border-top: 1px solid var(--color-bg-border);
}

.memory-preview__runtime-header {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 12px;
  padding-bottom: 4px;
}

.memory-preview__runtime-header-left {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
}

.memory-preview__runtime-title {
  margin: 0;
  font-size: var(--text-md);
  font-weight: var(--weight-semibold);
  color: var(--color-text-primary);
}

.memory-preview__runtime-hint {
  margin: 0;
  font-size: var(--text-xs);
  color: var(--color-text-muted);
  font-family: var(--font-mono);
}

.memory-preview__runtime-header-right {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-shrink: 0;
}

.memory-preview__runtime-count {
  font-size: var(--text-xs);
  font-family: var(--font-mono);
  color: var(--color-text-muted);
}

.memory-preview__runtime-list {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.runtime-memory {
  display: flex;
  align-items: flex-start;
  gap: 8px;
  padding: 8px 10px;
  background: var(--color-bg-elevated);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-md);
  transition: border-color var(--duration-base) var(--ease-out);
}

.runtime-memory:hover {
  border-color: var(--color-bg-border-strong);
}

.runtime-memory__main {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.runtime-memory__head {
  display: flex;
  align-items: center;
  gap: 6px;
  flex-wrap: wrap;
}

.runtime-memory__title {
  font-size: var(--text-sm);
  font-weight: var(--weight-medium);
  color: var(--color-text-primary);
}

.runtime-memory__badge {
  display: inline-block;
  padding: 1px 6px;
  border-radius: 3px;
  font-size: var(--text-2xs);
  font-family: var(--font-mono);
  border: 1px solid var(--color-bg-border);
  background: var(--color-bg-surface);
  color: var(--color-text-muted);
  line-height: 1.4;
}

/* kind badge tints — pick muted variants that read on the panel
   surface. Each kind gets its own hue from the project's existing
   color tokens. */
.runtime-memory__badge--kind-pitfall {
  color: var(--color-tool-error);
  border-color: color-mix(in srgb, var(--color-tool-error) 40%, transparent);
}
.runtime-memory__badge--kind-preference {
  color: var(--color-accent);
  border-color: color-mix(in srgb, var(--color-accent) 40%, transparent);
}
.runtime-memory__badge--kind-fact {
  color: var(--color-status-info, var(--color-accent));
  border-color: color-mix(in srgb, var(--color-accent) 30%, transparent);
}
.runtime-memory__badge--kind-decision {
  color: var(--color-status-warn);
  border-color: color-mix(in srgb, var(--color-status-warn) 40%, transparent);
}

/* scope + status badges stay neutral (the kind badge already
   carries the primary hue). */

.runtime-memory__content {
  margin: 0;
  font-size: var(--text-sm);
  color: var(--color-text-secondary);
  line-height: 1.5;
  word-break: break-word;
}

.runtime-memory__meta {
  display: flex;
  align-items: center;
  gap: 6px;
  flex-wrap: wrap;
}

.runtime-memory__timestamp {
  font-size: var(--text-2xs);
  font-family: var(--font-mono);
  color: var(--color-text-muted);
}

.runtime-memory__tag {
  font-size: var(--text-2xs);
  font-family: var(--font-mono);
  color: var(--color-accent);
}

.runtime-memory__delete {
  flex-shrink: 0;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 24px;
  height: 24px;
  padding: 0;
  background: transparent;
  border: 1px solid transparent;
  border-radius: var(--radius-sm);
  color: var(--color-text-muted);
  cursor: pointer;
  transition:
    background var(--duration-base) var(--ease-out),
    color var(--duration-base) var(--ease-out),
    border-color var(--duration-base) var(--ease-out);
}

.runtime-memory__delete:hover {
  background: color-mix(in srgb, var(--color-tool-error) 12%, transparent);
  border-color: color-mix(in srgb, var(--color-tool-error) 40%, transparent);
  color: var(--color-tool-error);
}
</style>
