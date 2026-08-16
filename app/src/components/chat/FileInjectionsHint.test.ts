// B1 (2026-08-16) image-multimodal — render tests for
// `FileInjectionsHint.vue`'s `injected_image` branch (an `@image`
// file was copied into the session attachments dir and injected as
// a real image block) plus the unknown-kind neutral fallback.
//
// The component is a thin renderer with no store / transport deps,
// so it mounts standalone.

import { describe, it, expect } from "vitest";
import { mount } from "@vue/test-utils";

import FileInjectionsHint from "./FileInjectionsHint.vue";
import type { InjectionEntry } from "../../stores/chat.types";

function mountHint(injections: InjectionEntry[]) {
  return mount(FileInjectionsHint, { props: { injections } });
}

describe("FileInjectionsHint — injected_image (B1)", () => {
  it("renders the ✓ 图片已注入 row with the token estimate", () => {
    const w = mountHint([
      {
        path: "screenshots/bar.png",
        action: {
          kind: "injected_image",
          file: "a1b2c3d4e5f6.png",
          media_type: "image/png",
          tokens_est: 1333,
        },
      },
    ]);
    const row = w.find(".file-injections-hint__row");
    expect(row.exists()).toBe(true);
    expect(row.text()).toContain("screenshots/bar.png");
    // ok glyph + status text with the (w×h)/750 estimate.
    expect(row.find(".file-injections-hint__status--ok").exists()).toBe(true);
    expect(row.text()).toContain("图片已注入");
    expect(row.text()).toContain("1333");
  });

  it("renders ✓ 图片已注入 without the estimate when tokens_est is null", () => {
    const w = mountHint([
      {
        path: "pic.jpg",
        action: {
          kind: "injected_image",
          file: "b2c3.png",
          media_type: "image/jpeg",
          tokens_est: null,
        },
      },
    ]);
    const row = w.find(".file-injections-hint__row");
    expect(row.find(".file-injections-hint__status--ok").exists()).toBe(true);
    expect(row.text()).toContain("图片已注入");
    expect(row.text()).not.toContain("tok");
  });

  it("renders an unknown action kind with the neutral raw-kind fallback", () => {
    // A newer backend emitted a variant this build doesn't know —
    // the row must not be mislabeled as 跳过; it falls back to the
    // raw kind string.
    const w = mountHint([
      {
        path: "x.md",
        action: { kind: "injected_video" } as unknown as InjectionEntry["action"],
      },
    ]);
    const row = w.find(".file-injections-hint__row");
    expect(row.exists()).toBe(true);
    expect(row.text()).toContain("injected_video");
    expect(row.text()).not.toContain("跳过");
  });
});
