// Tests for `transcriptPairing.ts` — B6 PR3 redesign (2026-06-21).
//
// Covers the four key behaviors of the pairing layer:
//   1. Normal pairing: call + result → merged `paired` card.
//   2. Pending call without result: stays as `pending_call` until
//      the 30s timeout, then falls back to `standalone`.
//   3. Orphan tool_result (no preceding call): standalone card.
//   4. chat_event / permission_ask: always standalone (not pairable).
//
// Plus the "call within 30s with no result still pending" path
// (stays `pending_call`, not yet timed out) and the missing-`tool_use_id`
// defensive fallback (a pre-redesign row with no id lands as
// standalone — we never drop entries).

import { describe, it, expect } from "vitest";

import {
  pairTranscript,
  PENDING_TIMEOUT_MS,
  isErrorResult,
  type TranscriptEntry,
} from "./transcriptPairing";

// ---------------------------------------------------------------------
// Fixture helpers
// ---------------------------------------------------------------------

function toolCall(id: string, name = "read_file", input: unknown = { path: "/foo" }): TranscriptEntry {
  return {
    kind: "tool_call",
    payload_json: { id, name, input, tool_use_id: id },
  };
}

function toolResult(
  id: string,
  content = "ok",
  isError = false,
  durationMs = 42,
): TranscriptEntry {
  return {
    kind: "tool_result",
    payload_json: {
      tool_use_id: id,
      content,
      is_error: isError,
      duration_ms: durationMs,
    },
  };
}

function chatEvent(text: string): TranscriptEntry {
  return { kind: "chat_event", payload_json: { text, kind: "delta" } };
}

function permissionAsk(toolName: string): TranscriptEntry {
  return {
    kind: "permission_ask",
    payload_json: { toolName, risk: "high" },
  };
}

// ---------------------------------------------------------------------
// 1. Normal pairing
// ---------------------------------------------------------------------

describe("pairTranscript — normal pairing", () => {
  it("a tool_call followed by its tool_result yields a single paired card", () => {
    const entries: TranscriptEntry[] = [
      toolCall("toolu_1", "read_file", { path: "/x" }),
      toolResult("toolu_1", "contents", false, 123),
    ];
    const out = pairTranscript(entries, 1_000_000, new Map());

    expect(out).toHaveLength(1);
    expect(out[0].kind).toBe("paired");
    if (out[0].kind !== "paired") throw new Error("expected paired");
    expect(out[0].tool_use_id).toBe("toolu_1");
    expect(out[0].call.payload_json.name).toBe("read_file");
    expect(out[0].result.payload_json.content).toBe("contents");
    expect(out[0].result.payload_json.duration_ms).toBe(123);
  });

  it("preserves the position of the result (the call's slot is absorbed into the result's card)", () => {
    // Sequence: chat_event, tool_call, tool_result, chat_event.
    // Expected position: 0 = chat_event (standalone), 1 = paired
    // (call+result merged), 2 = chat_event (standalone). 3 cards
    // total, NOT 4 — the call is absorbed.
    const entries: TranscriptEntry[] = [
      chatEvent("a"),
      toolCall("toolu_1"),
      toolResult("toolu_1"),
      chatEvent("b"),
    ];
    const out = pairTranscript(entries, 0, new Map());

    expect(out).toHaveLength(3);
    expect(out[0].kind).toBe("standalone");
    expect(out[1].kind).toBe("paired");
    expect(out[2].kind).toBe("standalone");
  });

  it("handles two consecutive pairs independently", () => {
    const entries: TranscriptEntry[] = [
      toolCall("toolu_a"),
      toolResult("toolu_a"),
      toolCall("toolu_b"),
      toolResult("toolu_b"),
    ];
    const out = pairTranscript(entries, 0, new Map());

    expect(out).toHaveLength(2);
    expect(out[0].kind).toBe("paired");
    expect(out[1].kind).toBe("paired");
    if (out[0].kind === "paired" && out[1].kind === "paired") {
      expect(out[0].tool_use_id).toBe("toolu_a");
      expect(out[1].tool_use_id).toBe("toolu_b");
    }
  });
});

// ---------------------------------------------------------------------
// 2. Pending call timeout
// ---------------------------------------------------------------------

describe("pairTranscript — pending call timeout", () => {
  it("a tool_call with no result within 30s stays as pending_call", () => {
    const entries: TranscriptEntry[] = [toolCall("toolu_orphan")];
    const out = pairTranscript(entries, 1_000_000, new Map());

    expect(out).toHaveLength(1);
    expect(out[0].kind).toBe("pending_call");
    if (out[0].kind !== "pending_call") throw new Error("expected pending_call");
    expect(out[0].tool_use_id).toBe("toolu_orphan");
  });

  it("a tool_call with no result past 30s falls back to standalone", () => {
    // The drawer's `pendingFirstSeenAt` map is mutated across
    // invocations so the call's `received_at` persists. We
    // simulate this by calling the function twice: first to
    // record the first-seen timestamp, then with an advanced
    // `now` to test the timeout flush.
    const entries: TranscriptEntry[] = [toolCall("toolu_orphan")];
    const firstSeen = new Map<string, number>();
    pairTranscript(entries, 1_000_000, firstSeen);
    // Re-invoke with `now` past the 30s window — the
    // first-seen map persists the original timestamp.
    const out = pairTranscript(entries, 1_000_000 + PENDING_TIMEOUT_MS + 1, firstSeen);

    expect(out).toHaveLength(1);
    expect(out[0].kind).toBe("standalone");
    if (out[0].kind !== "standalone") throw new Error("expected standalone");
    // The standalone entry carries the original tool_call so the
    // drawer can render a "未完成" card with the tool name etc.
    expect(out[0].entry.kind).toBe("tool_call");
    expect(out[0].entry.payload_json.tool_use_id).toBe("toolu_orphan");
  });

  it("the boundary is inclusive (exactly 30s past still flushes)", () => {
    const entries: TranscriptEntry[] = [toolCall("toolu_orphan")];
    const firstSeen = new Map<string, number>();
    pairTranscript(entries, 1_000_000, firstSeen);
    // received_at = 1_000_000, now = 1_000_000 + 30_000 (== PENDING_TIMEOUT_MS)
    const out = pairTranscript(entries, 1_000_000 + PENDING_TIMEOUT_MS, firstSeen);
    expect(out[0].kind).toBe("standalone");
  });

  it("a pending call that aged out stays standalone on subsequent invocations (no timer reset bug)", () => {
    // B6 PR3 check-phase fix (2026-06-21): the timeout-flush branch
    // used to call `pendingFirstSeenAt.delete(id)` then the next
    // invocation would `set(id, now)`, effectively resetting the
    // timer. This caused the card to flicker between standalone
    // and pending_call every 30s. After the fix, the entry stays
    // in the map with the original `received_at`, so subsequent
    // invocations continue to return standalone (no flicker).
    const entries: TranscriptEntry[] = [toolCall("toolu_stuck")];
    const firstSeen = new Map<string, number>();
    pairTranscript(entries, 1_000_000, firstSeen);
    // First timeout flush — standalone.
    const out1 = pairTranscript(entries, 1_000_000 + PENDING_TIMEOUT_MS + 100, firstSeen);
    expect(out1[0].kind).toBe("standalone");
    // Second invocation a moment later — should STILL be standalone
    // (no reset back to pending_call).
    const out2 = pairTranscript(entries, 1_000_000 + PENDING_TIMEOUT_MS + 200, firstSeen);
    expect(out2[0].kind).toBe("standalone");
    // A minute later — still standalone (the timer doesn't reset).
    const out3 = pairTranscript(entries, 1_000_000 + PENDING_TIMEOUT_MS + 60_000, firstSeen);
    expect(out3[0].kind).toBe("standalone");
  });

  it("a late-arriving tool_result after the timeout still pairs if it lands first", () => {
    // Sanity: the result-arrival path takes priority over the
    // timeout flush (the result is in the entries list, the
    // pending map is consulted at result time). The timeout
    // flush is the FALLBACK for entries with no result.
    const entries: TranscriptEntry[] = [toolCall("toolu_x"), toolResult("toolu_x")];
    const out = pairTranscript(entries, 1_000_000 + PENDING_TIMEOUT_MS + 5_000, new Map());

    expect(out).toHaveLength(1);
    expect(out[0].kind).toBe("paired");
  });
});

// ---------------------------------------------------------------------
// 3. Orphan tool_result
// ---------------------------------------------------------------------

describe("pairTranscript — orphan entries", () => {
  it("an orphan tool_result (no preceding call) is standalone", () => {
    const entries: TranscriptEntry[] = [toolResult("toolu_ghost", "partial")];
    const out = pairTranscript(entries, 0, new Map());

    expect(out).toHaveLength(1);
    expect(out[0].kind).toBe("standalone");
    if (out[0].kind !== "standalone") throw new Error("expected standalone");
    expect(out[0].entry.kind).toBe("tool_result");
  });

  it("a tool_call missing tool_use_id is standalone (defensive — pre-redesign row)", () => {
    // Pre-redesign row shape: the backend didn't inject
    // `tool_use_id`. We don't drop the entry — we surface it
    // standalone so the user still sees the call.
    const entries: TranscriptEntry[] = [
      { kind: "tool_call", payload_json: { name: "read_file", input: {} } },
    ];
    const out = pairTranscript(entries, 0, new Map());

    expect(out).toHaveLength(1);
    expect(out[0].kind).toBe("standalone");
  });

  it("a tool_result missing tool_use_id is standalone (defensive — pre-redesign row)", () => {
    const entries: TranscriptEntry[] = [
      { kind: "tool_result", payload_json: { content: "ok", is_error: false } },
    ];
    const out = pairTranscript(entries, 0, new Map());

    expect(out).toHaveLength(1);
    expect(out[0].kind).toBe("standalone");
  });
});

// ---------------------------------------------------------------------
// 4. chat_event / permission_ask routing
// ---------------------------------------------------------------------

describe("pairTranscript — non-pairable kinds", () => {
  it("chat_event entries are always standalone", () => {
    const entries: TranscriptEntry[] = [chatEvent("hello"), chatEvent("world")];
    const out = pairTranscript(entries, 0, new Map());

    expect(out).toHaveLength(2);
    expect(out.every((e) => e.kind === "standalone")).toBe(true);
  });

  it("permission_ask entries are always standalone", () => {
    const entries: TranscriptEntry[] = [permissionAsk("shell")];
    const out = pairTranscript(entries, 0, new Map());

    expect(out).toHaveLength(1);
    expect(out[0].kind).toBe("standalone");
  });

  it("a typical worker transcript (mixed kinds) routes correctly", () => {
    // Realistic sequence: chat_event → tool_call → tool_result
    // → chat_event → permission_ask → tool_call (orphan, ages out)
    const entries: TranscriptEntry[] = [
      chatEvent("start"),
      toolCall("toolu_1"),
      toolResult("toolu_1"),
      chatEvent("delta"),
      permissionAsk("shell"),
      toolCall("toolu_2"),
    ];
    const out = pairTranscript(entries, 0, new Map());

    expect(out).toHaveLength(5);
    expect(out[0].kind).toBe("standalone"); // chat_event start
    expect(out[1].kind).toBe("paired"); // toolu_1 merged
    expect(out[2].kind).toBe("standalone"); // chat_event delta
    expect(out[3].kind).toBe("standalone"); // permission_ask
    expect(out[4].kind).toBe("pending_call"); // toolu_2 still pending
  });
});

// ---------------------------------------------------------------------
// 5. isErrorResult helper
// ---------------------------------------------------------------------

describe("isErrorResult", () => {
  it("returns true for a tool_result with is_error=true", () => {
    expect(isErrorResult(toolResult("x", "boom", true))).toBe(true);
  });

  it("returns false for a tool_result with is_error=false", () => {
    expect(isErrorResult(toolResult("x", "ok", false))).toBe(false);
  });

  it("returns false for a tool_result with missing is_error (defensive)", () => {
    const e: TranscriptEntry = {
      kind: "tool_result",
      payload_json: { content: "ok", tool_use_id: "x", duration_ms: 5 },
    };
    expect(isErrorResult(e)).toBe(false);
  });

  it("returns false for non-tool_result entries", () => {
    expect(isErrorResult(toolCall("x"))).toBe(false);
    expect(isErrorResult(chatEvent("hi"))).toBe(false);
    expect(isErrorResult(permissionAsk("shell"))).toBe(false);
  });
});
