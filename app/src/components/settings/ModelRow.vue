<script setup lang="ts">
// ModelRow — single row in the Models list. Displays one model's
// name, model_id, tags (thinking / context window), inline test
// result, and three action buttons (test / edit / delete).
//
// PR5 follow-up: the per-row Test result is rendered inline in
// `.models-tab__row-test` (now `.model-row__test` after 8-PR3)
// and persists until either (a) the user clicks Test again, or
// (b) the row is deleted. The test state lives in the parent
// (ModelsTab) so it survives switching tabs — switching
// providers or editing the model fields intentionally does NOT
// clear the result.

import { computed } from "vue";
import type { ModelWithProvider } from "../../stores/models";
import Icon from "../Icon.vue";

export type TestState =
  | { kind: "running" }
  | { kind: "ok"; latencyMs: number }
  | { kind: "fail"; error: string };

const props = defineProps<{
    model: ModelWithProvider;
    /** Per-row test result. Undefined means "never tested". */
    test: TestState | undefined;
    /** Streaming flag for the chat session (currently unused by
     *  this row but kept on the props shape for future
     *  "disable-actions-during-stream" parity with WorktreeChip). */
    isStreaming: boolean;
    /** 2026-09-07 (provider-model-disable follow-up): 该行是否为
     *  当前全局默认模型(modelsStore.defaultModelId)。默认模型的
     *  禁用开关被禁用 —— 禁用是选用层开关,而默认模型是新会话的
     *  静默回退,禁用它会造成「开关已禁但新会话仍在用」的矛盾;
     *  要停用默认模型,先去 Default 页换默认。 */
    isDefault: boolean;
}>();

const emit = defineEmits<{
    /** User clicked the Test button — invoke the `test_model` IPC
     *  in the parent. */
    test: [];
    /** User clicked the Edit (pencil) button — switch the parent
     *  into `edit` mode and seed the form. */
    edit: [];
    /** User clicked the Delete (trash) button — open the parent's
     *  delete-confirm overlay. */
    delete: [];
    /** 2026-09-07 (provider-model-disable): user clicked the power
     *  button — flip THIS model's disabled flag via the parent
     *  (`modelsStore.setDisabled`). Provider-level disabling lives
     *  on ProvidersTab; here we only surface its effect. */
    "toggle-disabled": [];
}>();

/** 2026-09-07 (provider-model-disable): 有效禁用 = 模型级 OR 父
 *  provider 级。徽标文案区分来源,开关只翻模型级(provider 级去
 *  ProvidersTab 关)。 */
const modelDisabled = computed<boolean>(() => !!props.model.disabled);
const providerDisabledOnly = computed<boolean>(
    () => !props.model.disabled && !!props.model.providerDisabled,
);

/** 2026-09-07 follow-up:默认模型的「禁用」方向被拦截;若默认模型
 *  已处于禁用态(存量数据/竞态),「启用」方向仍放开 —— 那是恢复
 *  正常态,不该被误伤。 */
const disableToggleLocked = computed<boolean>(
    () => props.isDefault && !modelDisabled.value,
);

/** PR5: per-row Test result rendering helpers. Extracted from
 *  the template so the runtime narrowing happens in TypeScript
 *  (the template language doesn't allow `as` casts). */
const testClass = computed<Record<string, boolean>>(() => {
    const t = props.test;
    if (!t) return {} as Record<string, boolean>;
    return {
        "model-row__test--ok": t.kind === "ok",
        "model-row__test--fail": t.kind === "fail",
        "model-row__test--running": t.kind === "running",
    } as Record<string, boolean>;
});

const okLatency = computed<number>(() =>
    props.test?.kind === "ok" ? props.test.latencyMs : 0,
);

const failError = computed<string>(() =>
    props.test?.kind === "fail" ? props.test.error : "",
);

const isRunning = computed<boolean>(() => props.test?.kind === "running");

const testTitle = computed<string>(() =>
    isRunning.value ? "测试中…" : "测试此 model 连通性",
);
</script>

<template>
    <div class="model-row" :class="{ 'model-row--disabled': modelDisabled || providerDisabledOnly }">
        <div class="model-row__info">
            <span class="model-row__name">{{ model.displayName }}</span>
            <span class="model-row__model-id">{{ model.modelName }}</span>
            <!-- 2026-09-07 (provider-model-disable): 禁用徽标。模型级
                 显式「已禁用」;provider 级连坐时显式来源(该模型的
                 自身开关仍可能在启用位)。 -->
            <span
                v-if="modelDisabled"
                class="model-row__tag model-row__tag--disabled"
                title="已禁用:不出现在模型选择列表;已在用的会话不受影响"
            >
                已禁用
            </span>
            <span
                v-else-if="providerDisabledOnly"
                class="model-row__tag model-row__tag--disabled"
                title="所属 provider 已禁用(在 Providers 页启用后恢复可选)"
            >
                provider 已禁用
            </span>
            <span v-if="model.supportsThinking" class="model-row__tag">
                thinking
            </span>
            <!-- B1 R1: vision capability tag — same tag treatment as
                 thinking. -->
            <span v-if="model.supportsImages" class="model-row__tag">
                vision
            </span>
            <span class="model-row__tag model-row__tag--muted">
                {{
                    model.contextWindow >= 1000
                        ? `${model.contextWindow / 1000}k`
                        : model.contextWindow
                }}
            </span>
            <!-- PR5: per-row Test result, inline. The label appears
                 under the model_id so the row's vertical rhythm is
                 unchanged on the success / never-tested path. -->
            <span
                v-if="test"
                class="model-row__test"
                :class="testClass"
            >
                <template v-if="test.kind === 'running'">
                    测试中…
                </template>
                <template v-else-if="test.kind === 'ok'">
                    <Icon name="check" :size="12" />
                    通过 ({{ okLatency }}ms)
                </template>
                <template v-else>
                    <Icon name="warn" :size="12" />
                    {{ failError }}
                </template>
            </span>
        </div>
        <div class="model-row__actions">
            <button
                type="button"
                class="model-row__btn model-row__btn--ghost btn btn--icon btn--ghost"
                :class="{ 'model-row__btn--off': modelDisabled }"
                :disabled="isRunning || disableToggleLocked"
                :title="disableToggleLocked
                    ? '默认模型不能禁用——请先在 Default 页更换默认模型'
                    : (modelDisabled
                        ? '启用该模型(重新进入选择列表)'
                        : '禁用该模型(从选择列表隐藏,不影响已在用的会话)')"
                :aria-label="disableToggleLocked
                    ? `默认模型 ${model.displayName} 不能禁用`
                    : (modelDisabled ? `启用模型 ${model.displayName}` : `禁用模型 ${model.displayName}`)"
                :data-testid="`model-toggle-disabled-${model.id}`"
                @click="emit('toggle-disabled')"
            >
                <Icon name="power" :size="12" />
            </button>
            <button
                type="button"
                class="model-row__btn model-row__btn--ghost btn btn--icon btn--ghost"
                :disabled="isRunning"
                :title="testTitle"
                @click="emit('test')"
            >
                <Icon name="signal" :size="12" />
            </button>
            <button
                type="button"
                class="model-row__btn model-row__btn--ghost btn btn--icon btn--ghost"
                @click="emit('edit')"
            >
                <Icon name="pencil" :size="12" />
            </button>
            <button
                type="button"
                class="model-row__btn model-row__btn--ghost model-row__btn--danger btn btn--icon btn--danger-soft"
                @click="emit('delete')"
            >
                <Icon name="trash" :size="12" />
            </button>
        </div>
    </div>
</template>

<style scoped>
.model-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    padding: 8px 12px;
    background: var(--color-bg-elevated);
    border: 1px solid var(--color-bg-border);
    border-radius: var(--radius-md);
}

.model-row__info {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
    flex: 1;
}

.model-row__name {
    font-size: var(--text-base);
    font-weight: var(--weight-medium);
    color: var(--color-text-primary);
}

.model-row__model-id {
    font-size: var(--text-xs);
    color: var(--color-text-muted);
    font-family: var(--font-mono);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
}

.model-row__tag {
    font-size: var(--text-2xs);
    padding: 1px 6px;
    border-radius: 3px;
    background: var(--color-accent-muted);
    color: var(--color-accent-text);
    font-family: var(--font-mono);
    flex-shrink: 0;
}

.model-row__tag--muted {
    background: var(--color-bg-border);
    color: var(--color-text-muted);
}

/* 2026-09-07 (provider-model-disable): 禁用徽标 + 禁用行降透明。
   行保持可读可操作(编辑 / 启用 / 测试),只是一眼可辨"不在选用
   列表里"。 */
.model-row__tag--disabled {
    background: var(--color-bg-border);
    color: var(--color-text-muted);
    flex-shrink: 0;
}

.model-row--disabled .model-row__name,
.model-row--disabled .model-row__model-id {
    opacity: 0.55;
}

.model-row__btn--off {
    color: var(--color-text-muted);
}

/* PR5: per-row Test result badge. Inline with the model_id so
   the row's vertical rhythm matches the pre-PR5 layout. The
   running state uses the muted text color (it'll resolve to ok
   or fail shortly); the success / fail states use the same
   tool-color tokens as the rest of the settings tabs. */
.model-row__test {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    font-size: var(--text-2xs);
    font-family: var(--font-mono);
    color: var(--color-text-muted);
    flex-shrink: 0;
}

.model-row__test--ok {
    color: var(--color-tool-write);
}

.model-row__test--fail {
    color: var(--color-tool-error-text);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 240px;
}

.model-row__test--running {
    color: var(--color-text-muted);
}

.model-row__actions {
    display: flex;
    gap: 4px;
    flex-shrink: 0;
}

/* 行内 icon 钮样式由全局 .btn 家族承载(ghost = icon ghost /
   danger = icon danger-soft);纯 icon 无文字,原 font-weight 无渲染
   效果不保留。下方移动端块是 32px 触摸目标守卫的显式几何,保留。 */

/* --- S6b 移动端适配(08-13-mobile-settings, 320-430px) ---
 * Models tab 溢出检查:行内 3 个操作按钮被 44px 全局块放大后,长
 * provider/model 名在窄屏被挤爆 —— 轻量补 name ellipsis 守卫,不改布局。 */
@media (max-width: 767px) {
    /* 真机迭代第三轮(2026-08-13):"放不下就弄两行"。
       桌面 model-row 单行: name + id + thinking-badge + token-badge + test-badge
       + 3 个 actions。在 360px 单行根本塞不下,且 actions 已 32px,
       信息堆叠反而看不出主信息。 → 移动端分两行:
         Row 1: name(可缩略) + actions(右靠)
         Row 2: id + thinking + token + test(muted 一行小字,辅助信息)
       name 单独一行 + actions 单独靠右,信息层级清晰。 */
    .model-row {
        flex-wrap: wrap;
        align-items: center;
    }
    .model-row__info {
        flex-wrap: wrap;
        row-gap: 4px;
        /* name 占第一行;id+tag+test 占第二行 */
    }
    .model-row__name {
        flex-basis: 100%;
        order: 0;
        white-space: nowrap;
        overflow: hidden;
        text-overflow: ellipsis;
        max-width: 100%;
    }
    .model-row__model-id,
    .model-row__tag,
    .model-row__test {
        order: 1;
    }
    .model-row__model-id {
        font-size: var(--text-2xs);
        /* id 在第二行可换行(不受 nowrap 限制) */
        white-space: normal;
        overflow-wrap: anywhere;
    }
    /* 真机迭代第二轮(2026-08-13):3 个 action 按钮被全局 44px 撑成
       132px,把 name/info 区挤到 0 宽(name 截成 "Min…")。回缩到
       32px,与 ProvidersTab 卡片内 actions 风格一致(DEC-6 精神:
       44px 只给主操作,卡片内低频 icon 操作 32px)。
       actions 单独靠右,自然换行;name 宽度不受其影响。 */
    .model-row__btn {
        width: 32px;
        height: 32px;
        min-width: 32px;
        min-height: 32px;
        padding: 0;
        justify-content: center;
    }
}
</style>