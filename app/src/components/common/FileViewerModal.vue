<script setup lang="ts">
// FileViewerModal — 非图片文件路径预览弹层(2026-09-13)。
//
// markdown/工具输出里的本地文件路径(utils/markdown.ts linkifyLocalPaths
// 产物)经 useCodeBlockCopy 的点击委托 useFileViewer.open(原始路径) 打开
// 这里。全局唯一实例挂 App.vue(ImageViewerModal 旁),所有绑了
// onMarkdownClick 的 markdown 面共用。
//
// 结构镜像 ImageViewerModal(reka-ui Dialog 六件套:Esc / 遮罩点击 /
// focus trap / Portal 语义开箱即用;取数状态由 useFileViewer 模块级单例
// 驱动,fetch 在 open() 里做 —— cwd 解析也在彼时完成,见其模块注释)。
//
// 渲染分派(按扩展名,composable 的 mode):
//   - `.md`/`.markdown` → renderMarkdown 管线(与聊天气泡同路径),根上
//     绑 onMarkdownClick —— 弹层内容里的嵌套图片/文件路径递归可点
//     (spec §5 坑:新增 markdown 容器忘绑委托 = 交互静默失效);
//   - 其余文本类 → 构造 `{type:'code_block'}` UiPrimitive 复用
//     CodeBlockPrimitive(hljs 高亮 + 复制按钮免费;hljs 未知名走
//     highlightAuto);
//   - `.pdf` 根本不进弹层(open() 直接 window.open,见 useFileViewer)。
//
// 错误态:fetch 非 200(daemon 400 非白名单/相对路径 / 404 文件没了 /
// 413 超限)与网络失败统一文案 + 「新标签打开」兜底(ImageViewerModal
// 同款;文本类新标签见到的也是 text/plain 源码,语义一致)。

import {
  DialogRoot,
  DialogPortal,
  DialogOverlay,
  DialogContent,
  DialogTitle,
  DialogClose,
} from "reka-ui";

import { computed } from "vue";
import { useFileViewer } from "../../composables/useFileViewer";
import { useCodeBlockCopy } from "../../composables/useCodeBlockCopy";
import { renderMarkdown } from "../../utils/markdown";
import { fileUrl } from "../../utils/imageUrl";
import CodeBlockPrimitive from "../chat/primitives/CodeBlockPrimitive.vue";
import type { UiPrimitive } from "../chat/uiCard.types";
import Icon from "../Icon.vue";

const viewer = useFileViewer();
// md 模式的委托交互层:嵌套路径点击 + 围栏代码块复制都走这里。
const { onMarkdownClick } = useCodeBlockCopy();

/** 文件名(最后一段)做标题;完整路径放 footer(mono 小字)。 */
const fileName = computed<string>(() => {
  const p = viewer.rawPath.value ?? "";
  return p.slice(p.lastIndexOf("/") + 1) || p;
});

/** md 模式正文:renderMarkdown 管线(含 XSS 消毒);其余模式为空串。 */
const bodyHtml = computed<string>(() =>
  viewer.status.value === "ok" && viewer.mode.value === "markdown"
    ? renderMarkdown(viewer.content.value)
    : "",
);

/** code 模式:内容包装成 code_block UiPrimitive 交给 CodeBlockPrimitive
 *  (language 用扩展名 —— rs/ts/py 等是 hljs 别名;未知名自动降级
 *  highlightAuto)。 */
const codePrimitive = computed<UiPrimitive>(() => ({
  type: "code_block",
  code: viewer.content.value,
  language: viewer.ext.value,
  title: fileName.value,
}));

function close(): void {
  viewer.close();
}

/** 「新标签打开」兜底:fetch 失败时走它;文本类新标签按 text/plain 展示
 *  源码,pdf 走浏览器原生 viewer。 */
function openInNewTab(): void {
  if (viewer.absPath.value) {
    window.open(fileUrl(viewer.absPath.value), "_blank", "noreferrer");
  }
}
</script>

<template>
  <DialogRoot :open="viewer.isOpen.value" @update:open="(v: boolean) => !v && close()">
    <DialogPortal>
      <DialogOverlay class="file-viewer__overlay" />
      <DialogContent
        class="file-viewer"
        :aria-describedby="undefined"
        @pointerdown-outside="close"
      >
        <header class="file-viewer__header">
          <DialogTitle class="file-viewer__title" :title="viewer.rawPath.value ?? ''">
            <Icon name="document" :size="14" />
            <span class="file-viewer__title-text">{{ fileName }}</span>
          </DialogTitle>
          <div class="file-viewer__actions">
            <button
              type="button"
              class="btn btn--sm btn--ghost file-viewer__open-btn"
              data-testid="file-viewer-open-tab"
              @click="openInNewTab"
            >
              <Icon name="expand" :size="12" />
              <span>新标签打开</span>
            </button>
            <DialogClose as-child>
              <button
                type="button"
                class="file-viewer__close btn btn--icon btn--ghost"
                aria-label="关闭"
                data-testid="file-viewer-close"
                @click="close"
              >
                <Icon name="x" :size="14" />
              </button>
            </DialogClose>
          </div>
        </header>
        <div class="file-viewer__body">
          <!-- loading 态:fetch 在途;文本 2 MiB 上限内通常一闪而过。 -->
          <div v-if="viewer.status.value === 'loading'" class="file-viewer__hint" data-testid="file-viewer-loading">
            加载中…
          </div>
          <div v-else-if="viewer.status.value === 'error'" class="file-viewer__hint" data-testid="file-viewer-error">
            无法加载文件(不存在 / 类型不支持 / 超过大小上限),可用「新标签打开」重试
          </div>
          <!-- md 模式:委托交互层必须绑在 v-html 容器根上(嵌套路径递归可点)。 -->
          <div
            v-else-if="viewer.mode.value === 'markdown'"
            class="file-viewer__markdown"
            data-testid="file-viewer-markdown"
            @click="onMarkdownClick"
            v-html="bodyHtml"
          />
          <!-- code 模式:复用 CodeBlockPrimitive(高亮 + 复制按钮)。 -->
          <CodeBlockPrimitive
            v-else
            class="file-viewer__code"
            data-testid="file-viewer-code"
            :primitive="codePrimitive"
          />
        </div>
        <footer class="file-viewer__footer">
          <span class="file-viewer__path" :title="viewer.absPath.value">{{ viewer.absPath.value }}</span>
        </footer>
      </DialogContent>
    </DialogPortal>
  </DialogRoot>
</template>

<style scoped>
/* Portal/Teleport 子元素的 scoped 生效性:Vue 3.5 实证 data-v-* 会传播
   到 Teleport 子树(ImageViewerModal 同款注释);若未来 Vue 升级破坏该
   假设,按 reka-ui-usage.md 包 :deep()。 */

.file-viewer__overlay {
  position: fixed;
  inset: 0;
  background: color-mix(in srgb, var(--color-bg-app) 78%, transparent);
  backdrop-filter: blur(4px);
  z-index: var(--z-modal-overlay);
}

.file-viewer {
  position: fixed;
  top: 50%;
  left: 50%;
  transform: translate(-50%, -50%);
  /* 文档阅读面:80vw 限宽 + 视口高度上限,窄屏往里收。 */
  width: min(90vw, 900px);
  max-height: 86vh;
  background: var(--color-bg-surface);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-lg);
  display: flex;
  flex-direction: column;
  overflow: hidden;
  box-shadow: var(--shadow-xl);
  z-index: var(--z-modal);
  outline: none;
  animation: file-viewer-zoom var(--duration-modal-in) var(--ease-modal-in) both;
}

.file-viewer[data-state="closed"] {
  animation: file-viewer-zoom-out var(--duration-modal-out) var(--ease-accelerate) forwards;
}

@keyframes file-viewer-zoom {
  from { opacity: 0; transform: translate(-50%, -50%) scale(0.96); }
  to   { opacity: 1; transform: translate(-50%, -50%) scale(1); }
}

@keyframes file-viewer-zoom-out {
  from { opacity: 1; transform: translate(-50%, -50%) scale(1); }
  to   { opacity: 0; transform: translate(-50%, -50%) scale(0.96); }
}

.file-viewer__header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: 10px 16px;
  border-bottom: 1px solid var(--color-bg-border);
  background: var(--color-bg-elevated);
  flex-shrink: 0;
}

.file-viewer__title {
  margin: 0;
  display: inline-flex;
  align-items: center;
  gap: 8px;
  min-width: 0;
  flex: 1;
  font-size: var(--text-base);
  font-weight: var(--weight-semibold);
  color: var(--color-text-primary);
}

.file-viewer__title-text {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.file-viewer__actions {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  flex-shrink: 0;
}

.file-viewer__open-btn {
  display: inline-flex;
  align-items: center;
  gap: 4px;
}

/* 按钮样式由全局 .btn 家族承载(close = ghost icon);flex 几何保留。 */
.file-viewer__close {
  flex-shrink: 0;
}

.file-viewer__body {
  flex: 1;
  overflow-y: auto;
  min-height: 0;
  padding: 16px 20px;
  background: var(--color-bg-app);
}

.file-viewer__hint {
  padding: 24px;
  text-align: center;
  color: var(--color-text-secondary);
  font-size: var(--text-sm);
}

/*
 * md 模式的排版 —— message-list-and-markdown.md §2 镜像块第六处
 * (MessageItem / DiscussionSummaryCard / MarkdownDetailModal /
 * SubagentDrawer / MemoryLayerItem 之外的新 markdown 容器,必须镜像
 * 这段节奏,改节奏时六处一起改;grep `.msg__markdown` 找全消费方)。
 * 行高 1.6(--leading-relaxed)+ 块级 margin + list marker 显式补回
 * (preflight 吃 marker,CH4-2)。
 */
.file-viewer__markdown {
  font-size: var(--text-base);
  line-height: var(--leading-relaxed);
  color: var(--color-text-primary);
  /* 中英混排间距,Chromium ≥140 生效,WebKit 忽略(style.css 同款)。 */
  text-autospace: ideograph-alpha;
}

.file-viewer__markdown :deep(p) {
  margin: 0 0 var(--space-3) 0;
}

.file-viewer__markdown :deep(li) {
  margin: var(--space-1) 0;
}

.file-viewer__markdown :deep(ul),
.file-viewer__markdown :deep(ol) {
  margin: var(--space-1) 0 var(--space-3);
  padding-left: var(--space-6);
}

.file-viewer__markdown :deep(h1),
.file-viewer__markdown :deep(h2),
.file-viewer__markdown :deep(h3),
.file-viewer__markdown :deep(h4) {
  margin: var(--space-4) 0 var(--space-1);
  font-weight: var(--weight-semibold);
}

.file-viewer__markdown :deep(ul) {
  list-style: disc;
}

.file-viewer__markdown :deep(ol) {
  list-style: decimal;
}

.file-viewer__markdown :deep(ul:last-child),
.file-viewer__markdown :deep(ol:last-child),
.file-viewer__markdown :deep(p:last-child) {
  margin-bottom: 0;
}

.file-viewer__markdown :deep(a) {
  color: var(--color-accent-text);
  text-decoration: none;
}

.file-viewer__markdown :deep(a:hover) {
  text-decoration: underline;
}

.file-viewer__markdown :deep(code) {
  font-family: var(--font-mono);
  font-size: 0.9em;
  padding: 2px 5px;
  border-radius: 3px;
  background: color-mix(in srgb, var(--color-text-primary) 8%, transparent);
}

/* code 模式的 CodeBlockPrimitive 自带边框卡;阅读面里压掉外层留白。 */
.file-viewer__code {
  margin: 0;
}

.file-viewer__footer {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 8px 16px;
  border-top: 1px solid var(--color-bg-border);
  background: var(--color-bg-elevated);
  flex-shrink: 0;
}

.file-viewer__path {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-family: var(--font-mono);
  font-size: var(--text-xs);
  color: var(--color-text-secondary);
}
</style>
