// Tests for `ReadToolCard.vue` — read 族(glob / list_dir / read_file)
// 紧凑卡(2026-09-19, task `09-19-tool-card-compact-read`)。
//
// 覆盖 PRD R1-R4 / AC5:
//   1. headline:chip(目标)+ meta(规模)进同一行,报错态 meta 留空。
//   2. 展开/收起:默认收起、输出不在 DOM;点击/键盘切换;二次点击收起。
//   3. 出口径:展开区含输出 pre(解 envelope)+ input details;
//      报错展开是错误原文。
//   4. 审批:命中 pendingAsk 时审批区无条件渲染(与展开态解耦),
//      接线照 ToolCallCard(transport mock + 真实 Pinia store)。
//   5. 读图:result.images 渲染 ToolResultImages(需要 sessionId)。

import { describe, it, expect, beforeEach, vi } from "vitest";
import { mount } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";

const invokeMock = vi.fn();
vi.mock("../../transport", () => ({
  transport: {
    invoke: (...args: unknown[]) => invokeMock(...args),
    listen: vi.fn(async () => () => {}),
  },
}));

import ReadToolCard from "./ReadToolCard.vue";
import {
  usePermissionsStore,
  type PermissionAsk,
} from "../../stores/permissions";
import { useChatStore } from "../../stores/chat";
import type { ToolCallInfo, ToolResultInfo } from "../../stores/chat.types";

function makeCall(overrides: Partial<ToolCallInfo> = {}): ToolCallInfo {
  return {
    id: "tu-1",
    name: "read_file",
    input: { path: "app/src/App.vue" },
    ...overrides,
  };
}

function makeResult(overrides: Partial<ToolResultInfo> = {}): ToolResultInfo {
  return {
    toolUseId: "tu-1",
    content: JSON.stringify({
      result: "\t1\t<script setup lang=\"ts\">\n\t2\tconst a = 1;",
      cwd: "/repo",
    }),
    isError: false,
    ...overrides,
  };
}

function makeAsk(overrides: Partial<PermissionAsk> = {}): PermissionAsk {
  return {
    rid: "rid-1",
    sessionId: "sess-1",
    toolUseId: "tu-1",
    toolName: "read_file",
    toolInput: { path: "/etc/hosts" },
    risk: "medium",
    ...overrides,
  };
}

describe("ReadToolCard", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    invokeMock.mockReset();
    invokeMock.mockResolvedValue(true);
  });

  function mountCard(props: {
    call: ToolCallInfo;
    result?: ToolResultInfo;
    sessionId?: string;
  }) {
    return mount(ReadToolCard, {
      props,
      global: { stubs: { Icon: true } },
    });
  }

  /** 把当前 session 指到 sess-1 并挂一个匹配 call.id 的 pending ask。 */
  function armPending(askOverrides: Partial<PermissionAsk> = {}) {
    const chat = useChatStore();
    const perm = usePermissionsStore();
    chat.currentSessionId = "sess-1";
    perm.setPending(makeAsk(askOverrides));
    return { chat, perm };
  }

  // ----------------------------------------------------------------
  // 1. headline
  // ----------------------------------------------------------------
  describe("headline", () => {
    it("chip 显示路径,meta 显示行范围(同一行)", () => {
      const w = mountCard({ call: makeCall(), result: makeResult() });
      expect(w.get(".tool-call-header__chip").text()).toContain("app/src/App.vue");
      expect(w.get(".rocard__meta").text()).toBe("L1–2");
    });

    it("glob 的 chip 是 pattern(不是 path)", () => {
      const w = mountCard({
        call: makeCall({ name: "glob", input: { pattern: "src/**/*.vue" } }),
        result: makeResult({
          content: JSON.stringify({ result: "a.vue\nb.vue", cwd: "/repo" }),
        }),
      });
      expect(w.get(".tool-call-header__chip").text()).toContain("src/**/*.vue");
      expect(w.get(".rocard__meta").text()).toBe("2 matches");
    });

    it("list_dir 无 path 时 chip 是 cwd 占位", () => {
      const w = mountCard({
        call: makeCall({ name: "list_dir", input: {} }),
        result: makeResult({
          content: JSON.stringify({ result: "a\nb\nc", cwd: "/repo" }),
        }),
      });
      expect(w.get(".tool-call-header__chip").text()).toContain("cwd");
      expect(w.get(".rocard__meta").text()).toBe("3 entries");
    });

    it("报错态:meta 留空、卡着 error、status 是 error", () => {
      const w = mountCard({
        call: makeCall(),
        result: makeResult({ content: "Failed to read file '/x'", isError: true }),
      });
      expect(w.find(".rocard__meta").exists()).toBe(false);
      expect(w.find(".rocard--error").exists()).toBe(true);
      expect(w.get(".tool-call-header__status").text()).toContain("error");
    });

    it("running(无 result):status running…,meta 留空", () => {
      const w = mountCard({ call: makeCall() });
      expect(w.find(".rocard--running").exists()).toBe(true);
      expect(w.get(".tool-call-header__status").text()).toContain("running…");
      expect(w.find(".rocard__meta").exists()).toBe(false);
    });
  });

  // ----------------------------------------------------------------
  // 2. 展开 / 收起
  // ----------------------------------------------------------------
  describe("expand / collapse", () => {
    it("默认收起:body 不在 DOM(长输出不参与渲染)", () => {
      const w = mountCard({ call: makeCall(), result: makeResult() });
      expect(w.find(".rocard__body").exists()).toBe(false);
      expect(w.find(".tool-output-body__pre").exists()).toBe(false);
      expect(w.get(".rocard__row").attributes("aria-expanded")).toBe("false");
    });

    it("点击行展开:输出 pre(已解 envelope)+ input details", async () => {
      const w = mountCard({ call: makeCall(), result: makeResult() });
      await w.get(".rocard__row").trigger("click");
      expect(w.get(".rocard__row").attributes("aria-expanded")).toBe("true");
      const pre = w.get(".tool-output-body__pre");
      expect(pre.text()).toContain('<script setup lang="ts">');
      expect(pre.text()).not.toContain('{"result"');
      expect(w.find(".tool-input-body").exists()).toBe(true);
    });

    it("再点收起:body 又离开 DOM", async () => {
      const w = mountCard({ call: makeCall(), result: makeResult() });
      await w.get(".rocard__row").trigger("click");
      expect(w.find(".rocard__body").exists()).toBe(true);
      await w.get(".rocard__row").trigger("click");
      expect(w.find(".rocard__body").exists()).toBe(false);
    });

    it("键盘 Enter / Space 同样切换(role=button 可达)", async () => {
      const w = mountCard({ call: makeCall(), result: makeResult() });
      await w.get(".rocard__row").trigger("keydown.enter");
      expect(w.find(".rocard__body").exists()).toBe(true);
      await w.get(".rocard__row").trigger("keydown.space");
      expect(w.find(".rocard__body").exists()).toBe(false);
    });

    it("展开区输出超长走 500 字截断(沿用 ToolOutputBody 契约)", async () => {
      const long = "x".repeat(900);
      const w = mountCard({
        call: makeCall(),
        result: makeResult({ content: JSON.stringify({ result: long, cwd: "/repo" }) }),
      });
      await w.get(".rocard__row").trigger("click");
      expect(w.get(".tool-output-body__pre").text()).toContain("more chars");
    });

    it("报错展开是错误原文(error 样式保留)", async () => {
      const w = mountCard({
        call: makeCall(),
        result: makeResult({
          content: JSON.stringify({
            result: "Failed to read file '/x': No such file or directory",
            cwd: "/repo",
          }),
          isError: true,
        }),
      });
      await w.get(".rocard__row").trigger("click");
      expect(w.get(".tool-output-body__pre").text()).toContain(
        "No such file or directory",
      );
      expect(w.find(".tool-output-body__pre--error").exists()).toBe(true);
    });
  });

  // ----------------------------------------------------------------
  // 3. 审批(接线照 ToolCallCard)
  // ----------------------------------------------------------------
  describe("inline approval", () => {
    it("命中 pendingAsk → 审批区渲染,且不受收起态影响", () => {
      const w = mountCard({ call: makeCall() });
      armPending();
      return w.vm.$nextTick().then(() => {
        expect(w.find(".rocard__approval").exists()).toBe(true);
        expect(w.find(".rocard__body").exists()).toBe(false);
      });
    });

    it("ask 的 toolUseId 不匹配本卡 → 不渲染审批区", async () => {
      const w = mountCard({ call: makeCall() });
      armPending({ toolUseId: "tu-other" });
      await w.vm.$nextTick();
      expect(w.find(".rocard__approval").exists()).toBe(false);
    });

    it("已有结果 → 审批区撤下 + store 里的 pending 被清掉(120s 超时 toast 护栏)", async () => {
      const w = mountCard({ call: makeCall() });
      const { perm } = armPending();
      await w.vm.$nextTick();
      expect(w.find(".rocard__approval").exists()).toBe(true);
      // ask → 决策 → 工具执行 → 结果到达:同一张卡的 result prop 落地。
      await w.setProps({ result: makeResult() });
      expect(w.find(".rocard__approval").exists()).toBe(false);
      expect(perm.getPending("sess-1")).toBeUndefined();
    });
  });

  // ----------------------------------------------------------------
  // 4. 读图
  // ----------------------------------------------------------------
  describe("image results", () => {
    it("有 images + sessionId → 渲染缩略图组件", async () => {
      const w = mountCard({
        call: makeCall({ input: { path: "a.png" } }),
        result: makeResult({
          content: JSON.stringify({ result: "[image: /repo/a.png — 已作为图片块发送]", cwd: "/repo" }),
          images: [{ file: "f1", media_type: "image/png", source: "attachment" }],
        }),
        sessionId: "sess-1",
      });
      await w.vm.$nextTick();
      expect(w.find(".tool-result-images").exists()).toBe(true);
      expect(w.get(".rocard__meta").text()).toBe("image");
    });

    it("无 sessionId(跨会话只读预览)→ 不渲染缩略图", () => {
      const w = mountCard({
        call: makeCall({ input: { path: "a.png" } }),
        result: makeResult({
          images: [{ file: "f1", media_type: "image/png", source: "attachment" }],
        }),
      });
      expect(w.find(".tool-result-images").exists()).toBe(false);
    });
  });
});
