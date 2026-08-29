// Regression tests for the falsy-zero seq guard (BUGLIST CH4-1,
// 2026-08-29 GUI full-test): the session's FIRST user message has
// `seq === 0` (backend counter starts at 0, `chat_loop/init.rs`
// `unwrap_or(0)`), and a falsy `!message().seq` guard used to
// reject it with "消息缺少 seq,无法编辑" while the actions menu
// gate (`MessageItem.vue`, `seq !== undefined`) happily showed the
// entry — every first message was uneditable, reload included.
//
// The guard must be `seq == null` (only queued placeholder rows
// lack a seq). These tests lock both directions:
//   1. seq=0 → edit save goes through (store action called with 0,
//      edit mode closes).
//   2. seq=0 → resend goes through (store action called with 0).
//   3. seq=undefined → edit blocked with the inline error, store
//      action NOT called.
//   4. seq=undefined → resend blocked with the error toast.

import { describe, it, expect, vi, beforeEach } from "vitest";
import { setActivePinia, createPinia } from "pinia";

const invokeMock = vi.fn();

vi.mock("../../transport", () => ({
  transport: {
    invoke: (...args: unknown[]) => invokeMock(...args),
    listen: async () => () => {},
  },
}));

import { useMessageEditing } from "./useMessageEditing";
import { useChatStore } from "../../stores/chat";
import { useProjectsStore } from "../../stores/projects";
import type { ChatMessage } from "../../stores/chat.types";

function makeMessage(overrides: Partial<ChatMessage> = {}): ChatMessage {
  return {
    id: "m1",
    role: "user",
    content: "原始内容",
    ...overrides,
  };
}

describe("useMessageEditing — seq=0 (first message) is editable", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    invokeMock.mockReset();
  });

  it("handleSave: seq=0 走保存路径,不报「缺少 seq」", async () => {
    const chatStore = useChatStore();
    chatStore.currentSessionId = "sess-1";
    chatStore.editingMessageSeq = 0; // edit mode open on the first message
    const editMessageSpy = vi
      .spyOn(chatStore, "editMessage")
      .mockResolvedValue(undefined);

    const msg = makeMessage({ seq: 0 });
    const { handleSave, editError } = useMessageEditing(() => msg, () => false);

    await handleSave("改后的内容");

    expect(editMessageSpy).toHaveBeenCalledWith("sess-1", 0, "改后的内容");
    expect(editError.value).toBeNull();
    // Success path closes edit mode.
    expect(chatStore.editingMessageSeq).toBeNull();
  });

  it("handleResend: seq=0 走重发路径,不弹「缺少 seq」toast", async () => {
    const chatStore = useChatStore();
    chatStore.currentSessionId = "sess-1";
    const resendSpy = vi
      .spyOn(chatStore, "resendMessage")
      .mockResolvedValue(undefined);
    const projectsStore = useProjectsStore();
    const toastSpy = vi.spyOn(projectsStore, "showToast");

    const msg = makeMessage({ seq: 0 });
    const { handleResend } = useMessageEditing(() => msg, () => false);

    await handleResend();

    expect(resendSpy).toHaveBeenCalledWith("sess-1", 0, "原始内容");
    expect(toastSpy).not.toHaveBeenCalled();
  });

  it("handleSave: seq 缺失(排队占位)仍拦下并报错", async () => {
    const chatStore = useChatStore();
    chatStore.currentSessionId = "sess-1";
    const editMessageSpy = vi.spyOn(chatStore, "editMessage");

    const msg = makeMessage(); // no seq — queued placeholder shape
    const { handleSave, editError } = useMessageEditing(() => msg, () => false);

    await handleSave("改后的内容");

    expect(editError.value).toBe("消息缺少 seq,无法编辑");
    expect(editMessageSpy).not.toHaveBeenCalled();
  });

  it("handleResend: seq 缺失(排队占位)仍拦下并 toast", async () => {
    const chatStore = useChatStore();
    chatStore.currentSessionId = "sess-1";
    const resendSpy = vi.spyOn(chatStore, "resendMessage");
    const projectsStore = useProjectsStore();
    const toastSpy = vi.spyOn(projectsStore, "showToast");

    const msg = makeMessage();
    const { handleResend } = useMessageEditing(() => msg, () => false);

    await handleResend();

    expect(resendSpy).not.toHaveBeenCalled();
    expect(toastSpy).toHaveBeenCalledWith("重发失败: 消息缺少 seq", "error");
  });
});
