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

import { describe, it, expect } from "vitest";
import { rehydrateMessages } from "./streamController";

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
