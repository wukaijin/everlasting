// B1 (2026-08-16) image-multimodal — paste interception contract:
//   - image files in the clipboard → intercepted (preventDefault) +
//     one array callback to `opts.onPasteImages`
//   - plain-text paste (no files / non-image files) → fully passed
//     through (handler returns false, no callback, no
//     preventDefault)
//
// Two levels: the pure `imageFilesFromClipboard` helper (no CM), and
// a mounted-EditorView integration test driving a synthetic `paste`
// event through CM's domEventHandlers wiring.

import { describe, it, expect, vi } from "vitest";
import { defineComponent, h, ref } from "vue";
import { mount } from "@vue/test-utils";
import {
  imageFilesFromClipboard,
  useChatInputCodeMirror,
} from "./chatInputCodeMirror";

function pngFile(name = "shot.png"): File {
  return new File([new Uint8Array([1, 2, 3])], name, { type: "image/png" });
}

/** jsdom has no `DataTransfer` constructor — stub the fields the
 *  paste path reads (`files`; `Array.from` accepts a plain array)
 *  plus `getData` which CM's own text-paste fallback calls after
 *  our handler declines the event. */
function fakeDataTransfer(files: File[]): DataTransfer {
  return {
    files,
    getData: () => "",
  } as unknown as DataTransfer;
}

describe("imageFilesFromClipboard (pure)", () => {
  it("returns image files", () => {
    expect(imageFilesFromClipboard(fakeDataTransfer([pngFile()]))).toHaveLength(1);
  });

  it("returns [] for text-only pastes", () => {
    const dt = fakeDataTransfer([
      new File(["hello"], "a.txt", { type: "text/plain" }),
    ]);
    expect(imageFilesFromClipboard(dt)).toEqual([]);
  });

  it("returns [] for null clipboardData", () => {
    expect(imageFilesFromClipboard(null)).toEqual([]);
  });

  it("keeps only the images from a mixed paste", () => {
    const dt = fakeDataTransfer([
      new File(["x"], "a.txt", { type: "text/plain" }),
      pngFile(),
      new File([new Uint8Array([9])], "b.jpg", { type: "image/jpeg" }),
    ]);
    const out = imageFilesFromClipboard(dt);
    expect(out.map((f) => f.name)).toEqual(["shot.png", "b.jpg"]);
  });

  it("surfaces non-whitelisted image types (gif) — rejection is the caller's job", () => {
    const dt = fakeDataTransfer([
      new File([new Uint8Array([1])], "a.gif", { type: "image/gif" }),
    ]);
    expect(imageFilesFromClipboard(dt)).toHaveLength(1);
  });
});

/** Tiny host component: mounts the composable the same way
 *  ChatInput.vue does (div ref + sending/placeholder refs). */
const Host = defineComponent({
  props: { onPasteImages: { type: Function, required: true } },
  setup(props) {
    const host = ref<HTMLDivElement | null>(null);
    // The composable's return value isn't read here — mounting it
    // is what wires the paste handler into the CodeMirror view.
    useChatInputCodeMirror({
      host,
      sending: ref(false),
      placeholder: ref(undefined),
      onSubmit: () => {},
      onPasteImages: props.onPasteImages as (files: File[]) => void,
    });
    return () => h("div", { ref: host }, [h("div")]);
  },
});

function pasteEvent(files: File[]): Event {
  const ev = new Event("paste", { bubbles: true, cancelable: true });
  Object.defineProperty(ev, "clipboardData", {
    value: fakeDataTransfer(files),
  });
  return ev;
}

describe("CM paste wiring (domEventHandlers)", () => {
  // EditorView.domEventHandlers registers listeners on the CM
  // **contentDOM** (the editable `.cm-content`), not the outer
  // `.cm-editor` shell — paste events must be dispatched there.
  function contentEl(wrapper: ReturnType<typeof mount>): HTMLElement {
    const el = wrapper.element.querySelector(".cm-content");
    expect(el).toBeTruthy();
    return el as HTMLElement;
  }

  it("image paste → preventDefault + one array callback", () => {
    const onPasteImages = vi.fn();
    const wrapper = mount(Host, { props: { onPasteImages } });
    // CM mounts synchronously in onMounted; if jsdom failed to
    // create the view this test would silently pass — assert the
    // editor DOM exists first.
    expect(wrapper.element.querySelector(".cm-editor")).toBeTruthy();

    const file = pngFile();
    const ev = pasteEvent([file]);
    contentEl(wrapper).dispatchEvent(ev);
    expect(ev.defaultPrevented).toBe(true);
    expect(onPasteImages).toHaveBeenCalledTimes(1);
    expect(onPasteImages).toHaveBeenCalledWith([file]);
    wrapper.unmount();
  });

  it("text-only paste → untouched (no callback)", () => {
    const onPasteImages = vi.fn();
    const wrapper = mount(Host, { props: { onPasteImages } });
    expect(wrapper.element.querySelector(".cm-editor")).toBeTruthy();

    const ev = pasteEvent([new File(["hi"], "a.txt", { type: "text/plain" })]);
    contentEl(wrapper).dispatchEvent(ev);
    // NOTE: `defaultPrevented` will be true here too — CM's own
    // built-in paste handler always preventDefaults (it manages the
    // doc itself and re-inserts via dispatch). That is out of scope
    // for this contract; what we pin is that OUR layer declines the
    // event (no callback) and lets CM's text path run.
    expect(onPasteImages).not.toHaveBeenCalled();
    wrapper.unmount();
  });
});
