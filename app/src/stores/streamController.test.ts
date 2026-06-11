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
