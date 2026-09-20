<script setup lang="ts">
// DiffModal — diff overlay, modal-family tier.
//
// Session worktree diff: triggered by the "diff" chip in the chat
// panel header. N2 PR2 (2026-09-20): reused by MessageItem as
// 「本轮 diff」(turn checkpoint diff) via the `title` prop. Closes
// on backdrop click or the close button (Esc handling lives in the
// parent — see ChatPanel's onKeyDown).
//
// 2026-09-20 modal rework (jjh-mono session 实测回归的四处收敛):
// 1. <Teleport to="body"> —— MessageList 的虚拟行用 inline
//    `transform: translateY()` 定位,transform 祖先会把 position:fixed
//    弹层困进该行的 stacking context(z-index 出不了行,邻行内容
//    直接盖在弹窗上)。Teleport 到 body 根上下文是结构解,任何
//    挂载点(ChatPanel / MessageItem)都不再受祖先层影响。
// 2. z 档 --z-sheet → --z-modal-overlay:shadow-xl 规范本就把
//    Diff 列在 modal 家族(shadow-zindex-tokens.md),原先挂 sheet
//    档是与 SubagentDrawer 同档的遗留错位。
// 3. body min-height + 专属空态:loading / error / 空载荷不再把
//    弹窗压成一条缝;空载荷(0 文件)在 modal 层拦截,不再落到
//    DiffView 里 session 语境的空态文案。
// 4. 头部重排:file-diff 图标 + 标题 + 文件数 / ±行数 meta,
//    替代原 "(N files)" 纯文本尾巴。

import { computed } from "vue";
import DiffView from "./DiffView.vue";
import Icon from "../Icon.vue";

const props = defineProps<{
    /** Open/closed state. Driven by parent. */
    isOpen: boolean;
    /** True while the parent is fetching the diff. Renders a
     *  loading placeholder in the body. */
    isLoading: boolean;
    /** Error message from the last fetch. Renders a styled
     *  placeholder in the body when non-null. */
    error: string | null;
    /** Cached diff result. When null and not loading, the body
     *  is empty (parent hasn't fetched yet). */
    result: { files: import("./DiffView.vue").FileDiff[] } | null;
    /** N2 PR2 (2026-09-20): header title. The session worktree diff
     *  keeps "Session diff"; the turn checkpoint diff mounts this
     *  modal with「本轮 diff」. Optional — defaults to the original. */
    title?: string;
}>();

const emit = defineEmits<{
    close: [];
}>();

const fileCount = computed(() => props.result?.files.length ?? 0);

/** 头部 ±行数汇总(跨文件求和;0 的方向不渲染)。 */
const totals = computed(() => ({
    added: props.result?.files.reduce((s, f) => s + f.added, 0) ?? 0,
    removed: props.result?.files.reduce((s, f) => s + f.removed, 0) ?? 0,
}));
</script>

<template>
    <Teleport to="body">
        <Transition name="diff-modal">
            <div
                v-if="isOpen"
                class="diff-modal-backdrop"
                @click.self="emit('close')"
            >
                <div
                    class="diff-modal"
                    role="dialog"
                    aria-modal="true"
                    :aria-label="title ?? 'Session diff'"
                >
                    <header class="diff-modal__header">
                        <Icon
                            name="file-diff"
                            :size="15"
                            icon-class="diff-modal__icon"
                        />
                        <h2 class="diff-modal__title">
                            {{ title ?? "Session diff" }}
                        </h2>
                        <span v-if="result" class="diff-modal__meta">
                            <span class="diff-modal__count">
                                {{ fileCount }}
                                {{ fileCount === 1 ? "file" : "files" }}
                            </span>
                            <span
                                v-if="totals.added > 0"
                                class="diff-modal__plus"
                            >+{{ totals.added }}</span>
                            <span
                                v-if="totals.removed > 0"
                                class="diff-modal__minus"
                            >−{{ totals.removed }}</span>
                        </span>
                        <button
                            type="button"
                            class="diff-modal__close btn btn--icon btn--ghost"
                            @click="emit('close')"
                            aria-label="Close"
                        >
                            <Icon name="x" :size="14" />
                        </button>
                    </header>
                    <div class="diff-modal__body">
                        <div v-if="isLoading" class="diff-modal__state">
                            Loading diff…
                        </div>
                        <div
                            v-else-if="error"
                            class="diff-modal__state diff-modal__state--error"
                        >
                            {{ error }}
                        </div>
                        <div
                            v-else-if="result && fileCount === 0"
                            class="diff-modal__state"
                        >
                            No file changes.
                        </div>
                        <DiffView v-else-if="result" :files="result.files" />
                    </div>
                </div>
            </div>
        </Transition>
    </Teleport>
</template>

<style scoped>
/* -----------------------------------------------------------------------
 * Diff modal, modal-family tier (2026-09-20 rework). backdrop = family
 * 遮罩色 + blur;z 走 --z-modal-overlay(见组件头注释 1/2)。全屏
 * overlay,内层 .diff-modal 居中,40px 边距。滚动发生在
 * .diff-modal__body,头部 + 关闭键钉住;body 有 min-height,
 * loading / error / 空载荷不塌缩(头注释 3)。
 * -------------------------------------------------------------------- */
.diff-modal-backdrop {
    position: fixed;
    inset: 0;
    background: color-mix(in srgb, var(--color-bg-app) 70%, transparent);
    backdrop-filter: blur(4px);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: var(--z-modal-overlay);
    padding: 40px;
}

.diff-modal {
    background: var(--color-bg-surface);
    border: 1px solid var(--color-bg-border);
    border-radius: var(--radius-lg);
    width: 100%;
    max-width: 1100px;
    max-height: 100%;
    display: flex;
    flex-direction: column;
    overflow: hidden;
    box-shadow: var(--shadow-xl);
}

.diff-modal__header {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px 16px;
    border-bottom: 1px solid var(--color-bg-border);
    background: var(--color-bg-elevated);
    flex-shrink: 0;
}

.diff-modal__icon {
    color: var(--color-accent);
    display: inline-flex;
    flex-shrink: 0;
}

.diff-modal__title {
    margin: 0;
    font-size: var(--text-base);
    font-weight: var(--weight-semibold);
    color: var(--color-text-primary);
}

.diff-modal__meta {
    display: inline-flex;
    align-items: baseline;
    gap: 8px;
    font-size: var(--text-xs);
}

.diff-modal__count {
    color: var(--color-text-muted);
}

.diff-modal__plus {
    color: var(--color-tool-write);
    font-weight: var(--weight-semibold);
}

.diff-modal__minus {
    color: var(--color-tool-error-text);
    font-weight: var(--weight-semibold);
}

.diff-modal__close {
    margin-left: auto;
}

/* 按钮样式由全局 .btn 家族承载(close = ghost icon)。 */

.diff-modal__body {
    flex: 1;
    overflow-y: auto;
    padding: 12px 16px;
    background: var(--color-bg-app);
    /* 空态下限:loading / error / 0 文件载荷不会把弹窗压成一条缝。 */
    min-height: 220px;
}

.diff-modal__state {
    display: flex;
    align-items: center;
    justify-content: center;
    min-height: 195px;
    padding: 24px;
    text-align: center;
    color: var(--color-text-muted);
    font-size: var(--text-base);
}

.diff-modal__state--error {
    color: var(--color-tool-error-text);
}

/* R4 popup animation: 仅 content 做 scale 0.96→1 + opacity 过渡；
 * backdrop（Transition 根元素）opacity 始终 1、无视觉动画，transition-
 * duration 仅用于让 Vue Transition 的 enter/leave 计时与 content 同步，
 * 避免 active class 提前移除而中断 content 过渡 (07-02-modal-motion-rhythm)。 */
.diff-modal-enter-active {
    transition: opacity var(--duration-modal-in);
}

.diff-modal-leave-active {
    transition: opacity var(--duration-modal-out);
}

.diff-modal-enter-active .diff-modal,
.diff-modal-leave-active .diff-modal {
    transition: opacity var(--duration-modal-in) var(--ease-modal-in), transform var(--duration-modal-in) var(--ease-modal-in);
}

.diff-modal-enter-from .diff-modal {
    opacity: 0;
    transform: scale(0.96);
}

.diff-modal-leave-to .diff-modal {
    opacity: 0;
}

.diff-modal-leave-active .diff-modal {
    transition-duration: var(--duration-modal-out);
    transition-timing-function: var(--ease-accelerate);
}
</style>
