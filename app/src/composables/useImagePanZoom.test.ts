// useImagePanZoom — 锚点缩放/平移数学的单测。坐标系模型见 composable
// 头注释:锚点 (ax, ay) 相对 stage 中心,锚点不变公式
// t' = a − (s'/s)·(a − t)。

import { describe, it, expect } from "vitest";
import { useImagePanZoom } from "./useImagePanZoom";

describe("useImagePanZoom", () => {
  it("starts at identity (scale 1, no pan)", () => {
    const z = useImagePanZoom();
    expect(z.scale.value).toBe(1);
    expect(z.tx.value).toBe(0);
    expect(z.ty.value).toBe(0);
    expect(z.canPan.value).toBe(false);
    expect(z.transformCss.value).toBe(
      "translate(calc(-50% + 0px), calc(-50% + 0px)) scale(1)",
    );
  });

  it("zooms keeping the anchor content fixed (t' = a − k·(a − t))", () => {
    const z = useImagePanZoom();
    // 1→2,锚 (100, 50):tx' = 100 − 2·(100−0) = −100;ty' = 50 − 2·50 = −50
    z.zoomTo(2, 100, 50);
    expect(z.scale.value).toBe(2);
    expect(z.tx.value).toBeCloseTo(-100);
    expect(z.ty.value).toBeCloseTo(-50);
    // 再 2→4,同锚:tx'' = 100 − 2·(100 − (−100)) = −300
    z.zoomTo(4, 100, 50);
    expect(z.tx.value).toBeCloseTo(-300);
    expect(z.ty.value).toBeCloseTo(-150);
  });

  it("clamps scale into [1, 8]", () => {
    const z = useImagePanZoom();
    z.zoomTo(0.3);
    expect(z.scale.value).toBe(1);
    z.zoomTo(100, 10, 10);
    expect(z.scale.value).toBe(8);
    // 钳制后的锚点数学仍以钳后倍率计算(8/1 的 k)。
    expect(z.tx.value).toBeCloseTo(10 - 8 * 10);
  });

  it("clears pan when returning to min scale (image re-centers)", () => {
    const z = useImagePanZoom();
    z.zoomTo(3, 40, 40);
    z.panBy(25, -15);
    expect(z.tx.value).not.toBe(0);
    z.zoomTo(1);
    expect(z.scale.value).toBe(1);
    expect(z.tx.value).toBe(0);
    expect(z.ty.value).toBe(0);
  });

  it("zoomBy multiplies the current scale", () => {
    const z = useImagePanZoom();
    z.zoomBy(1.25);
    expect(z.scale.value).toBeCloseTo(1.25);
    z.zoomBy(1.25);
    expect(z.scale.value).toBeCloseTo(1.5625);
  });

  it("panBy is a no-op at min scale, accumulates when zoomed", () => {
    const z = useImagePanZoom();
    z.panBy(30, 30);
    expect(z.tx.value).toBe(0);
    expect(z.canPan.value).toBe(false);
    z.zoomTo(2);
    z.panBy(10, -5);
    z.panBy(10, -5);
    expect(z.tx.value).toBe(20);
    expect(z.ty.value).toBe(-10);
    expect(z.canPan.value).toBe(true);
  });

  it("reset returns to identity", () => {
    const z = useImagePanZoom();
    z.zoomTo(4, 100, 100);
    z.panBy(-40, 60);
    z.reset();
    expect(z.scale.value).toBe(1);
    expect(z.tx.value).toBe(0);
    expect(z.ty.value).toBe(0);
  });
});
