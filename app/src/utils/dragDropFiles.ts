// dragDropFiles — B1 follow-up (08-21-b1-image-followups) D4:聊天区
// 拖拽文件分流(纯函数,ChatPanel 的 drop handler 只做 6 行接线)。
// 只收图片;非图片文件提示走 @ 引用(与 B2 @注入通道互补不重叠)。

/** 拖放批次的分流结果。 */
export interface DropClassification {
  /** image/* 文件(白名单与张数闸由 addStagedImages 把关,这里只按 mime 粗分)。 */
  images: File[];
  /** 批次中存在非图片文件(混合批次图片照常入暂存,非图片只提示一次)。 */
  nonImage: boolean;
}

export function classifyDroppedFiles(files: File[]): DropClassification {
  const images: File[] = [];
  let nonImage = false;
  for (const f of files) {
    if (f.type.startsWith("image/")) images.push(f);
    else nonImage = true;
  }
  return { images, nonImage };
}
