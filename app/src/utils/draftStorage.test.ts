// Tests for `draftStorage` — BUGLIST CH5-4 (2026-08-29) composer draft
// persistence. Locks the contract:
//   - key scoping: session id wins, then "new conversation by project",
//     null/null → no key (persistence disabled)
//   - save("") REMOVES the key — a sent/cleared draft must not
//     resurrect on the next visit
//   - storage failures (private mode / quota) degrade silently

import { describe, it, expect, vi, afterEach } from "vitest";
import {
  draftStorageKey,
  loadDraft,
  saveDraft,
} from "./draftStorage";

afterEach(() => {
  localStorage.clear();
  vi.restoreAllMocks();
});

describe("draftStorageKey", () => {
  it("session id wins over project", () => {
    expect(draftStorageKey("s1", "p1")).toBe("everlasting:draft:sess:s1");
  });

  it("null session (unsaved new conversation) keys by project", () => {
    expect(draftStorageKey(null, "p1")).toBe("everlasting:draft:new:p1");
  });

  it("null / null → null (persistence disabled)", () => {
    expect(draftStorageKey(null, null)).toBeNull();
  });
});

describe("loadDraft / saveDraft", () => {
  it("roundtrips text under the session key", () => {
    saveDraft("s1", "p1", "未发送的草稿");
    expect(loadDraft("s1", "p1")).toBe("未发送的草稿");
    // Session scope is independent of the project's new-conversation scope.
    expect(loadDraft(null, "p1")).toBe("");
  });

  it("save with empty text removes the key (no ghost drafts)", () => {
    saveDraft("s1", null, "draft");
    saveDraft("s1", null, "");
    expect(loadDraft("s1", null)).toBe("");
    expect(localStorage.getItem("everlasting:draft:sess:s1")).toBeNull();
  });

  it("load with nothing stored returns empty string", () => {
    expect(loadDraft("nope", null)).toBe("");
  });

  it("null / null context is a no-op (never writes)", () => {
    saveDraft(null, null, "x");
    expect(localStorage.length).toBe(0);
    expect(loadDraft(null, null)).toBe("");
  });

  it("storage failures degrade silently (private mode / quota)", () => {
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new DOMException("quota", "QuotaExceededError");
    });
    expect(() => saveDraft("s1", null, "x")).not.toThrow();

    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new Error("unavailable");
    });
    expect(loadDraft("s1", null)).toBe("");
  });
});
