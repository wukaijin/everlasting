// 08-26-f5-verify-followups P1 — `@` 文件面板浅层列表刷新契约:
//   - shallow(默认 `@`,3 层小 walk):每次面板打开即重拉 —— 面板
//     关闭再打开必须触发第二次 `fileItemsSource` 调用;fetch 进行中
//     重复 open 共享同一次 in-flight fetch(不产生重复请求);fetch
//     失败不缓存空列表,下一次打开重试。
//   - system_root(`@/`,~1s walk + 5000 文件 cap):维持会话级缓存,
//     面板重开不重拉。
//   - resetFilePanelState(项目切换):detach 进行中的 fetch,迟到
//     写回被 gen 守卫丢弃;下一次打开发起全新 fetch。
//
// 复用 chatInputCodeMirror.paste.test.ts 的 Host 挂载模式:被测对象是
// composable 本身,`fileItemsSource` 是注入的 spy(ChatInput.vue 里它
// 才落到 transport.invoke —— 那层已是薄胶水,不在此重复覆盖)。

import { describe, it, expect, vi } from "vitest";
import { defineComponent, h, ref, type PropType } from "vue";
import { mount, type VueWrapper } from "@vue/test-utils";
import {
  useChatInputCodeMirror,
  type ChatInputCodeMirrorApi,
  type UseChatInputCodeMirrorOpts,
} from "./chatInputCodeMirror";
import type { TriggerMenuItem } from "../components/chat/TriggerMenu.vue";

type FileItemsSource = NonNullable<UseChatInputCodeMirrorOpts["fileItemsSource"]>;

/** mount 是同步的,setup 运行后立即填充 —— 每个 test 挂载前置 null,
 * 防止跨用例串号。 */
let mountedApi: ChatInputCodeMirrorApi | null = null;

/** Tiny host component — same wiring ChatInput.vue uses (div ref +
 * sending/placeholder refs). The composable API is captured into the
 * module-level `mountedApi` (VTU 的 wrapper.exposed 在本仓库版本不可
 * 用,闭包捕获最简)。 */
const Host = defineComponent({
  props: {
    fileItemsSource: {
      type: Function as PropType<FileItemsSource>,
      required: true,
    },
  },
  setup(props) {
    const host = ref<HTMLDivElement | null>(null);
    const api = useChatInputCodeMirror({
      host,
      sending: ref(false),
      placeholder: ref(undefined),
      onSubmit: () => {},
      fileItemsSource: props.fileItemsSource,
    });
    mountedApi = api;
    return () => h("div", { ref: host });
  },
});

function mountHost(source: FileItemsSource): {
  wrapper: VueWrapper;
  api: ChatInputCodeMirrorApi;
} {
  mountedApi = null;
  const wrapper = mount(Host, { props: { fileItemsSource: source } });
  // CM mounts synchronously in onMounted — assert it really did, or
  // the doc-driven palette sync below would silently no-op.
  expect(wrapper.element.querySelector(".cm-editor")).toBeTruthy();
  const api = mountedApi;
  expect(api).toBeTruthy();
  return { wrapper, api: api! };
}

/** panelItems 的 name 列表(断言用)。 */
function names(api: ChatInputCodeMirrorApi): string[] {
  return api.panelItems.value.map((i) => i.name);
}

/** 排空所有 pending microtask(settled promise 的 continuation +
 * loadFilesForMode 的写回都跑在下一个 macrotask 之前)。 */
function flushMicrotasks(): Promise<void> {
  return new Promise((r) => setTimeout(r, 0));
}

function item(name: string): TriggerMenuItem {
  return { key: name, name };
}

// jsdom 的 Range 没有 layout API。本文件会 dispatch CM doc 变更
// (replaceDoc),触发 CM 的 rAF measure → clientRectsFor(textNode) →
// Range.getClientRects,在 jsdom 下抛 "not a function"(paste 测试
// 不改 doc 所以从未踩到)。返回空列表即可让 CM 优雅回退
// (rects.length != 1 → 默认字号路径);被测的面板同步是纯 state
// 逻辑,不依赖任何 layout 结果。只在本文件内打补丁,不动全局 setup。
{
  const rangeProto = Range.prototype as unknown as {
    getClientRects?: () => DOMRectList;
    getBoundingClientRect?: () => DOMRect;
  };
  if (!rangeProto.getClientRects) {
    rangeProto.getClientRects = () => [] as unknown as DOMRectList;
    rangeProto.getBoundingClientRect = () =>
      ({ width: 0, height: 0, top: 0, left: 0, right: 0, bottom: 0, x: 0, y: 0 }) as DOMRect;
  }
}

describe("file palette: shallow refetch per open (08-26-f5 P1)", () => {
  it("shallow: close → reopen re-invokes fileItemsSource (no loaded-flag skip)", async () => {
    const source = vi.fn(async () => [item("a.txt")]);
    const { wrapper, api } = mountHost(source);

    api.replaceDoc("@", 1);
    expect(api.filePaletteOpen.value).toBe(true);
    await vi.waitFor(() => {
      expect(names(api)).toEqual(["a.txt"]);
    });
    expect(source).toHaveBeenCalledTimes(1);
    expect(source).toHaveBeenLastCalledWith("shallow");

    api.closeFilePalette();
    expect(api.filePaletteOpen.value).toBe(false);

    // 重开(doc change → syncFilePalette → openFilePalette)必须再次
    // fetch —— 这是"同项目新 copy 的文件重开面板即可见"(AC3)的保证。
    api.replaceDoc("@b", 2);
    expect(api.filePaletteOpen.value).toBe(true);
    await vi.waitFor(() => {
      expect(source).toHaveBeenCalledTimes(2);
    });
    expect(source).toHaveBeenLastCalledWith("shallow");
    wrapper.unmount();
  });

  it("shallow: concurrent opens share ONE in-flight fetch (dedup, not skip)", async () => {
    let resolveFetch!: (items: TriggerMenuItem[]) => void;
    const source = vi.fn(
      () =>
        new Promise<TriggerMenuItem[]>((res) => {
          resolveFetch = res;
        }),
    );
    const { wrapper, api } = mountHost(source);

    api.replaceDoc("@", 1);
    expect(source).toHaveBeenCalledTimes(1);

    // 第一次 fetch 仍未 settle 时 close + reopen:不得产生重复请求。
    api.closeFilePalette();
    api.replaceDoc("@a", 2);
    expect(api.filePaletteOpen.value).toBe(true);
    await flushMicrotasks();
    expect(source).toHaveBeenCalledTimes(1);

    // settle 之后再次重开:又 fetch(去重只作用于 in-flight,不是
    // loaded 标志 —— 与上一用例共同钉死语义)。
    resolveFetch([item("n.txt")]);
    await vi.waitFor(() => {
      expect(names(api)).toEqual(["n.txt"]);
    });
    api.closeFilePalette();
    api.replaceDoc("@c", 2);
    expect(source).toHaveBeenCalledTimes(2);
    wrapper.unmount();
  });

  it("shallow: a failed fetch is not cached — the next open retries", async () => {
    const errSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    let fail = true;
    const source = vi.fn(async (): Promise<TriggerMenuItem[]> => {
      if (fail) throw new Error("ipc down");
      return [item("ok.txt")];
    });
    const { wrapper, api } = mountHost(source);

    api.replaceDoc("@", 1);
    await vi.waitFor(() => {
      expect(names(api)).toEqual([]); // 出错 → 空列表
    });
    expect(source).toHaveBeenCalledTimes(1);

    api.closeFilePalette();
    fail = false;
    api.replaceDoc("@a", 2);
    await vi.waitFor(() => {
      expect(names(api)).toEqual(["ok.txt"]); // 重试且落盘
    });
    expect(source).toHaveBeenCalledTimes(2);
    errSpy.mockRestore();
    wrapper.unmount();
  });

  it("system_root: fetched once per session — close → reopen does NOT refetch", async () => {
    const source = vi.fn(async (mode: "shallow" | "system_root") =>
      mode === "system_root" ? [item("/etc/hosts")] : [],
    );
    const { wrapper, api } = mountHost(source);

    api.replaceDoc("@/", 2);
    expect(api.fileViewMode.value).toBe("system_root");
    await vi.waitFor(() => {
      expect(names(api)).toEqual(["/etc/hosts"]);
    });
    expect(source).toHaveBeenCalledTimes(1);

    api.closeFilePalette();
    api.replaceDoc("@/e", 3);
    expect(api.filePaletteOpen.value).toBe(true);
    await flushMicrotasks();
    // 会话级缓存生效:重开不再重跑 ~1s 的 `/` walk。
    expect(source).toHaveBeenCalledTimes(1);
    expect(names(api)).toEqual(["/etc/hosts"]);
    wrapper.unmount();
  });

  it("resetFilePanelState drops a late shallow write (project-switch semantics)", async () => {
    let resolveOld!: (items: TriggerMenuItem[]) => void;
    const source = vi.fn(
      () =>
        new Promise<TriggerMenuItem[]>((res) => {
          resolveOld = res;
        }),
    );
    const { wrapper, api } = mountHost(source);

    api.replaceDoc("@", 1);
    expect(source).toHaveBeenCalledTimes(1);

    // fetch 进行中发生项目切换(reset):旧 fetch 的迟到结果不得写回
    // 已清空的面板状态。
    api.resetFilePanelState();
    resolveOld([item("old.txt")]);
    await flushMicrotasks();
    expect(names(api)).toEqual([]);

    // 下一次打开发起全新 fetch(detach 掉的那次不再占 in-flight 槽)。
    api.replaceDoc("@n", 2);
    expect(source).toHaveBeenCalledTimes(2);
    wrapper.unmount();
  });
});
