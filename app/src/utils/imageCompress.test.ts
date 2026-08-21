// imageCompress tests — B1 follow-up (08-21-b1-image-followups) D3。
// canvas 全部经 CompressDeps 注入假实现(jsdom 无 createImageBitmap/toBlob);
// 纯决策函数直接断言。
import { describe, expect, it } from "vitest";
import {
  compressImage,
  decideCompression,
  jpegFileName,
  shouldKeepOriginal,
  targetDimensions,
  type CompressDeps,
  type DecodedImage,
} from "./imageCompress";

function fakeDeps(opts: {
  w: number;
  h: number;
  alpha?: boolean;
  blobSize?: number;
  blobNull?: boolean;
  decodeFail?: boolean;
}): CompressDeps {
  return {
    async decode(): Promise<DecodedImage | null> {
      if (opts.decodeFail) return null;
      return { w: opts.w, h: opts.h, source: {} as CanvasImageSource };
    },
    // jsdom 无 canvas 实现 —— 全部返回桩对象,压缩决策不依赖真实像素。
    createCanvas(_source, w, h) {
      const canvas = { width: w, height: h } as HTMLCanvasElement;
      const ctx = { canvas } as unknown as CanvasRenderingContext2D;
      return { canvas, ctx };
    },
    hasAlpha: () => opts.alpha ?? false,
    async toBlob() {
      if (opts.blobNull) return null;
      return new Blob([new Uint8Array(opts.blobSize ?? 1024)], { type: "image/png" });
    },
  };
}

function pngFile(size = 2000, name = "shot.png"): File {
  return new File([new Uint8Array(size)], name, { type: "image/png" });
}

describe("targetDimensions", () => {
  it("长边超限等比缩至 1568", () => {
    expect(targetDimensions(3840, 2160)).toEqual({ w: 1568, h: 882 });
    expect(targetDimensions(1000, 3000)).toEqual({ w: 523, h: 1568 });
  });
  it("不放大:小图原样返回", () => {
    expect(targetDimensions(800, 600)).toEqual({ w: 800, h: 600 });
    expect(targetDimensions(1568, 1568)).toEqual({ w: 1568, h: 1568 });
  });
});

describe("decideCompression", () => {
  it("双条件都不满足:零动作", () => {
    const d = decideCompression(800, 600, 500 * 1024, false);
    expect(d.downscale).toBe(false);
    expect(d.reencodeJpeg).toBe(false);
  });
  it("长边超限:降采样;bytes 不超限且无透明时不重编码", () => {
    const d = decideCompression(2000, 2000, 500 * 1024, false);
    expect(d.downscale).toBe(true);
    expect(d.reencodeJpeg).toBe(false);
    expect(d.targetW).toBe(1568);
  });
  it("bytes 超限且无透明:重编码 JPEG(尺寸不超限也不缩)", () => {
    const d = decideCompression(1000, 800, 2 * 1024 * 1024, false);
    expect(d.downscale).toBe(false);
    expect(d.reencodeJpeg).toBe(true);
    expect(d.targetW).toBe(1000);
  });
  it("有透明:即使 bytes 超限也不换格式(仅降采样)", () => {
    const d = decideCompression(2000, 1000, 2 * 1024 * 1024, true);
    expect(d.downscale).toBe(true);
    expect(d.reencodeJpeg).toBe(false);
  });
  it("双触发:既缩又换 JPEG", () => {
    const d = decideCompression(4000, 3000, 3 * 1024 * 1024, false);
    expect(d.downscale).toBe(true);
    expect(d.reencodeJpeg).toBe(true);
    expect(d.targetW).toBe(1568);
    expect(d.targetH).toBe(1176);
  });
});

describe("shouldKeepOriginal / jpegFileName", () => {
  it("产物 ≥ 原件保留原件", () => {
    expect(shouldKeepOriginal(1000, 1000)).toBe(true);
    expect(shouldKeepOriginal(1000, 2000)).toBe(true);
    expect(shouldKeepOriginal(1000, 999)).toBe(false);
  });
  it("扩展名替换 / 无扩展名补 .jpg", () => {
    expect(jpegFileName("shot.png")).toBe("shot.jpg");
    expect(jpegFileName("a.b/shot.webp")).toBe("a.b/shot.jpg");
    expect(jpegFileName("shot")).toBe("shot.jpg");
  });
});

describe("compressImage", () => {
  it("双条件不满足:原样放行不碰 canvas", async () => {
    const r = await compressImage(pngFile(), fakeDeps({ w: 800, h: 600 }));
    expect(r.compressed).toBe(false);
    expect(r.w).toBe(800);
    expect(r.file.name).toBe("shot.png");
  });
  it("无透明大 PNG:降采样 + JPEG 重编码,尺寸/文件名/标注正确", async () => {
    const r = await compressImage(pngFile(3 * 1024 * 1024, "4k.png"), fakeDeps({ w: 3840, h: 2160, blobSize: 100 * 1024 }));
    expect(r.compressed).toBe(true);
    expect(r.w).toBe(1568);
    expect(r.h).toBe(882);
    expect(r.origW).toBe(3840);
    expect(r.origBytes).toBe(3 * 1024 * 1024);
    expect(r.file.type).toBe("image/jpeg");
    expect(r.file.name).toBe("4k.jpg");
  });
  it("有透明大 PNG:仅降采样保持 png", async () => {
    const r = await compressImage(pngFile(3 * 1024 * 1024), fakeDeps({ w: 3840, h: 2160, alpha: true, blobSize: 100 * 1024 }));
    expect(r.compressed).toBe(true);
    expect(r.file.type).toBe("image/png");
    expect(r.file.name).toBe("shot.png");
  });
  it("JPEG 大 bytes 小尺寸:只重编码不缩", async () => {
    const f = new File([new Uint8Array(3 * 1024 * 1024)], "photo.jpg", { type: "image/jpeg" });
    const r = await compressImage(f, fakeDeps({ w: 1200, h: 900, blobSize: 500 * 1024 }));
    expect(r.compressed).toBe(true);
    expect(r.w).toBe(1200);
    expect(r.file.type).toBe("image/jpeg");
  });
  it("守卫:压缩产物更大保留原件", async () => {
    const r = await compressImage(pngFile(1000, "small.png"), fakeDeps({ w: 2000, h: 2000, blobSize: 5000 }));
    expect(r.compressed).toBe(false);
    expect(r.file.size).toBe(1000);
  });
  it("toBlob 不可用(null):fail-open 原样放行", async () => {
    const r = await compressImage(pngFile(3 * 1024 * 1024), fakeDeps({ w: 3840, h: 2160, blobNull: true }));
    expect(r.compressed).toBe(false);
  });
  it("解码失败:fail-open 返回 0 尺寸(调用方 tokensEst 兜底)", async () => {
    const r = await compressImage(pngFile(), fakeDeps({ w: 0, h: 0, decodeFail: true }));
    expect(r.compressed).toBe(false);
    expect(r.w).toBe(0);
    expect(r.h).toBe(0);
  });
});
