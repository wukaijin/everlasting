// Tests for the B12 Checklist store (PR2 frontend, 2026-06-19).
//
// Covers:
//   1. `parseAndCoerceItems` — defensive parsing + status coercion.
//   2. `coerceAtMostOneInProgress` — mirrors PR1's Rust pure fn
//      (`coerce_at_most_one_in_progress`): keep LAST `in_progress`,
//      demote earlier ones to `pending`.
//   3. `handleToolCall` — live `tool:call` event → coerced state.
//   4. `rehydrateFromMessages` + `findLastCommittedChecklist`:
//      - finds the LAST `update_checklist` tool_use with a
//        committed (is_error === false) tool_result
//      - SKIPS candidates whose tool_result is_error === true
//        (the cancelled-update case — the trellis-check constraint
//        on PR1)
//      - SKIPS candidates with NO tool_result at all
//        (uncommitted)
//      - returns null when no committed candidate exists
//   5. Per-session isolation + clearForNewRun / clearSession.

import { describe, it, expect, beforeEach } from "vitest";
import { setActivePinia, createPinia } from "pinia";
import {
  useChecklistStore,
  coerceAtMostOneInProgress,
  parseAndCoerceItems,
  findLastCommittedChecklist,
  CHECKLIST_TOOL_NAME,
  type ChecklistItem,
  type ChecklistRehydrateMessage,
} from "./checklist";

const SID = "test-session";
const SID_OTHER = "other-session";

function item(content: string, status: ChecklistItem["status"]): ChecklistItem {
  return { content, status };
}

describe("coerceAtMostOneInProgress", () => {
  it("keeps a single in_progress untouched", () => {
    const input = [
      item("a", "done"),
      item("b", "in_progress"),
      item("c", "pending"),
    ];
    const out = coerceAtMostOneInProgress(input);
    expect(out.map((i) => i.status)).toEqual([
      "done",
      "in_progress",
      "pending",
    ]);
  });

  it("keeps the LAST in_progress and demotes earlier ones", () => {
    const input = [
      item("first", "in_progress"),
      item("middle", "in_progress"),
      item("last", "in_progress"),
    ];
    const out = coerceAtMostOneInProgress(input);
    expect(out[0].status).toBe("pending");
    expect(out[1].status).toBe("pending");
    expect(out[2].status).toBe("in_progress");
    // Contents preserved.
    expect(out[0].content).toBe("first");
    expect(out[2].content).toBe("last");
  });

  it("does not mutate the input", () => {
    const input = [item("a", "in_progress"), item("b", "in_progress")];
    const snapshot = input.map((i) => ({ ...i }));
    coerceAtMostOneInProgress(input);
    expect(input).toEqual(snapshot);
  });

  it("handles empty input", () => {
    expect(coerceAtMostOneInProgress([])).toEqual([]);
  });

  it("leaves all-pending/all-done untouched", () => {
    const input = [item("a", "pending"), item("b", "done")];
    const out = coerceAtMostOneInProgress(input);
    expect(out.map((i) => i.status)).toEqual(["pending", "done"]);
  });
});

describe("parseAndCoerceItems", () => {
  it("parses a well-formed items array", () => {
    const raw = [
      { content: "a", status: "done" },
      { content: "b", status: "in_progress" },
      { content: "c", status: "pending" },
    ];
    const out = parseAndCoerceItems(raw);
    expect(out).toHaveLength(3);
    expect(out[1].status).toBe("in_progress");
  });

  it("returns empty for non-array input", () => {
    expect(parseAndCoerceItems(undefined)).toEqual([]);
    expect(parseAndCoerceItems(null)).toEqual([]);
    expect(parseAndCoerceItems({})).toEqual([]);
    expect(parseAndCoerceItems("foo")).toEqual([]);
  });

  it("skips entries missing `content`", () => {
    const raw = [
      { content: "ok", status: "pending" },
      { status: "pending" }, // missing content
      { content: 42, status: "pending" }, // non-string content
      { content: "also ok", status: "done" },
    ];
    const out = parseAndCoerceItems(raw);
    expect(out).toHaveLength(2);
    expect(out[0].content).toBe("ok");
    expect(out[1].content).toBe("also ok");
  });

  it("coerces unknown status to pending", () => {
    const raw = [
      { content: "weird", status: "blocked" },
      { content: "missing-status" },
    ];
    const out = parseAndCoerceItems(raw);
    expect(out).toHaveLength(2);
    expect(out.every((i) => i.status === "pending")).toBe(true);
  });

  it("runs the at-most-one-in_progress coerce", () => {
    const raw = [
      { content: "first", status: "in_progress" },
      { content: "last", status: "in_progress" },
    ];
    const out = parseAndCoerceItems(raw);
    expect(out).toHaveLength(2);
    const inProgress = out.filter((i) => i.status === "in_progress");
    expect(inProgress).toHaveLength(1);
    expect(inProgress[0].content).toBe("last");
  });
});

describe("findLastCommittedChecklist", () => {
  function asstWithUse(
    id: string,
    items: unknown,
  ): ChecklistRehydrateMessage {
    return {
      role: "assistant",
      toolCalls: [
        {
          id,
          name: CHECKLIST_TOOL_NAME,
          input: { items },
        },
      ],
    };
  }

  function asstWithUseAndResult(
    id: string,
    items: unknown,
    isError: boolean,
  ): ChecklistRehydrateMessage {
    return {
      role: "assistant",
      toolCalls: [
        {
          id,
          name: CHECKLIST_TOOL_NAME,
          input: { items },
        },
      ],
      // The rehydrate step copies the following user message's
      // tool_result blocks onto the preceding assistant for the
      // UI's "done / running" lookup. We model that here.
      toolResults: [{ toolUseId: id, isError }],
    };
  }

  it("returns null when no update_checklist tool_use exists", () => {
    const msgs: ChecklistRehydrateMessage[] = [
      { role: "user" },
      {
        role: "assistant",
        toolCalls: [
          { id: "tu-shell", name: "shell", input: { command: "ls" } },
        ],
        toolResults: [{ toolUseId: "tu-shell", isError: false }],
      },
    ];
    expect(findLastCommittedChecklist(msgs)).toBeNull();
  });

  it("returns the items when a single committed update exists", () => {
    const msgs: ChecklistRehydrateMessage[] = [
      asstWithUseAndResult(
        "tu-1",
        [{ content: "step 1", status: "done" }],
        false,
      ),
    ];
    const out = findLastCommittedChecklist(msgs);
    expect(out).not.toBeNull();
    expect(out).toHaveLength(1);
    expect(out![0].content).toBe("step 1");
  });

  it("returns the LAST committed update when multiple exist", () => {
    const msgs: ChecklistRehydrateMessage[] = [
      asstWithUseAndResult(
        "tu-1",
        [{ content: "old", status: "done" }],
        false,
      ),
      asstWithUseAndResult(
        "tu-2",
        [
          { content: "new-a", status: "pending" },
          { content: "new-b", status: "in_progress" },
        ],
        false,
      ),
    ];
    const out = findLastCommittedChecklist(msgs);
    expect(out).not.toBeNull();
    expect(out!.map((i) => i.content)).toEqual(["new-a", "new-b"]);
  });

  it("SKIPS an update whose tool_result is_error === true (cancelled)", () => {
    // Two updates: first committed, second cancelled. The scan
    // must return the FIRST (the only committed one), not the
    // cancelled one — even though the cancelled one is later.
    const msgs: ChecklistRehydrateMessage[] = [
      asstWithUseAndResult(
        "tu-committed",
        [{ content: "committed", status: "done" }],
        false,
      ),
      asstWithUseAndResult(
        "tu-cancelled",
        [{ content: "cancelled-state", status: "in_progress" }],
        true, // cancelled → synthetic is_error: true tool_result
      ),
    ];
    const out = findLastCommittedChecklist(msgs);
    expect(out).not.toBeNull();
    expect(out!).toHaveLength(1);
    expect(out![0].content).toBe("committed");
  });

  it("returns null when the ONLY update was cancelled", () => {
    const msgs: ChecklistRehydrateMessage[] = [
      asstWithUseAndResult(
        "tu-cancelled",
        [{ content: "interrupted", status: "in_progress" }],
        true,
      ),
    ];
    expect(findLastCommittedChecklist(msgs)).toBeNull();
  });

  it("SKIPS an update with no tool_result at all (uncommitted)", () => {
    // E.g. a session reloaded mid-stream where the tool_result
    // hasn't landed in the DB yet.
    const msgs: ChecklistRehydrateMessage[] = [
      asstWithUse("tu-no-result", [{ content: "x", status: "pending" }]),
    ];
    expect(findLastCommittedChecklist(msgs)).toBeNull();
  });

  it("coerces items from the committed tool_use", () => {
    // The model passed two in_progress; the Rust execute coerces
    // to one before persisting the tool_result. The rehydrate
    // reads the model's RAW input (pre-coerce), so the frontend
    // re-coerces to match the persisted state.
    const msgs: ChecklistRehydrateMessage[] = [
      asstWithUseAndResult(
        "tu-1",
        [
          { content: "first", status: "in_progress" },
          { content: "last", status: "in_progress" },
        ],
        false,
      ),
    ];
    const out = findLastCommittedChecklist(msgs);
    expect(out).not.toBeNull();
    const inProgress = out!.filter((i) => i.status === "in_progress");
    expect(inProgress).toHaveLength(1);
    expect(inProgress[0].content).toBe("last");
  });
});

describe("useChecklistStore", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
  });

  it("getChecklist returns null when no checklist exists", () => {
    const store = useChecklistStore();
    expect(store.getChecklist(SID)).toBeNull();
  });

  it("handleToolCall parses + coerces input into the session's checklist", () => {
    const store = useChecklistStore();
    store.handleToolCall(SID, CHECKLIST_TOOL_NAME, {
      items: [
        { content: "a", status: "done" },
        { content: "b", status: "in_progress" },
      ],
    });
    const items = store.getChecklist(SID);
    expect(items).not.toBeNull();
    expect(items).toHaveLength(2);
    expect(items![1].status).toBe("in_progress");
  });

  it("handleToolCall ignores other tool names", () => {
    const store = useChecklistStore();
    store.handleToolCall(SID, "shell", {
      items: [{ content: "x", status: "pending" }],
    });
    expect(store.getChecklist(SID)).toBeNull();
  });

  it("handleToolCall coerces at-most-one in_progress client-side", () => {
    const store = useChecklistStore();
    store.handleToolCall(SID, CHECKLIST_TOOL_NAME, {
      items: [
        { content: "first", status: "in_progress" },
        { content: "last", status: "in_progress" },
      ],
    });
    const items = store.getChecklist(SID)!;
    const inProgress = items.filter((i) => i.status === "in_progress");
    expect(inProgress).toHaveLength(1);
    expect(inProgress[0].content).toBe("last");
  });

  it("handleToolCall with empty items still sets the session's checklist (empty array)", () => {
    const store = useChecklistStore();
    store.handleToolCall(SID, CHECKLIST_TOOL_NAME, { items: [] });
    // Empty array (model cleared) vs. null (no update seen).
    // The card renders an empty placeholder for [], hides for null.
    expect(store.getChecklist(SID)).toEqual([]);
  });

  it("per-session isolation: each session has its own checklist", () => {
    const store = useChecklistStore();
    store.handleToolCall(SID, CHECKLIST_TOOL_NAME, {
      items: [{ content: "a", status: "pending" }],
    });
    store.handleToolCall(SID_OTHER, CHECKLIST_TOOL_NAME, {
      items: [{ content: "b", status: "done" }],
    });
    expect(store.getChecklist(SID)![0].content).toBe("a");
    expect(store.getChecklist(SID_OTHER)![0].content).toBe("b");
  });

  it("clearForNewRun drops the session's checklist", () => {
    const store = useChecklistStore();
    store.handleToolCall(SID, CHECKLIST_TOOL_NAME, {
      items: [{ content: "a", status: "pending" }],
    });
    expect(store.getChecklist(SID)).not.toBeNull();
    store.clearForNewRun(SID);
    expect(store.getChecklist(SID)).toBeNull();
  });

  it("clearForNewRun only affects the named session", () => {
    const store = useChecklistStore();
    store.handleToolCall(SID, CHECKLIST_TOOL_NAME, {
      items: [{ content: "a", status: "pending" }],
    });
    store.handleToolCall(SID_OTHER, CHECKLIST_TOOL_NAME, {
      items: [{ content: "b", status: "pending" }],
    });
    store.clearForNewRun(SID);
    expect(store.getChecklist(SID)).toBeNull();
    expect(store.getChecklist(SID_OTHER)).not.toBeNull();
  });

  it("clearSession drops the session's checklist", () => {
    const store = useChecklistStore();
    store.handleToolCall(SID, CHECKLIST_TOOL_NAME, {
      items: [{ content: "a", status: "pending" }],
    });
    store.clearSession(SID);
    expect(store.getChecklist(SID)).toBeNull();
  });

  describe("rehydrateFromMessages", () => {
    it("sets the checklist from the last committed update", () => {
      const store = useChecklistStore();
      const msgs: ChecklistRehydrateMessage[] = [
        {
          role: "assistant",
          toolCalls: [
            {
              id: "tu-1",
              name: CHECKLIST_TOOL_NAME,
              input: {
                items: [{ content: "from-history", status: "done" }],
              },
            },
          ],
          toolResults: [{ toolUseId: "tu-1", isError: false }],
        },
      ];
      store.rehydrateFromMessages(SID, msgs);
      const items = store.getChecklist(SID);
      expect(items).not.toBeNull();
      expect(items![0].content).toBe("from-history");
    });

    it("drops prior live state when no committed checklist exists", () => {
      const store = useChecklistStore();
      // Seed with a live call.
      store.handleToolCall(SID, CHECKLIST_TOOL_NAME, {
        items: [{ content: "live", status: "in_progress" }],
      });
      expect(store.getChecklist(SID)).not.toBeNull();
      // Reload with no committed history → live state cleared.
      store.rehydrateFromMessages(SID, []);
      expect(store.getChecklist(SID)).toBeNull();
    });

    it("skips cancelled updates (is_error === true)", () => {
      const store = useChecklistStore();
      const msgs: ChecklistRehydrateMessage[] = [
        {
          role: "assistant",
          toolCalls: [
            {
              id: "tu-cancelled",
              name: CHECKLIST_TOOL_NAME,
              input: {
                items: [{ content: "interrupted", status: "in_progress" }],
              },
            },
          ],
          toolResults: [{ toolUseId: "tu-cancelled", isError: true }],
        },
      ];
      store.rehydrateFromMessages(SID, msgs);
      // The only candidate was cancelled → no committed checklist.
      expect(store.getChecklist(SID)).toBeNull();
    });

    it("picks the last committed when later updates were cancelled", () => {
      const store = useChecklistStore();
      const msgs: ChecklistRehydrateMessage[] = [
        {
          role: "assistant",
          toolCalls: [
            {
              id: "tu-committed",
              name: CHECKLIST_TOOL_NAME,
              input: {
                items: [{ content: "committed", status: "done" }],
              },
            },
          ],
          toolResults: [{ toolUseId: "tu-committed", isError: false }],
        },
        {
          role: "assistant",
          toolCalls: [
            {
              id: "tu-cancelled",
              name: CHECKLIST_TOOL_NAME,
              input: {
                items: [{ content: "interrupted", status: "in_progress" }],
              },
            },
          ],
          toolResults: [{ toolUseId: "tu-cancelled", isError: true }],
        },
      ];
      store.rehydrateFromMessages(SID, msgs);
      const items = store.getChecklist(SID);
      expect(items).not.toBeNull();
      expect(items![0].content).toBe("committed");
    });

    it("re-coerces items from the raw input", () => {
      const store = useChecklistStore();
      const msgs: ChecklistRehydrateMessage[] = [
        {
          role: "assistant",
          toolCalls: [
            {
              id: "tu-1",
              name: CHECKLIST_TOOL_NAME,
              input: {
                items: [
                  { content: "first", status: "in_progress" },
                  { content: "last", status: "in_progress" },
                ],
              },
            },
          ],
          toolResults: [{ toolUseId: "tu-1", isError: false }],
        },
      ];
      store.rehydrateFromMessages(SID, msgs);
      const items = store.getChecklist(SID)!;
      const inProgress = items.filter((i) => i.status === "in_progress");
      expect(inProgress).toHaveLength(1);
      expect(inProgress[0].content).toBe("last");
    });
  });
});
