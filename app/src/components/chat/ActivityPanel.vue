<script lang="ts">
// ActivityPanel — the unified "run status" floating overlay (2026-09-02,
// task `09-02-chat-task-panel`). Three sections in one panel:
//   1. 子代理 — live subagent rows from `subagentRuns.runSummaryBySession`
//      (click → the existing SubagentDrawer via `openDrawer`).
//   2. 后台命令 — background shells from `useBackgroundShellsStore`
//      (new IPC `list_background_shells` pull + `background_shell:update`
//      event push; click → inline stdout/stderr preview; running rows
//      get an inline kill button).
//   3. 清单 — the B12 checklist, rendered verbatim from the old
//      `<ChecklistCard>` (this component REPLACES it; the checklist
//      store's logic is untouched — render-only migration).
//
// This non-setup script block exports the pure helpers the template
// ordering depends on, so vitest exercises the exact comparators.
// (`<script setup>` cannot carry exports.)
import type { SubagentRunSummary } from "../../stores/subagentRuns.types";

/** Subagent row ordering (PRD R2): running first, the rest by start
 *  time descending. `startedAt` is an ISO wall-clock string (unlike
 *  the shells' monotonic ms — do NOT mix the two sources). */
export function compareSubagentRuns(
  a: SubagentRunSummary,
  b: SubagentRunSummary,
): number {
  const aRunning = a.status === "running" ? 0 : 1;
  const bRunning = b.status === "running" ? 0 : 1;
  if (aRunning !== bRunning) return aRunning - bRunning;
  return Date.parse(b.startedAt) - Date.parse(a.startedAt);
}

/** Compact human duration for chips ("42s" / "3m 5s" / "1h 2m"). */
export function formatDuration(ms: number): string {
  if (ms < 1000) return "<1s";
  const totalSec = Math.floor(ms / 1000);
  const sec = totalSec % 60;
  const totalMin = Math.floor(totalSec / 60);
  const min = totalMin % 60;
  const h = Math.floor(totalMin / 60);
  if (h > 0) return min > 0 ? `${h}h ${min}m` : `${h}h`;
  if (min > 0) return sec > 0 ? `${min}m ${sec}s` : `${min}m`;
  return `${totalSec}s`;
}
</script>

<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import type { ChecklistItem, ChecklistStatus } from "../../stores/checklist";
import { useSubagentRunsStore } from "../../stores/subagentRuns";
// SubagentRunSummary is imported in the non-setup block above (shared
// module scope — a second import here would be a duplicate identifier).
import type { SubagentStatus } from "../../stores/subagentRuns.types";
import {
  useBackgroundShellsStore,
  type BackgroundShellStatus,
  type BackgroundShellSummary,
} from "../../stores/backgroundShells";
import { useProjectsStore } from "../../stores/projects";
import { extractErrorMessage } from "../../utils/useErrorBus";
import Icon from "../Icon.vue";

// ChecklistCard.vue (the predecessor overlay) documented props the
// checklist semantics; `items === null` hides the checklist section
// (no update_checklist seen), `[]` renders the empty state.
const props = defineProps<{
    /** The current session's checklist items (checklist store, via the
     *  parent ChatPanel's `currentChecklist` computed). */
    items: ChecklistItem[] | null;
    /** The current session id — keys both stores' per-session caches
     *  and the fetch/kill IPC args. `null` (no active session) hides
     *  the panel. */
    sessionId: string | null;
}>();

const subagentRuns = useSubagentRunsStore();
const shellsStore = useBackgroundShellsStore();
const projectsStore = useProjectsStore();

// -----------------------------------------------------------------------
// Local UI state
// -----------------------------------------------------------------------

/** Expanded ⇄ minimized. Defaults to expanded; a first-appearance
 *  auto-expand still fires (below) if the panel mounts while hidden. */
const expanded = ref<boolean>(true);

/** Single-select inline-expanded shell row (stdout/stderr preview).
 *  `null` = all collapsed; reset on session switch. */
const expandedShellId = ref<string | null>(null);

/** 5s light tick (design §6 trade-off): drives the running rows'
 *  elapsed chips without per-row timers. Subagent rows read wall
 *  clock (`startedAt` is ISO); shell rows read the store's
 *  monotonic-snapshot + wall-offset (`elapsedOf`). */
const tick = ref(0);
let tickTimer: ReturnType<typeof setInterval> | null = null;
onMounted(() => {
    // Global event listener — idempotent inside the store; component
    // unmount does NOT stop it (same lifecycle as subagentRuns).
    void shellsStore.ensureStarted();
    tickTimer = setInterval(() => {
        tick.value += 1;
    }, 5000);
});
onUnmounted(() => {
    if (tickTimer !== null) {
        clearInterval(tickTimer);
        tickTimer = null;
    }
});

/** Wall-clock snapshot that re-evaluates on every tick (the template
 *  passes it to `elapsedOf`; the tick keeps running rows growing). */
const tickNow = computed<number>(() => {
    void tick.value;
    return Date.now();
});

// -----------------------------------------------------------------------
// Session-scoped data
// -----------------------------------------------------------------------

/** History visibility: both stores' fetches run on mount AND on every
 *  session switch (design §3.2 — the subagent store's fetch otherwise
 *  only fires from ToolCallCard lazy-load + eager event paths, so a
 *  historic session's runs would be invisible until then). */
watch(
    () => props.sessionId,
    (sid) => {
        expandedShellId.value = null;
        if (!sid) return;
        void subagentRuns.fetchForSession(sid);
        void shellsStore.fetchForSession(sid);
    },
    { immediate: true },
);

const subagents = computed<SubagentRunSummary[]>(() => {
    const sid = props.sessionId;
    if (!sid) return [];
    const list = subagentRuns.runSummaryBySession.get(sid) ?? [];
    return [...list].sort(compareSubagentRuns);
});

const shells = computed<BackgroundShellSummary[]>(() => {
    const sid = props.sessionId;
    if (!sid) return [];
    // The store keeps lists sorted (running-first + newest-start);
    // sort defensively so the panel contract holds even if a future
    // writer forgets.
    return [...(shellsStore.shellsBySession.get(sid) ?? [])].sort(
        (a, b) =>
            (a.status === "running" ? 0 : 1) - (b.status === "running" ? 0 : 1) ||
            b.startedAtMs - a.startedAtMs,
    );
});

// -----------------------------------------------------------------------
// Visibility / counters
// -----------------------------------------------------------------------

const total = computed<number>(() => props.items?.length ?? 0);
const doneCount = computed<number>(
    () => props.items?.filter((i) => i.status === "done").length ?? 0,
);
const inProgressCount = computed<number>(
    () => props.items?.filter((i) => i.status === "in_progress").length ?? 0,
);
const allDone = computed<boolean>(
    () => total.value > 0 && doneCount.value === total.value,
);

/** Rows currently doing something (drives the「N 运行中」badge, the
 *  ball badge and the breathing ring). in_progress checklist items
 *  are NOT counted in the badge (they'd double-count with the
 *  checklist progress) but DO trip the breathing ring. */
const runningCount = computed<number>(
    () =>
        subagents.value.filter((s) => s.status === "running").length +
        shells.value.filter((s) => s.status === "running").length,
);

const showPanel = computed<boolean>(
    () =>
        props.sessionId !== null &&
        (props.items !== null || subagents.value.length > 0 || shells.value.length > 0),
);

/** First appearance auto-expands; a manual minimize afterwards is
 *  respected (migrated ChecklistCard behavior, applied to panel
 *  visibility instead of checklist-only). */
watch(
    () => showPanel.value,
    (nowVisible, wasVisible) => {
        if (nowVisible && !wasVisible) {
            expanded.value = true;
        }
    },
);

function toggleExpanded(): void {
    expanded.value = !expanded.value;
}

// -----------------------------------------------------------------------
// Subagent rows
// -----------------------------------------------------------------------

function openSubagent(s: SubagentRunSummary): void {
    void subagentRuns.openDrawer(s.id);
}

/** `startedAt` is a wall-clock ISO string — `Date.now()` math is
 *  sanctioned HERE (unlike the shells' monotonic ms). Running rows
 *  read `tick` so the elapsed re-renders on the 5s tick. */
function subagentDuration(s: SubagentRunSummary): string {
    const startedMs = Date.parse(s.startedAt);
    if (s.status === "running") {
        if (Number.isNaN(startedMs)) return "—";
        void tick.value;
        return formatDuration(Math.max(0, Date.now() - startedMs));
    }
    if (!s.finishedAt) return "—";
    const endMs = Date.parse(s.finishedAt);
    if (Number.isNaN(startedMs) || Number.isNaN(endMs)) return "—";
    return formatDuration(Math.max(0, endMs - startedMs));
}

function subagentIcon(status: SubagentStatus): string {
    switch (status) {
        case "running":
            return "loader"; // CSS app-spin rotates it (see marker CSS)
        case "completed":
            return "check-mini";
        case "error":
            return "x";
        default:
            return "circle"; // cancelled / incomplete — neutral
    }
}

function subagentMarkerClass(status: SubagentStatus): string {
    switch (status) {
        case "running":
            return "activity-panel__marker--running";
        case "completed":
            return "activity-panel__marker--done";
        case "error":
            return "activity-panel__marker--error";
        default:
            return "activity-panel__marker--neutral";
    }
}

// -----------------------------------------------------------------------
// Shell rows
// -----------------------------------------------------------------------

function toggleShell(s: BackgroundShellSummary): void {
    expandedShellId.value =
        expandedShellId.value === s.shellSessionId ? null : s.shellSessionId;
}

function shellIcon(status: BackgroundShellStatus): string {
    switch (status) {
        case "running":
            return "loader";
        case "completed":
            return "check-mini";
        case "failed":
        case "spawn_failed":
            return "x";
        default:
            return "square"; // killed / timed_out — neutral
    }
}

function shellMarkerClass(status: BackgroundShellStatus): string {
    switch (status) {
        case "running":
            return "activity-panel__marker--running";
        case "completed":
            return "activity-panel__marker--done";
        case "failed":
        case "spawn_failed":
            return "activity-panel__marker--error";
        default:
            return "activity-panel__marker--neutral";
    }
}

/** Kill via the registry (idempotent). Terminal state arrives via the
 *  `exited` event — no local fabrication. Failures toast; the panel's
 *  state is untouched (PRD R3). */
async function killShell(s: BackgroundShellSummary): Promise<void> {
    if (!props.sessionId) return;
    try {
        await shellsStore.kill(props.sessionId, s.shellSessionId);
    } catch (e) {
        projectsStore.showToast(
            `终止后台命令失败：${extractErrorMessage(e)}`,
            "warn",
        );
    }
}

// -----------------------------------------------------------------------
// Checklist section (migrated verbatim from ChecklistCard.vue)
// -----------------------------------------------------------------------

/** The UI is intentionally decoupled from the Rust `render_checklist`
 *  fn's text markers — icons here, `[ ]`/`[~]`/`[x]` for the LLM. */
function checklistIcon(status: ChecklistStatus): string {
    switch (status) {
        case "done":
            return "check-mini";
        case "in_progress":
            return "loader";
        case "pending":
        default:
            return "circle";
    }
}

function checklistClass(status: ChecklistStatus): string {
    switch (status) {
        case "done":
            return "activity-panel__marker--done";
        case "in_progress":
            return "activity-panel__marker--running";
        case "pending":
        default:
            return "activity-panel__marker--pending";
    }
}
</script>

<template>
    <div
        v-if="showPanel"
        :class="[
            'activity-panel',
            {
                'activity-panel--minimized': !expanded,
                'activity-panel--all-done': allDone,
                'activity-panel--active': runningCount + inProgressCount > 0,
            },
        ]"
        role="region"
        aria-label="运行状态面板"
    >
        <!--
      Minimized floating ball: running badge (top-right, >0 only) +
      checklist done/total (bottom, checklist present only) + breathing
      ring when anything is in flight.
    -->
        <button
            v-if="!expanded"
            type="button"
            class="activity-panel__ball btn btn--muted btn--circle"
            :aria-label="`展开运行状态(${runningCount} 运行中, ${doneCount}/${total})`"
            :title="`展开运行状态 (${runningCount} 运行中 · ${doneCount}/${total})`"
            @click="toggleExpanded"
        >
            <span class="activity-panel__ball-ring" />
            <span
                class="activity-panel__ball-icon"
                :class="{
                    'activity-panel__ball-icon--solo': items === null,
                }"
            >
                <Icon name="activity" :size="14" />
            </span>
            <span
                v-if="runningCount > 0"
                class="activity-panel__ball-badge"
                >{{ runningCount }}</span
            >
            <span v-if="items !== null" class="activity-panel__ball-count"
                >{{ doneCount }}/{{ total }}</span
            >
        </button>

        <!--
      Expanded panel: header (title + running badge / progress +
      minimize) above the scrollable three-section body.
    -->
        <div v-else class="activity-panel__panel">
            <header class="activity-panel__header">
                <span class="activity-panel__title">
                    <Icon
                        name="activity"
                        :size="16"
                        icon-class="activity-panel__title-icon"
                    />
                    <span class="ml-2"> 运行状态 </span>
                </span>
                <span
                    v-if="runningCount > 0"
                    class="activity-panel__running-badge"
                >
                    {{ runningCount }} 运行中
                </span>
                <span
                    v-else-if="items !== null"
                    class="activity-panel__progress"
                    :title="`${doneCount} 已完成 / ${total} 共计`"
                >
                    {{ doneCount }}/{{ total }}
                </span>
                <button
                    type="button"
                    class="activity-panel__minimize btn btn--ghost btn--icon"
                    :title="'最小化'"
                    aria-label="最小化运行状态面板"
                    @click="toggleExpanded"
                >
                    <Icon name="minus" :size="12" />
                </button>
            </header>

            <div class="activity-panel__body">
                <!-- ── Section: 子代理 ─────────────────────────── -->
                <section v-if="subagents.length > 0" class="activity-panel__section">
                    <div class="activity-panel__section-head">
                        <Icon
                            name="brain"
                            :size="12"
                            icon-class="activity-panel__section-icon"
                        />
                        <span class="activity-panel__section-title">子代理</span>
                        <span class="activity-panel__section-count">{{
                            subagents.length
                        }}</span>
                    </div>
                    <ul class="activity-panel__rows">
                        <li
                            v-for="s in subagents"
                            :key="s.id"
                            class="activity-panel__row activity-panel__row--clickable"
                            :title="'查看子代理详情'"
                            @click="openSubagent(s)"
                        >
                            <span
                                :class="[
                                    'activity-panel__marker',
                                    subagentMarkerClass(s.status),
                                ]"
                                aria-hidden="true"
                            >
                                <Icon :name="subagentIcon(s.status)" :size="14" />
                            </span>
                            <span class="activity-panel__row-main">
                                <span
                                    class="activity-panel__row-title"
                                    >{{ s.subagentName }}</span
                                >
                                <span
                                    v-if="s.modelDisplay"
                                    class="activity-panel__chip"
                                    >{{ s.modelDisplay }}</span
                                >
                            </span>
                            <span
                                class="activity-panel__row-meta"
                                >{{ subagentDuration(s) }}</span
                            >
                        </li>
                    </ul>
                </section>

                <!-- ── Section: 后台命令 ───────────────────────── -->
                <section v-if="shells.length > 0" class="activity-panel__section">
                    <div class="activity-panel__section-head">
                        <Icon
                            name="terminal"
                            :size="12"
                            icon-class="activity-panel__section-icon"
                        />
                        <span class="activity-panel__section-title">后台命令</span>
                        <span class="activity-panel__section-count">{{
                            shells.length
                        }}</span>
                    </div>
                    <ul class="activity-panel__rows">
                        <template
                            v-for="sh in shells"
                            :key="sh.shellSessionId"
                        >
                            <li
                                class="activity-panel__row activity-panel__row--clickable"
                                :title="
                                    sh.status === 'running'
                                        ? '查看输出'
                                        : '查看输出预览'
                                "
                                @click="toggleShell(sh)"
                            >
                                <span
                                    :class="[
                                        'activity-panel__marker',
                                        shellMarkerClass(sh.status),
                                    ]"
                                    aria-hidden="true"
                                >
                                    <Icon
                                        :name="shellIcon(sh.status)"
                                        :size="14"
                                    />
                                </span>
                                <span class="activity-panel__row-main">
                                    <span
                                        class="activity-panel__row-title activity-panel__row-title--mono"
                                        >{{ sh.command }}</span
                                    >
                                </span>
                                <span class="activity-panel__row-meta">
                                    <span
                                        v-if="sh.status === 'running'"
                                        class="activity-panel__chip activity-panel__chip--running"
                                        >{{
                                            formatDuration(
                                                shellsStore.elapsedOf(sh, tickNow),
                                            )
                                        }}</span
                                    >
                                    <template v-else>
                                        <span
                                            v-if="sh.exitCode !== null"
                                            :class="[
                                                'activity-panel__chip',
                                                {
                                                    'activity-panel__chip--error':
                                                        sh.exitCode !== 0,
                                                },
                                            ]"
                                            >exit {{ sh.exitCode }}</span
                                        >
                                        <span
                                            class="activity-panel__row-duration"
                                            >{{ formatDuration(sh.elapsedMs) }}</span
                                        >
                                    </template>
                                    <button
                                        v-if="sh.status === 'running'"
                                        type="button"
                                        class="activity-panel__kill btn btn--ghost btn--icon"
                                        title="终止该后台命令"
                                        aria-label="终止该后台命令"
                                        @click.stop="killShell(sh)"
                                    >
                                        <Icon name="x" :size="12" />
                                    </button>
                                </span>
                            </li>
                            <!-- Inline preview (single-select expand). -->
                            <li
                                v-if="expandedShellId === sh.shellSessionId"
                                class="activity-panel__preview"
                            >
                                <div
                                    v-if="
                                        !sh.stdoutPreview && !sh.stderrPreview
                                    "
                                    class="activity-panel__preview-empty"
                                >
                                    {{
                                        sh.status === "running"
                                            ? "运行中，尚无输出可读"
                                            : "无输出"
                                    }}
                                </div>
                                <template v-else>
                                    <pre
                                        v-if="sh.stdoutPreview"
                                        class="activity-panel__pre"
                                        >{{ sh.stdoutPreview }}</pre
                                    >
                                    <pre
                                        v-if="sh.stderrPreview"
                                        class="activity-panel__pre activity-panel__pre--stderr"
                                        >{{ sh.stderrPreview }}</pre
                                    >
                                </template>
                                <div
                                    v-if="sh.fullOutputPath"
                                    class="activity-panel__preview-hint"
                                >
                                    完整输出已落盘：{{ sh.fullOutputPath }}
                                </div>
                            </li>
                        </template>
                    </ul>
                </section>

                <!-- ── Section: 清单 (ChecklistCard migration) ─── -->
                <section v-if="items !== null" class="activity-panel__section">
                    <div class="activity-panel__section-head">
                        <Icon
                            name="clipboard-list"
                            :size="12"
                            icon-class="activity-panel__section-icon"
                        />
                        <span class="activity-panel__section-title">清单</span>
                        <span class="activity-panel__section-count"
                            >{{ doneCount }}/{{ total }}</span
                        >
                    </div>
                    <ul v-if="total > 0" class="activity-panel__check-items">
                        <li
                            v-for="(item, idx) in items"
                            :key="idx"
                            :class="[
                                'activity-panel__check-item',
                                `activity-panel__check-item--${item.status}`,
                            ]"
                        >
                            <span
                                :class="[
                                    'activity-panel__marker',
                                    checklistClass(item.status),
                                ]"
                                aria-hidden="true"
                            >
                                <Icon
                                    :name="checklistIcon(item.status)"
                                    :size="16"
                                />
                            </span>
                            <span class="activity-panel__check-content">{{
                                item.content
                            }}</span>
                        </li>
                    </ul>
                    <div v-else class="activity-panel__preview-empty">
                        清单为空
                    </div>
                </section>
            </div>
        </div>
    </div>
</template>

<style scoped>
/* The panel is a `position: absolute` overlay inside ChatPanel
   (migrated from ChecklistCard). Anchored bottom-right, above the
   ChatInput bar; z-index 50 stays BELOW the modal layer (modals
   teleport to body at 1000+) — the chat floating-card micro-local
   band (design-tokens Z ladder exception). */
.activity-panel {
    position: absolute;
    right: 20px;
    bottom: 156px;
    z-index: 50;
    font-family: var(--font-sans);
    color: var(--color-text-primary);
    pointer-events: auto;
}

/* Expanded panel shape (migrated from .checklist-card__panel). */
.activity-panel__panel {
    width: 280px;
    max-height: 60vh;
    display: flex;
    flex-direction: column;
    background: var(--color-bg-surface);
    border: 1px solid var(--color-bg-border);
    border-radius: var(--radius-lg);
    /* design-tokens shadow-exceptions: the floating checklist panel
       value migrated verbatim from ChecklistCard (between md/lg). */
    box-shadow: 0 6px 24px rgba(0, 0, 0, 0.35);
    overflow: hidden;
}

/* All-done tint: subtle green left border ("the run is wrapping
   up"). Whole-card per design §3.2 (keeps the old visual). */
.activity-panel--all-done .activity-panel__panel {
    border-left: 3px solid var(--color-tool-write);
}

.activity-panel__header {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 10px;
    border-bottom: 1px solid var(--color-bg-border);
    background: var(--color-bg-elevated);
    cursor: pointer;
    user-select: none;
}

.activity-panel__title {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    flex: 1;
    font-size: var(--text-sm);
    font-weight: var(--weight-semibold);
    color: var(--color-text-primary);
    min-width: 0;
}

.activity-panel__title-icon {
    color: var(--color-text-secondary);
    flex-shrink: 0;
}

.activity-panel--all-done .activity-panel__title-icon {
    color: var(--color-tool-write);
}

/* 「N 运行中」badge (replaces the old header progress chip while
   anything is running). */
.activity-panel__running-badge {
    font-size: var(--text-xs);
    font-family: var(--font-mono);
    color: var(--color-tool-shell);
    padding: 1px 6px;
    background: color-mix(in srgb, var(--color-tool-shell) 10%, transparent);
    border-radius: var(--radius-sm);
    flex-shrink: 0;
}

.activity-panel__progress {
    font-size: var(--text-xs);
    font-family: var(--font-mono);
    color: var(--color-text-secondary);
    padding: 1px 6px;
    background: var(--color-bg-app);
    border-radius: var(--radius-sm);
    flex-shrink: 0;
}

/* 08-24 btn-family:20px 固定 icon 钮本体由 ghost·icon 家族承载;
   本地仅保留固定几何(w/h + padding:0)。 */
.activity-panel__minimize {
    flex-shrink: 0;
    width: 20px;
    height: 20px;
    padding: 0;
}

/* Scrollable section body (max-height comes from the panel's
   60vh cap). */
.activity-panel__body {
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    min-height: 0;
}

.activity-panel__section {
    display: flex;
    flex-direction: column;
    padding: 6px 4px;
}

.activity-panel__section + .activity-panel__section {
    border-top: 1px solid var(--color-bg-border);
}

.activity-panel__section-head {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 2px 6px 4px;
    font-size: var(--text-2xs);
    color: var(--color-text-muted);
    text-transform: none;
}

.activity-panel__section-icon {
    color: var(--color-text-muted);
    flex-shrink: 0;
}

.activity-panel__section-title {
    flex: 1;
    font-weight: var(--weight-medium);
}

.activity-panel__section-count {
    font-family: var(--font-mono);
}

.activity-panel__rows {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
}

.activity-panel__row {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 4px 6px;
    border-radius: var(--radius-sm);
    font-size: var(--text-sm);
    line-height: 1.45;
    color: var(--color-text-primary);
    transition: background var(--duration-fast) var(--ease-out);
}

.activity-panel__row--clickable {
    cursor: pointer;
}

.activity-panel__row--clickable:hover {
    background: var(--color-bg-hover);
}

/* Left status marker (shared by all three sections). */
.activity-panel__marker {
    flex-shrink: 0;
    line-height: 1.45;
    min-width: 20px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
}

.activity-panel__marker--pending,
.activity-panel__marker--neutral {
    color: var(--color-text-muted);
}

.activity-panel__marker--done {
    color: var(--color-tool-write);
}

.activity-panel__marker--running {
    color: var(--color-tool-shell);
}

.activity-panel__marker--error {
    color: var(--color-tool-error);
}

/* Spin animation runs on the SVG ITSELF with `transform-box:
   fill-box` so the rotation pivots around the icon's own bounding-box
   center (migrated ChecklistCard gotcha — spinning the wrapper span
   wobbles). Keyframes come from the global app-spin primitive. */
.activity-panel__marker--running :deep(svg) {
    transform-box: fill-box;
    transform-origin: 50% 50%;
    animation: app-spin 1s linear infinite;
}

.activity-panel__row-main {
    flex: 1;
    min-width: 0;
    display: inline-flex;
    align-items: center;
    gap: 6px;
}

.activity-panel__row-title {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
}

.activity-panel__row-title--mono {
    font-family: var(--font-mono);
    font-size: var(--text-xs);
}

.activity-panel__chip {
    flex-shrink: 0;
    font-size: var(--text-2xs);
    font-family: var(--font-mono);
    color: var(--color-text-secondary);
    background: var(--color-bg-elevated);
    border-radius: var(--radius-sm);
    padding: 0 5px;
    line-height: 1.6;
}

.activity-panel__chip--running {
    color: var(--color-tool-shell);
}

.activity-panel__chip--error {
    /* 错误文字用 400 档 token(填充 500 档不够 AA,design-tokens)。 */
    color: var(--color-tool-error-text);
    background: color-mix(in srgb, var(--color-tool-error) 12%, transparent);
}

.activity-panel__row-meta {
    flex-shrink: 0;
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: var(--text-2xs);
    font-family: var(--font-mono);
    color: var(--color-text-muted);
}

/* Inline kill button — visible on row hover only (keeps the row
   quiet while idle). */
.activity-panel__kill {
    width: 18px;
    height: 18px;
    padding: 0;
    color: var(--color-text-muted);
}

.activity-panel__row:hover .activity-panel__kill {
    color: var(--color-tool-error-text);
}

/* Inline stdout/stderr preview. */
.activity-panel__preview {
    padding: 4px 6px 6px 30px;
    display: flex;
    flex-direction: column;
    gap: 6px;
}

.activity-panel__pre {
    margin: 0;
    padding: 6px 8px;
    background: var(--color-bg-app);
    border: 1px solid var(--color-bg-border);
    border-radius: var(--radius-sm);
    font-family: var(--font-mono);
    font-size: var(--text-2xs);
    line-height: 1.5;
    color: var(--color-text-secondary);
    white-space: pre-wrap;
    word-break: break-word;
    max-height: 140px;
    overflow-y: auto;
}

.activity-panel__pre--stderr {
    color: var(--color-tool-error-text);
}

.activity-panel__preview-empty {
    font-size: var(--text-xs);
    color: var(--color-text-muted);
}

.activity-panel__preview-hint {
    font-size: var(--text-2xs);
    color: var(--color-text-muted);
    word-break: break-all;
}

/* ---- Checklist section (migrated verbatim from ChecklistCard's
   items CSS, classes renamed) ---- */

.activity-panel__check-items {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
}

.activity-panel__check-item {
    display: flex;
    align-items: flex-start;
    gap: 8px;
    padding: 4px 6px;
    border-radius: var(--radius-sm);
    font-size: var(--text-sm);
    line-height: 1.45;
    color: var(--color-text-primary);
    transition: background var(--duration-fast) var(--ease-out);
}

.activity-panel__check-item:hover {
    background: var(--color-bg-elevated);
}

.activity-panel__check-item--pending {
    color: var(--color-text-secondary);
}

.activity-panel__check-item--done .activity-panel__check-content {
    text-decoration: line-through;
    text-decoration-color: var(--color-text-muted);
    color: var(--color-text-muted);
}

.activity-panel__check-item--in_progress {
    background: color-mix(in srgb, var(--color-tool-shell) 8%, transparent);
}

.activity-panel__check-content {
    flex: 1;
    min-width: 0;
    word-break: break-word;
}

/* ---- Minimized floating ball (migrated) ----
   08-24 btn-family:本体由 muted·circle 家族承载;本地保留 44px 固定
   几何/FAB 阴影(design-tokens 特例表)/scale 按压放大(transition 含
   transform,家族未含)/overflow 给 ball-ring 外溢。 */
.activity-panel__ball {
    position: relative;
    width: 44px;
    height: 44px;
    padding: 0;
    box-shadow: 0 4px 14px rgba(0, 0, 0, 0.3);
    transition:
        transform var(--duration-fast) var(--ease-out),
        background var(--duration-fast) var(--ease-out),
        border-color var(--duration-fast) var(--ease-out);
    overflow: visible;
}

.activity-panel__ball:hover {
    transform: scale(1.06);
}

.activity-panel__ball-icon {
    position: absolute;
    top: 8px;
    left: 0;
    right: 0;
    margin: 0 auto;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    color: var(--color-text-secondary);
}

.activity-panel--all-done .activity-panel__ball-icon {
    color: var(--color-tool-write);
}

/* Solo icon (no checklist → no done/total count below): the
   top-anchored icon + bottom count pair only reads as centered
   TOGETHER; with the count absent the icon must center itself. */
.activity-panel__ball-icon--solo {
    top: 50%;
    transform: translateY(-50%);
}

.activity-panel__ball-count {
    position: absolute;
    bottom: 6px;
    left: 0;
    right: 0;
    margin: 0 auto;
    font-size: 9px;
    font-family: var(--font-mono);
    font-weight: var(--weight-semibold);
    color: var(--color-text-muted);
    line-height: 1;
}

/* Running badge pinned to the ball's top-right corner. */
.activity-panel__ball-badge {
    position: absolute;
    top: -4px;
    right: -4px;
    min-width: 16px;
    height: 16px;
    padding: 0 4px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    font-size: 9px;
    font-family: var(--font-mono);
    font-weight: var(--weight-semibold);
    color: var(--color-text-on-accent);
    background: var(--color-tool-shell);
    border-radius: var(--radius-pill);
    line-height: 1;
}

/* Breathing ring around the ball while anything is in flight
   (running subagents / shells / in_progress checklist). */
.activity-panel__ball-ring {
    position: absolute;
    inset: -3px;
    border-radius: 50%;
    pointer-events: none;
}

.activity-panel--active .activity-panel__ball-ring {
    border: 2px solid var(--color-tool-shell);
    animation: activity-breathe 2.2s ease-in-out infinite;
}

@keyframes activity-breathe {
    0%,
    100% {
        opacity: 0.4;
        transform: scale(1);
    }
    50% {
        opacity: 0.9;
        transform: scale(1.18);
    }
}

/* When all done + minimized, swap the ring color to green for a
   calmer "done" cue. */
.activity-panel--all-done.activity-panel--active .activity-panel__ball-ring,
.activity-panel--all-done .activity-panel__ball-ring {
    border-color: var(--color-tool-write);
    animation: none;
    opacity: 0.5;
}
</style>
