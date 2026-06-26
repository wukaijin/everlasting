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
  pairSections,
  useTranscriptPairing,
  PENDING_TIMEOUT_MS,
  isErrorResult,
  type TranscriptEntry,
  type SectionToolEntry,
} from "./transcriptPairing";
import type { TranscriptSection } from "../stores/subagentRuns.types";

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

// =====================================================================
// B6 redesign PR5 (2026-06-21): pairSections — section-level pairing
// =====================================================================
//
// Covers the new section-based pairing layer the drawer's Tools
// segment consumes. Each test seeds a `TranscriptSection[]`
// (post-accumulator shape) and asserts the paired / pending /
// permission_ask entries come out with the right mapping to the
// canonical `ToolCallInfo` / `ToolResultInfo` types the
// `DrawerToolCallCard` consumes.

function toolCallSection(id: string, name = "read_file", input: unknown = { path: "/foo" }): TranscriptSection {
  return {
    kind: "ToolCall",
    payload_json: { name, input, tool_use_id: id },
  };
}

function toolResultSection(
  id: string,
  content = "ok",
  isError = false,
  durationMs = 42,
): TranscriptSection {
  return {
    kind: "ToolResult",
    payload_json: {
      tool_use_id: id,
      content,
      is_error: isError,
      duration_ms: durationMs,
    },
  };
}

function permissionAskSection(toolName: string, rid?: string): TranscriptSection {
  const payload: Record<string, unknown> = { toolName, risk: "high" };
  if (rid !== undefined) payload.rid = rid;
  return {
    kind: "PermissionAsk",
    payload_json: payload,
  };
}

/** 2026-06-22 (RULE-WorkerAsk-001): helper to build a
 * `PermissionAskResolvedSection` with the canonical
 * `{ rid, outcome }` payload shape. Mirrors what the Rust
 * `SubagentBufferSink::emit_permission_ask_resolved` produces. */
function permissionAskResolvedSection(
  rid: string,
  outcome: "allow" | "deny" | "timeout" | "cancel",
): TranscriptSection {
  return {
    kind: "PermissionAskResolved",
    payload_json: { rid, outcome },
  };
}

function thinkingSection(text: string): TranscriptSection {
  return { kind: "Thinking", text, chars: text.length, closed: true };
}

function textSection(text: string): TranscriptSection {
  return { kind: "Text", text, chars: text.length };
}

describe("pairSections — section-level pairing", () => {
  it("collapses a ToolCall + ToolResult pair into a single paired entry with canonical ToolCallInfo/ToolResultInfo", () => {
    const sections: TranscriptSection[] = [
      toolCallSection("tu_1", "grep", { pattern: "foo" }),
      toolResultSection("tu_1", "matches: 3", false, 123),
    ];
    const out = pairSections(sections, 1_000_000, new Map());

    expect(out).toHaveLength(1);
    expect(out[0].kind).toBe("paired");
    if (out[0].kind !== "paired") throw new Error("expected paired");
    // Snake_case payload_json → camelCase ToolCallInfo.
    expect(out[0].call.id).toBe("tu_1");
    expect(out[0].call.name).toBe("grep");
    expect(out[0].call.input).toEqual({ pattern: "foo" });
    // Snake_case payload_json → camelCase ToolResultInfo.
    expect(out[0].result.toolUseId).toBe("tu_1");
    expect(out[0].result.content).toBe("matches: 3");
    expect(out[0].result.isError).toBe(false);
    expect(out[0].result.durationMs).toBe(123);
  });

  it("emits a pending_call entry when a ToolCall has no matching result", () => {
    const sections: TranscriptSection[] = [
      toolCallSection("tu_p", "shell", { command: "ls" }),
    ];
    const out = pairSections(sections, 1_000_000, new Map());

    expect(out).toHaveLength(1);
    expect(out[0].kind).toBe("pending_call");
    if (out[0].kind !== "pending_call") throw new Error("expected pending_call");
    expect(out[0].call.name).toBe("shell");
    expect(out[0].timedOut).toBe(false);
  });

  it("flips pending_call.timedOut to true once 30s elapse without a result", () => {
    const sections: TranscriptSection[] = [
      toolCallSection("tu_p"),
    ];
    const firstSeen = new Map<string, number>();
    // First call: received_at = 1_000_000.
    let out = pairSections(sections, 1_000_000, firstSeen);
    expect(out[0].kind).toBe("pending_call");
    if (out[0].kind !== "pending_call") throw new Error("expected pending_call");
    expect(out[0].timedOut).toBe(false);

    // Advance the clock past 30s. Same Map carries the first-seen.
    out = pairSections(sections, 1_000_000 + PENDING_TIMEOUT_MS + 1, firstSeen);
    expect(out[0].kind).toBe("pending_call");
    if (out[0].kind !== "pending_call") throw new Error("expected pending_call");
    expect(out[0].timedOut).toBe(true);
  });

  it("passes PermissionAskSection through as SectionPermissionAsk (no outcome when no resolved entry)", () => {
    // 2026-06-22 (RULE-WorkerAsk-001): when no matching
    // `PermissionAskResolved` section exists, the ask passes
    // through unchanged (outcome === undefined). This is the
    // backward-compat path — pre-this-task transcripts have no
    // resolved entries, so historical cards render the neutral
    // ask-context line.
    const sections: TranscriptSection[] = [permissionAskSection("write_file")];
    const out = pairSections(sections, 1_000_000, new Map());

    expect(out).toHaveLength(1);
    expect(out[0].kind).toBe("permission_ask");
    if (out[0].kind !== "permission_ask") throw new Error("expected permission_ask");
    expect(out[0].payload_json.toolName).toBe("write_file");
    expect(out[0].outcome).toBeUndefined();
  });

  it("drops orphan ToolResultSection (no preceding call) silently", () => {
    const sections: TranscriptSection[] = [
      toolResultSection("tu_ghost", "orphan content"),
    ];
    const out = pairSections(sections, 1_000_000, new Map());
    expect(out).toHaveLength(0);
  });

  it("drops ToolCallSection without tool_use_id (defensive against malformed payload)", () => {
    const sections: TranscriptSection[] = [
      { kind: "ToolCall", payload_json: { name: "grep" } },
    ];
    const out = pairSections(sections, 1_000_000, new Map());
    expect(out).toHaveLength(0);
  });

  it("skips Thinking / Text sections (they belong to other segments)", () => {
    const sections: TranscriptSection[] = [
      thinkingSection("thoughts"),
      textSection("reply text"),
      toolCallSection("tu_1"),
      toolResultSection("tu_1"),
    ];
    const out = pairSections(sections, 1_000_000, new Map());
    expect(out).toHaveLength(1);
    expect(out[0].kind).toBe("paired");
  });

  it("coerces missing input to {} so DrawerToolCallCard's empty-input guard works", () => {
    const sections: TranscriptSection[] = [
      { kind: "ToolCall", payload_json: { name: "shell", tool_use_id: "tu_x" } },
    ];
    const out = pairSections(sections, 1_000_000, new Map());
    expect(out[0].kind).toBe("pending_call");
    if (out[0].kind !== "pending_call") throw new Error("expected pending_call");
    expect(out[0].call.input).toEqual({});
  });

  it("coerces missing durationMs to undefined (omit-duration contract for pre-F5 rows)", () => {
    const sections: TranscriptSection[] = [
      toolCallSection("tu_1"),
      // No duration_ms field — pre-redesign row.
      {
        kind: "ToolResult",
        payload_json: { tool_use_id: "tu_1", content: "ok", is_error: false },
      },
    ];
    const out = pairSections(sections, 1_000_000, new Map());
    expect(out[0].kind).toBe("paired");
    if (out[0].kind !== "paired") throw new Error("expected paired");
    expect(out[0].result.durationMs).toBeUndefined();
  });

  it("preserves call ordering across multiple concurrent calls + results", () => {
    const sections: TranscriptSection[] = [
      toolCallSection("tu_1", "read_file"),
      toolCallSection("tu_2", "grep"),
      toolResultSection("tu_1", "a"),
      toolResultSection("tu_2", "b"),
    ];
    const out = pairSections(sections, 1_000_000, new Map());
    expect(out).toHaveLength(2);
    expect(out[0].kind).toBe("paired");
    expect(out[1].kind).toBe("paired");
    if (out[0].kind !== "paired" || out[1].kind !== "paired") {
      throw new Error("expected both paired");
    }
    // Pair order = result arrival order.
    expect(out[0].toolUseId).toBe("tu_1");
    expect(out[1].toolUseId).toBe("tu_2");
  });

  it("returns SectionToolEntry[] type-narrow discriminant (compile-time check)", () => {
    // This is a compile-time guard: if the discriminated union
    // breaks, TS will error here. Runtime is trivial.
    const sections: TranscriptSection[] = [toolCallSection("tu_1")];
    const out: SectionToolEntry[] = pairSections(sections, 1_000_000, new Map());
    expect(out.length).toBeGreaterThan(0);
  });

  // ====================================================================
  // 2026-06-22 (RULE-WorkerAsk-001): permission_ask_resolved pairing
  // ====================================================================
  //
  // The new `PermissionAskResolvedSection` carries `{ rid, outcome }`.
  // `pairSections` pairs it with the matching `PermissionAskSection`
  // by `rid` and surfaces the `outcome` onto the ask card. The
  // resolved section itself is dropped from the output (it is
  // consumed by the pairing layer — the drawer never renders it
  // directly).

  it("surfaces outcome on PermissionAsk when matching resolved section exists (by rid)", () => {
    // The canonical case: ask arrives first, resolve arrives
    // after (the worker's `ask_path` resolves AFTER emitting the
    // ask). `pairSections` should attach the outcome to the ask.
    const sections: TranscriptSection[] = [
      permissionAskSection("write_file", "rid-allow-1"),
      permissionAskResolvedSection("rid-allow-1", "allow"),
    ];
    const out = pairSections(sections, 1_000_000, new Map());

    expect(out).toHaveLength(1);
    expect(out[0].kind).toBe("permission_ask");
    if (out[0].kind !== "permission_ask") throw new Error("expected permission_ask");
    expect(out[0].outcome).toBe("allow");
  });

  it("surfaces each of the four outcomes correctly", () => {
    // One test per outcome wire string (DEBT-locked four-state).
    for (const outcome of ["allow", "deny", "timeout", "cancel"] as const) {
      const sections: TranscriptSection[] = [
        permissionAskSection("shell", `rid-${outcome}`),
        permissionAskResolvedSection(`rid-${outcome}`, outcome),
      ];
      const out = pairSections(sections, 1_000_000, new Map());
      expect(out).toHaveLength(1);
      expect(out[0].kind).toBe("permission_ask");
      if (out[0].kind !== "permission_ask") throw new Error("expected permission_ask");
      expect(out[0].outcome).toBe(outcome);
    }
  });

  it("does NOT surface outcome when no matching resolved section exists (backward compat)", () => {
    // Pre-this-task transcript (no resolved entries) → ask card
    // renders with outcome === undefined → neutral ask-context
    // line (no outcome badge). Critical for backward compat —
    // old transcripts must not crash or render a misleading badge.
    const sections: TranscriptSection[] = [
      permissionAskSection("write_file", "rid-old-1"),
    ];
    const out = pairSections(sections, 1_000_000, new Map());
    expect(out).toHaveLength(1);
    expect(out[0].kind).toBe("permission_ask");
    if (out[0].kind !== "permission_ask") throw new Error("expected permission_ask");
    expect(out[0].outcome).toBeUndefined();
  });

  it("does NOT surface outcome when rid does not match (defensive)", () => {
    // A resolved entry with a rid that does not match any ask
    // (should not happen in practice — the backend emits the
    // resolved entry with the SAME rid as the ask — but the
    // pairing layer is defensive). The unmatched ask renders
    // without an outcome; the orphan resolved entry is dropped.
    const sections: TranscriptSection[] = [
      permissionAskSection("write_file", "rid-ask-A"),
      permissionAskResolvedSection("rid-ask-OTHER", "allow"),
    ];
    const out = pairSections(sections, 1_000_000, new Map());
    expect(out).toHaveLength(1);
    expect(out[0].kind).toBe("permission_ask");
    if (out[0].kind !== "permission_ask") throw new Error("expected permission_ask");
    expect(out[0].outcome).toBeUndefined();
  });

  it("drops PermissionAskResolvedSection from the output (consumed by pairing)", () => {
    // The resolved section is NEVER rendered directly — it is
    // consumed by the pairing layer. Even when there's no
    // matching ask (orphan resolved), the resolved section is
    // dropped from the output (the drawer's Tools template would
    // otherwise render a noise card between the ask and the next
    // tool_call).
    const sections: TranscriptSection[] = [
      permissionAskSection("write_file", "rid-1"),
      permissionAskResolvedSection("rid-1", "deny"),
      toolCallSection("tu_after", "read_file"),
      toolResultSection("tu_after"),
    ];
    const out = pairSections(sections, 1_000_000, new Map());
    // Expected: 1 ask (with outcome) + 1 paired tool = 2 entries.
    // The resolved section is NOT in the output.
    expect(out).toHaveLength(2);
    expect(out[0].kind).toBe("permission_ask");
    expect(out[1].kind).toBe("paired");
  });

  it("pairs multiple asks with their respective resolved entries (independent rids)", () => {
    // Two asks, two resolves, each pair matched by its own rid.
    // Order in output = order of asks in the input (asks are
    // emitted in transcript order; the resolved entries are
    // matched by rid, not by position).
    const sections: TranscriptSection[] = [
      permissionAskSection("write_file", "rid-A"),
      permissionAskSection("shell", "rid-B"),
      permissionAskResolvedSection("rid-B", "deny"),
      permissionAskResolvedSection("rid-A", "allow"),
    ];
    const out = pairSections(sections, 1_000_000, new Map());
    expect(out).toHaveLength(2);
    expect(out[0].kind).toBe("permission_ask");
    expect(out[1].kind).toBe("permission_ask");
    if (out[0].kind !== "permission_ask" || out[1].kind !== "permission_ask") {
      throw new Error("expected both permission_ask");
    }
    // The order follows the input ask order (rid-A first, rid-B second).
    expect(out[0].payload_json.rid).toBe("rid-A");
    expect(out[0].outcome).toBe("allow");
    expect(out[1].payload_json.rid).toBe("rid-B");
    expect(out[1].outcome).toBe("deny");
  });

  it("handles resolved entry arriving BEFORE the ask (defensive ordering)", () => {
    // The canonical order is ask → resolved (the worker emits the
    // resolved AFTER the ask). But a pre-scan (vs interleaved)
    // makes the pairing layer robust to either ordering — if a
    // future refactor ever emits the resolved first, the ask
    // still picks up its outcome.
    const sections: TranscriptSection[] = [
      permissionAskResolvedSection("rid-X", "timeout"),
      permissionAskSection("write_file", "rid-X"),
    ];
    const out = pairSections(sections, 1_000_000, new Map());
    expect(out).toHaveLength(1);
    expect(out[0].kind).toBe("permission_ask");
    if (out[0].kind !== "permission_ask") throw new Error("expected permission_ask");
    expect(out[0].outcome).toBe("timeout");
  });

  it("defensive: malformed resolved payload (missing rid) is dropped silently", () => {
    const sections: TranscriptSection[] = [
      permissionAskSection("write_file", "rid-good"),
      // Malformed: missing rid.
      { kind: "PermissionAskResolved", payload_json: { outcome: "allow" } },
    ];
    const out = pairSections(sections, 1_000_000, new Map());
    expect(out).toHaveLength(1);
    expect(out[0].kind).toBe("permission_ask");
    if (out[0].kind !== "permission_ask") throw new Error("expected permission_ask");
    expect(out[0].outcome).toBeUndefined();
  });

  it("defensive: malformed resolved payload (missing outcome) is dropped silently", () => {
    const sections: TranscriptSection[] = [
      permissionAskSection("write_file", "rid-good"),
      // Malformed: missing outcome.
      { kind: "PermissionAskResolved", payload_json: { rid: "rid-good" } },
    ];
    const out = pairSections(sections, 1_000_000, new Map());
    expect(out).toHaveLength(1);
    expect(out[0].kind).toBe("permission_ask");
    if (out[0].kind !== "permission_ask") throw new Error("expected permission_ask");
    expect(out[0].outcome).toBeUndefined();
  });
});

// ======================================================================
// useTranscriptPairing — composable 封装 pending Map
// (RULE-FrontSubagent-002, 2026-06-25)
// ======================================================================
// 验证 composable 把 pending Map 封进闭包: 调用方不再传第三参 Map, 但
// 30s timeout 仍跨调用推进 (债的核心 —— 旧签名若新调用方每次传 new Map(),
// receivedAt 永远 = 当前 now → 永远 pending)。另验 reset() 清状态 + 实例隔离。

describe("useTranscriptPairing — composable 封装 pending Map (RULE-FrontSubagent-002)", () => {
  it("pairSections 跨调用共享同一 Map → 30s timeout 推进 (不重置 receivedAt)", () => {
    // 债的核心: 旧签名第三参若每次传 new Map(), receivedAt 永远 = 当前 now
    // → 永远 pending。composable 闭包持同一 Map → receivedAt 锁定首次见时。
    const { pairSections } = useTranscriptPairing();
    const call = [toolCallSection("call_1")];

    // 第一次 (now=T): 未配对 → pending, 未超时
    const out1 = pairSections(call, 1_000_000);
    expect(out1).toHaveLength(1);
    expect(out1[0].kind).toBe("pending_call");
    if (out1[0].kind === "pending_call") expect(out1[0].timedOut).toBe(false);

    // 第二次 (now=T+30s+1, 无新 section): 同一 Map → timedOut=true
    const out2 = pairSections(call, 1_000_000 + PENDING_TIMEOUT_MS + 1);
    expect(out2).toHaveLength(1);
    expect(out2[0].kind).toBe("pending_call");
    if (out2[0].kind === "pending_call") expect(out2[0].timedOut).toBe(true);
  });

  it("reset() 清空 Map → 下次调用 receivedAt 重置 (不再继承旧时间戳)", () => {
    const { pairSections, reset } = useTranscriptPairing();
    const call = [toolCallSection("call_1")];

    pairSections(call, 1_000_000); // first-seen 记录为 T
    reset(); // 清空

    // reset 后用同 now 再调 → receivedAt 重置为 now → 未超时
    const out = pairSections(call, 1_000_000);
    expect(out).toHaveLength(1);
    expect(out[0].kind).toBe("pending_call");
    if (out[0].kind === "pending_call") expect(out[0].timedOut).toBe(false);
  });

  it("配对成功后 pending Map 清除 → 同 id 复用不继承上次时间戳", () => {
    const { pairSections } = useTranscriptPairing();
    // call + result 配对成功 (pairSections 命中时 delete(id))
    const paired = pairSections(
      [toolCallSection("c1"), toolResultSection("c1")],
      1_000_000,
    );
    expect(paired).toHaveLength(1);
    expect(paired[0].kind).toBe("paired");

    // 同 id 新 call, now 已远超 30s —— Map 已清 → first-seen 重置 → 未超时
    const out = pairSections(
      [toolCallSection("c1")],
      1_000_000 + PENDING_TIMEOUT_MS + 100,
    );
    expect(out).toHaveLength(1);
    expect(out[0].kind).toBe("pending_call");
    if (out[0].kind === "pending_call") expect(out[0].timedOut).toBe(false);
  });

  it("实例隔离: 两实例 first-seen Map 独立 (a.reset 不影响 b)", () => {
    // 注意 pairSections 的 pending 是每次调用 local 重建 —— 跨调用持久的
    // 是 pendingFirstSeenAt 里的 first-seen 时间戳， 不是"记住消失的 call"。
    // 故每次调用都要带 ToolCall section 才能 flush 出 pending_call。
    const a = useTranscriptPairing();
    const b = useTranscriptPairing();

    // a, b 都在 T 见 c1 (各自实例记 first-seen)
    a.pairSections([toolCallSection("c1")], 1_000_000);
    b.pairSections([toolCallSection("c1")], 1_000_000);

    a.reset(); // 只清 a 的 Map

    // b 不受影响: b 的 c1 first-seen 仍 = T → T+31s 超时
    const outB = b.pairSections([toolCallSection("c1")], 1_000_000 + PENDING_TIMEOUT_MS + 1);
    expect(outB).toHaveLength(1);
    expect(outB[0].kind).toBe("pending_call");
    if (outB[0].kind === "pending_call") expect(outB[0].timedOut).toBe(true);

    // a reset 后 c1 first-seen 重置为现在 → 未超时
    const outA = a.pairSections([toolCallSection("c1")], 1_000_000 + PENDING_TIMEOUT_MS + 1);
    expect(outA).toHaveLength(1);
    expect(outA[0].kind).toBe("pending_call");
    if (outA[0].kind === "pending_call") expect(outA[0].timedOut).toBe(false);
  });

  it("pairEntries (legacy raw-entry 路径) 同样跨调用持久 → aged out 成 standalone", () => {
    const { pairEntries } = useTranscriptPairing();
    const call = [toolCall("pending_only")];

    const out1 = pairEntries(call, 1_000_000);
    expect(out1[0].kind).toBe("pending_call");

    // pairTranscript (legacy) 超时 → standalone (区别于 pairSections 的 timedOut)
    const out2 = pairEntries(call, 1_000_000 + PENDING_TIMEOUT_MS + 1);
    expect(out2[0].kind).toBe("standalone");
  });
});
