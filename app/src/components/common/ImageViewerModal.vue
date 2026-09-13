<script setup lang="ts">
// ImageViewerModal — 图片路径预览弹层(2026-09-13)。
//
// markdown 里的本地图片路径(utils/markdown.ts linkifyImagePaths 产物)
// 经 useCodeBlockCopy 的点击委托 useImageViewer.open(原始路径) 打开
// 这里。全局唯一实例挂 App.vue(与 CloseGuardDialog 同层),所有绑了
// onMarkdownClick 的 markdown 面共用。
//
// 为什么 reka-ui Dialog(照 MarkdownDetailModal 的六件套模式):
// Esc / 遮罩点击 / focus trap / Portal 语义开箱即用;项目里
// MemoryModal / SettingsModal / MarkdownDetailModal 全是这个模式。
//
// 路径解析时机:open() 传入的是 linkify 原文(可能 `out/x.png` 相对
// 形态),cwd 解析推迟到本组件 computed 里做 —— "点击那一刻"的
// chatStore.currentCwd 才是语义正确的基准(渲染时刻的 cwd 会随会话
// 切换漂移),且 App.vue 常驻组件里 useChatStore 时 pinia 已激活。
//
// 缩放/平移(同日增强):数学核心在 composables/useImagePanZoom(纯逻辑
// 可单测),本组件只留事件薄壳 —— wheel 缩放(光标锚点,`.prevent` 防
// 页面滚动)、pointer 拖拽(scale>1 时,canPan 门控)、双击 放大↔复位、
// header 控件(− 倍率 + 复位)。舞台是 overflow:hidden 的 stage:pan/zoom
// 全由 img transform 承载,不走滚动条(transform 溢出不会与容器滚动
// 打架)。触摸 pinch 不做(见 useImagePanZoom 尾注)。
//
// 加载失败态:daemon 路由 400(非白名单/相对路径)/404(文件没了)/
// 413(超限)都表现为 <img> onerror —— 统一给"无法加载"文案 + 完整
// 路径,再给"新标签打开"兜底(经 imageUrl 构造的 URL,pwa-remote 模式
// 自带 access_token)。

import {
  DialogRoot,
  DialogPortal,
  DialogOverlay,
  DialogContent,
  DialogTitle,
  DialogClose,
} from "reka-ui";

import { computed, ref, watch } from "vue";
import { useImageViewer } from "../../composables/useImageViewer";
import {
  useImagePanZoom,
  PAN_ZOOM_DBLCLICK_SCALE,
  PAN_ZOOM_MAX_SCALE,
  PAN_ZOOM_MIN_SCALE,
} from "../../composables/useImagePanZoom";
import { useChatStore } from "../../stores/chat";
import { imageUrl, resolveImagePath } from "../../utils/imageUrl";
import Icon from "../Icon.vue";

const viewer = useImageViewer();
const chatStore = useChatStore();
const zoom = useImagePanZoom();

/** 弹层打开时预览的原始路径(linkify 原文)。 */
const rawPath = computed(() => viewer.path.value);

/** daemon 可接受形态:绝对路径或 `~/` 前缀(相对路径按当前会话 cwd 解析)。 */
const absPath = computed<string>(() =>
  rawPath.value ? resolveImagePath(rawPath.value, chatStore.currentCwd) : "",
);

/** <img> / "新标签打开" 共用的取数 URL。 */
const src = computed<string>(() =>
  absPath.value ? imageUrl(absPath.value) : "",
);

/** 文件名(最后一段)做标题;完整路径放 footer(mono 小字)。 */
const fileName = computed<string>(() => {
  const p = rawPath.value ?? "";
  return p.slice(p.lastIndexOf("/") + 1) || p;
});

/** header 倍率读数(125% 形态)。 */
const zoomLabel = computed(() => `${Math.round(zoom.scale.value * 100)}%`);

// 加载失败态;src 变化(打开新图)时复位 —— 失败标记与缩放/平移一起归零。
const failed = ref(false);
const stageEl = ref<HTMLElement | null>(null);
watch(src, () => {
  failed.value = false;
  zoom.reset();
});

function close(): void {
  viewer.close();
}

function openInNewTab(): void {
  if (src.value) window.open(src.value, "_blank", "noreferrer");
}

// --- 缩放/平移事件薄壳(数学在 useImagePanZoom) ----------------------------

/** 控件按钮的固定步进倍率。 */
const ZOOM_STEP = 1.25;

function anchorFromEvent(e: WheelEvent | MouseEvent): { ax: number; ay: number } {
  // 锚点 = 事件点相对 stage 中心的偏移(useImagePanZoom 的坐标模型)。
  const rect = stageEl.value?.getBoundingClientRect();
  if (!rect) return { ax: 0, ay: 0 };
  return {
    ax: e.clientX - (rect.left + rect.width / 2),
    ay: e.clientY - (rect.top + rect.height / 2),
  };
}

/** wheel: deltaY → 指数因子(指针滚轮 ±100 一格 ≈ ×1.16;触控板小
 *  delta 平滑缩放)。模板上 `.prevent` 已挡页面滚动。 */
function onWheel(e: WheelEvent): void {
  if (failed.value || !src.value) return;
  const { ax, ay } = anchorFromEvent(e);
  zoom.zoomBy(Math.exp(-e.deltaY * 0.0015), ax, ay);
}

function zoomIn(): void {
  zoom.zoomBy(ZOOM_STEP);
}
function zoomOut(): void {
  zoom.zoomBy(1 / ZOOM_STEP);
}

/** 双击:未放大 → 以点击点为锚放大;已放大 → 复位。 */
function onDblClick(e: MouseEvent): void {
  if (failed.value || !src.value) return;
  if (zoom.scale.value > 1) {
    zoom.reset();
    return;
  }
  const { ax, ay } = anchorFromEvent(e);
  zoom.zoomTo(PAN_ZOOM_DBLCLICK_SCALE, ax, ay);
}

// pointer 拖拽:按下时记录起点并捕获指针(移出 stage 仍跟踪),move 喂
// panBy(canPan 门控在 composable),抬起点释放。jsdom/旧引擎的
// setPointerCapture 可能缺位/拒收,try/catch 降级为普通冒泡跟踪。
const dragging = ref(false);
let dragLastX = 0;
let dragLastY = 0;

function onPointerDown(e: PointerEvent): void {
  if (!zoom.canPan.value || failed.value) return;
  dragging.value = true;
  dragLastX = e.clientX;
  dragLastY = e.clientY;
  try {
    stageEl.value?.setPointerCapture(e.pointerId);
  } catch {
    // pointer capture unavailable → 跟踪退化为 stage 内移动
  }
}

function onPointerMove(e: PointerEvent): void {
  if (!dragging.value) return;
  zoom.panBy(e.clientX - dragLastX, e.clientY - dragLastY);
  dragLastX = e.clientX;
  dragLastY = e.clientY;
}

function endDrag(e: PointerEvent): void {
  if (!dragging.value) return;
  dragging.value = false;
  try {
    stageEl.value?.releasePointerCapture(e.pointerId);
  } catch {
    // 同上:捕获未建立时释放是 no-op
  }
}
</script>

<template>
  <DialogRoot :open="viewer.isOpen.value" @update:open="(v: boolean) => !v && close()">
    <DialogPortal>
      <DialogOverlay class="image-viewer__overlay" />
      <DialogContent
        class="image-viewer"
        :aria-describedby="undefined"
        @pointerdown-outside="close"
      >
        <header class="image-viewer__header">
          <DialogTitle class="image-viewer__title" :title="rawPath ?? ''">
            <Icon name="eye" :size="14" />
            <span class="image-viewer__title-text">{{ fileName }}</span>
          </DialogTitle>
          <div class="image-viewer__zoom">
            <button
              type="button"
              class="btn btn--icon btn--ghost image-viewer__zoom-btn"
              aria-label="缩小"
              data-testid="image-viewer-zoom-out"
              :disabled="!viewer.isOpen.value || zoom.scale.value <= PAN_ZOOM_MIN_SCALE"
              @click="zoomOut"
            >
              <Icon name="minus" :size="14" />
            </button>
            <span class="image-viewer__zoom-label" data-testid="image-viewer-zoom-label">{{ zoomLabel }}</span>
            <button
              type="button"
              class="btn btn--icon btn--ghost image-viewer__zoom-btn"
              aria-label="放大"
              data-testid="image-viewer-zoom-in"
              :disabled="!viewer.isOpen.value || zoom.scale.value >= PAN_ZOOM_MAX_SCALE"
              @click="zoomIn"
            >
              <Icon name="plus" :size="14" />
            </button>
            <button
              type="button"
              class="btn btn--icon btn--ghost image-viewer__zoom-btn"
              aria-label="复位缩放"
              title="复位缩放"
              data-testid="image-viewer-zoom-reset"
              :disabled="!viewer.isOpen.value || zoom.scale.value <= PAN_ZOOM_MIN_SCALE"
              @click="zoom.reset()"
            >
              <Icon name="shrink" :size="14" />
            </button>
            <span class="image-viewer__header-sep" aria-hidden="true" />
            <DialogClose as-child>
              <button
                type="button"
                class="image-viewer__close btn btn--icon btn--ghost"
                aria-label="关闭"
                data-testid="image-viewer-close"
                @click="close"
              >
                <Icon name="x" :size="14" />
              </button>
            </DialogClose>
          </div>
        </header>
        <!--
          stage:overflow:hidden 的缩放舞台 —— pan/zoom 全由 img 的
          transform 承载(useImagePanZoom 坐标模型假设 img 绝对居中 +
          transform-origin:center)。wheel 用 .prevent 挡掉整页滚动;
          拖拽 cursor 与 user-select 在 CSS 侧按 dragging/canPan 切换。
        -->
        <div
          ref="stageEl"
          class="image-viewer__stage"
          :class="{ 'image-viewer__stage--grab': zoom.canPan.value && !dragging, 'image-viewer__stage--grabbing': dragging }"
          @wheel.prevent="onWheel"
          @pointerdown="onPointerDown"
          @pointermove="onPointerMove"
          @pointerup="endDrag"
          @pointercancel="endDrag"
          @dblclick="onDblClick"
        >
          <img
            v-if="src && !failed"
            class="image-viewer__img"
            :src="src"
            :alt="fileName"
            :style="{ transform: zoom.transformCss.value }"
            draggable="false"
            data-testid="image-viewer-img"
            @error="failed = true"
          />
          <div v-else-if="failed" class="image-viewer__error" data-testid="image-viewer-error">
            无法加载图片(文件不存在 / 类型不支持 / 超过 32 MiB)
          </div>
        </div>
        <footer class="image-viewer__footer">
          <span class="image-viewer__path" :title="absPath">{{ absPath }}</span>
          <button
            type="button"
            class="btn btn--sm btn--ghost image-viewer__open-btn"
            data-testid="image-viewer-open-tab"
            @click="openInNewTab"
          >
            <Icon name="expand" :size="12" />
            <span>新标签打开</span>
          </button>
        </footer>
      </DialogContent>
    </DialogPortal>
  </DialogRoot>
</template>

<style scoped>
/* Portal/Teleport 子元素的 scoped 生效性:Vue 3.5 实证 data-v-* 会传播
   到 Teleport 子树(MarkdownDetailModal 同款注释);若未来 Vue 升级
   破坏该假设,按 reka-ui-usage.md 包 :deep()。 */

.image-viewer__overlay {
  position: fixed;
  inset: 0;
  background: color-mix(in srgb, var(--color-bg-app) 78%, transparent);
  backdrop-filter: blur(4px);
  z-index: var(--z-modal-overlay);
}

.image-viewer {
  position: fixed;
  top: 50%;
  left: 50%;
  transform: translate(-50%, -50%);
  /* 图片查看要吃满视口:90vw/80vh 上限,小屏往下限收。 */
  width: min(90vw, 1100px);
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
  animation: image-viewer-zoom var(--duration-modal-in) var(--ease-modal-in) both;
}

.image-viewer[data-state="closed"] {
  animation: image-viewer-zoom-out var(--duration-modal-out) var(--ease-accelerate) forwards;
}

@keyframes image-viewer-zoom {
  from { opacity: 0; transform: translate(-50%, -50%) scale(0.96); }
  to   { opacity: 1; transform: translate(-50%, -50%) scale(1); }
}

@keyframes image-viewer-zoom-out {
  from { opacity: 1; transform: translate(-50%, -50%) scale(1); }
  to   { opacity: 0; transform: translate(-50%, -50%) scale(0.96); }
}

.image-viewer__header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: 10px 16px;
  border-bottom: 1px solid var(--color-bg-border);
  background: var(--color-bg-elevated);
  flex-shrink: 0;
}

.image-viewer__title {
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

.image-viewer__title-text {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.image-viewer__zoom {
  display: inline-flex;
  align-items: center;
  gap: 2px;
  flex-shrink: 0;
}

.image-viewer__zoom-btn {
  /* btn--icon 默认 4px 呼吸留白对 14px 图标略挤,压一档。 */
  padding: 4px;
}

.image-viewer__zoom-label {
  min-width: 44px;
  text-align: center;
  font-family: var(--font-mono);
  font-size: var(--text-xs);
  color: var(--color-text-secondary);
  font-variant-numeric: tabular-nums;
}

.image-viewer__header-sep {
  width: 1px;
  height: 18px;
  margin: 0 8px;
  background: var(--color-bg-border);
}

/* stage:overflow:hidden 的缩放舞台。img 绝对居中(left/top 50%),
   transform 由 useImagePanZoom 生成 —— translate 的 -50% 基底 + pan
   偏移,配 transform-origin:center(与 composable 的锚点数学成对,
   改一处必须同步另一处)。 */
.image-viewer__stage {
  position: relative;
  flex: 1;
  min-height: 0;
  overflow: hidden;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 16px;
  background: var(--color-bg-app);
  /* 拖拽/双击不选中文案。 */
  user-select: none;
  touch-action: none;
}

.image-viewer__stage--grab {
  cursor: grab;
}

.image-viewer__stage--grabbing {
  cursor: grabbing;
}

.image-viewer__img {
  position: absolute;
  left: 50%;
  top: 50%;
  max-width: 100%;
  max-height: 64vh;
  object-fit: contain;
  border-radius: var(--radius-sm);
  transform-origin: center;
  /* WebKit 的原生图片拖拽幻影会吃掉 pointer 拖拽。 */
  -webkit-user-drag: none;
  pointer-events: none;
}

.image-viewer__error {
  position: absolute;
  left: 50%;
  top: 50%;
  transform: translate(-50%, -50%);
  color: var(--color-text-secondary);
  font-size: var(--text-sm);
  padding: 24px;
  text-align: center;
  white-space: nowrap;
}

.image-viewer__footer {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 8px 16px;
  border-top: 1px solid var(--color-bg-border);
  background: var(--color-bg-elevated);
  flex-shrink: 0;
}

.image-viewer__path {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-family: var(--font-mono);
  font-size: var(--text-xs);
  color: var(--color-text-secondary);
}

.image-viewer__open-btn {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  flex-shrink: 0;
}
</style>
