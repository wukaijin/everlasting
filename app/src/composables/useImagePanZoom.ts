// useImagePanZoom — 09-13 图片预览弹层的缩放/平移数学核心。
//
// 为什么是纯逻辑 composable(不绑 DOM):锚点缩放的坐标系换算值得脱离
// jsdom 事件模拟精确单测;组件侧只留事件薄壳(wheel 坐标提取、pointer
// 拖拽跟踪),模板/断言都在 ImageViewerModal.test.ts。
//
// 坐标模型(与组件 CSS 配对,见 ImageViewerModal 的 stage 注释):
//   - img 绝对定位于 stage 中心(left/top 50%),transform =
//     `translate(calc(-50% + tx), calc(-50% + ty)) scale(s)`,
//     transform-origin: center;
//   - 锚点 (ax, ay) 语义 = **相对 stage 中心的偏移**(组件 handler 用
//     `clientX - (rect.left + rect.width / 2)` 换算);
//   - 锚点不变的推导:变换后内容点位置 = 中心 + t + s·q,要求锚点处
//     内容不动 ⇒ t' = a − (s'/s)·(a − t)。
//
// 有意的简化:
//   - 平移不设软边界(自由 pan,复位按钮兜底)——边界计算需要 img 实际
//     渲染尺寸,引入 ResizeObserver 的复杂度不值得;
//   - 触摸 pinch 不做(pointer 拖拽天然可用,缩放走按钮)——桌面
//     webview 是主场景,移动端需求出现时在组件层加双指跟踪。
import { computed, ref } from "vue";

export const PAN_ZOOM_MIN_SCALE = 1;
export const PAN_ZOOM_MAX_SCALE = 8;

/** 双击放大/复位的目标倍率(复位用 reset(),这里只管"放大到多少")。 */
export const PAN_ZOOM_DBLCLICK_SCALE = 2.5;

function clampScale(s: number): number {
  return Math.min(PAN_ZOOM_MAX_SCALE, Math.max(PAN_ZOOM_MIN_SCALE, s));
}

export function useImagePanZoom() {
  const scale = ref(1);
  const tx = ref(0);
  const ty = ref(0);

  const transformCss = computed(
    () =>
      `translate(calc(-50% + ${tx.value}px), calc(-50% + ${ty.value}px)) scale(${scale.value})`,
  );

  /** 平移仅在放大后有意义(scale=1 时图完整可见,拖拽应是滚动/no-op)。 */
  const canPan = computed(() => scale.value > PAN_ZOOM_MIN_SCALE);

  /** 缩放到 `next`,以 (ax, ay)(相对 stage 中心)为不动点。 */
  function zoomTo(next: number, ax = 0, ay = 0): void {
    const s = clampScale(next);
    const k = s / scale.value;
    tx.value = ax - k * (ax - tx.value);
    ty.value = ay - k * (ay - ty.value);
    scale.value = s;
    // 回到 1 时图已完整居中,平移量清零(否则残留偏移把图推出视野)。
    if (s === PAN_ZOOM_MIN_SCALE) {
      tx.value = 0;
      ty.value = 0;
    }
  }

  /** 按倍率因子缩放(>1 放大,<1 缩小),锚点语义同 zoomTo。 */
  function zoomBy(factor: number, ax = 0, ay = 0): void {
    zoomTo(scale.value * factor, ax, ay);
  }

  function panBy(dx: number, dy: number): void {
    if (!canPan.value) return;
    tx.value += dx;
    ty.value += dy;
  }

  function reset(): void {
    scale.value = 1;
    tx.value = 0;
    ty.value = 0;
  }

  return { scale, tx, ty, transformCss, canPan, zoomTo, zoomBy, panBy, reset };
}
