// Tests for `ShellCard.vue` — shell / run_background_shell 专属卡片
// (2026-08-30, task `08-30-shell-description` PR3)。
//
// 覆盖 design §4 矩阵:
//   1. Header chip 三级兜底(description → 命令首行 → 隐藏)。
//   2. 命令块常驻:`$` 前缀 + command 原文;cwd 次行有/无。
//   3. background pill(仅 run_background_shell)。
//   4. 待审批态:status "等待审批" + 风险条 + 4 按钮渲染 +
//      onRespond 接线(mock permStore,仿 ToolCallCard/EditFileCard
//      测试法:transport mock + 真实 Pinia store)。
//   5. 命令不重复:一体化审批下命令全文只出现一次(无独立
//      "需要权限"盒子)。
//   6. done 折叠 output / error 红框常显。
//   7. command 畸形 → ToolInputBody 降级。

import { describe, it, expect, beforeEach, vi } from "vitest";
import { mount, flushPromises } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";

const invokeMock = vi.fn();
vi.mock("../../transport", () => ({
  transport: {
    invoke: (...args: unknown[]) => invokeMock(...args),
    listen: vi.fn(async () => () => {}),
  },
}));

import ShellCard from "./ShellCard.vue";
import {
  usePermissionsStore,
  type PermissionAsk,
} from "../../stores/permissions";
import { useChatStore } from "../../stores/chat";
import type { ToolCallInfo, ToolResultInfo } from "../../stores/chat.types";

function makeCall(overrides: Partial<ToolCallInfo> = {}): ToolCallInfo {
  return {
    id: "tu-1",
    name: "shell",
    input: { command: "cargo test -p everlasting --lib" },
    ...overrides,
  };
}

function makeResult(overrides: Partial<ToolResultInfo> = {}): ToolResultInfo {
  return {
    toolUseId: "tu-1",
    content: "test result: ok",
    isError: false,
    ...overrides,
  };
}

function makeAsk(overrides: Partial<PermissionAsk> = {}): PermissionAsk {
  return {
    rid: "rid-1",
    sessionId: "sess-1",
    toolUseId: "tu-1",
    toolName: "shell",
    toolInput: { command: "cargo test -p everlasting --lib" },
    risk: "high",
    ...overrides,
  };
}

describe("ShellCard", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    invokeMock.mockReset();
    invokeMock.mockResolvedValue(true);
  });

  function mountCard(props: { call: ToolCallInfo; result?: ToolResultInfo }) {
    return mount(ShellCard, {
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
  // 1. Header chip 三级兜底
  // ----------------------------------------------------------------
  describe("header chip fallback chain", () => {
    it("level 1: shows input.description when present", () => {
      const w = mountCard({
        call: makeCall({
          input: { command: "cargo test", description: "跑 shell 工具单测" },
        }),
      });
      expect(w.find(".tool-call-header__chip").text()).toContain(
        "跑 shell 工具单测",
      );
    });

    it("level 2: falls back to the command first line without description", () => {
      const w = mountCard({
        call: makeCall({ input: { command: "cargo test\n--lib" } }),
      });
      expect(w.find(".tool-call-header__chip").text()).toContain("cargo test");
    });

    it("level 3: hides the chip when neither description nor command exists", () => {
      const w = mountCard({ call: makeCall({ input: {} }) });
      expect(w.find(".tool-call-header__chip").exists()).toBe(false);
    });
  });

  // ----------------------------------------------------------------
  // 2. 命令块(常驻)
  // ----------------------------------------------------------------
  describe("command block", () => {
    it("renders the $ prefix and the verbatim command", () => {
      const w = mountCard({ call: makeCall() });
      const cmd = w.get(".shell-card__cmd");
      expect(cmd.text()).toContain("$ ");
      expect(cmd.text()).toContain("cargo test -p everlasting --lib");
      expect(w.find(".shell-card__command").exists()).toBe(true);
    });

    it("renders the cwd row when working_directory is set", () => {
      const w = mountCard({
        call: makeCall({
          input: { command: "ls", working_directory: "/repo/app" },
        }),
      });
      const cwd = w.get(".shell-card__cwd");
      expect(cwd.text()).toContain("↳");
      expect(cwd.text()).toContain("/repo/app");
      expect(cwd.attributes("title")).toBe("/repo/app");
    });

    it("omits the cwd row when working_directory is absent", () => {
      const w = mountCard({ call: makeCall() });
      expect(w.find(".shell-card__cwd").exists()).toBe(false);
    });

    it("omits the cwd row for a malformed working_directory (PRD R5 兼容)", () => {
      const w = mountCard({
        call: makeCall({
          input: { command: "ls", working_directory: 12345 },
        }),
      });
      expect(w.find(".shell-card__cwd").exists()).toBe(false);
    });
  });

  // ----------------------------------------------------------------
  // 3. background pill
  // ----------------------------------------------------------------
  describe("background pill", () => {
    it("renders the pill for run_background_shell", () => {
      const w = mountCard({
        call: makeCall({ name: "run_background_shell" }),
      });
      expect(w.get(".shell-card__pill").text()).toBe("background");
    });

    it("renders no pill for the synchronous shell", () => {
      const w = mountCard({ call: makeCall({ name: "shell" }) });
      expect(w.find(".shell-card__pill").exists()).toBe(false);
    });
  });

  // ----------------------------------------------------------------
  // 4. 一体化审批(等待审批态)
  // ----------------------------------------------------------------
  describe("integrated approval (waiting state)", () => {
    it("shows 等待审批 status while a matching ask is pending", () => {
      armPending();
      const w = mountCard({ call: makeCall() });
      expect(w.find(".tool-call-header__status").text()).toContain("等待审批");
    });

    it("keeps running… status when no ask is pending", () => {
      const chat = useChatStore();
      chat.currentSessionId = "sess-1";
      const w = mountCard({ call: makeCall() });
      expect(w.find(".tool-call-header__status").text()).toContain("running…");
    });

    it("renders the risk bar (dot + 风险 label + reason) without a 需要权限 box", () => {
      armPending({ risk: "high", reason: "matches asklist: cargo" });
      const w = mountCard({ call: makeCall() });
      const risk = w.get(".shell-card__risk");
      expect(risk.text()).toContain("风险:");
      expect(risk.text()).toContain("高");
      expect(risk.text()).toContain("matches asklist: cargo");
      // 不渲染独立"需要权限"容器(PRD R3:一体化,去独立盒子)。
      expect(w.text()).not.toContain("需要权限");
    });

    it("renders the 4 approval buttons", () => {
      armPending();
      const w = mountCard({ call: makeCall() });
      const text = w.text();
      expect(text).toContain("仅一次");
      expect(text).toContain("始终允许");
      expect(text).toContain("拒绝");
      expect(text).toContain("拒绝并说明");
    });

    it("wires 仅一次 through respondApproval to the permission_response IPC", async () => {
      armPending();
      const w = mountCard({ call: makeCall() });
      await w.get(".permission-ask-body__btn--once").trigger("click");
      await flushPromises();
      expect(invokeMock).toHaveBeenCalledWith("permission_response", {
        rid: "rid-1",
        decision: "allow_once",
        reason: undefined,
      });
    });

    it("拒绝并说明 opens the textarea and submits the reason through onRespond", async () => {
      armPending();
      const w = mountCard({ call: makeCall() });
      await w.findAll(".permission-ask-body__btn--deny")[1].trigger("click");
      expect(w.find(".permission-ask-body__textarea").exists()).toBe(true);
      await w.get("textarea").setValue("改用 cargo nextest");
      await w
        .get(
          ".permission-ask-body__feedback-actions .permission-ask-body__btn--deny",
        )
        .trigger("click");
      await flushPromises();
      expect(invokeMock).toHaveBeenCalledWith("permission_response", {
        rid: "rid-1",
        decision: "deny",
        reason: "改用 cargo nextest",
      });
    });

    it("does NOT render the approval UI when the pending toolUseId ≠ call.id", () => {
      armPending({ toolUseId: "other-tu" });
      const w = mountCard({ call: makeCall() });
      expect(w.find(".shell-card__approval").exists()).toBe(false);
      expect(w.find(".permission-ask-body__btn--once").exists()).toBe(false);
    });

    it("hides the approval UI once a result arrives", () => {
      armPending();
      const w = mountCard({
        call: makeCall(),
        result: makeResult(),
      });
      expect(w.find(".shell-card__approval").exists()).toBe(false);
    });

    it("shows the command exactly once in the waiting state (命令不重复)", () => {
      // chip 用 description(≠命令文本),命令全文只应在命令块出现一次
      // —— 没有第二个 PermissionAskBody 盒子重复它。
      armPending({
        toolInput: {
          command: "find . -name '*.ts' -print0 | xargs -0 wc -l",
          description: "统计 ts 行数",
        },
      });
      const w = mountCard({
        call: makeCall({
          input: {
            command: "find . -name '*.ts' -print0 | xargs -0 wc -l",
            description: "统计 ts 行数",
          },
        }),
      });
      const command = "find . -name '*.ts' -print0 | xargs -0 wc -l";
      const occurrences = w.text().split(command).length - 1;
      expect(occurrences).toBe(1);
    });
  });

  // ----------------------------------------------------------------
  // 5. 输出呈现
  // ----------------------------------------------------------------
  describe("output rendering", () => {
    it("done → folded ToolOutputBody, no error pre", () => {
      const w = mountCard({
        call: makeCall(),
        result: makeResult({ content: "ok\n[exit code: 0]" }),
      });
      expect(w.find(".tool-output-body").exists()).toBe(true);
      expect(w.find(".shell-card__error-out").exists()).toBe(false);
    });

    it("error → red-framed pre always visible, no folded output", () => {
      const w = mountCard({
        call: makeCall(),
        result: makeResult({
          content: "thread panicked\n[exit code: 101]",
          isError: true,
        }),
      });
      const err = w.get(".shell-card__error-out");
      expect(err.text()).toContain("thread panicked");
      expect(w.find(".tool-output-body").exists()).toBe(false);
    });

    it("error output longer than 500 chars is truncated", () => {
      const long = "x".repeat(2000);
      const w = mountCard({
        call: makeCall(),
        result: makeResult({ content: long, isError: true }),
      });
      const err = w.get(".shell-card__error-out");
      expect(err.text()).toContain("more chars");
      // Truncated at 500 — not the full 2000 chars.
      expect(err.text().length).toBeLessThan(600);
    });

    it("no output section while running (命令块即全部)", () => {
      const w = mountCard({ call: makeCall() });
      expect(w.find(".tool-output-body").exists()).toBe(false);
      expect(w.find(".shell-card__error-out").exists()).toBe(false);
    });

    // R6: done 态 header 着成功色(error 不着),锁 ToolCallHeader isSuccess 接线。
    it("done → header --success on; error → header --success off", () => {
      const done = mountCard({
        call: makeCall(),
        result: makeResult({ content: "ok" }),
      });
      expect(done.find(".tool-call-header--success").exists()).toBe(true);
      const errored = mountCard({
        call: makeCall(),
        result: makeResult({ content: "boom", isError: true }),
      });
      expect(errored.find(".tool-call-header--success").exists()).toBe(false);
    });
  });

  // ----------------------------------------------------------------
  // 6. 畸形降级
  // ----------------------------------------------------------------
  describe("degradation", () => {
    it("missing command → ToolInputBody fallback, no command block", () => {
      const w = mountCard({ call: makeCall({ input: {} }) });
      expect(w.find(".shell-card__command").exists()).toBe(false);
      expect(w.find(".tool-input-body").exists()).toBe(true);
    });

    it("non-string command → ToolInputBody fallback", () => {
      const w = mountCard({
        call: makeCall({ input: { command: 12345 } }),
      });
      expect(w.find(".shell-card__command").exists()).toBe(false);
      expect(w.find(".tool-input-body").exists()).toBe(true);
    });
  });
});
