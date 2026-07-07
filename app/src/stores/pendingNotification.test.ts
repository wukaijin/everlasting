// Tests for `buildPendingNotification` — the pure decision + message
// builder extracted from streamController's `maybeNotifyPending`
// (2026-07-08 `cross-session-pending-indicator`, C档 toast gate).
//
// Locks the Q3 constraint: a pending interaction on the CURRENT
// session is NOT toasted (the inline card is already visible), so
// the function must return null in that case. Everything else
// (other session in-project, or cross-project) returns a
// `{ message, sessionId }` payload for `showToast`.

import { describe, it, expect } from "vitest";
import { buildPendingNotification } from "./streamController";

describe("buildPendingNotification", () => {
  const sessions = [
    { id: "s1", title: "会话一" },
    { id: "s2", title: "会话二" },
  ];

  it("pending on current session → null (Q3: 不打扰当前会话)", () => {
    expect(
      buildPendingNotification("s1", "mode_change", "s1", sessions, "edit"),
    ).toBeNull();
    expect(
      buildPendingNotification("s1", "question", "s1", sessions),
    ).toBeNull();
  });

  it("mode_change on another in-project session → 文案含标题 + targetMode + sessionId", () => {
    const n = buildPendingNotification("s2", "mode_change", "s1", sessions, "yolo");
    expect(n).not.toBeNull();
    expect(n!.sessionId).toBe("s2");
    expect(n!.message).toContain("会话二");
    expect(n!.message).toContain("yolo");
    expect(n!.message).toContain("切换到");
  });

  it("question on another in-project session → 问题文案含标题", () => {
    const n = buildPendingNotification("s2", "question", "s1", sessions);
    expect(n).not.toBeNull();
    expect(n!.sessionId).toBe("s2");
    expect(n!.message).toContain("会话二");
    expect(n!.message).toContain("有问题等你回答");
  });

  it("跨 project (session 不在 sessions) → 「另一项目的会话」降级文案", () => {
    const n = buildPendingNotification("sX", "mode_change", "s1", sessions, "plan");
    expect(n).not.toBeNull();
    expect(n!.sessionId).toBe("sX");
    expect(n!.message).toContain("另一项目的会话");
    expect(n!.message).toContain("plan");
  });

  it("currentSessionId = null 时,任何 session 都视为非当前 → 触发", () => {
    const n = buildPendingNotification("s2", "question", null, sessions);
    expect(n).not.toBeNull();
    expect(n!.sessionId).toBe("s2");
  });
});
