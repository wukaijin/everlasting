<script setup lang="ts">
// WorktreeChip — tri-state worktree chip + dropdown menu.
//
// D6 header: replaced the static "Everlasting / vibe coding
// workbench / cwd" trio with a per-session header that shows the
// session title plus this worktree chip. The chip is the primary
// action (open diff / attach), the dropdown is for the secondary
// actions (copy path / branch / detach / delete).
//
// Step 4 follow-up: the chip is tri-state:
//   - `none` (no worktree ever) → "attach worktree" button
//   - `active` (worktree bound)  → "diff (N)" + dropdown with
//     copy-path / copy-branch / detach / delete
//   - `detached` (was active)    → "上次 worktree" + dropdown
//     with the same actions.
//
// This component owns the chip + dropdown UI and the clipboard
// copy logic. The parent (ChatPanel) handles attach/detach/delete
// IPC + toasts + the delete-confirm modal. The popover follows
// the project's hand-rolled pattern (onDocumentClick + Esc close)
// per `.trellis/spec/frontend/popover-pattern.md`.

import { computed, onUnmounted, ref } from "vue";
import { useProjectsStore } from "../../stores/projects";
import Icon from "../Icon.vue";

export type WorktreeState = "none" | "active" | "detached";

const props = defineProps<{
    /** Tri-state: none | active | detached. Drives chip shape and
     *  dropdown visibility. */
    state: WorktreeState;
    /** Pre-computed label rendered inside the main chip button
     *  (e.g. "diff (3 files)", "attach worktree"). */
    chipLabel: string;
    /** Pre-computed `title` tooltip on the main chip button. */
    chipTitle: string;
    /** Git branch name; used only by the "copy branch" menu item. */
    branchName: string;
    /** Worktree path for the "copy path" menu item. Null hides the
     *  menu item (e.g. when state is "none"). */
    pathForDisplay: string | null;
    /** True while the current session is streaming. Disables the
     *  Detach and Delete menu items (REQ-13). The copy buttons
     *  are NOT disabled (REQ-26). */
    isStreaming: boolean;
}>();

const emit = defineEmits<{
    /** Copy the worktree path to clipboard. */
    "copy-path": [];
    /** Copy the branch name to clipboard. */
    "copy-branch": [];
    /** Main chip click — for `none` this means attach, otherwise
     *  it means open the diff. */
    "chip-click": [];
    /** User picked "解绑 (detach)" from the dropdown. */
    "detach-click": [];
    /** User picked "删除 worktree" from the dropdown. */
    "delete-click": [];
}>();

const projectsStore = useProjectsStore();

// --- Dropdown state (hand-rolled popover, see popover-pattern.md) ---

const menuOpen = ref(false);
const menuRoot = ref<HTMLElement | null>(null);

function toggleMenu() {
    menuOpen.value = !menuOpen.value;
}

function closeMenu() {
    menuOpen.value = false;
}

function onDocumentClick(e: MouseEvent) {
    if (!menuOpen.value) return;
    const target = e.target as Node | null;
    if (menuRoot.value && target && !menuRoot.value.contains(target)) {
        menuOpen.value = false;
    }
}

function onKeydown(e: KeyboardEvent) {
    if (menuOpen.value && e.key === "Escape") {
        menuOpen.value = false;
    }
}

if (typeof document !== "undefined") {
    document.addEventListener("click", onDocumentClick);
    document.addEventListener("keydown", onKeydown);
    onUnmounted(() => {
        document.removeEventListener("click", onDocumentClick);
        document.removeEventListener("keydown", onKeydown);
    });
}

// --- Menu item disabled predicates ---

/** Detach/delete are disabled while streaming (REQ-13); the copy
 *  buttons are NOT (REQ-26). Attach is allowed mid-stream. */
const detachDisabled = computed<boolean>(() => props.isStreaming);
const deleteDisabled = computed<boolean>(() => props.isStreaming);

// --- Clipboard copy ---

async function copyToClipboard(value: string, label: string) {
    try {
        if (navigator.clipboard?.writeText) {
            await navigator.clipboard.writeText(value);
        } else {
            // Fallback: legacy `document.execCommand("copy")` for
            // non-secure contexts (some embedded webviews).
            const ta = document.createElement("textarea");
            ta.value = value;
            ta.setAttribute("readonly", "");
            ta.style.position = "absolute";
            ta.style.left = "-9999px";
            document.body.appendChild(ta);
            ta.select();
            document.execCommand("copy");
            document.body.removeChild(ta);
        }
        projectsStore.showToast(`已复制 ${label}`, "info", 2000);
    } catch (e) {
        projectsStore.showToast(`复制失败: ${String(e)}`, "error");
    }
}

function onCopyPath() {
    const p = props.pathForDisplay;
    if (!p) return;
    void copyToClipboard(p, "worktree path");
    closeMenu();
    emit("copy-path");
}

function onCopyBranch() {
    if (!props.branchName) return;
    void copyToClipboard(props.branchName, "branch name");
    closeMenu();
    emit("copy-branch");
}

function onDetach() {
    closeMenu();
    emit("detach-click");
}

function onDelete() {
    closeMenu();
    emit("delete-click");
}
</script>

<template>
    <div ref="menuRoot" class="worktree-chip">
        <button
            type="button"
            :class="[
                'worktree-chip__main',
                {
                    'worktree-chip__main--alone': state === 'none',
                },
            ]"
            :title="chipTitle"
            @click="emit('chip-click')"
        >
            <Icon name="document" :size="12" />
            {{ chipLabel }}
        </button>
        <button
            v-if="state !== 'none'"
            type="button"
            class="worktree-chip__toggle"
            aria-label="worktree options"
            title="worktree options"
            @click.stop="toggleMenu"
        >
            <Icon
                :name="menuOpen ? 'chevron-down' : 'chevron-right'"
                :size="12"
            />
        </button>
        <Transition name="worktree-popover">
            <div
                v-if="menuOpen && state !== 'none'"
                class="worktree-chip__menu"
                role="menu"
            >
                <button
                    v-if="pathForDisplay"
                    type="button"
                    class="worktree-chip__menu-item"
                    role="menuitem"
                    @click="onCopyPath"
                >
                    <Icon name="document" :size="12" />
                    复制 worktree path
                </button>
                <button
                    type="button"
                    class="worktree-chip__menu-item"
                    role="menuitem"
                    @click="onCopyBranch"
                >
                    <Icon name="refresh" :size="12" />
                    复制 branch name
                </button>
                <div class="worktree-chip__menu-sep" />
                <button
                    type="button"
                    class="worktree-chip__menu-item"
                    role="menuitem"
                    :disabled="detachDisabled"
                    @click="onDetach"
                >
                    <Icon name="minus" :size="12" />
                    解绑 (detach)
                </button>
                <button
                    type="button"
                    class="worktree-chip__menu-item worktree-chip__menu-item--danger"
                    role="menuitem"
                    :disabled="deleteDisabled"
                    @click="onDelete"
                >
                    <Icon name="warn" :size="12" />
                    删除 worktree
                </button>
            </div>
        </Transition>
    </div>
</template>

<style scoped>
.worktree-chip {
    position: relative;
    display: inline-flex;
    align-items: stretch;
}

.worktree-chip__main {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    font-size: var(--text-xs);
    background: var(--color-accent-muted);
    color: var(--color-accent);
    border: 1px solid var(--color-accent);
    border-top-right-radius: 0;
    border-bottom-right-radius: 0;
    border-right: 0;
    padding: 2px 8px;
    font-family: var(--font-mono);
    white-space: nowrap;
    cursor: pointer;
    font: inherit;
    font-size: var(--text-xs);
}

/* When the chevron toggle is absent (state === 'none'), the main
 * chip is the only button in the group — restore its right border
 * and right radius to make it a complete pill. */
.worktree-chip__main--alone {
    border-top-right-radius: 4px;
    border-bottom-right-radius: 4px;
    border-right: 1px solid var(--color-accent);
}

.worktree-chip__main:hover {
    background: var(--color-accent);
    color: var(--color-bg-app);
}

.worktree-chip__toggle {
    background: var(--color-accent-muted);
    color: var(--color-accent);
    border: 1px solid var(--color-accent);
    border-top-left-radius: 0;
    border-bottom-left-radius: 0;
    border-top-right-radius: 4px;
    border-bottom-right-radius: 4px;
    cursor: pointer;
    font: inherit;
    font-size: var(--text-xs);
    padding: 2px 4px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
}

.worktree-chip__toggle:hover {
    background: var(--color-accent);
    color: var(--color-bg-app);
}

.worktree-chip__menu {
    position: absolute;
    top: calc(100% + 4px);
    right: 0;
    background: var(--color-bg-surface);
    border: 1px solid var(--color-bg-border);
    border-radius: var(--radius-md);
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.4);
    min-width: 200px;
    z-index: 100;
    padding: 4px;
    display: flex;
    flex-direction: column;
}

.worktree-chip__menu-item {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 6px 8px;
    background: transparent;
    border: 0;
    color: var(--color-text-primary);
    font: inherit;
    font-size: var(--text-sm);
    text-align: left;
    cursor: pointer;
    border-radius: var(--radius-sm);
}

.worktree-chip__menu-item:hover:not(:disabled) {
    background: var(--color-bg-elevated);
}

.worktree-chip__menu-item:disabled {
    color: var(--color-text-muted);
    cursor: not-allowed;
}

.worktree-chip__menu-item--danger {
    color: var(--color-tool-error);
}

.worktree-chip__menu-item--danger:hover:not(:disabled) {
    background: var(--color-bg-elevated);
}

.worktree-chip__menu-sep {
    height: 1px;
    background: var(--color-bg-border);
    margin: 4px 0;
}

/* Popover enter/leave: fade + slide-down 4px (worktree dropdown
 * opens downward). Matches the project's 150ms / 100ms convention
 * (see popover-pattern.md §Animation). */
.worktree-popover-enter-active,
.worktree-popover-leave-active {
    transition: opacity var(--duration-base) var(--ease-out), transform var(--duration-base) var(--ease-out);
    transform-origin: top right;
}

.worktree-popover-enter-from,
.worktree-popover-leave-to {
    opacity: 0;
    transform: translateY(-4px);
}

.worktree-popover-leave-active {
    transition-duration: 100ms;
    transition-timing-function: ease-in;
}
</style>