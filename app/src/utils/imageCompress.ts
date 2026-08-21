// imageCompress — B1 follow-up (08-21-b1-image-followups) D3: 前端图片
// 自动压缩(粘贴/拖拽入口共用)。
//
// 触发条件(任一):长边 > 1568px(Anthropic 官方推荐上限,超出后
// 视觉收益归零但 `(w×h)/750` 的 images_token 线性涨)或 bytes > 1MB。
// 长边超限 → 等比降采样至 1568;无透明通道且 bytes 超限 → 重编码
// JPEG q0.85(大色块截图收益最大);有透明 → 保持原格式仅降采样;
// 压缩产物 ≥ 原件 → 保留原件;解码失败 → fail-open 原样放行(最坏
// 退化 = 压缩前行为)。
//
// canvas 依赖集中在 `CompressDeps`(默认 real 实现),测试注入假
// 实现 —— jsdom 无 createImageBitmap / toBlob,纯决策函数
// (`decideCompression` / `targetDimensions` / `shouldKeepOriginal`)
// 与 shell 都必须可离线单测。

/** 降采样目标长边(Anthropic 推荐上限)。 */
export const MAX_EDGE = 1568;

/** 触发重编码的 bytes 阈值。 */
export const REENCODE_THRESHOLD_BYTES = 1024 * 1024;

/** JPEG 重编码质量。 */
export const JPEG_QUALITY = 0.85;

/** 压缩决策(纯函数产物,不含 I/O)。 */
export interface CompressDecision {
  /** 需要降采样(长边 > MAX_EDGE)。 */
  downscale: boolean;
  /** 需要重编码为 JPEG(无透明 && bytes 超限)。 */
  reencodeJpeg: boolean;
  /** 画布目标尺寸(downscale=false 时 = 原尺寸)。 */
  targetW: number;
  targetH: number;
}

/** 等比缩放至长边 = maxEdge(不放大;超限时才缩)。纯函数。 */
export function targetDimensions(w: number, h: number, maxEdge = MAX_EDGE): { w: number; h: number } {
  const long = Math.max(w, h);
  if (long <= maxEdge || long === 0) return { w, h };
  const scale = maxEdge / long;
  return { w: Math.max(1, Math.round(w * scale)), h: Math.max(1, Math.round(h * scale)) };
}

/** 判定压缩动作。`hasAlpha` 只影响 JPEG 重编码(透明图保持原格式)。纯函数。 */
export function decideCompression(
  w: number,
  h: number,
  bytes: number,
  hasAlpha: boolean,
): CompressDecision {
  const downscale = Math.max(w, h) > MAX_EDGE;
  const reencodeJpeg = !hasAlpha && bytes > REENCODE_THRESHOLD_BYTES;
  const t = downscale ? targetDimensions(w, h) : { w, h };
  return { downscale, reencodeJpeg, targetW: t.w, targetH: t.h };
}

/** 守卫:压缩产物不小于原件时保留原件(纯函数)。 */
export function shouldKeepOriginal(origBytes: number, newBytes: number): boolean {
  return newBytes >= origBytes;
}

/** 重编码 JPEG 后的文件名(扩展名换 .jpg;无扩展名则补)。 */
export function jpegFileName(name: string): string {
  return /\.[^./\\]+$/.test(name) ? name.replace(/\.[^./\\]+$/, ".jpg") : `${name}.jpg`;
}

// ---------------------------------------------------------------------------
// canvas 依赖注入(jsdom 无实现,测试传假 deps)
// ---------------------------------------------------------------------------

export interface DecodedImage {
  w: number;
  h: number;
  /** 可 drawImage 的源(ImageBitmap / HTMLImageElement)。 */
  source: CanvasImageSource;
}

export interface CompressDeps {
  /** 解码文件;失败返回 null(fail-open)。 */
  decode(file: File): Promise<DecodedImage | null>;
  /** 建画布并高质量绘制到目标尺寸(真实实现 = document canvas)。 */
  createCanvas(
    source: CanvasImageSource,
    w: number,
    h: number,
  ): { canvas: HTMLCanvasElement; ctx: CanvasRenderingContext2D };
  /** 画布 alpha 扫描(存在任一 alpha<255 像素即 true)。 */
  hasAlpha(ctx: CanvasRenderingContext2D, w: number, h: number): boolean;
  /** 编码;不支持时返回 null。 */
  toBlob(canvas: HTMLCanvasElement, mime: string, quality?: number): Promise<Blob | null>;
}

/** 压缩结果。`compressed=false` 时 file/w/h 即原值。 */
export interface CompressResult {
  file: File;
  w: number;
  h: number;
  compressed: boolean;
  /** 原始尺寸与大小(压缩标注用;未压缩时同 w/h/size)。 */
  origW: number;
  origH: number;
  origBytes: number;
}

const realDeps: CompressDeps = {
  async decode(file) {
    // EXIF 方向归一后再采样,否则旋转照片的 w/h 与像素流不一致。
    if (typeof createImageBitmap === "function") {
      try {
        const bmp = await createImageBitmap(file, { imageOrientation: "from-image" });
        return { w: bmp.width, h: bmp.height, source: bmp };
      } catch {
        // fall through to objectURL decode
      }
    }
    return new Promise((resolve) => {
      const url = URL.createObjectURL(file);
      const img = new Image();
      const done = (r: DecodedImage | null) => {
        URL.revokeObjectURL(url);
        resolve(r);
      };
      img.onload = () => done({ w: img.naturalWidth || 0, h: img.naturalHeight || 0, source: img });
      img.onerror = () => done(null);
      img.src = url;
    });
  },
  createCanvas(source, w, h) {
    const canvas = document.createElement("canvas");
    canvas.width = w;
    canvas.height = h;
    const ctx = canvas.getContext("2d");
    if (!ctx) throw new Error("canvas 2d context unavailable");
    ctx.imageSmoothingEnabled = true;
    ctx.imageSmoothingQuality = "high";
    ctx.drawImage(source, 0, 0, w, h);
    return { canvas, ctx };
  },
  hasAlpha(ctx, w, h) {
    const data = ctx.getImageData(0, 0, w, h).data;
    // 步进采样 + 早退:大图全扫代价一次可接受,但透明图往往很快命中。
    for (let i = 3; i < data.length; i += 4) {
      if (data[i] < 255) return true;
    }
    return false;
  },
  toBlob(canvas, mime, quality) {
    return new Promise((resolve) => {
      canvas.toBlob((b) => resolve(b), mime, quality);
    });
  },
};

/**
 * 压缩入口。任何失败(解码/画布/编码)都 fail-open 返回原文件 —— 压缩
 * 是优化不是闸门,坏图走 addStagedImages 既有的拒绝路径。
 * 解码彻底失败时 w/h 为 0(调用方 tokensEst 兜底 1,同 B1 现状)。
 */
export async function compressImage(file: File, deps: CompressDeps = realDeps): Promise<CompressResult> {
  const passThrough = (w = 0, h = 0): CompressResult => ({
    file, w, h, compressed: false, origW: w, origH: h, origBytes: file.size,
  });

  // 快速预检:两触发条件都不满足时不碰 canvas。
  const decoded = await deps.decode(file);
  if (!decoded || decoded.w === 0 || decoded.h === 0) return passThrough();
  if (decoded.w <= MAX_EDGE && decoded.h <= MAX_EDGE && file.size <= REENCODE_THRESHOLD_BYTES) {
    return passThrough(decoded.w, decoded.h);
  }

  // JPEG 无透明;PNG/WebP 只在 bytes 超限(重编码候选)时才需要 alpha 判定。
  const needsAlpha = file.type !== "image/jpeg" && file.size > REENCODE_THRESHOLD_BYTES;

  let canvas: HTMLCanvasElement;
  let ctx: CanvasRenderingContext2D;
  try {
    const t = targetDimensions(decoded.w, decoded.h);
    const c = deps.createCanvas(decoded.source, t.w, t.h);
    canvas = c.canvas;
    ctx = c.ctx;
  } catch {
    return passThrough(decoded.w, decoded.h);
  }

  const hasAlpha = needsAlpha && safeHasAlpha(deps, ctx);
  const decision = decideCompression(decoded.w, decoded.h, file.size, hasAlpha);
  if (!decision.downscale && !decision.reencodeJpeg) {
    return passThrough(decoded.w, decoded.h);
  }

  const mime = decision.reencodeJpeg ? "image/jpeg" : file.type;
  const blob = await deps.toBlob(canvas, mime, decision.reencodeJpeg ? JPEG_QUALITY : undefined);
  if (!blob || shouldKeepOriginal(file.size, blob.size)) {
    return passThrough(decoded.w, decoded.h);
  }
  const name = decision.reencodeJpeg ? jpegFileName(file.name) : file.name;
  const out = new File([blob], name, { type: mime });
  return {
    file: out,
    w: decision.targetW,
    h: decision.targetH,
    compressed: true,
    origW: decoded.w,
    origH: decoded.h,
    origBytes: file.size,
  };
}

function safeHasAlpha(deps: CompressDeps, ctx: CanvasRenderingContext2D): boolean {
  try {
    return deps.hasAlpha(ctx, ctx.canvas.width, ctx.canvas.height);
  } catch {
    // alpha 探测失败按有透明处理(保守:不换有损格式,只降采样)。
    return true;
  }
}
