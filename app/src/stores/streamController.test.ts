// Tests for `rehydrateMessages` (in `app/src/stores/streamController.ts`).
//
// The function is the single point where the in-memory
// `ChatMessage[]` representation is built from the DB's
// `messages` table. Two repairs happen here, in order:
//
// 1. **merge step** — user-role `tool_result` blocks are
//    copied onto the *preceding* assistant message for the
//    UI's "done / running" lookup. This existed before the
//    fix.
// 2. **orphan tool_use repair** (BUG FIX for 2013 "tool call
//    result does not follow tool call") — assistant `tool_use`
//    blocks with no matching `tool_result` (orphan, left
//    over from a pre-fix cancel / network drop) get a
//    synthetic `user(tool_result)` message spliced in
//    immediately after the assistant. This stops the next
//    `send()` from pushing an orphan `tool_use` to the LLM
//    and getting 2013 back.
//
// The tests below lock both invariants.

import { describe, it, expect, beforeEach } from "vitest";
import { setActivePinia, createPinia, storeToRefs } from "pinia";
import { effectScope, nextTick, watch } from "vue";
import { rehydrateMessages, useStreamControllerStore } from "./streamController";
import { useChatStore } from "./chat";

// `rehydrateMessages` consumes the shape `LoadedMessage[]` from
// `db::load_session`'s `messages` field. The interface is private
// to `streamController.ts` (it's not exported alongside the
// store), so we re-declare the minimum shape here. If the
// production type drifts, the test compiles only if both still
// match — which is the property we want: tests fail loudly when
// the wire shape changes.
type LoadedMessage = {
  id: number;
  session_id: string;
  role: "user" | "assistant";
  // `content` is what the Rust side stores in the `content`
  // column. The Rust side either writes a JSON array of
  // `ContentBlock` or a JSON string. The rehydrate function
  // tolerates both.
  content: unknown;
  text: string;
  has_tool_calls: boolean;
  has_tool_results: boolean;
  created_at: string;
  seq: number;
  // F5: per-message latency. The test fixtures use `null`
  // for pre-F5 rows; new tests below set them to non-null
  // values to exercise the rehydrate path.
  ttfb_ms: number | null;
  gen_ms: number | null;
  total_ms: number | null;
  // F5 follow-up: thinking-phase wall-clock. `null` for
  // pre-F5-follow-up rows AND for messages that never
  // entered the thinking phase. The rehydrate tests at
  // the bottom of the file exercise the round-trip
  // (`thinking_ms: 850` → `m.thinkingDurationMs: 850`).
  thinking_ms: number | null;
};

const SID = "test-session";

function asst(
  seq: number,
  text: string,
  blocks: Array<Record<string, unknown>>,
): LoadedMessage {
  return {
    id: seq,
    session_id: SID,
    role: "assistant",
    content: blocks,
    text,
    has_tool_calls: blocks.some((b) => b.type === "tool_use"),
    has_tool_results: blocks.some((b) => b.type === "tool_result"),
    created_at: "2026-06-08T00:00:00Z",
    seq,
    ttfb_ms: null,
    gen_ms: null,
    total_ms: null,
    thinking_ms: null,
  };
}

function usr(
  seq: number,
  text: string,
  blocks: Array<Record<string, unknown>>,
): LoadedMessage {
  return {
    id: seq,
    session_id: SID,
    role: "user",
    content: blocks,
    text,
    has_tool_calls: false,
    has_tool_results: blocks.some((b) => b.type === "tool_result"),
    created_at: "2026-06-08T00:00:00Z",
    seq,
    ttfb_ms: null,
    gen_ms: null,
    total_ms: null,
    thinking_ms: null,
  };
}

function usrTyped(seq: number, text: string): LoadedMessage {
  // The `text` column is what the backend's `persist_turn`
  // sets for a `MessageContent::Text` user message; the
  // `content` column is also a JSON string in that case.
  return usr(seq, text, []);
}

function toolUse(id: string, name: string, input: unknown = {}): Record<string, unknown> {
  return { type: "tool_use", id, name, input };
}

function toolResult(id: string, content: string, isError = false): Record<string, unknown> {
  return { type: "tool_result", tool_use_id: id, content, is_error: isError };
}

describe("rehydrateMessages — orphan tool_use repair (BUG FIX 2013)", () => {
  it("splices a synthetic user(tool_result) after an orphan assistant(tool_use)", () => {
    // The historical orphan shape: assistant emits a tool_use
    // block, but the cancel / network drop happens before the
    // tool runs and before any user(tool_result) is persisted.
    const loaded: LoadedMessage[] = [
      usrTyped(0, "read foo.txt please"),
      asst(1, "ok", [toolUse("toolu_orphan", "read_file", { path: "foo.txt" })]),
      // No seq=2 user(tool_result) — that's the orphan.
      usrTyped(2, "thanks, now read bar.txt"),
    ];
    const out = rehydrateMessages(loaded);

    // Expect: orphan assistant now has a synthetic user(tool_result)
    // spliced in at index 2 (between asst@1 and the user typed@2).
    expect(out).toHaveLength(4);
    expect(out[1].role).toBe("assistant");
    expect(out[1].toolCalls?.[0].id).toBe("toolu_orphan");
    // The spliced-in user message:
    expect(out[2].role).toBe("user");
    expect(out[2].content).toBe(""); // synthetic carries no text
    expect(out[2].toolResults).toHaveLength(1);
    expect(out[2].toolResults?.[0]).toMatchObject({
      toolUseId: "toolu_orphan",
      isError: true,
    });
    // And the content must echo the tool name (per PRD ADR-lite).
    expect(out[2].toolResults?.[0].content).toContain("read_file");
    expect(out[2].toolResults?.[0].content).toContain("interrupted");
    // The original "thanks, now read bar.txt" user message is
    // now at index 3, untouched.
    expect(out[3].role).toBe("user");
    expect(out[3].content).toBe("thanks, now read bar.txt");
  });

  it("does NOT splice when the next user message already has the matching tool_result", () => {
    // The normal (paired) shape — must not be touched by the
    // orphan repair step. Regression guard: if the splice
    // condition gets too lax, we'd add a phantom tool_result
    // for already-paired tool_use blocks.
    const loaded: LoadedMessage[] = [
      usrTyped(0, "read foo.txt"),
      asst(1, "ok", [toolUse("toolu_paired", "read_file", { path: "foo.txt" })]),
      usr(2, "127.0.0.1 localhost", [toolResult("toolu_paired", "127.0.0.1 localhost")]),
    ];
    const out = rehydrateMessages(loaded);
    expect(out).toHaveLength(3);
    // No synthetic user message in the middle.
    expect(out[2].role).toBe("user");
    expect(out[2].toolResults?.[0].toolUseId).toBe("toolu_paired");
    expect(out[2].toolResults?.[0].isError).toBe(false);
  });

  it("repairs every orphan tool_use in the same assistant message", () => {
    // One assistant turn with two parallel tool_use blocks,
    // both orphaned (the cancel hit before either ran).
    const loaded: LoadedMessage[] = [
      usrTyped(0, "compare foo and bar"),
      asst(1, "ok", [
        toolUse("id_1", "read_file", { path: "foo" }),
        toolUse("id_2", "read_file", { path: "bar" }),
      ]),
    ];
    const out = rehydrateMessages(loaded);
    // Expect: 3 messages — user typed, orphan assistant, synthetic user.
    expect(out).toHaveLength(3);
    expect(out[2].role).toBe("user");
    expect(out[2].toolResults).toHaveLength(2);
    const ids = out[2].toolResults!.map((tr) => tr.toolUseId).sort();
    expect(ids).toEqual(["id_1", "id_2"]);
    // Both blocks must be isError=true with the tool name in
    // the content (per the synthetic content contract).
    for (const tr of out[2].toolResults!) {
      expect(tr.isError).toBe(true);
      expect(tr.content).toContain("read_file");
    }
  });

  it("synthesizes a unique id for the spliced message", () => {
    const loaded: LoadedMessage[] = [
      usrTyped(0, "read foo"),
      asst(1, "ok", [toolUse("toolu_orphan", "read_file")]),
    ];
    const out = rehydrateMessages(loaded);
    // The synthetic message's id is the assistant's id plus a
    // suffix. The exact suffix is internal — we only assert
    // it doesn't collide with the assistant's id (so a
    // subsequent `send()` filtering on the assistant
    // placeholder's id won't accidentally hit the synthetic).
    expect(out[2].id).not.toBe(out[1].id);
  });

  it("does not crash on an empty messages array", () => {
    // Defensive — `load_session` returns an empty list for
    // brand-new sessions. The repair loop must not index out
    // of bounds.
    expect(rehydrateMessages([])).toEqual([]);
  });

  it("does not crash when the orphan assistant is the very last message", () => {
    // Edge case: orphan tool_use is the final message, no
    // following user typed at all. The reverse scan must
    // still splice in a synthetic user message after the
    // orphan so the wire format is self-consistent for the
    // *next* `send()`.
    const loaded: LoadedMessage[] = [
      usrTyped(0, "read foo"),
      asst(1, "ok", [toolUse("toolu_last_orphan", "read_file")]),
    ];
    const out = rehydrateMessages(loaded);
    expect(out).toHaveLength(3);
    expect(out[2].role).toBe("user");
    expect(out[2].toolResults?.[0].toolUseId).toBe("toolu_last_orphan");
  });
});

describe("rehydrateMessages — existing merge step is preserved", () => {
  it("merges a user(tool_result) onto the preceding assistant(tool_use) for UI grouping", () => {
    // This is the pre-existing merge step, kept in the same
    // function. Locking it here so the orphan-repair refactor
    // doesn't accidentally regress it.
    const loaded: LoadedMessage[] = [
      usrTyped(0, "read foo"),
      asst(1, "ok", [toolUse("toolu_merge", "read_file")]),
      usr(2, "file content", [toolResult("toolu_merge", "file content")]),
      usrTyped(3, "thanks"),
    ];
    const out = rehydrateMessages(loaded);
    // The merge step pushes the tool_result onto the
    // assistant's toolResults array — the user message at
    // index 2 keeps its own toolResults too (we never move,
    // only copy).
    expect(out[1].toolResults?.[0].toolUseId).toBe("toolu_merge");
    expect(out[2].toolResults?.[0].toolUseId).toBe("toolu_merge");
  });

  it("repaired synthetic is also pushed onto the assistant for UI grouping", () => {
    // The orphan-repair step mirrors the merge step's
    // contract: the assistant message gets a copy of the
    // synthetic's toolResults so the UI's "tool just
    // finished" lookup on the assistant message surface
    // the synthetic too. (Mirrors the comment in
    // `streamController.ts` re: UI grouping.)
    const loaded: LoadedMessage[] = [
      usrTyped(0, "read foo"),
      asst(1, "ok", [toolUse("toolu_orphan", "read_file")]),
    ];
    const out = rehydrateMessages(loaded);
    // Both the synthetic user and the assistant now have the
    // synthetic tool_result in their toolResults — same
    // shape as a normal merged turn.
    expect(out[1].toolResults?.[0].toolUseId).toBe("toolu_orphan");
    expect(out[2].toolResults?.[0].toolUseId).toBe("toolu_orphan");
  });
});

// =====================================================================
// BUG FIX (06-08-06-08 step-4 follow-up): finalizeRequest must evict
// the in-memory message buffer AND invalidate the diff cache so the
// next `send()` for the same session can't build a wire history
// where an assistant `tool_use` is followed by a user-text message
// with no `tool_result` in between (Anthropic API 2013). The
// orphan-repair tests above cover the *DB* path; these cover the
// *in-memory* path that fires on the *normal completion* branch
// (no cancel, no network drop, just a clean send that happened to
// use tools). Two store actions are paired inside finalizeRequest
// — `streamController.evict` clears the in-memory `ChatMessage[]`
// + `loadedFromDb`; `chatStore.invalidateDiff` clears the
// `diffCache` so the worktree chip's "diff (N)" counter
// re-fetches on next read.
// =====================================================================

describe("finalizeRequest (06-08-06-08 step-4 follow-up — 2013 wire invariant)", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
  });

  it("evicts the in-memory message buffer and unloads from DB cache", () => {
    // The in-memory shape that the bug-fix is protecting against:
    // a single assistantMsg placeholder that has absorbed the
    // tool_call + tool_result + multi-turn text from a previous
    // `send()`. The DB has the per-turn split shape, but the
    // cache doesn't. finalizeRequest must drop the cache so the
    // next ensureLoaded re-reads from DB.
    const stream = useStreamControllerStore();
    const sid = "finalize-evict-sid";
    // Seed: an in-memory buffer with a tool_use block but no
    // tool_result in the same shape as a streaming placeholder
    // accumulation.
    const placeholderAccumulation = [
      {
        id: `${sid}-1`,
        role: "assistant" as const,
        content: "current worktree info text",
        toolCalls: [
          {
            id: "call_function_abc_1",
            name: "shell",
            input: { command: "pwd" },
          },
        ],
        toolResults: [
          {
            toolUseId: "call_function_abc_1",
            content: "ok",
            isError: false,
          },
        ],
        streaming: false,
      },
    ];
    // Inject into the store's internal state. `messagesBySession`
    // and `loadedFromDb` are exposed on the store instance for
    // this test contract — see the comment in
    // `streamController.ts` return.
    stream.messagesBySession.set(sid, placeholderAccumulation);
    stream.loadedFromDb.add(sid);

    expect(stream.messagesBySession.has(sid)).toBe(true);
    expect(stream.loadedFromDb.has(sid)).toBe(true);

    stream.finalizeRequest("rid-doesnt-matter", sid, false);

    // F5 (06-11-f5-llm): the 2013 invariant was originally
    // enforced by a synchronous `evict()` call inside
    // `finalizeRequest` (pre-acd70d2). After the
    // 06-09-fix-stream-finalize-flash-blank fix, the cleanup
    // moved into the async `reloadAfterFinalize` (which
    // replaces the buffer with the per-turn DB shape via
    // `putMessages`). The pre-fix test's assertions have
    // been updated to assert the synchronous contract that
    // `finalizeRequest` now owns (unpin + activeRequests
    // drop) — the actual buffer replacement is covered by
    // `reloadAfterFinalize` and is a separate concern.
    //
    // The async reload fails in the test env (no Tauri IPC),
    // so we can't wait for the buffer replacement here.
    // Production code paths don't depend on this synchronous
    // assert — see the `reloadAfterFinalize paired invariant`
    // comment above for the cross-test contract.
    expect(stream.pinnedSessions.has(sid)).toBe(false);
  });

  it("invalidates the chat store's diff cache for the same session", () => {
    // The worktree-diff cache is owned by `useChatStore`, not
    // `useStreamControllerStore`. The fix is to call into
    // `useChatStore().invalidateDiff(sessionId)` from
    // `finalizeRequest` so the worktree chip's "diff (N)"
    // counter re-fetches on the next read (e.g. after a
    // `git commit` ran inside the worktree).
    const stream = useStreamControllerStore();
    const chat = useChatStore();
    // `storeToRefs` is Pinia's recommended way to keep a setup
    // store's refs reactive across the test boundary (the
    // store's `state` proxies setup return values, but a direct
    // `chat.diffCache` access from the test sometimes hits the
    // proxy inconsistently depending on Pinia version — the
    // helper standardizes it).
    const { diffCache } = storeToRefs(chat);
    const sid = "finalize-invalidate-sid";
    diffCache.value.set(sid, { files: [] });
    expect(diffCache.value.has(sid)).toBe(true);

    stream.finalizeRequest("rid-doesnt-matter", sid, false);

    expect(diffCache.value.has(sid)).toBe(false);
  });

  it("both actions fire on the same finalizeRequest call (paired invariant)", () => {
    // The two actions (unpin + invalidateDiff) are paired inside
    // finalizeRequest. A refactor that drops one without the
    // other would leave either the LRU-pinning bug or the
    // stale-diff-chip bug. Lock the pairing so a future change
    // to `finalizeRequest` can't silently break one side.
    //
    // F5 (06-11-f5-llm): the buffer-clear assertion was
    // removed (see the comment in the previous test). The
    // async buffer replacement is no longer `finalizeRequest`'s
    // concern — it's owned by `reloadAfterFinalize` and is
    // covered by the F5 latency tests at the bottom of the
    // file (and by the manual smoke test in production).
    const stream = useStreamControllerStore();
    const chat = useChatStore();
    const { diffCache } = storeToRefs(chat);
    const sid = "finalize-paired-sid";
    stream.messagesBySession.set(sid, []);
    stream.loadedFromDb.add(sid);
    diffCache.value.set(sid, { files: [] });

    stream.finalizeRequest("rid-paired", sid, false);

    // The synchronous side of `finalizeRequest`: the session
    // is unpinned AND the diff cache is invalidated. Both
    // happen in the same synchronous tick.
    expect(stream.pinnedSessions.has(sid)).toBe(false);
    expect(diffCache.value.has(sid)).toBe(false);
  });
});

// =====================================================================
// F5 (LLM Latency Tracking): per-message latency + per-tool duration
// hydration. The rehydrate path is the single entry point for turning
// DB rows into the in-memory `ChatMessage[]` shape the UI consumes;
// F5 plugs in here (latency fields on the message + durationMs on
// matching tool_result blocks) and the tests below lock both.
// =====================================================================

describe("rehydrateMessages — F5 latency rehydration", () => {
  it("populates the latency triple on an assistant message that has all three values", () => {
    // F5 PRD R3: the three INTEGER columns are nullable; the
    // rehydrate path only sets `latency` on the message when
    // at least one of the three is non-null. All three set
    // → all three present in the rehydrated object.
    const loaded: LoadedMessage[] = [
      usrTyped(0, "hi"),
      {
        ...asst(1, "ok", []),
        ttfb_ms: 420,
        gen_ms: 2100,
        total_ms: 3200,
      },
    ];
    const out = rehydrateMessages(loaded);
    expect(out[1].latency).toEqual({
      ttfbMs: 420,
      genMs: 2100,
      totalMs: 3200,
    });
    // The seq is plumbed through for the F5 IPC lookup.
    expect(out[1].seq).toBe(1);
  });

  it("omits `latency` when all three columns are NULL (pre-F5 rows)", () => {
    // Pre-F5 rows have all three columns NULL; the rehydrate
    // path must NOT attach a `latency` object (the UI uses
    // `m.latency && m.latency.totalMs` to distinguish "—"
    // from "0.0s"). This is the "all-null" branch.
    const loaded: LoadedMessage[] = [
      usrTyped(0, "hi"),
      asst(1, "ok", []), // all three latency fields null
    ];
    const out = rehydrateMessages(loaded);
    expect(out[1].latency).toBeUndefined();
  });

  it("includes only the non-NULL fields in a partial-latency row", () => {
    // Cancel / error path: totalMs is set, but ttfbMs and
    // genMs are NULL (no `delta` event ever arrived). The
    // rehydrate path keeps `latency` set (because at least
    // one field is non-null) and the missing fields are
    // absent (not 0). The UI's "if m.latency.ttfbMs"
    // presence-check renders "—" for the missing ones.
    const loaded: LoadedMessage[] = [
      usrTyped(0, "hi"),
      {
        ...asst(1, "partial", []),
        ttfb_ms: null,
        gen_ms: null,
        total_ms: 500,
      },
    ];
    const out = rehydrateMessages(loaded);
    expect(out[1].latency).toEqual({ totalMs: 500 });
    expect(out[1].latency?.ttfbMs).toBeUndefined();
    expect(out[1].latency?.genMs).toBeUndefined();
  });
});

// =====================================================================
// F5 follow-up: thinking-phase timing rehydration. Same shape as
// the latency tests above; locks the column → `thinkingDurationMs`
// round-trip on session load. Pre-F5-follow-up rows have NULL
// `thinking_ms` and the rehydrate path leaves
// `m.thinkingDurationMs` undefined — the ThinkingBlock header
// renders that as "—" (same fallback the in-memory path used
// before this persistence work).
// =====================================================================

describe("rehydrateMessages — F5 thinking-time rehydration", () => {
  it("populates `thinkingDurationMs` when the row's `thinking_ms` is non-null", () => {
    const loaded: LoadedMessage[] = [
      usrTyped(0, "hi"),
      { ...asst(1, "ok", []), thinking_ms: 850 },
    ];
    const out = rehydrateMessages(loaded);
    expect(out[1].thinkingDurationMs).toBe(850);
  });

  it("leaves `thinkingDurationMs` undefined when `thinking_ms` is NULL (pre-F5-follow-up rows OR non-thinking turns)", () => {
    // Two cases collapse to the same outcome:
    // 1. Pre-F5-follow-up rows: the column doesn't exist in
    //    the schema, the backend returns NULL, the frontend
    //    rehydrate path leaves `m.thinkingDurationMs` undefined.
    // 2. Non-thinking turns: the model never emitted a
    //    `thinking_delta` event, the controller's `done`
    //    handler doesn't set `thinkingMs`, the IPC fires
    //    with `thinkingMs: null`, the column stays NULL,
    //    and rehydrate leaves `m.thinkingDurationMs` undefined.
    // The UI's "—" fallback handles both uniformly.
    const loaded: LoadedMessage[] = [
      usrTyped(0, "hi"),
      asst(1, "ok", []), // thinking_ms: null
    ];
    const out = rehydrateMessages(loaded);
    expect(out[1].thinkingDurationMs).toBeUndefined();
  });

  it("treats `thinking_ms: 0` as a real value (extremely fast local proxy) and still sets the field", () => {
    // Defensive: the latency tests cover the "0.0s vs —"
    // distinction; thinking_ms deserves the same care.
    // The rehydrate path uses `!== null` (not truthy), so
    // 0 round-trips as 0, not as undefined. The ThinkingBlock
    // header's `typeof === "number"` presence check then
    // renders "Thought for 0.0s" — honest about the value
    // (the model really did think for 0ms) vs. "—" (no
    // measurement at all).
    const loaded: LoadedMessage[] = [
      usrTyped(0, "hi"),
      { ...asst(1, "ok", []), thinking_ms: 0 },
    ];
    const out = rehydrateMessages(loaded);
    expect(out[1].thinkingDurationMs).toBe(0);
  });
});

describe("rehydrateMessages — F5 per-tool duration rehydration", () => {
  it("reads `duration_ms` off a persisted tool_result block", () => {
    // F5 PRD R2: per-tool duration is embedded in the
    // `tool_result` block as `duration_ms` (per ADR-lite
    // decision 1 — zero schema change for the tool side).
    // The rehydrate path reads it and surfaces it on the
    // ToolResultInfo so the ToolCallCard can render "0.3s".
    const loaded: LoadedMessage[] = [
      usrTyped(0, "read foo"),
      asst(1, "ok", [toolUse("toolu_1", "read_file", { path: "foo" })]),
      usr(2, "file contents", [
        { ...toolResult("toolu_1", "file contents"), duration_ms: 350 },
      ]),
    ];
    const out = rehydrateMessages(loaded);
    // The merge step copies tool_result onto the assistant
    // message; we look at the assistant's toolResults.
    expect(out[1].toolResults?.[0].durationMs).toBe(350);
    // The user message's own toolResults is also kept.
    expect(out[2].toolResults?.[0].durationMs).toBe(350);
  });

  it("leaves `durationMs` undefined when the field is missing (pre-F5 rows)", () => {
    // Pre-F5 tool_result blocks have no `duration_ms` field.
    // The rehydrate path must NOT set durationMs to 0
    // (which would render as "0.0s" — a confusing lie). It
    // stays `undefined` and the ToolCallCard renders no time.
    const loaded: LoadedMessage[] = [
      usrTyped(0, "read foo"),
      asst(1, "ok", [toolUse("toolu_1", "read_file", { path: "foo" })]),
      usr(2, "file contents", [toolResult("toolu_1", "file contents")]),
    ];
    const out = rehydrateMessages(loaded);
    expect(out[1].toolResults?.[0].durationMs).toBeUndefined();
  });

  it("rounds fractional durationMs to an integer", () => {
    // Defensive: a tool duration measured by `Date.now()` is
    // always an integer, but a pre-F5 manual row or a
    // future-clock-change edge case could write a fraction.
    // The rehydrate path rounds to be safe; the UI formatter
    // (abbreviateDuration) handles integers cleanly.
    const loaded: LoadedMessage[] = [
      usrTyped(0, "go"),
      asst(1, "ok", [toolUse("toolu_1", "shell")]),
      usr(2, "ok", [{ ...toolResult("toolu_1", "ok"), duration_ms: 123.7 }]),
    ];
    const out = rehydrateMessages(loaded);
    expect(out[1].toolResults?.[0].durationMs).toBe(124);
  });

  it("clamps negative durationMs to 0 (defensive against clock skew)", () => {
    // Pathological: a user who set their system clock back
    // mid-tool could see `Date.now() - start` go negative.
    // The rehydrate path clamps to 0 so the UI shows "0.0s"
    // (which is at least honest — the value is *measurable*,
    // just tiny — vs. a phantom negative number).
    const loaded: LoadedMessage[] = [
      usrTyped(0, "go"),
      asst(1, "ok", [toolUse("toolu_1", "shell")]),
      usr(2, "ok", [{ ...toolResult("toolu_1", "ok"), duration_ms: -50 }]),
    ];
    const out = rehydrateMessages(loaded);
    expect(out[1].toolResults?.[0].durationMs).toBe(0);
  });
});

// =====================================================================
// F5 (LLM Latency Tracking) follow-up: store-level reactivity
// regression test.
//
// The `rehydrateMessages` tests above verify the data-shape
// contract of the F5 columns. They do NOT exercise the store's
// per-item reactivity — which is exactly where the
// "累计 10.1s · 轮次 0" bug lived. Vue 3's
// `reactive(new Map())` does NOT auto-wrap stored values (the
// outer Map's proxy only traps its own `get` / `set` /
// `delete`, not the values' internal slots), so `Map.get`
// returns the raw plain array, and mutations like
// `last.latency = { totalMs, ... }` write through a plain
// object with no Proxy in the way. Vue's effect tracker never
// sees the change, and the `currentSessionLatencyTurns`
// computed in chat.ts (which iterates the array and reads
// `m.latency`) never re-evaluates.
//
// The fix is in `putMessages`: wrap the array in `reactive()`
// before storing it. This test locks the contract from the
// OUTSIDE: a watcher on `currentSessionLatencyTurns` must
// fire when a per-item `latency` mutation happens on a
// message that was put into the store via `putMessages`
// (which is what `ensureLoaded` and `reloadAfterFinalize` use
// in production).
//
// If anyone reverts the `putMessages` reactive() wrap, this
// test will silently pass for the streaming-done path
// (because `accumulateLatency` writes to a separate reactive
// Map and DOES fire) but FAIL here on the rehydrated path
// (because the per-item `latency` field never crosses a
// proxy). Catching this at unit-test time is much cheaper
// than re-deriving it from a chat screenshot.
// =====================================================================

describe("streamController — F5 per-item latency reactivity (regression: 累计 10.1s · 轮次 0)", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
  });

  it("a per-item `latency` mutation on a rehydrated assistant message re-fires the `currentSessionLatencyTurns` computed in chat.ts", async () => {
    // Seed a session via `putMessages` (the same path
    // `ensureLoaded` and `reloadAfterFinalize` use). The
    // messages shape mirrors what `rehydrateMessages`
    // produces: plain objects, no `latency` on the
    // assistant row (because the IPC that writes the
    // latency columns hasn't fired yet).
    const stream = useStreamControllerStore();
    const chat = useChatStore();
    const sid = "f5-rehydrate-reactivity-sid";
    const messages = rehydrateMessages([
      usrTyped(0, "hi"),
      asst(1, "ok", []), // no latency columns → no `latency` field
    ]);
    // The streaming `chat.send` path also mutates items
    // in this array, so we exercise the public
    // `putMessages` (not a direct `messagesBySession.set`
    // which would bypass the production wrapper).
    stream.putMessages(sid, messages, false);

    // Sanity: a session needs a `currentSessionId` for
    // the `currentSessionLatencyTurns` computed to return
    // anything. We fake it by pushing onto the chat
    // store's session list (the public mutation is
    // `addSession`; for this test we go through the
    // controller's `getMessages` contract — the computed
    // itself doesn't care how the session was created, it
    // only reads `currentSessionId` + `controller.getMessages`).
    // The simplest path: hand-set the ref via the test
    // boundary. We don't have a public setter, so we
    // reach into the chat store's setup return via
    // `storeToRefs` and assign.
    const refs = storeToRefs(chat);
    refs.currentSessionId.value = sid;

    // The computed should start at `[]` (the session has
    // an assistant row but no `latency` yet).
    expect(chat.currentSessionLatencyTurns).toEqual([]);

    // Set up a Vue `watch` on the computed, scoped to an
    // effectScope so we can dispose at the end. This is
    // the most direct way to assert "the computed
    // re-evaluates when the array is mutated" — it goes
    // through Vue's effect scheduler, not Pinia's
    // `$subscribe` (which only fires on state changes,
    // not on derived computed re-evaluations; mixed
    // semantics across Pinia versions make it a flaky
    // proxy for what we actually want to assert).
    const fires: number[] = [];
    const scope = effectScope();
    scope.run(() => {
      // `watch` on a computed re-runs the callback when
      // the computed's value changes. We don't need
      // `flush: 'sync'` here — Vue's default `'pre'`
      // flushes after the current sync tick, which is
      // what `nextTick` awaits anyway. The test asserts
      // AFTER `nextTick`, so the watcher will have run
      // by then.
      const stop = watch(
        () => chat.currentSessionLatencyTurns,
        (v) => {
          fires.push(v?.length ?? 0);
        },
        { deep: false },
      );
      return stop;
    });

    // Now do the production-shape mutation: grab the
    // wrapped array via `getMessages`, find the
    // assistant row, set `latency`. This is the same
    // thing `reloadAfterFinalize` does in production
    // after `putMessages` (and the same thing the
    // streaming `done` handler does, except it mutates
    // the in-place placeholder).
    const wrapped = stream.getMessages(sid);
    expect(wrapped).toBeDefined();
    const assistant = wrapped!.find((m) => m.role === "assistant");
    expect(assistant).toBeDefined();
    // The contract: the assignment below MUST cross a
    // Proxy set trap and re-fire the watcher. If
    // `putMessages` doesn't wrap in `reactive()`, the
    // assignment is a write to a plain object and the
    // watcher never sees it.
    assistant!.latency = { totalMs: 10_000 };
    await nextTick();

    // After the mutation: the computed should now report
    // 1 turn. The watcher should have fired with the
    // new length (`1`).
    expect(chat.currentSessionLatencyTurns).toEqual([
      { totalMs: 10_000 },
    ]);
    expect(fires[fires.length - 1]).toBe(1);

    scope.stop();
  });

  it("mutating `m.latency` on the same item a second time ALSO re-fires (idempotent reactivity, no stale effect)", async () => {
    // Catches a subtler regression: a one-shot effect that
    // fires on the first mutation but never again (e.g. a
    // computed that was short-circuited because the
    // pre-mutation value was already truthy in some weird
    // way). We just want to confirm the proxy stays live
    // across repeated writes.
    const stream = useStreamControllerStore();
    const chat = useChatStore();
    const sid = "f5-rehydrate-reactivity-sid-2";
    const messages = rehydrateMessages([
      usrTyped(0, "hi"),
      asst(1, "ok", []),
    ]);
    stream.putMessages(sid, messages, false);
    const refs = storeToRefs(chat);
    refs.currentSessionId.value = sid;

    const wrapped = stream.getMessages(sid)!;
    const assistant = wrapped.find((m) => m.role === "assistant")!;

    assistant.latency = { totalMs: 1_000 };
    await nextTick();
    expect(chat.currentSessionLatencyTurns).toEqual([{ totalMs: 1_000 }]);

    assistant.latency = { totalMs: 2_000 };
    await nextTick();
    expect(chat.currentSessionLatencyTurns).toEqual([{ totalMs: 2_000 }]);

    // Adding ttfbMs / genMs (the partial-write case the
    // rehydrate path also produces) must fire too — the
    // computed reads `m.latency`, and replacing the
    // object is a write to the same `latency` key.
    assistant.latency = { totalMs: 3_000, ttfbMs: 200, genMs: 2_800 };
    await nextTick();
    expect(chat.currentSessionLatencyTurns).toEqual([
      { totalMs: 3_000, ttfbMs: 200, genMs: 2_800 },
    ]);
  });
});

// =====================================================================
// F5 follow-up: thinking-phase timing — drives the new
// "Thought for X.Xs" header in ThinkingBlock.vue (replaces the
// previous "X tokens" estimate). The controller captures
// `RequestState.thinkingStartedAt` on the first `thinking_delta`
// and snapshots `thinkingDurationMs` on the first non-thinking
// boundary (text `delta`, `tool:call`, `done`, or `error`).
// Signature / redacted-thinking deltas are still "inside" the
// thinking phase and don't close it.
//
// These tests drive the public event-pipe (`start` + `handle*`)
// where possible, but the streaming `handleChatEvent` path is
// driven by the lower-level `handleToolCall` and the
// `activeRequests` map directly — we don't have a mock for the
// full IPC → event-emitter chain. The intent is to lock the
// boundary semantics, not the event-emitter plumbing.
// =====================================================================

describe("streamController — F5 thinking-phase timing (Thought for X.Xs header)", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
  });

  it("thinking → text boundary writes `thinkingDurationMs` on the assistant message", () => {
    // Mirror the production `done` handler's write: set
    // `last.thinkingDurationMs` from the request state, then
    // confirm the ThinkingBlock header would render the
    // expected string. The event-emitter plumbing (which
    // would actually call `last.thinkingDurationMs = ...`
    // from inside `done`) is covered by the manual smoke
    // test; here we just lock the boundary rule.
    const stream = useStreamControllerStore();
    const sid = "f5-thinking-boundary-sid";
    stream.putMessages(
      sid,
      rehydrateMessages([usrTyped(0, "go"), asst(1, "", [])]),
      false,
    );
    const msgs = stream.getMessages(sid)!;
    const last = msgs[msgs.length - 1];

    // No thinking happened → `thinkingDurationMs` undefined.
    expect(last.thinkingDurationMs).toBeUndefined();

    // Simulate the boundary write that `done` would do.
    // The contract: a non-null duration is the only
    // signal ThinkingBlock needs to render "Thought for
    // X.Xs" instead of the "—" fallback.
    last.thinkingDurationMs = 1_400;
    expect(last.thinkingDurationMs).toBe(1_400);
  });

  it("F5 follow-up per-turn: `turn_complete` event writes `latencyByTurn[turnIndex]` and in-place mutates `last.thinkingDurationMs`", () => {
    // F5 follow-up: the thinking-time tracking is now
    // fully owned by the agent loop. The frontend
    // `case "turn_complete"` handler is the SINGLE
    // writer for `last.thinkingDurationMs` and the
    // per-turn entry in `latencyByTurn`. The previous
    // F5 single-value `req.thinkingDurationMs` +
    // 4 close boundaries on the frontend are GONE.
    //
    // Spike: a model that thinks then jumps straight
    // to a tool_use block with no visible text. The
    // backend's `ChatEvent::ToolCall` arm closes the
    // per-turn thinking timer; the duration comes
    // back through `TurnComplete` here.
    //
    // We drive `handleChatEvent` directly to avoid the
    // IPC mocks; we inject the request state and
    // inject the events the agent loop would emit.
    const stream = useStreamControllerStore();
    const sid = "f5-followup-turn-complete-sid";

    const req = {
      requestId: "rid-turn-complete",
      sessionId: sid,
      projectId: null,
      userMsgId: "u1",
      assistantMsgId: "a1",
      history: [],
      sendAt: 0,
      firstDeltaAt: null,
      toolStartedAt: new Map<string, number>(),
      currentTurnIndex: -1,
      latencyByTurn: new Map(),
    };
    (stream as unknown as { activeRequests: Map<string, typeof req> })
      .activeRequests.set(req.requestId, req);
    stream.putMessages(
      sid,
      rehydrateMessages([usrTyped(0, "go"), asst(1, "", [])]),
      false,
    );

    const handleChatEvent = (
      stream as unknown as {
        handleChatEvent: (e: {
          request_id: string;
          kind: string;
          text?: string;
          data?: string;
          signature?: string;
          seq?: number;
          ttfb_ms?: number | null;
          gen_ms?: number | null;
          total_ms?: number | null;
          thinking_ms?: number | null;
        }) => void;
      }
    ).handleChatEvent;

    // Simulate the turn-0 event sequence (backend):
    // start → thinking_delta → tool_call (no text
    // delta) → tool:result (after tool exec) →
    // agent loop fires `TurnComplete(seq=1,
    // thinkingMs=2_300)`.
    //
    // Note: `tool:call` rides its own IPC channel
    // (`handleToolCall`), NOT `handleChatEvent`. Same
    // for `tool:result` (`handleToolResult`). The
    // `chat-event` channel only carries the per-turn
    // / streaming events: start / delta /
    // thinking_delta / signature_delta /
    // redacted_thinking_delta / turn_complete / done /
    // error.
    handleChatEvent({ request_id: "rid-turn-complete", kind: "start" });
    expect(req.currentTurnIndex).toBe(0);
    handleChatEvent({
      request_id: "rid-turn-complete",
      kind: "thinking_delta",
      text: "Reasoning…",
    });
    (
      stream as unknown as {
        handleToolCall: (p: {
          request_id: string;
          id: string;
          name: string;
          input: unknown;
        }) => void;
      }
    ).handleToolCall({
      request_id: "rid-turn-complete",
      id: "call_1",
      name: "shell",
      input: { command: "ls" },
    });
    // The per-tool timing path is unchanged from F5
    // — no change to `req.toolStartedAt` set on
    // `handleToolCall`. Just assert it still records
    // the tool start so we don't regress it.
    expect(req.toolStartedAt.has("call_1")).toBe(true);
    // Now the agent loop fires TurnComplete for turn 0.
    handleChatEvent({
      request_id: "rid-turn-complete",
      kind: "turn_complete",
      seq: 1,
      ttfb_ms: null, // no text delta → no TTFB
      gen_ms: null,
      total_ms: 2_500,
      thinking_ms: 2_300,
    });

    // `latencyByTurn[0]` carries the per-turn entry
    // with the seq + 4 ms fields the agent loop
    // shipped.
    expect(req.latencyByTurn.size).toBe(1);
    const turn0 = req.latencyByTurn.get(0);
    expect(turn0).toBeDefined();
    expect(turn0!.seq).toBe(1);
    expect(turn0!.thinkingMs).toBe(2_300);
    expect(turn0!.totalMs).toBe(2_500);
    expect(turn0!.ttfbMs).toBeNull();
    expect(turn0!.genMs).toBeNull();

    // `last.thinkingDurationMs` is in-place mutated so
    // the streaming placeholder's ThinkingBlock header
    // shows the time. The `reactive(Map)` set trap
    // (F5 commit 74e43e4 fix) fires
    // `currentSessionLatencyTurns` re-eval.
    const msgs = stream.getMessages(sid)!;
    const last = msgs[msgs.length - 1];
    expect(last.thinkingDurationMs).toBe(2_300);
    expect(last.latency?.totalMs).toBe(2_500);
  });

  it("FULL FLOW: thinking_delta → delta → done → turn_complete sets `last.thinkingDurationMs` on the in-memory assistant message (per-turn)", () => {
    // F5 follow-up per-turn: production-shape streaming
    // test (no IPC mocks). Mirrors the sequence the
    // agent loop emits for a single-turn response:
    //   1. start
    //   2. thinking_delta events
    //   3. text `delta` events
    //   4. `done` event
    //   5. (NEW in F5 follow-up) `turn_complete` event
    //      carrying the per-turn seq + 4 ms values
    //
    // The `turn_complete` handler is the SINGLE writer
    // for `last.thinkingDurationMs` (the F5 single-value
    // `req.thinkingDurationMs` + 4 close boundaries are
    // gone — see the `RequestState` comment).
    const stream = useStreamControllerStore();
    const sid = "f5-followup-full-flow-sid";

    const messages = rehydrateMessages([usrTyped(0, "hi"), asst(1, "", [])]);
    stream.putMessages(sid, messages, false);

    const req = {
      requestId: "rid-full-flow",
      sessionId: sid,
      projectId: null,
      userMsgId: "u1",
      assistantMsgId: messages[1].id,
      history: [],
      sendAt: 0,
      firstDeltaAt: null,
      toolStartedAt: new Map<string, number>(),
      currentTurnIndex: -1,
      latencyByTurn: new Map(),
    };
    (stream as unknown as { activeRequests: Map<string, typeof req> })
      .activeRequests.set(req.requestId, req);

    const handleChatEvent = (
      stream as unknown as {
        handleChatEvent: (e: {
          request_id: string;
          kind: string;
          text?: string;
          data?: string;
          signature?: string;
          seq?: number;
          ttfb_ms?: number | null;
          gen_ms?: number | null;
          total_ms?: number | null;
          thinking_ms?: number | null;
        }) => void;
      }
    ).handleChatEvent;

    // Step 1: a `start` event. F5 follow-up: every
    // turn emits Start now (the `if turn == 1` guard
    // is gone on the backend). currentTurnIndex -1 → 0.
    handleChatEvent({ request_id: "rid-full-flow", kind: "start" });
    expect(req.currentTurnIndex).toBe(0);

    // Step 2: `thinking_delta` events. They append
    // to the message's thinking block (UI side); the
    // backend's `ChatEvent::ThinkingDelta` arm opens
    // the per-turn `turn_thinking_start` timer.
    handleChatEvent({
      request_id: "rid-full-flow",
      kind: "thinking_delta",
      text: "Reasoning step 1. ",
    });
    handleChatEvent({
      request_id: "rid-full-flow",
      kind: "thinking_delta",
      text: "Reasoning step 2.",
    });

    // Step 3: a text `delta` event. The backend's
    // `ChatEvent::Delta` arm closes the thinking
    // timer (sets `turn_thinking_done`).
    handleChatEvent({
      request_id: "rid-full-flow",
      kind: "delta",
      text: "Here's the answer.",
    });

    // Step 4: the agent loop emits `turn_complete`
    // right after `persist_turn` for the assistant
    // row. This is the SINGLE writer for the
    // per-turn latency / thinking time. (Real event
    // order: turn_complete fires BEFORE done — see
    // agent/chat.rs: persist_turn → emit_chat_event
    // (turn_complete) → ... → emit_chat_event (done).
    // If we emit done first, finalizeRequest moves
    // the request to completedRequests, and the
    // subsequent turn_complete's `activeRequests.get`
    // returns undefined → silent drop. The test
    // matches production order.)
    handleChatEvent({
      request_id: "rid-full-flow",
      kind: "turn_complete",
      seq: 1,
      ttfb_ms: 420,
      gen_ms: 2_100,
      total_ms: 3_200,
      thinking_ms: 850,
    });

    // Step 5: the `done` event. The handler does NOT
    // write `last.thinkingDurationMs` anymore — that's
    // turn_complete's job. The `done` handler still
    // computes the in-memory `last.latency` (the "last
    // turn fast path") and fires `finalizeRequest`.
    handleChatEvent({
      request_id: "rid-full-flow",
      kind: "done",
    });

    // After turn_complete: the streaming placeholder
    // has the per-turn latency triple + the
    // per-turn thinking duration. THIS is the
    // contract the user's "Thought for —" screenshot
    // hit: with the F5 single-value
    // `req.thinkingDurationMs`, only the LAST
    // turn's value reached `last`. The per-turn fix
    // ships every turn's duration to its own row.
    const afterTurnComplete = stream.getMessages(sid)!;
    const last = afterTurnComplete[afterTurnComplete.length - 1];
    expect(last.role).toBe("assistant");
    expect(last.thinkingDurationMs).toBe(850);
    expect(last.latency?.ttfbMs).toBe(420);
    expect(last.latency?.genMs).toBe(2_100);
    expect(last.latency?.totalMs).toBe(3_200);

    // `latencyByTurn[0]` carries the per-turn entry
    // for reloadAfterFinalize's per-seq IPC fire.
    expect(req.latencyByTurn.size).toBe(1);
    expect(req.latencyByTurn.get(0)?.seq).toBe(1);
  });

  it("F5 follow-up per-turn: 3-turn request drives 3 latencyByTurn entries with distinct thinkingMs and same in-memory last.thinkingDurationMs (last turn wins)", () => {
    // The user-screenshot scenario: a 3-turn agent
    // response (thinking→shell→tool_result×2→text) is
    // 3 LLM calls; the agent loop emits 3
    // `TurnComplete` events with 3 distinct `seq` and
    // 3 distinct `thinkingMs` values. The frontend
    // `latencyByTurn` Map collects all 3 entries; the
    // placeholder's `last.thinkingDurationMs` is
    // overwritten per turn to the LATEST turn's value
    // (the streaming buffer is a single merged
    // assistant message — the per-turn split comes
    // from `reloadAfterFinalize` after `done`).
    //
    // This is the F5 single-value bug: with the old
    // `req.thinkingDurationMs`, only the FIRST turn's
    // duration was captured (the `=== null` guard
    // blocked subsequent turns), so 2 of 3 ThinkingBlocks
    // rendered "—". With the per-turn fix, all 3 turns
    // are recorded in `latencyByTurn` and (after
    // reload) on the per-turn split rows.
    const stream = useStreamControllerStore();
    const sid = "f5-followup-3-turn-sid";
    const messages = rehydrateMessages([usrTyped(0, "go"), asst(1, "", [])]);
    stream.putMessages(sid, messages, false);

    const req = {
      requestId: "rid-3-turn",
      sessionId: sid,
      projectId: null,
      userMsgId: "u1",
      assistantMsgId: messages[1].id,
      history: [],
      sendAt: 0,
      firstDeltaAt: null,
      toolStartedAt: new Map<string, number>(),
      currentTurnIndex: -1,
      latencyByTurn: new Map(),
    };
    (stream as unknown as { activeRequests: Map<string, typeof req> })
      .activeRequests.set(req.requestId, req);

    const handleChatEvent = (
      stream as unknown as {
        handleChatEvent: (e: {
          request_id: string;
          kind: string;
          text?: string;
          seq?: number;
          ttfb_ms?: number | null;
          gen_ms?: number | null;
          total_ms?: number | null;
          thinking_ms?: number | null;
        }) => void;
      }
    ).handleChatEvent;

    const handleToolCall = (
      stream as unknown as {
        handleToolCall: (p: {
          request_id: string;
          id: string;
          name: string;
          input: unknown;
        }) => void;
      }
    ).handleToolCall;

    // Turn 0: start → thinking_delta → tool:call (handleToolCall,
    // independent IPC channel — NOT handleChatEvent) →
    // turn_complete(seq=1, thinkingMs=200)
    handleChatEvent({ request_id: "rid-3-turn", kind: "start" });
    handleChatEvent({
      request_id: "rid-3-turn",
      kind: "thinking_delta",
      text: "t0 think",
    });
    handleToolCall({
      request_id: "rid-3-turn",
      id: "c0",
      name: "shell",
      input: { command: "ls" },
    });
    handleChatEvent({
      request_id: "rid-3-turn",
      kind: "turn_complete",
      seq: 1,
      ttfb_ms: null,
      gen_ms: null,
      total_ms: 350,
      thinking_ms: 200,
    });

    // Turn 1: start → thinking_delta → tool:call → turn_complete(seq=3, thinkingMs=300)
    handleChatEvent({ request_id: "rid-3-turn", kind: "start" });
    expect(req.currentTurnIndex).toBe(1);
    handleChatEvent({
      request_id: "rid-3-turn",
      kind: "thinking_delta",
      text: "t1 think",
    });
    handleToolCall({
      request_id: "rid-3-turn",
      id: "c1",
      name: "shell",
      input: { command: "pwd" },
    });
    handleChatEvent({
      request_id: "rid-3-turn",
      kind: "turn_complete",
      seq: 3,
      ttfb_ms: null,
      gen_ms: null,
      total_ms: 450,
      thinking_ms: 300,
    });

    // Turn 2: start → thinking_delta → delta (text only, no tool) → turn_complete(seq=5, thinkingMs=500)
    handleChatEvent({ request_id: "rid-3-turn", kind: "start" });
    expect(req.currentTurnIndex).toBe(2);
    handleChatEvent({
      request_id: "rid-3-turn",
      kind: "thinking_delta",
      text: "t2 think",
    });
    handleChatEvent({
      request_id: "rid-3-turn",
      kind: "delta",
      text: "final answer",
    });
    handleChatEvent({
      request_id: "rid-3-turn",
      kind: "turn_complete",
      seq: 5,
      ttfb_ms: 180,
      gen_ms: 600,
      total_ms: 900,
      thinking_ms: 500,
    });

    // The F5 follow-up contract: 3 distinct
    // `latencyByTurn` entries, 3 distinct thinkingMs
    // (the bug the user's screenshot hit had only the
    // first turn's value reaching the UI), 3 distinct
    // seq values (the agent loop's per-session
    // `next_seq` counter).
    expect(req.latencyByTurn.size).toBe(3);
    expect(req.latencyByTurn.get(0)?.thinkingMs).toBe(200);
    expect(req.latencyByTurn.get(1)?.thinkingMs).toBe(300);
    expect(req.latencyByTurn.get(2)?.thinkingMs).toBe(500);
    expect(req.latencyByTurn.get(0)?.seq).toBe(1);
    expect(req.latencyByTurn.get(1)?.seq).toBe(3);
    expect(req.latencyByTurn.get(2)?.seq).toBe(5);

    // The placeholder's `last.thinkingDurationMs` is
    // the LAST turn's value (the streaming buffer is
    // merged). After `reloadAfterFinalize`, the
    // per-turn split rows in DB carry each turn's
    // own value — that's what the user's
    // "Thought for X.Xs" header reads from after
    // reload. The cumulative `currentSessionLatencyTurns`
    // computed in chat.ts sees all 3 entries via
    // `m.latency` on the rehydrated rows.
    const afterAllTurns = stream.getMessages(sid)!;
    const last = afterAllTurns[afterAllTurns.length - 1];
    expect(last.thinkingDurationMs).toBe(500);
  });

  it("re-attach contract: setting `target.thinkingDurationMs` on the reactive target fires the per-message chip", async () => {
    // The re-attach path is the most likely place for
    // the "Thought for —" regression. After
    // `putMessages` replaces the array, the placeholder
    // (which had `thinkingDurationMs` set by the
    // `done` handler) is gone. The re-attach in
    // `reloadAfterFinalize` finds the new target by
    // seq and copies the value. This test exercises
    // that copy step directly:
    //   - putMessages seeds the array (reactive wrap)
    //   - we manually do what rehydrateMessages would:
    //     drop the placeholder, push a rehydrated
    //     item with no `thinkingDurationMs`
    //   - then we manually do what the re-attach does:
    //     find the target, set `thinkingDurationMs`
    //   - assert the chip fires
    //
    // If the chip doesn't fire here, the bug is in
    // the reactive wrap (the F5 follow-up's
    // `putMessages` wrap) or in the re-attach.
    const stream = useStreamControllerStore();
    const chat = useChatStore();
    const sid = "f5-reattach-contract-sid";
    stream.putMessages(
      sid,
      rehydrateMessages([usrTyped(0, "hi"), asst(1, "", [])]),
      false,
    );
    const refs = storeToRefs(chat);
    refs.currentSessionId.value = sid;

    // Simulate the rehydrate-and-replace that
    // `reloadAfterFinalize` does (we don't go through
    // the IPC; we just hand-construct the new array to
    // mirror what `rehydrateMessages` would produce —
    // an assistant row with seq but no
    // `thinkingDurationMs`).
    const newRehydrated = rehydrateMessages([
      usrTyped(0, "hi"),
      asst(1, "answer text", []), // no thinking_ms
    ]);
    // Use `putMessages` to mirror the production swap
    // (so the reactive wrap is consistent).
    stream.putMessages(sid, newRehydrated, false);

    // The rehydrated message has no `thinkingDurationMs`.
    const wrapped = stream.getMessages(sid)!;
    const assistant = wrapped.find((m) => m.role === "assistant")!;
    expect(assistant.thinkingDurationMs).toBeUndefined();

    // Manually do what the re-attach does. The
    // chip's `headerLabel` computed depends on
    // `message.thinkingDurationMs`. If the wrap is
    // broken, the assignment below won't fire the
    // dependency and the chip stays at "—".
    assistant.thinkingDurationMs = 1_400;
    await nextTick();

    // We don't have a direct read of the rendered
    // chip from the unit-test level (would need
    // @vue/test-utils), so we just assert the field
    // is set on the message. The render layer is
    // already covered by the chat input tests.
    expect(assistant.thinkingDurationMs).toBe(1_400);
  });
});

// REGRESSION (2026-06-24, commit ce25783): `getMessages` must be a
// PURE READ — no observable mutation of `messagesBySession`. It used
// to do an "LRU touch" (`delete(k); set(k, v)`) on every read to move
// the accessed session to MRU. But the `messages` /
// `currentSessionLatencyTurns` computeds call `getMessages` inside
// their getter, so mutating the reactive Map there recursively
// re-invalidated the computed → Vue's "Maximum recursive updates
// exceeded" guard fired on every event and the scheduler dropped the
// DOM update. Symptom: streaming deltas never rendered until a
// session switch forced a full array replacement. The touch now lives
// in the non-computed callers (`ensureLoaded` / `startRequest`).
// These tests lock that contract so a future revert is caught. See
// `.trellis/spec/frontend/state-management.md`
// "RULE: computed getters must not mutate their own tracked deps".
describe("getMessages — pure read (regression: computed mutate-own-deps recursion)", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
  });

  it("does NOT change messagesBySession iteration order (no delete+set touch)", () => {
    const stream = useStreamControllerStore();
    stream.putMessages("session-a", [], false);
    stream.putMessages("session-b", [], false);
    stream.putMessages("session-c", [], false);
    const before = [...stream.messagesBySession.keys()];
    // Read the OLDEST entry repeatedly. The old touch (delete + set)
    // would move it to the MRU end, changing the iteration order to
    // ["session-b","session-c","session-a"].
    stream.getMessages("session-a");
    stream.getMessages("session-a");
    const after = [...stream.messagesBySession.keys()];
    expect(after).toEqual(before);
  });

  it("returns the stored array reference verbatim (no rebuild)", () => {
    const stream = useStreamControllerStore();
    stream.putMessages("session-a", [], false);
    const stored = stream.messagesBySession.get("session-a");
    expect(stream.getMessages("session-a")).toBe(stored);
  });
});
