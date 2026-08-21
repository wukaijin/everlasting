// dragDropFiles tests — B1 follow-up (08-21-b1-image-followups) D4。
import { describe, expect, it } from "vitest";
import { classifyDroppedFiles } from "./dragDropFiles";

function f(name: string, type: string): File {
  return new File([new Uint8Array(8)], name, { type });
}

describe("classifyDroppedFiles", () => {
  it("纯图片批次", () => {
    const r = classifyDroppedFiles([f("a.png", "image/png"), f("b.jpg", "image/jpeg")]);
    expect(r.images).toHaveLength(2);
    expect(r.nonImage).toBe(false);
  });
  it("纯非图片批次", () => {
    const r = classifyDroppedFiles([f("a.md", "text/markdown"), f("b.csv", "text/csv")]);
    expect(r.images).toHaveLength(0);
    expect(r.nonImage).toBe(true);
  });
  it("混合批次:图片照常入暂存,非图片只提示一次", () => {
    const r = classifyDroppedFiles([f("a.png", "image/png"), f("b.md", "text/markdown"), f("c.webp", "image/webp")]);
    expect(r.images.map((x) => x.name)).toEqual(["a.png", "c.webp"]);
    expect(r.nonImage).toBe(true);
  });
  it("空批次", () => {
    const r = classifyDroppedFiles([]);
    expect(r.images).toHaveLength(0);
    expect(r.nonImage).toBe(false);
  });
});
