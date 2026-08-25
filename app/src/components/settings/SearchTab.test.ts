// Tests for `SearchTab.vue` — Settings → 搜索 tab(F4 WP3)。
//
// 契约(AC4 前端半 / key 三态):
//   1. mount → `get_web_search_config` 拉取,provider 下拉回填存储值。
//   2. masked 回显:已存 key 时 placeholder 含 masked(明文永不下发)。
//   3. key 留空保存 → args **不含** `tavilyApiKey`(后端 None = 不动)。
//   4. 填 key 保存 → args 带 `tavilyApiKey` 明文(Some(非空) = 重加密落盘)。
//   5. 「清除已存 key」按钮仅在 keySet 时出现;点击 → `tavilyApiKey: ""`
//      (Some("") = 删行,auto 回落 DDG 不复活)。
//
// transport 全量 mock(store 先例,chatMode.test.ts 同款);projects
// store mock 掉 showToast(toast DOM 不属本测试面)。

import { describe, it, expect, beforeEach, vi } from "vitest";
import { mount, flushPromises } from "@vue/test-utils";
import { setActivePinia, createPinia } from "pinia";

const invokeMock = vi.fn();
vi.mock("../../transport", () => ({
  transport: {
    invoke: (...args: unknown[]) => invokeMock(...args),
    listen: async () => () => {},
  },
}));

const showToastMock = vi.fn();
vi.mock("../../stores/projects", () => ({
  useProjectsStore: () => ({ showToast: showToastMock }),
}));

import SearchTab from "./SearchTab.vue";

/** get 返回体;后续 set 调用后的回读也走它(顺序 stub)。 */
function stubGet(cfg: {
  provider?: string;
  tavilyKeySet?: boolean;
  tavilyKeyMasked?: string | null;
}) {
  invokeMock.mockImplementation(async (cmd: string) => {
    if (cmd === "get_web_search_config") {
      return {
        provider: cfg.provider ?? "auto",
        tavilyKeySet: cfg.tavilyKeySet ?? false,
        tavilyKeyMasked: cfg.tavilyKeyMasked ?? null,
      };
    }
    return null;
  });
}

async function mountTab() {
  const w = mount(SearchTab, { global: { plugins: [createPinia()] } });
  await flushPromises();
  return w;
}

beforeEach(() => {
  setActivePinia(createPinia());
  invokeMock.mockReset();
  showToastMock.mockClear();
});

describe("SearchTab", () => {
  it("mount 拉取配置并回填 provider 下拉", async () => {
    stubGet({ provider: "ddg" });
    const w = await mountTab();
    expect(invokeMock).toHaveBeenCalledWith("get_web_search_config");
    const select = w.find("select");
    expect((select.element as HTMLSelectElement).value).toBe("ddg");
  });

  it("已存 key 时 placeholder 显示 masked,不显示明文", async () => {
    stubGet({ tavilyKeySet: true, tavilyKeyMasked: "tvly-****1234" });
    const w = await mountTab();
    const placeholder = w.find("input[type='password']").attributes("placeholder");
    expect(placeholder).toContain("tvly-****1234");
    expect(placeholder).toContain("留空");
  });

  it("key 留空保存 → args 不含 tavilyApiKey(不动语义)", async () => {
    stubGet({ provider: "auto", tavilyKeySet: true, tavilyKeyMasked: "tvly-****9999" });
    const w = await mountTab();
    await w.find("select").setValue("tavily");
    // key 输入留空
    await w.find("button.btn--primary").trigger("click");
    await flushPromises();
    const setCall = invokeMock.mock.calls.find((c) => c[0] === "set_web_search_config");
    expect(setCall).toBeTruthy();
    expect(setCall?.[1]).toEqual({ provider: "tavily" });
    expect(setCall?.[1]).not.toHaveProperty("tavilyApiKey");
  });

  it("填 key 保存 → args 带明文 tavilyApiKey", async () => {
    stubGet({ provider: "auto" });
    const w = await mountTab();
    await w.find("input[type='password']").setValue("tvly-newkey-123");
    await w.find("button.btn--primary").trigger("click");
    await flushPromises();
    const setCall = invokeMock.mock.calls.find((c) => c[0] === "set_web_search_config");
    expect(setCall?.[1]).toEqual({
      provider: "auto",
      tavilyApiKey: "tvly-newkey-123",
    });
  });

  it("清除按钮仅在 keySet 时出现;点击传空串删行", async () => {
    stubGet({ provider: "auto", tavilyKeySet: true, tavilyKeyMasked: "tvly-****1234" });
    const w = await mountTab();
    const clearBtn = w.find("button.btn--ghost");
    expect(clearBtn.exists()).toBe(true);
    expect(clearBtn.text()).toContain("清除已存 key");
    await clearBtn.trigger("click");
    await flushPromises();
    const setCall = invokeMock.mock.calls.find((c) => c[0] === "set_web_search_config");
    expect(setCall?.[1]).toEqual({ provider: "auto", tavilyApiKey: "" });
  });

  it("无 key 时不渲染清除按钮", async () => {
    stubGet({ provider: "auto", tavilyKeySet: false });
    const w = await mountTab();
    expect(w.find("button.btn--ghost").exists()).toBe(false);
  });

  it("保存失败显示 error banner 且不崩", async () => {
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "get_web_search_config") {
        return { provider: "auto", tavilyKeySet: false, tavilyKeyMasked: null };
      }
      throw new Error("db locked");
    });
    const w = await mountTab();
    await w.find("button.btn--primary").trigger("click");
    await flushPromises();
    expect(w.find(".search-tab__error").exists()).toBe(true);
    expect(w.find(".search-tab__error").text()).toContain("db locked");
    expect(showToastMock).toHaveBeenCalled();
  });
});
