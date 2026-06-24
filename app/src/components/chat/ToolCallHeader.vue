<script setup lang="ts">
// ToolCallHeader — 共享 tool-card header（RULE-FrontSubagent-001, 2026-06-25）
//
// 抽自 ToolCallCard / DrawerToolCallCard / DrawerPermissionAskCard 三处
// 1:1 镜像的 header markup + CSS。这三处 header 原本各自重声明了 ~50-60
// 行 `.tool-card__header*` / `.drawer-tool-card__header*` /
// `.drawer-permission-ask-card__header*` 规则（class 改名避 scoped 碰撞，
// 规则体 1:1 相同）。本组件把 header 的 markup + CSS 收成单一来源。
//
// **推翻 frontend/chat.md 旧决策**（2026-06-25）：chat.md 原记「不抽
// ToolCallHeader.vue —— PRD Risk 表锁主 panel ToolCallCard 本体 0 改动」。
// 该约束是 B6 redesign PR4 时的临时保护（主路径 0 改动），**redesign
// PR1-6 已全部收尾，约束解除** —— 故本组件得以抽取（见本 task
// `06-25-debt-frontsubagent-refactor` PRD）。
//
// 设计：纯展示，0 store。接收调用方在 script setup 里算好的 props：
//   - ToolCallCard：iconName=toolIcon(name) / filePath / statusText（含
//     dispatch 分支 workerStatusText）/ statusIconName / durationLabel +
//     diff-btn 走 #status-extra slot
//   - DrawerToolCallCard：同 ToolCallCard 但无 slot（无 diff-btn）
//   - DrawerPermissionAskCard：iconName="shield-check" / suffix="权限询问" /
//     statusText / statusVariant="accent"（interactive 时）
//
// 差异点用可选 props 处理（filePath 仅 tool 变体 / suffix 仅 permission
// 变体 / statusIconName+durationLabel 仅 tool 变体）。error/running 颜色
// 改 isError/isRunning prop 驱动（不再靠外部 card root `--error` 后代
// 选择器），header 视觉自洽、可独立测试。

import Icon from "../Icon.vue";

const props = withDefaults(
  defineProps<{
    /** Tool icon name（heroicons key，调用方 toolIcon(name) 算好）。 */
    iconName: string;
    /** Header 标题 —— tool 变体是 tool name，permission 变体是 toolName/fallback。 */
    name: string;
    /** Tool 变体：文件路径 chip（"· /foo"）。permission 变体不传。null/空 则不渲染。 */
    filePath?: string | null;
    /** Permission 变体：后缀标签（如 "权限询问"）。tool 变体不传。 */
    suffix?: string;
    /** 状态文本（右侧）。调用方预算（ToolCallCard 含 dispatch 分支）。 */
    statusText: string;
    /** 状态 icon 名；不传则不渲染 status icon（permission 变体无）。 */
    statusIconName?: string;
    /** 单 tool 耗时标签（"0.3s" / "…"）；不传或空则不渲染。 */
    durationLabel?: string;
    /** 错误态 —— icon/name/status/duration 翻 error 色。 */
    isError?: boolean;
    /** 运行态 —— 驱动 status-icon pulse 动画。 */
    isRunning?: boolean;
    /** 状态行配色：default（muted）/ accent（permission interactive 强调）。 */
    statusVariant?: "default" | "accent";
  }>(),
  {
    filePath: null,
    suffix: undefined,
    statusIconName: undefined,
    durationLabel: undefined,
    isError: false,
    isRunning: false,
    statusVariant: "default",
  },
);
</script>

<template>
  <div
    class="tool-call-header"
    :class="{
      'tool-call-header--error': props.isError,
      'tool-call-header--running': props.isRunning,
      'tool-call-header--status-accent': props.statusVariant === 'accent',
    }"
  >
    <div class="tool-call-header__title">
      <span class="tool-call-header__icon">
        <Icon :name="props.iconName" :size="14" />
      </span>
      <span class="tool-call-header__name">{{ props.name }}</span>
      <span
        v-if="props.filePath"
        class="tool-call-header__path"
        :title="props.filePath"
      >
        · {{ props.filePath }}
      </span>
      <span v-if="props.suffix" class="tool-call-header__suffix">
        {{ props.suffix }}
      </span>
    </div>
    <div class="tool-call-header__status">
      <span
        v-if="props.statusIconName"
        :class="[
          'tool-call-header__status-icon',
          { 'tool-call-header__status-icon--running': props.isRunning },
        ]"
      >
        <Icon :name="props.statusIconName" :size="14" />
      </span>
      <span>{{ props.statusText }}</span>
      <span v-if="props.durationLabel" class="tool-call-header__duration">{{
        props.durationLabel
      }}</span>
      <!-- ToolCallCard 的 diff-btn 走此 slot；drawer 变体不传则空。 -->
      <slot name="status-extra" />
    </div>
  </div>
</template>

<style scoped>
/* 单一来源：原 .tool-card__header* / .drawer-tool-card__header* /
   .drawer-permission-ask-card__header* 三处 1:1 镜像规则的合并。全 design
   token，0 hex。error/running/accent 颜色由 prop 驱动的 modifier class 控制。 */

.tool-call-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  min-width: 0;
}

.tool-call-header__title {
  display: inline-flex;
  align-items: baseline;
  gap: 6px;
  min-width: 0;
  flex: 1;
  overflow: hidden;
  white-space: nowrap;
}

.tool-call-header__icon {
  flex-shrink: 0;
  display: inline-flex;
  align-items: center;
  color: var(--color-text-secondary);
}

.tool-call-header--error .tool-call-header__icon {
  color: var(--color-tool-error);
}

.tool-call-header__name {
  font-weight: 600;
  color: var(--color-text-primary);
}

.tool-call-header--error .tool-call-header__name {
  color: var(--color-tool-error);
}

.tool-call-header__path {
  color: var(--color-text-secondary);
  font-size: 11px;
  overflow: hidden;
  text-overflow: ellipsis;
  min-width: 0;
  flex: 1;
}

.tool-call-header__suffix {
  color: var(--color-text-muted);
  font-size: 11px;
}

.tool-call-header__status {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  font-size: 11px;
  color: var(--color-text-muted);
  flex-shrink: 0;
}

.tool-call-header__status-icon {
  display: inline-flex;
  align-items: center;
  line-height: 1;
}

.tool-call-header__status-icon--running {
  animation: tool-call-header-pulse 1.4s ease-in-out infinite;
}

@keyframes tool-call-header-pulse {
  0%,
  100% {
    opacity: 1;
  }
  50% {
    opacity: 0.35;
  }
}

.tool-call-header--error .tool-call-header__status {
  color: var(--color-tool-error);
}

/* Permission interactive：status 用 accent 色吸引用户注意（原
   .drawer-permission-ask-card--interactive .status 规则）。 */
.tool-call-header--status-accent .tool-call-header__status {
  color: var(--color-accent);
  font-weight: 600;
}

.tool-call-header__duration {
  display: inline-flex;
  align-items: center;
  margin-left: 2px;
  font-size: 11px;
  font-family: var(--font-mono);
  color: var(--color-text-secondary);
  font-weight: 500;
  user-select: none;
}

.tool-call-header--error .tool-call-header__duration {
  color: var(--color-tool-error);
}
</style>
