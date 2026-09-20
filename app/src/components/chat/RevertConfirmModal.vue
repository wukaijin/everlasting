<script setup lang="ts">
// RevertConfirmModal — N2 PR3 (2026-09-20, task
// `09-20-n2-checkpoint-revert`) 的 dangerous 确认弹窗:「回到此轮后」
// 两步 revert 的 preview 呈现 + 确认闸。模式对齐
// DeleteWorktreeConfirm(前端确认 + 后端执行 + 审计),数据流对齐
// DiffModal(父组件拥有 fetch / state,本组件纯呈现 + 抛意图)。
//
// 评审重排全清单(2026-09-20):
//   1. foreign 警告区**仅非空渲染** —— `foreign_delta` 为 null/空
//      时整个区块不存在(不是灰态、不是空壳);
//   2. 确认按钮文案**带还原文件数**(还原集 = checkout + delete 全集);
//   3. Unknown badge **中性色** —— 共享 cwd 下「无审计证据」是常态,
//      不是告警;tool_written / shell_write 是 agent 归属提示;
//   4. foreign 文案 = 「非本会话快照内变更」(PRD R3 核准措辞);
//   5. gitignore 双重不可见**常驻脚注** —— untracked 且 ignored 的
//      文件 diff 不显示、revert 也不还原(如 .env 类),恒披露;
//   6. **不做逐文件勾选** —— 勾选会造出任何轮次都不存在过的状态
//      (还原语义是整树回到 seq N,不是文件挑选)。
//
// 错误内联(评审修正:旧确认不得授权新还原集):
//   - kind=StalePreview(preview→confirm 之间磁盘再变)→ 弹窗内联
//     提示 + 「重新预览」按钮(emit repreview,父组件重跑 preview,
//     token 随新 gate 树更新);
//   - kind=SessionBusy → 明确文案(本 session 轮次进行中,先停止);
//   - 其余错误 → 通用内联消息(extractErrorMessage 的后端文案)。
//
// Esc = 取消、Enter = 确认(仅 preview 就绪且无错误时可确认),
// 焦点管理同 DeleteWorktreeConfirm(开窗聚焦确认键)。

import { computed, onUnmounted, ref, watch } from "vue";

import Icon from "../Icon.vue";
import type { RevertPreview, RevertAttribution } from "../../stores/turnCheckpoints";

const props = defineProps<{
  /** Open/closed state. Driven by parent. */
  open: boolean;
  /** Preview payload (null while loading / before first fetch). */
  preview: RevertPreview | null;
  /** True while the parent is fetching the preview. */
  loading: boolean;
  /** True while the parent is executing the revert (confirm in
   *  flight — the confirm button spins / ignores re-clicks). */
  executing: boolean;
  /** Error message from the last preview / execute call. */
  error: string | null;
  /** The AppCommandError.kind of `error` (StalePreview / SessionBusy
   *  get dedicated UI; null = generic). */
  errorKind: string | null;
}>();

const emit = defineEmits<{
  cancel: [];
  /** User confirmed the restore (preview token travels with the
   *  parent's state — this component carries no token itself). */
  confirm: [];
  /** StalePreview recovery: re-run the preview (new gate tree →
   *  new token), stay in the modal. */
  repreview: [];
}>();

const confirmButton = ref<HTMLButtonElement | null>(null);

/** Confirm is only meaningful when the preview is on screen and no
 *  error is pending. While executing, the button re-click is a
 *  no-op at the parent (guarded), and here it renders disabled. */
const canConfirm = computed(
  () => props.preview !== null && props.error === null && !props.executing,
);

/** 还原文件数(确认按钮文案):还原集全集 = checkout + delete。 */
const fileCount = computed(() => props.preview?.files.length ?? 0);

/** foreign 警告区:仅非空渲染(评审清单 1)。 */
const foreignPaths = computed(() =>
  props.preview?.foreign_delta?.map((f) => f.path) ?? [],
);

/** 归属 badge 文案 + 修饰类。Unknown 中性色(评审清单 3)。 */
const ATTRIBUTION_META: Record<
  RevertAttribution,
  { label: string; cls: string }
> = {
  tool_written: { label: "工具写入", cls: "revert-file__badge--tool" },
  shell_write: { label: "shell 写入", cls: "revert-file__badge--shell" },
  unknown: { label: "来源未知", cls: "revert-file__badge--unknown" },
};

/** Action 短语:checkout = 回写目标内容;delete = 该文件在目标轮
 *  之后才出现,回退即删除。 */
function actionLabel(action: "checkout" | "delete"): string {
  return action === "delete" ? "删除" : "还原";
}

function onKeyDown(e: KeyboardEvent) {
  if (!props.open) return;
  if (e.key === "Escape") {
    e.preventDefault();
    emit("cancel");
  } else if (e.key === "Enter" && canConfirm.value) {
    e.preventDefault();
    emit("confirm");
  }
}

if (typeof window !== "undefined") {
  window.addEventListener("keydown", onKeyDown);
  onUnmounted(() => window.removeEventListener("keydown", onKeyDown));
}

// Focus the confirm button on open so Enter doesn't have to be
// pressed twice (DeleteWorktreeConfirm pattern).
watch(
  () => props.open,
  (open) => {
    if (open) {
      setTimeout(() => confirmButton.value?.focus(), 0);
    }
  },
);
</script>

<template>
  <!-- 2026-09-20:Teleport 到 body —— 与 DiffModal 同款层级修复。
       MessageList 虚拟行的 inline transform 定位会把 fixed 弹层困进
       行级 stacking context(z-index 出不了行);确认弹窗挂在
       MessageItem 深处,必须传送到根上下文。z 仍守 --z-confirm 档。 -->
  <Teleport to="body">
    <Transition name="confirm-modal">
    <div
      v-if="open"
      class="confirm-backdrop"
      @click.self="emit('cancel')"
    >
      <div
        class="confirm-modal revert-modal"
        role="dialog"
        aria-modal="true"
        aria-label="回到此轮后确认"
        data-testid="revert-confirm-modal"
      >
        <header class="confirm-modal__header">
          <h2 class="confirm-modal__title">
            <Icon name="history" :size="14" icon-class="revert-modal__icon" />
            确认回到第 {{ preview?.target_seq ?? "…" }} 轮后?
          </h2>
          <button
            type="button"
            class="confirm-modal__close btn btn--icon btn--ghost"
            aria-label="Close"
            @click="emit('cancel')"
          >
            <Icon name="x" :size="14" />
          </button>
        </header>

        <div class="confirm-modal__body">
          <!-- loading(第一次 preview / 重新预览期间) -->
          <div
            v-if="loading"
            class="revert-modal__loading"
          >
            正在读取还原预览…
          </div>

          <!-- 错误内联:StalePreview 给恢复按钮,busy 给明确文案 -->
          <div
            v-else-if="error"
            class="revert-modal__error"
            data-testid="revert-error"
          >
            <p>{{ error }}</p>
            <button
              v-if="errorKind === 'StalePreview'"
              type="button"
              class="btn btn--muted"
              data-testid="revert-repreview-btn"
              @click="emit('repreview')"
            >
              重新预览
            </button>
          </div>

          <template v-else-if="preview">
            <!-- 还原集清单(整树还原,不做逐文件勾选 —— 评审清单 6) -->
            <p class="revert-modal__lead">
              将把工作区还原到该轮结束时的状态(共
              {{ fileCount }} 个文件):
            </p>
            <ul
              class="revert-modal__files"
              data-testid="revert-file-list"
            >
              <li
                v-for="f in preview.files"
                :key="f.path"
                class="revert-file"
                data-testid="revert-file-row"
              >
                <span
                  class="revert-file__action"
                  :class="`revert-file__action--${f.action}`"
                >{{ actionLabel(f.action) }}</span>
                <span class="revert-file__path">{{ f.path }}</span>
                <span
                  class="revert-file__badge"
                  :class="ATTRIBUTION_META[f.attribution].cls"
                  :data-testid="`revert-badge-${f.attribution}`"
                  :title="`归属:${ATTRIBUTION_META[f.attribution].label}`"
                >{{ ATTRIBUTION_META[f.attribution].label }}</span>
              </li>
            </ul>
            <p
              v-if="!preview.files.length"
              class="revert-modal__lead"
            >
              工作区与该轮快照一致,无需还原。
            </p>

            <!-- foreign 警告区:仅非空渲染(评审清单 1/4) -->
            <div
              v-if="foreignPaths.length"
              class="revert-modal__foreign"
              data-testid="revert-foreign-warning"
            >
              <p class="revert-modal__foreign-title">
                <Icon
                  name="warn"
                  :size="13"
                  icon-class="revert-modal__foreign-icon"
                />
                检测到{{ foreignPaths.length }} 个非本会话快照内变更
              </p>
              <p class="revert-modal__foreign-hint">
                以下文件在会话快照链之外被改动(可能是轮间隙的手工修改),
                回退将一并还原:
              </p>
              <ul class="revert-modal__foreign-paths">
                <li
                  v-for="p in foreignPaths"
                  :key="p"
                >{{ p }}</li>
              </ul>
            </div>

            <!-- gitignore 双重不可见:常驻脚注(评审清单 5) -->
            <p
              class="revert-modal__footnote"
              data-testid="revert-gitignore-note"
            >
              受 .gitignore 约束:未被 git 跟踪且被忽略的文件(如
              .env)不在快照内 —— 轮间 diff 不显示,回退也不会还原或删除。
            </p>
          </template>
        </div>

        <footer class="confirm-modal__actions">
          <button
            type="button"
            class="confirm-modal__btn confirm-modal__btn--cancel btn btn--muted"
            data-testid="revert-cancel-btn"
            @click="emit('cancel')"
          >
            取消
          </button>
          <button
            ref="confirmButton"
            type="button"
            class="confirm-modal__btn confirm-modal__btn--danger btn btn--danger"
            :disabled="!canConfirm"
            data-testid="revert-confirm-btn"
            @click="canConfirm && emit('confirm')"
          >
            {{ executing ? "还原中…" : `还原 ${fileCount} 个文件` }}
          </button>
        </footer>
      </div>
    </div>
    </Transition>
  </Teleport>
</template>

<style scoped>
/* confirm-backdrop / confirm-modal / __header / __title / __close /
   __actions / __btn 家族与动画复用 DeleteWorktreeConfirm 的全局形态
   (各组件 scoped 自持,本文件同款拷贝 —— .confirm-* 类不是全局
   样式,是组件内约定;此处仅列差异)。 */
.confirm-backdrop {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.6);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: var(--z-confirm);
  padding: 24px;
}

.confirm-modal {
  background: var(--color-bg-surface);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-lg);
  width: 100%;
  max-width: 460px;
  display: flex;
  flex-direction: column;
  overflow: hidden;
  box-shadow: var(--shadow-xl);
}

.confirm-modal__header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 12px 16px;
  border-bottom: 1px solid var(--color-bg-border);
  background: var(--color-bg-elevated);
}

.confirm-modal__title {
  margin: 0;
  font-size: var(--text-base);
  font-weight: var(--weight-semibold);
  color: var(--color-text-primary);
  display: inline-flex;
  align-items: center;
  gap: 6px;
}

.revert-modal__icon {
  color: var(--color-accent);
}

.confirm-modal__body {
  padding: 16px;
  font-size: var(--text-base);
  line-height: 1.5;
  color: var(--color-text-primary);
  /* 还原集可能很长:body 内滚动,header/footer 钉死。 */
  max-height: min(50vh, 420px);
  overflow-y: auto;
}

.confirm-modal__actions {
  display: flex;
  gap: 8px;
  padding: 12px 16px;
  border-top: 1px solid var(--color-bg-border);
  justify-content: flex-end;
}

/* --- 本组件私有块 -------------------------------------------------- */

.revert-modal__loading {
  padding: 16px 0;
  text-align: center;
  color: var(--color-text-muted);
}

.revert-modal__error {
  padding: 8px 0;
}

.revert-modal__error p {
  margin: 0 0 10px;
  color: var(--color-tool-error-text);
}

.revert-modal__lead {
  margin: 0 0 10px;
  color: var(--color-text-secondary);
}

.revert-modal__files {
  list-style: none;
  margin: 0 0 10px;
  padding: 0;
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-sm);
  max-height: 180px;
  overflow-y: auto;
}

.revert-file {
  display: grid;
  grid-template-columns: auto 1fr auto;
  align-items: center;
  gap: 8px;
  padding: 5px 10px;
  font-size: var(--text-sm);
}

.revert-file + .revert-file {
  border-top: 1px solid var(--color-bg-border);
}

.revert-file__path {
  font-family: var(--font-mono);
  color: var(--color-text-primary);
  overflow-wrap: anywhere;
}

.revert-file__action {
  font-size: var(--text-2xs);
  padding: 1px 6px;
  border-radius: 999px;
  white-space: nowrap;
}

.revert-file__action--checkout {
  background: color-mix(in srgb, var(--color-accent) 14%, transparent);
  color: var(--color-accent);
}

.revert-file__action--delete {
  background: color-mix(in srgb, var(--color-tool-error-text) 12%, transparent);
  color: var(--color-tool-error-text);
}

.revert-file__badge {
  font-size: var(--text-2xs);
  padding: 1px 6px;
  border-radius: 999px;
  white-space: nowrap;
}

/* tool / shell 归属 = agent 证据提示(accent 系);unknown = 中性
   (muted)—— 共享 cwd 下是常态,不做告警观感(评审清单 3)。 */
.revert-file__badge--tool {
  background: color-mix(in srgb, var(--color-accent) 12%, transparent);
  color: var(--color-accent);
}

.revert-file__badge--shell {
  background: color-mix(in srgb, var(--color-accent) 12%, transparent);
  color: var(--color-accent);
}

.revert-file__badge--unknown {
  background: var(--color-bg-hover, rgba(127, 127, 127, 0.12));
  color: var(--color-text-muted);
}

/* foreign 警告区(warn 语义,仅非空渲染)。 */
.revert-modal__foreign {
  border: 1px solid color-mix(in srgb, var(--color-status-warn, #fbbf24) 45%, transparent);
  background: color-mix(in srgb, var(--color-status-warn, #fbbf24) 8%, transparent);
  border-radius: var(--radius-sm);
  padding: 8px 10px;
  margin: 0 0 10px;
}

.revert-modal__foreign-title {
  display: flex;
  align-items: center;
  gap: 6px;
  margin: 0 0 4px;
  font-weight: var(--weight-semibold);
  font-size: var(--text-sm);
}

.revert-modal__foreign-icon {
  color: var(--color-status-warn, #fbbf24);
}

.revert-modal__foreign-hint {
  margin: 0 0 4px;
  font-size: var(--text-sm);
  color: var(--color-text-secondary);
}

.revert-modal__foreign-paths {
  list-style: none;
  margin: 0;
  padding: 0;
  font-family: var(--font-mono);
  font-size: var(--text-sm);
}

/* gitignore 常驻脚注(muted,恒渲染 —— 与条件性的 foreign 区分)。 */
.revert-modal__footnote {
  margin: 0;
  font-size: var(--text-2xs);
  color: var(--color-text-muted);
}

/* R4 modal animation(DeleteWorktreeConfirm 同款)。 */
.confirm-modal-enter-active,
.confirm-modal-leave-active {
  transition: opacity var(--duration-base) var(--ease-out);
}

.confirm-modal-enter-active .confirm-modal,
.confirm-modal-leave-active .confirm-modal {
  transition: opacity var(--duration-base) var(--ease-out), transform var(--duration-base) var(--ease-out);
}

.confirm-modal-enter-from,
.confirm-modal-leave-to {
  opacity: 0;
}

.confirm-modal-enter-from .confirm-modal,
.confirm-modal-leave-to .confirm-modal {
  opacity: 0;
  transform: scale(0.96);
}

.confirm-modal-leave-active,
.confirm-modal-leave-active .confirm-modal {
  transition-duration: 100ms;
  transition-timing-function: ease-in;
}
</style>
