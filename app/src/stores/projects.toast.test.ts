// Tests for `useProjectsStore.showToast` sessionId extension
// (2026-07-08 `cross-session-pending-indicator`, C档基础).
//
// `showToast` gained a 4th param `opts?: { sessionId?: string }` so
// cross-session pending-interaction toasts can carry their target
// session id (AppShell's click handler same-project-jumps on it).
// Existing project-operation toasts pass no 4th param, so their
// `toast.sessionId` must stay undefined (backward compat).

import { describe, it, expect, beforeEach } from "vitest";
import { setActivePinia, createPinia } from "pinia";
import { useProjectsStore } from "./projects";

describe("useProjectsStore — showToast sessionId (cross-session-pending)", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
  });

  it("第四参 opts.sessionId 写入 toast", () => {
    const s = useProjectsStore();
    s.showToast("msg", "info", 1000, { sessionId: "s9" });
    expect(s.toast?.sessionId).toBe("s9");
    expect(s.toast?.message).toBe("msg");
    expect(s.toast?.kind).toBe("info");
  });

  it("不传 opts → sessionId undefined(向后兼容,现有调用)", () => {
    const s = useProjectsStore();
    s.showToast("msg", "info");
    expect(s.toast?.sessionId).toBeUndefined();
    expect(s.toast?.message).toBe("msg");
  });

  it("传 durationMs 不传 opts → sessionId 仍 undefined", () => {
    const s = useProjectsStore();
    s.showToast("msg", "warn", 5000);
    expect(s.toast?.sessionId).toBeUndefined();
    expect(s.toast?.kind).toBe("warn");
  });
});
