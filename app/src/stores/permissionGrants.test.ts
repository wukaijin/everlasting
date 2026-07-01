// Tests for `usePermissionGrantsStore` — the permission-grant
// management UI store (task 07-01-permission-grant-list-ui).
//
// Coverage targets:
//   1. `loadForSession` invokes `list_session_tool_permissions` with
//      `{ sessionId }` and fills `grants`.
//   2. `loadForSession` IPC failure sets `error` and keeps the prior
//      `grants` (defensive — the modal renders stale state, not a
//      cleared list).
//   3. `revoke` invokes `revoke_tool_permission` with the full PK
//      four-tuple, including `matchValue: null` for the tool kind
//      (design D2 — the NULL must reach the backend as JSON null,
//      not be dropped).
//   4. `revoke` removes ONLY the matching PK row locally — sibling
//      grants on the same tool under a different match_value survive.
//   5. `revoke` IPC failure sets `error` and leaves the row in place.
//   6. `refresh` re-fetches the last-loaded session.

import { describe, it, expect, beforeEach, vi } from "vitest";
import { setActivePinia, createPinia } from "pinia";

const invokeMock = vi.fn();

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

import { usePermissionGrantsStore, type PermissionGrantRow } from "./permissionGrants";

const toolGrant = (sid = "sess-1"): PermissionGrantRow => ({
  sessionId: sid,
  toolName: "web_fetch",
  matchKind: "tool",
  matchValue: null,
  grantedAt: "2026-07-01 10:00:00",
});

const pathGrant = (glob: string, sid = "sess-1"): PermissionGrantRow => ({
  sessionId: sid,
  toolName: "read_file",
  matchKind: "path",
  matchValue: glob,
  grantedAt: "2026-07-01 10:00:01",
});

const prefixGrant = (tok: string, sid = "sess-1"): PermissionGrantRow => ({
  sessionId: sid,
  toolName: "shell",
  matchKind: "prefix",
  matchValue: tok,
  grantedAt: "2026-07-01 10:00:02",
});

describe("usePermissionGrantsStore", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    invokeMock.mockReset();
  });

  it("loadForSession invokes list_session_tool_permissions + fills grants", async () => {
    invokeMock.mockResolvedValue([toolGrant(), pathGrant("src/*"), prefixGrant("git")]);
    const store = usePermissionGrantsStore();
    await store.loadForSession("sess-1");
    expect(invokeMock).toHaveBeenCalledWith("list_session_tool_permissions", {
      sessionId: "sess-1",
    });
    expect(store.grants).toHaveLength(3);
    expect(store.loading).toBe(false);
    expect(store.error).toBeNull();
    expect(store.lastSessionId).toBe("sess-1");
  });

  it("loadForSession on IPC error sets error and keeps prior grants", async () => {
    invokeMock.mockResolvedValue([toolGrant()]);
    const store = usePermissionGrantsStore();
    await store.loadForSession("sess-1");
    expect(store.grants).toHaveLength(1);
    invokeMock.mockRejectedValue(new Error("db down"));
    await store.loadForSession("sess-1");
    expect(store.error).toBe("db down");
    expect(store.grants).toHaveLength(1);
  });

  it("revoke sends the full PK incl. matchValue:null for tool kind (design D2)", async () => {
    invokeMock.mockResolvedValue([toolGrant()]);
    const store = usePermissionGrantsStore();
    await store.loadForSession("sess-1");
    invokeMock.mockResolvedValue(undefined);
    await store.revoke(toolGrant());
    expect(invokeMock).toHaveBeenCalledWith("revoke_tool_permission", {
      sessionId: "sess-1",
      toolName: "web_fetch",
      matchKind: "tool",
      // null — NOT undefined. The key must be present as JSON null
      // so serde reads Option<String> = None.
      matchValue: null,
    });
  });

  it("revoke removes ONLY the matching PK row (null matchValue / tool kind)", async () => {
    invokeMock.mockResolvedValue([toolGrant(), pathGrant("src/*"), pathGrant("docs/*")]);
    const store = usePermissionGrantsStore();
    await store.loadForSession("sess-1");
    invokeMock.mockResolvedValue(undefined);
    await store.revoke(toolGrant());
    expect(store.grants).toHaveLength(2);
    expect(store.grants.find((g) => g.toolName === "web_fetch")).toBeUndefined();
    // The two path rows survive.
    expect(store.grants.filter((g) => g.toolName === "read_file")).toHaveLength(2);
  });

  it("revoke does NOT touch sibling grants on the same tool (per-PK)", async () => {
    invokeMock.mockResolvedValue([pathGrant("src/*"), pathGrant("docs/*")]);
    const store = usePermissionGrantsStore();
    await store.loadForSession("sess-1");
    invokeMock.mockResolvedValue(undefined);
    await store.revoke(pathGrant("src/*"));
    expect(invokeMock).toHaveBeenCalledWith("revoke_tool_permission", {
      sessionId: "sess-1",
      toolName: "read_file",
      matchKind: "path",
      matchValue: "src/*",
    });
    expect(store.grants).toHaveLength(1);
    expect(store.grants[0].matchValue).toBe("docs/*");
  });

  it("revoke on IPC failure sets error and leaves the row in place", async () => {
    invokeMock.mockResolvedValue([toolGrant()]);
    const store = usePermissionGrantsStore();
    await store.loadForSession("sess-1");
    invokeMock.mockRejectedValue(new Error("delete failed"));
    await store.revoke(toolGrant());
    expect(store.error).toBe("delete failed");
    expect(store.grants).toHaveLength(1);
  });

  it("refresh re-fetches the last-loaded session", async () => {
    invokeMock.mockResolvedValue([toolGrant()]);
    const store = usePermissionGrantsStore();
    await store.loadForSession("sess-1");
    invokeMock.mockResolvedValue([]);
    await store.refresh();
    expect(store.grants).toHaveLength(0);
    expect(invokeMock).toHaveBeenLastCalledWith("list_session_tool_permissions", {
      sessionId: "sess-1",
    });
  });
});
