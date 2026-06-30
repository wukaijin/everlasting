<script setup lang="ts">
// AskUserQuestionCard — inline message card for the `ask_user_question`
// blocking reverse-question tool (Phase D of
// `06-30-ask-user-question-tool`, 2026-06-30).
//
// Per PRD R6 / design §5.5 (UI 红线):
//   - **inline card**, NEVER modal — the DOM is a child of the
//     message stream (mounted by MessageItem.vue's tool-name
//     dispatch in Phase E, sitting below `<ToolCallCard>`)
//   - NO reka-ui `Dialog`/`Popover`/`AlertDialog` portals
//   - NO `<Teleport to="body">`
//   - NO floating overlay / mask / backdrop
//
// Why no modal: PRD R9-R11 requires session-switch to preserve
// the pending question (the user can switch to another session,
// work there, switch back, and the card is still answerable).
// Modals can't survive session-switch re-renders cleanly, and
// a persistent overlay would obscure other sessions' content.
// Inline cards ride on the message stream's normal scroll /
// render lifecycle; when the user switches back, the card is
// still in the same assistant turn's row.
//
// Card shape (PRD R8): single card with multiple question sections
// (one section per question in the payload, 1..=4 total). Each
// section has its own header chip + question body + options +
// optional description + optional collapsible preview.
//
// Selection semantics (PRD R7 RED LINE): "整体提交语义" —
// option clicks ONLY mutate local state (radio for single-select,
// checkbox for multi-select). NO immediate invoke. The bottom
// "提交" button collects ALL question answers and fires ONE
// `invoke("resolve_tool_question", { answer: [...] })`. The
// "跳过" button always fires `{ cancelled: true }` for the whole
// card. Disabled-while-incomplete: "提交" is disabled when ANY
// question has no selection (single-select → must have 1 label;
// multi-select → must have ≥1 label).
//
// Post-answer shape (PRD R7a): the card stays EXPANDED with full
// content visible after submit OR skip (no collapse) — lets the
// user review the full Q&A history across session switches. The
// bottom action row transitions:
//   pending  → 提交 + 跳过 buttons
//   answered → "已回答" status pill + selected-labels summary
//   cancelled → "已跳过" status pill
//
// Visual contract (PRD R6, design §5.5): reuses the existing
// ToolCallCard card chrome tokens (--color-bg-surface, --color-bg-border,
// --radius-md, spacing tokens) — the user sees a familiar tool-
// card-shaped container; the inner sections use project tokens
// for chips / options / descriptions. No new color tokens.

import { computed, ref } from "vue";

import Icon from "../Icon.vue";
import { resolveToolQuestion } from "../../utils/toolQuestion";
import type {
  Question,
  QuestionCardState,
  ToolQuestionAnswer,
} from "../../stores/questionCards.types";

const props = withDefaults(
  defineProps<{
    /** Active session id (for routing the resolve IPC). */
    sessionId: string;
    /** The tool_use_id from the LLM's `ToolUse(ask_user_question)`
     *  block — echoed back in the resolve payload so the backend
     *  can sanity-check the routing (QuestionStore keys by
     *  session_id; tool_use_id is the matching aid). */
    toolUseId: string;
    /** The questions array (1..=4 entries). Backend-validated
     *  bounds (see design §5.5); the card assumes valid input. */
    questions: Question[];
    /** Initial state — typically "pending" when freshly mounted
     *  from the live `tool:question` event. Defaults to "pending"
     *  so the parent (MessageItem) doesn't need to thread state
     *  for the common mount case. The card flips to "answered"
     *  or "cancelled" on its own after a successful submit / skip. */
    state?: QuestionCardState;
    /** Selected-answer snapshot — only populated when the card is
     *  rehydrated into the "answered" state (e.g. from a cached
     *  historical view). The live "pending" path builds its own
     *  selection state via `selectedLabels` below; this prop is
     *  for reload-after-restart patterns. */
    selectedAnswer?: ToolQuestionAnswer[];
  }>(),
  {
    state: "pending",
    selectedAnswer: undefined,
  },
);

const emit = defineEmits<{
  /** Fired on successful submit — parent can re-derive state
   *  (e.g. update MessageItem's local view if needed). The card
   *  flips its own internal `localState` to "answered" right
   *  before this emit fires. */
  (e: "answered", answer: ToolQuestionAnswer[]): void;
  /** Fired on successful skip — card flips to "cancelled"
   *  right before. */
  (e: "cancelled"): void;
}>();

// ---------------------------------------------------------------------
// Local state — option selection accumulates per-question
// ---------------------------------------------------------------------

/** Per-question selected option labels. Outer array aligns with
 *  `props.questions[i]`; inner array is the labels the user
 *  picked (single-select → 1 element; multi-select → N elements).
 *  Pre-submit: `new Set<string>()` per question; on submit: each
 *  Set is converted to a sorted array (preserves stable order on
 *  the LLM side). */
const selectedByQuestion = ref<Set<string>[]>(
  props.questions.map(() => new Set<string>()),
);

/** Reactive view of "did the user answer every question?".
 *  Single-select → 1 label required; multi-select → ≥1 label
 *  required. A missing `options` array on a question (defensive
 *  — backend should always send it) fails the gate (the question
 *  has no selectable options, so it can't be answered). */
const allAnswered = computed<boolean>(() =>
  props.questions.every((q, i) => {
    if (!Array.isArray(q.options) || q.options.length === 0) return false;
    const sel = selectedByQuestion.value[i];
    if (!sel) return false;
    if (q.multi_select) return sel.size >= 1;
    return sel.size === 1;
  }),
);

/** Card's local view of the state. Tracks the prop on mount
 *  but flips on submit / skip so the bottom action row
 *  transitions to "answered" / "cancelled" without parent
 *  re-rendering. */
const localState = ref<QuestionCardState>(props.state);

/** In-flight submit guard — disables both buttons while the
 *  resolve IPC is pending. The optimistic state flip (localState
 *  → "answered") happens AFTER the IPC resolves; if the IPC
 *  rejects, we revert and the user can retry. */
const submitting = ref<boolean>(false);

/** Last submit error (string message). Shown inline above the
 *  bottom row in the pending state when non-null; cleared on
 *  the next submit attempt. */
const submitError = ref<string | null>(null);

// ---------------------------------------------------------------------
// Selection helpers
// ---------------------------------------------------------------------

function toggleSelection(qIndex: number, label: string): void {
  const q = props.questions[qIndex];
  if (!q) return;
  const set = selectedByQuestion.value[qIndex];
  if (!set) return;
  if (q.multi_select) {
    // Multi-select: toggle membership.
    if (set.has(label)) set.delete(label);
    else set.add(label);
    // Force reactivity — `ref<Set<...>[]>` doesn't deep-track
    // Set mutations. Mutating an entry doesn't trip the proxy;
    // replacing the slot does. Cheap (one Set per render at most
    // — no ObservableMap needed for a 1..=4 entry list).
    selectedByQuestion.value = [...selectedByQuestion.value];
    return;
  }
  // Single-select: replace the slot with a singleton Set.
  selectedByQuestion.value[qIndex] = new Set([label]);
  // Force reactivity (re-place to notify dependents).
  selectedByQuestion.value = [...selectedByQuestion.value];
}

/** Click handler on the `<li>` row — catches clicks anywhere in
 *  the option (label, input, description, header chip). The
 *  `<input>` element itself uses `@click.stop` so a direct
 *  click on the radio / checkbox input does NOT fire this
 *  handler twice (the input's `@change` handles the state
 *  mutation; we just suppress the bubble). The preview
 *  `<details>` is `@click.stop`-ed too — expanding the preview
 *  panel must NOT toggle the option.
 *
 *  The event-target check guards against clicks landing on
 *  nested interactive children that haven't yet opted out via
 *  `.stop` (defensive — `.stop` on the input + details covers
 *  the current DOM, but future expansions like a per-option
 *  tooltip must respect the guard). */
function onOptionClick(event: MouseEvent, qIndex: number, label: string): void {
  if (localState.value !== "pending") return;
  const target = event.target as HTMLElement | null;
  if (!target) return;
  // Defensive: ignore clicks on interactive children that
  // haven't already .stop()-ed propagation. Currently the
  // input + details use `.stop`; this is belt-and-suspenders.
  const interactiveTags = new Set(["INPUT", "SUMMARY", "DETAILS", "A", "BUTTON"]);
  if (interactiveTags.has(target.tagName)) return;
  toggleSelection(qIndex, label);
}

function isSelected(qIndex: number, label: string): boolean {
  return !!selectedByQuestion.value[qIndex]?.has(label);
}

// ---------------------------------------------------------------------
// Build the answer array (per PRD R4 / wire §4.2 shape)
// ---------------------------------------------------------------------

function buildAnswer(): ToolQuestionAnswer[] {
  return props.questions.map((q, i) => {
    const set = selectedByQuestion.value[i] ?? new Set<string>();
    // Stable order: iterate `q.options` in payload order (the LLM
    // authored this list, so the user's selection order matches
    // the option list order — keeps the answer deterministic).
    const ordered = q.options
      .map((o) => o.label)
      .filter((label) => set.has(label));
    const answer: ToolQuestionAnswer = {
      question: q.question,
      options: ordered,
      multi_select: !!q.multi_select,
    };
    if (q.header !== undefined) answer.header = q.header;
    return answer;
  });
}

// ---------------------------------------------------------------------
// Submit / skip handlers (R7 / R7a / AC6 / AC9)
// ---------------------------------------------------------------------

async function handleSubmit(): Promise<void> {
  if (!allAnswered.value || submitting.value) return;
  submitting.value = true;
  submitError.value = null;
  const answer = buildAnswer();
  try {
    await resolveToolQuestion({
      session_id: props.sessionId,
      tool_use_id: props.toolUseId,
      answer,
    });
    // Optimistic state flip AFTER the IPC resolves — the user
    // sees the "已回答" pill immediately. If the IPC had
    // rejected, we'd have hit the catch below and reverted.
    localState.value = "answered";
    emit("answered", answer);
  } catch (e) {
    submitError.value = String(e);
  } finally {
    submitting.value = false;
  }
}

async function handleSkip(): Promise<void> {
  if (submitting.value) return;
  submitting.value = true;
  submitError.value = null;
  try {
    await resolveToolQuestion({
      session_id: props.sessionId,
      tool_use_id: props.toolUseId,
      cancelled: true,
    });
    localState.value = "cancelled";
    emit("cancelled");
  } catch (e) {
    submitError.value = String(e);
  } finally {
    submitting.value = false;
  }
}

// ---------------------------------------------------------------------
// Derived display helpers
// ---------------------------------------------------------------------

/** Map a single question + the local `selectedByQuestion` slot
 *  into the answer wire shape — used by the "answered" state
 *  summary row (no IPC; the local state already mirrors what was
 *  sent). */
const answeredSummary = computed<ToolQuestionAnswer[]>(() => {
  if (localState.value !== "answered") return [];
  // Prefer the parent's `selectedAnswer` prop (the canonical
  // post-submit answer) when present (rehydrated historical
  // card); fall back to building from local selection state for
  // the live-submit path.
  if (props.selectedAnswer && props.selectedAnswer.length > 0) {
    return props.selectedAnswer;
  }
  return buildAnswer();
});

function labelsForAnswer(answer: ToolQuestionAnswer): string[] {
  return answer.options;
}
</script>

<template>
  <div class="ask-card" data-testid="ask-card">
    <div class="ask-card__head">
      <span class="ask-card__head-icon">
        <Icon name="clipboard-list" :size="14" />
      </span>
      <span class="ask-card__head-title">等待你回答</span>
      <span
        v-if="localState === 'answered'"
        class="ask-card__state ask-card__state--answered"
        data-testid="ask-card-state-answered"
      >✓ 已回答</span>
      <span
        v-else-if="localState === 'cancelled'"
        class="ask-card__state ask-card__state--cancelled"
        data-testid="ask-card-state-cancelled"
      >⊘ 已跳过</span>
    </div>

    <div class="ask-card__sections">
      <section
        v-for="(q, qIndex) in questions"
        :key="qIndex"
        class="ask-card__section"
        :data-testid="`ask-card-question-${qIndex}`"
      >
        <header class="ask-card__section-header">
          <span
            v-if="q.header"
            class="ask-card__section-chip"
          >{{ q.header }}</span>
          <span class="ask-card__section-q">{{ q.question }}</span>
          <span
            v-if="q.multi_select"
            class="ask-card__section-mode"
            data-testid="ask-card-multi-badge"
          >多选</span>
        </header>

        <ul class="ask-card__options">
          <li
            v-for="opt in q.options"
            :key="opt.label"
            class="ask-card__option"
            :class="{
              'ask-card__option--selected': isSelected(qIndex, opt.label),
              'ask-card__option--disabled': localState !== 'pending',
            }"
            :data-testid="`ask-card-option-${qIndex}-${opt.label}`"
            role="button"
            :aria-pressed="localState === 'pending' ? isSelected(qIndex, opt.label) : undefined"
            @click="onOptionClick($event, qIndex, opt.label)"
          >
            <label class="ask-card__option-label">
              <input
                v-if="q.multi_select"
                type="checkbox"
                class="ask-card__option-input"
                :checked="isSelected(qIndex, opt.label)"
                :disabled="localState !== 'pending'"
                @change="toggleSelection(qIndex, opt.label)"
                @click.stop
              />
              <input
                v-else
                type="radio"
                class="ask-card__option-input"
                :name="`ask-card-q-${qIndex}`"
                :value="opt.label"
                :checked="isSelected(qIndex, opt.label)"
                :disabled="localState !== 'pending'"
                @change="toggleSelection(qIndex, opt.label)"
                @click.stop
              />
              <span class="ask-card__option-text">
                <span class="ask-card__option-label-text">{{ opt.label }}</span>
                <span
                  v-if="opt.description"
                  class="ask-card__option-desc"
                >{{ opt.description }}</span>
                <details
                  v-if="opt.preview"
                  class="ask-card__option-preview"
                  @click.stop
                >
                  <summary class="ask-card__option-preview-summary">预览</summary>
                  <pre class="ask-card__option-preview-body">{{ opt.preview }}</pre>
                </details>
              </span>
            </label>
          </li>
        </ul>
      </section>
    </div>

    <!--
      Bottom action row. Three states:
        - pending  → 提交 + 跳过 buttons + optional error line
        - answered → "已回答" summary (per-question label list)
        - cancelled → "已跳过" note
      R7a: card stays expanded in answered / cancelled (full
      questions + selected highlight remain visible). The status
      pill + summary row carry the post-submit signal without
      collapsing the content.
    -->
    <div v-if="localState === 'pending'" class="ask-card__actions">
      <p
        v-if="submitError"
        class="ask-card__error"
        role="alert"
        data-testid="ask-card-error"
      >提交失败: {{ submitError }}</p>
      <button
        type="button"
        class="ask-card__btn ask-card__btn--primary"
        :disabled="!allAnswered || submitting"
        data-testid="ask-card-submit"
        @click="handleSubmit"
      >
        <Icon name="check" :size="12" />
        提交
      </button>
      <button
        type="button"
        class="ask-card__btn"
        :disabled="submitting"
        data-testid="ask-card-skip"
        @click="handleSkip"
      >跳过</button>
    </div>

    <div
      v-else-if="localState === 'answered'"
      class="ask-card__summary"
      data-testid="ask-card-summary"
    >
      <p
        v-for="(answer, aIndex) in answeredSummary"
        :key="aIndex"
        class="ask-card__summary-row"
      >
        <span
          v-if="answer.header"
          class="ask-card__summary-chip"
        >{{ answer.header }}</span>
        <span class="ask-card__summary-q">{{ answer.question }}</span>
        <span class="ask-card__summary-labels">
          <span
            v-for="label in labelsForAnswer(answer)"
            :key="label"
            class="ask-card__summary-label"
          >{{ label }}</span>
        </span>
      </p>
    </div>

    <div
      v-else
      class="ask-card__cancelled-note"
      data-testid="ask-card-cancelled-note"
    >用户跳过此问题,LLM 会按取消处理。</div>
  </div>
</template>

<style scoped>
/*
 * Visual contract — reuses the project's ToolCallCard chrome
 * tokens (per PRD R6 / design §5.5: "复用 ToolCallCard 现有
 * Card 样式系统"). No new color tokens; spacing is the 4-based
 * scale. All CSS references project tokens — no hardcoded hex
 * (design-tokens.md: "组件 CSS MUST reference the tokens").
 *
 * Root class `.ask-card` is namespaced (no collision with
 * `.tool-card` / `.permission-ask-body` etc).
 */
.ask-card {
  margin-top: 8px;
  padding: var(--space-3);
  background: var(--color-bg-surface);
  border: 1px solid var(--color-bg-border);
  border-left: 3px solid var(--color-accent);
  border-radius: var(--radius-md);
  font-family: var(--font-sans);
  color: var(--color-text-primary);
  display: flex;
  flex-direction: column;
  gap: var(--space-3);
}

.ask-card__head {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  font-size: var(--text-sm);
  font-weight: var(--weight-semibold);
}

.ask-card__head-icon {
  display: inline-flex;
  color: var(--color-accent);
  flex-shrink: 0;
}

.ask-card__head-title {
  flex: 1;
}

/* Status pill (answered / cancelled). Uses the existing tool
   color tokens — `--color-tool-write` (emerald) for the
   successful answer state, `--color-tool-error` (red) for
   the cancelled state. Mirrors `PermissionAskBody` outcome
   badge color conventions (same tokens, no one-off colors). */
.ask-card__state {
  font-size: var(--text-xs);
  font-family: var(--font-mono);
  padding: 2px 8px;
  border-radius: var(--radius-pill);
  border: 1px solid currentColor;
}

.ask-card__state--answered {
  color: var(--color-tool-write);
}

.ask-card__state--cancelled {
  color: var(--color-tool-error);
}

.ask-card__sections {
  display: flex;
  flex-direction: column;
  gap: var(--space-3);
}

.ask-card__section {
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
}

.ask-card__section-header {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  flex-wrap: wrap;
}

.ask-card__section-chip {
  font-family: var(--font-mono);
  font-size: var(--text-2xs);
  text-transform: uppercase;
  letter-spacing: 0.04em;
  padding: 1px 6px;
  border: 1px solid var(--color-bg-border-strong);
  border-radius: var(--radius-sm);
  color: var(--color-text-secondary);
  background: var(--color-bg-elevated);
  flex-shrink: 0;
}

.ask-card__section-q {
  font-size: var(--text-sm);
  font-weight: var(--weight-medium);
  flex: 1;
  min-width: 0;
}

.ask-card__section-mode {
  font-size: var(--text-2xs);
  font-family: var(--font-mono);
  color: var(--color-text-muted);
  border: 1px solid var(--color-bg-border-strong);
  border-radius: var(--radius-sm);
  padding: 1px 6px;
  flex-shrink: 0;
}

.ask-card__options {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: var(--space-1);
}

.ask-card__option {
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-sm);
  background: var(--color-bg-app);
  transition: border-color var(--duration-fast) var(--ease-out),
    background-color var(--duration-fast) var(--ease-out);
}

.ask-card__option:hover {
  border-color: var(--color-bg-border-strong);
}

.ask-card__option--selected {
  border-color: var(--color-accent);
  background: var(--color-bg-selected);
}

.ask-card__option--disabled {
  cursor: default;
}

.ask-card__option--disabled:hover {
  border-color: var(--color-bg-border);
}

.ask-card__option-label {
  display: flex;
  align-items: flex-start;
  gap: var(--space-2);
  padding: var(--space-2) var(--space-3);
  cursor: pointer;
}

.ask-card__option--disabled .ask-card__option-label {
  cursor: default;
}

/* Hide the native radio / checkbox visuals and replace with the
   project-styled accent dot/checkmark. We keep the native input
   for a11y (keyboard navigation, focus, form semantics) but
   visually replace it. */
.ask-card__option-input {
  margin: 2px 0 0 0;
  width: 14px;
  height: 14px;
  flex-shrink: 0;
  cursor: pointer;
  accent-color: var(--color-accent);
}

.ask-card__option--disabled .ask-card__option-input {
  cursor: default;
}

.ask-card__option-text {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.ask-card__option-label-text {
  font-size: var(--text-sm);
  color: var(--color-text-primary);
}

.ask-card__option-desc {
  font-size: var(--text-xs);
  color: var(--color-text-muted);
  line-height: var(--leading-normal);
}

.ask-card__option-preview {
  margin-top: var(--space-1);
  font-size: var(--text-xs);
}

.ask-card__option-preview-summary {
  cursor: pointer;
  color: var(--color-accent);
  user-select: none;
  padding: 2px 0;
}

.ask-card__option-preview-body {
  margin: 4px 0 0 0;
  padding: var(--space-2);
  background: var(--color-bg-app);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-sm);
  font-family: var(--font-mono);
  font-size: var(--text-xs);
  color: var(--color-text-secondary);
  white-space: pre-wrap;
  word-break: break-word;
  max-height: 160px;
  overflow: auto;
}

/* Bottom action row (R7 / R7a / AC6). */
.ask-card__actions {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  flex-wrap: wrap;
}

.ask-card__btn {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 6px 12px;
  background: var(--color-bg-elevated);
  color: var(--color-text-primary);
  border: 1px solid var(--color-bg-border-strong);
  border-radius: var(--radius-sm);
  font-size: var(--text-sm);
  font-family: inherit;
  cursor: pointer;
  transition: background-color var(--duration-fast) var(--ease-out),
    border-color var(--duration-fast) var(--ease-out);
}

.ask-card__btn:hover:not(:disabled) {
  background: var(--color-bg-hover);
  border-color: var(--color-accent);
}

.ask-card__btn:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}

.ask-card__btn--primary {
  background: var(--color-accent);
  color: var(--color-text-on-accent);
  border-color: var(--color-accent);
}

.ask-card__btn--primary:hover:not(:disabled) {
  background: var(--color-accent-hover);
  border-color: var(--color-accent-hover);
}

.ask-card__error {
  flex: 1;
  margin: 0;
  font-size: var(--text-xs);
  color: var(--color-tool-error);
  font-family: var(--font-mono);
}

/* Answered-state summary (R7a: "答完保留展开全程"). Each row
   shows the question's header chip + body + the labels the
   user picked (rendered as small accent-tinted pills). Keeps
   the full card visible alongside the summary. */
.ask-card__summary {
  display: flex;
  flex-direction: column;
  gap: var(--space-1);
}

.ask-card__summary-row {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  flex-wrap: wrap;
  margin: 0;
  font-size: var(--text-xs);
  color: var(--color-text-secondary);
}

.ask-card__summary-chip {
  font-family: var(--font-mono);
  font-size: var(--text-2xs);
  text-transform: uppercase;
  letter-spacing: 0.04em;
  padding: 1px 6px;
  border: 1px solid var(--color-bg-border-strong);
  border-radius: var(--radius-sm);
  color: var(--color-text-secondary);
  background: var(--color-bg-elevated);
  flex-shrink: 0;
}

.ask-card__summary-q {
  color: var(--color-text-muted);
}

.ask-card__summary-labels {
  display: inline-flex;
  flex-wrap: wrap;
  gap: 4px;
}

.ask-card__summary-label {
  font-family: var(--font-mono);
  font-size: var(--text-2xs);
  padding: 1px 6px;
  border: 1px solid var(--color-accent);
  color: var(--color-accent);
  border-radius: var(--radius-pill);
  background: var(--color-accent-muted);
}

.ask-card__cancelled-note {
  font-size: var(--text-xs);
  color: var(--color-tool-error);
  font-style: italic;
}
</style>