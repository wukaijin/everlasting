<script setup lang="ts">
// ToastProvider — reka-ui 2.9.9 Toast primitive 包装(A5 R1 / scope B,
// 2026-07-17)。
//
// 设计动机:reka-ui 2.9.9 已锁定的 8 类 primitive 之一(新增,第 8 类 —
// Dialog/Tabs/Select/Checkbox/RadioGroup/Label/Tooltip + Toast),整个项目
// `useToast()` 队列渲染的承载者。挂载在 `AppShell.vue`(F3)顶层,1 次 mount
// 全局可见。
//
// 关键设计:
// - **六件套**(同 Tooltip / Dialog 模式):ToastProvider + ToastViewport +
//   (per toast) ToastRoot + ToastTitle + ToastDescription + ToastClose。
//   `ToastProvider` 必须 wrap,否则 `ToastProviderContext` 找不到,运行时崩
//   (同 TooltipProvider 教训)。
// - **颜色**:4 类 category 走项目 CSS variable —
//   Auth → --color-accent / RateLimit → --color-status-warn /
//   Server → --color-tool-error / Network → --color-tool-shell。
//   Description 行的低对比色文本,适配 dark theme。
// - **位置**:右上(ToastViewport 默认 `position: fixed; top/right`)。
// - **portal**:reka-ui `ToastViewport` 内部 portal 到 `<body>`,`:deep()`
//   必加(见 `.trellis/spec/frontend/reka-ui-usage.md` §"Gotcha")。
// - **手动 × 关闭**:reka-ui `ToastClose` 自动 wire `update:open` 触发
//   `dismiss` —— 不需要手写 click handler。
//
// 与现有 toast 的关系:`projectsStore.toast`(AppShell 底部居中,单 slot)
// 仍然管项目内 IPC 失败反馈;本 toast 管「全局兜底」类错误(4 类路由决策
// 由 `useErrorBus.routeByCategory` 决定,接入范围见
// research/05-useErrorBus-stub-callsites.md)。

import { useToast, type Toast } from "../../composables/useToast";
import {
  ToastProvider,
  ToastViewport,
  ToastRoot,
  ToastTitle,
  ToastDescription,
  ToastClose,
} from "reka-ui";

const { toasts, dismiss } = useToast();

/** Map toast category → semantic color token (CSS variable)。 */
function categoryColor(category: Toast["category"]): string {
  switch (category) {
    case "Auth":
      return "var(--color-accent)";
    case "RateLimit":
      return "var(--color-status-warn)";
    case "Server":
      return "var(--color-tool-error)";
    case "Network":
      return "var(--color-tool-shell)";
  }
}

/** Map toast category → 中文短标题前缀(用户在 description 看完整 message)。 */
function categoryPrefix(category: Toast["category"]): string {
  switch (category) {
    case "Auth":
      return "鉴权失败";
    case "RateLimit":
      return "请求过于频繁";
    case "Server":
      return "服务端错误";
    case "Network":
      return "网络问题";
  }
}

/** Render hooks — 把 toast 的 id 透给 `dismiss`。ToastClose 的 click
 *  会 emit `update:open=false`,reka-ui 自动 wire 到 v-model;我们手动
 *  也 wired 一个 fallback click handler 防御 reka-ui `@close` 没触发
 * 的极端场景。 */
function onToastOpenChange(open: boolean, id: string): void {
  if (!open) dismiss(id);
}
</script>

<template>
  <ToastProvider :duration="5000" :swipe-direction="'right'">
    <ToastViewport
      class="toast-provider__viewport"
      data-testid="toast-viewport"
    />

    <ToastRoot
      v-for="t in toasts"
      :key="t.id"
      class="toast-provider__root"
      :class="`toast-provider__root--${t.category.toLowerCase()}`"
      :duration="t.ttl"
      :style="{
        '--toast-accent': categoryColor(t.category),
      }"
      :data-testid="`toast-root-${t.category}`"
      @update:open="(open: boolean) => onToastOpenChange(open, t.id)"
    >
      <ToastTitle class="toast-provider__title">
        {{ categoryPrefix(t.category) }}
      </ToastTitle>
      <ToastDescription
        v-if="t.description"
        class="toast-provider__description"
      >
        {{ t.description }}
      </ToastDescription>
      <ToastClose
        class="toast-provider__close"
        aria-label="关闭通知"
        @click="dismiss(t.id)"
      >
        ×
      </ToastClose>
    </ToastRoot>
  </ToastProvider>
</template>

<style scoped>
/* Viewport portals to <body> (reka-ui ToastViewport default),所以
   MUST 使用 :deep(),匹配 portal 出去的 DOM。背景透明让 toast root
   拥有自己的背景色;位置固定右上,z-index 高于 modal mask(9999)
   但低于 settings 弹窗(5000+,见 popover-pattern.md)。 */
:deep(.toast-provider__viewport) {
  position: fixed;
  top: 16px;
  right: 16px;
  display: flex;
  flex-direction: column;
  gap: 8px;
  width: 360px;
  max-width: calc(100vw - 32px);
  z-index: 5500;
  padding: 0;
  margin: 0;
  list-style: none;
  outline: none;
}

:deep(.toast-provider__root) {
  --toast-accent: var(--color-accent);
  display: grid;
  grid-template-areas:
    "title close"
    "desc desc";
  grid-template-columns: 1fr auto;
  align-items: start;
  column-gap: 8px;
  padding: 12px 14px;
  border-radius: var(--radius-lg);
  background: var(--color-bg-elevated);
  border: 1px solid var(--color-bg-border-strong);
  box-shadow: var(--shadow-lg);
  color: var(--color-text-primary);
  font-size: var(--text-sm);
  font-family: var(--font-sans);
  /* 左侧 3px 色条 + 背景提亮 8% 该 category 色。 */
  border-left: 3px solid var(--toast-accent);
  background: color-mix(
    in srgb,
    var(--toast-accent) 8%,
    var(--color-bg-elevated)
  );
}

/* 4 类 category → 不同左色条 + 背景提亮值。
   Auth 用 --color-accent(蓝),RateLimit 用 warn(琥珀),Server 用 error(红),
   Network 用 shell(琥珀,与 RateLimit 区别仅依赖 description 文本)。
   颜色 token 复用项目已有 — 详见 design.md §1.1。 */
:deep(.toast-provider__root--auth) {
  --toast-accent: var(--color-accent);
}
:deep(.toast-provider__root--ratelimit) {
  --toast-accent: var(--color-status-warn);
}
:deep(.toast-provider__root--server) {
  --toast-accent: var(--color-tool-error);
}
:deep(.toast-provider__root--network) {
  --toast-accent: var(--color-tool-shell);
}

:deep(.toast-provider__title) {
  grid-area: title;
  margin: 0;
  font-weight: var(--weight-semibold);
  font-size: var(--text-sm);
  color: var(--color-text-primary);
}

:deep(.toast-provider__description) {
  grid-area: desc;
  margin: 4px 0 0 0;
  font-size: var(--text-xs);
  color: var(--color-text-secondary);
  line-height: var(--leading-normal);
  word-break: break-word;
}

:deep(.toast-provider__close) {
  grid-area: close;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 22px;
  height: 22px;
  margin: 0;
  padding: 0;
  background: transparent;
  border: 0;
  border-radius: var(--radius-sm);
  color: var(--color-text-muted);
  font-size: var(--text-md);
  line-height: 1;
  cursor: pointer;
  user-select: none;
  font-family: inherit;
}
:deep(.toast-provider__close:hover) {
  background: var(--color-bg-hover);
  color: var(--color-text-primary);
}

/* reka-ui ToastRoot 在 enter/leave 用 data-state 切动画。
   入场 240ms + ease-out(参考 popover-pattern.md "Toast(AppShell)" motion),
   离场 100ms + ease-in — 与 popover 一致。 */
:deep(.toast-provider__root[data-state="open"]) {
  animation: toast-provider-in var(--duration-slow) var(--ease-out);
}
:deep(.toast-provider__root[data-state="closed"]) {
  animation: toast-provider-out var(--duration-fast) ease-in;
}

@keyframes toast-provider-in {
  from {
    opacity: 0;
    transform: translateX(8px);
  }
  to {
    opacity: 1;
    transform: translateX(0);
  }
}
@keyframes toast-provider-out {
  from {
    opacity: 1;
    transform: translateX(0);
  }
  to {
    opacity: 0;
    transform: translateX(8px);
  }
}
</style>
