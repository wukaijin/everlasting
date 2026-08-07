// streamController 的 wire 重水合(拆分自 streamController.ts,
// 08-07-large-file-splitting)。纯函数:DB 载荷 → 内存 ChatMessage。

import { markRaw } from "vue";

import type { ChatMessage, InjectionEntry } from "./chat.types";

export interface LoadedMessage {
  id: number;
  session_id: string;
  role: "user" | "assistant";
  content: unknown;
  text: string;
  has_tool_calls: boolean;
  has_tool_results: boolean;
  created_at: string;
  seq: number;
  /** F5 (LLM Latency Tracking): per-message latency breakdown.
   *  All three are `null` for pre-F5 rows. Rehydrated into
   *  the assistant message's `latency` field; the
   *  `MessageItem` footer renders `totalMs` and the hover
   *  tooltip shows the three lines. */
  ttfb_ms: number | null;
  gen_ms: number | null;
  total_ms: number | null;
  /** F5 follow-up: thinking-phase wall-clock duration in ms.
   *  `null` for messages that never entered the thinking
   *  phase AND for pre-F5-follow-up rows. Rehydrated into
   *  the assistant message's `thinkingDurationMs` field;
   *  the `ThinkingBlock` header renders it as
   *  "Thought for X.Xs" (replacing the previous "X tokens"
   *  estimate). Persisted by `update_message_latency`'s
   *  new 4th-column UPDATE — same IPC, one extra bind. */
  thinking_ms: number | null;
  /** Group chat (07-29-group-chat, Phase 4 TODO-B/D3): the
   *  originating speaker for this message. Round-tripped
   *  from the Rust `MessageRow.speaker` column. `undefined`
   *  / `null` for classic chat / subagent / review messages
   *  (the pre-Phase 4 default + unchanged). For group-chat
   *  sessions, set to the fixed identifier `"moderator"` for
   *  moderator turns or the participant's `name` for
   *  participant turns. Rehydrated into the
   *  `ChatMessage.speaker` field; the `MessageItem` renders
   *  a chip + accent color when present. Optional in the
   *  type so pre-Phase-4 test fixtures without `speaker` still
   *  typecheck — the rehydrate path treats `undefined` as
   *  `null` (no chip). */
  speaker?: string | null;
  /** B2 PR3: optional per-user-turn injection manifest
   *  JSON, written by the agent loop's `update_message_metadata`
   *  SQL after `inject_at_tokens` produces the list. `null`
   *  for non-user rows AND for user rows without
   *  `@relpath` tokens. The rehydrate path parses this
   *  into `ChatMessage.injections` so the hint row
   *  survives a session reload. The shape is the same
   *  wire-format tagged-union as the live `FileInjections`
   *  event — see `InjectionEntry` / `InjectionRecord`.
   *  Optional in the type so existing test fixtures that
   *  don't model metadata still typecheck; the production
   *  IPC always sends `metadata` (NULL for non-user rows
   *  per `db::MessageRow::metadata`). */
  metadata?: unknown;
}

export interface LoadedSession {
  session: {
    id: string;
    title: string;
    created_at: string;
    updated_at: string;
    model: string;
    project_id: string;
    current_cwd: string;
    /** Step 4 follow-up: tri-state worktree state. The `none`
     *  default lets pre-follow-up + post-follow-up sessions
     *  load identically; the UI uses this to render the
     *  three-state worktree chip in ChatPanel. */
    worktree_state: "none" | "active" | "detached";
    worktree_path: string | null;
    last_worktree_path: string | null;
    /** PR4 of multi-model: per-session model override. `null`
     *  means the session uses the global default model. The
     *  ModelSelect popover in the chat input reads/writes this
     *  via the `update_session_model_id` IPC. */
    model_id: string | null;
    /** A4 (Token Usage Tracking): per-session cumulative
     *  token totals. `null` for pre-A4 sessions (the columns
     *  are nullable; a legacy session's first post-upgrade
     *  turn starts the counter from 0). The frontend uses
     *  these to render the ChatInput hint area's
     *  "14.2K · 7% / 200K" line. */
    input_tokens_total: number | null;
    output_tokens_total: number | null;
    cache_creation_total: number | null;
    cache_read_total: number | null;
    /** Group chat (07-29-group-chat, Phase 4 TODO-D1):
     *  session type discriminator. `"chat"` is the default
     *  (existing classic-chat + review + subagent sessions);
     *  `"group_chat"` is the new free-form multi-LLM
     *  session type. Backed by the `sessions.session_type`
     *  column (Phase 1 migration, default `"chat"`). Drives
     *  the chat entry-point branch in `chat_inner` (classic
     *  vs. `run_group_chat_loop`) + the frontend `SessionList`
     *  type badge + the `GroupChatConfigModal` access path. */
    session_type: "chat" | "group_chat";
    /** Group chat (07-29-group-chat, Phase 4 TODO-D1):
     *  per-session free-form JSON metadata. Classic chat
     *  sessions have `null` (no metadata). Group-chat
     *  sessions store `{participants: ParticipantConfig[]}`.
     *  Optional in the type so existing test fixtures that
     *  don't model group chat still typecheck; the production
     *  IPC always sends `metadata` (NULL for non-group-chat
     *  rows per `db::SessionRow::metadata`). */
    metadata?: Record<string, any> | null;
  };
  messages: LoadedMessage[];
}

// Lifted from chat.ts so the controller can own message shape
// without depending on chat.ts (which will in turn import the
// controller). Identical logic — kept here to break the cycle.
//
// Exported (re-exported as a named binding below) so the
// `streamController.test.ts` file can call it directly. The
// public Pinia store API does not re-export this function;
// callers should go through `ensureLoaded`.
export function rehydrateMessages(loaded: LoadedMessage[]): ChatMessage[] {
  const out: ChatMessage[] = loaded.map((m) => {
    const blocks = Array.isArray(m.content) ? (m.content as Array<Record<string, unknown>>) : [];
    const toolCalls: ChatMessage["toolCalls"] = [];
    const toolResults: ChatMessage["toolResults"] = [];
    const thinkingBlocks: ChatMessage["thinkingBlocks"] = [];
    const redactedThinkingData: string[] = [];
    // 交错思考: 按 DB content 数组原序构建 `contentBlocks`,供
    // `MessageRunGroup` 做流式渲染。与上面的分桶数组共享同一个遍历
    // 循环(避免二次遍历),每个有效分支同时 push 到分桶 + contentBlocks。
    // 注意 `continue`(坏块跳过)也会跳过 contentBlocks,保持一致。
    const contentBlocks: ChatMessage["contentBlocks"] = [];
    for (const b of blocks) {
      if (!b || typeof b.type !== "string") continue;
      if (b.type === "thinking") {
        thinkingBlocks.push({
          text: (b.thinking as string) ?? "",
          signature: (b.signature as string) ?? "",
        });
        contentBlocks.push({
          kind: "thinking",
          text: (b.thinking as string) ?? "",
          signature: (b.signature as string) ?? "",
        });
      } else if (b.type === "redacted_thinking" && typeof b.data === "string") {
        redactedThinkingData.push(b.data);
        contentBlocks.push({ kind: "redacted_thinking", data: b.data });
      } else if (
        b.type === "tool_use" &&
        typeof b.id === "string" &&
        typeof b.name === "string"
      ) {
        toolCalls.push({ id: b.id, name: b.name, input: (b.input as Record<string, unknown>) ?? {} });
        contentBlocks.push({
          kind: "tool_use",
          id: b.id,
          name: b.name,
          input: (b.input as Record<string, unknown>) ?? {},
        });
      } else if (b.type === "tool_result" && typeof b.tool_use_id === "string") {
        // F5: per-tool duration is embedded in the tool_result
        // block as `duration_ms` (per R2 / ADR-lite decision 1).
        // Read it here so the ToolCallCard can display "0.3s"
        // on reload. Pre-F5 blocks (no `duration_ms` field) leave
        // it `undefined` → the card renders nothing.
        const durationRaw = b.duration_ms;
        const durationMs =
          typeof durationRaw === "number" && Number.isFinite(durationRaw)
            ? Math.max(0, Math.round(durationRaw))
            : undefined;
        toolResults.push({
          toolUseId: b.tool_use_id,
          content: (b.content as string) ?? "",
          isError: !!b.is_error,
          ...(durationMs !== undefined ? { durationMs } : {}),
        });
        contentBlocks.push({
          kind: "tool_result",
          toolUseId: b.tool_use_id,
          content: (b.content as string) ?? "",
          isError: !!b.is_error,
          ...(durationMs !== undefined ? { durationMs } : {}),
        });
      } else if (b.type === "text" && typeof b.text === "string") {
        // 交错思考: text 块也透传(后端流序落库后,一个 turn 可能有
        // 多个 text 块——思考夹在两段文本之间时)。
        contentBlocks.push({ kind: "text", text: b.text });
      }
    }
    const msg: ChatMessage = {
      id: `${m.session_id}-${m.seq}`,
      role: m.role,
      content: m.text,
    };
    if (toolCalls.length) msg.toolCalls = toolCalls;
    if (toolResults.length) msg.toolResults = toolResults;
    if (thinkingBlocks.length) msg.thinkingBlocks = thinkingBlocks;
    if (redactedThinkingData.length) msg.redactedThinkingData = redactedThinkingData;
    // 交错思考: 仅当有透传块时挂上(避免给纯文本 user 消息挂空数组)。
    if (contentBlocks.length) msg.contentBlocks = contentBlocks;
    // F5: per-message latency. All three fields are nullable
    // in the DB; only the assistant rows that ran an LLM turn
    // will have non-null values. We attach `latency` only when
    // at least one field is present, so the UI can use the
    // presence-check (`m.latency && m.latency.totalMs`) to
    // distinguish "—" from "0.0s" (which is a real value
    // — extremely fast local proxy).
    const hasLatency =
      m.ttfb_ms !== null ||
      m.gen_ms !== null ||
      m.total_ms !== null;
    if (hasLatency) {
      msg.latency = {
        ...(m.ttfb_ms !== null ? { ttfbMs: m.ttfb_ms } : {}),
        ...(m.gen_ms !== null ? { genMs: m.gen_ms } : {}),
        ...(m.total_ms !== null ? { totalMs: m.total_ms } : {}),
      };
    }
    // F5 follow-up: thinking-phase wall-clock. Mirrors the
    // `latency` triple's "only set if at least one field is
    // present" rule — the ThinkingBlock header uses the
    // `thinkingDurationMs !== undefined` presence check to
    // distinguish "—" from "0.0s" (a real, extremely fast
    // local-proxy value). Pre-F5-follow-up rows have the
    // column NULL and fall through to undefined, which the
    // UI renders as "—" — the same fallback the in-memory
    // path used before this persistence work.
    if (m.thinking_ms !== null) {
      msg.thinkingDurationMs = m.thinking_ms;
    }
    // B2 PR3: parse the `metadata` JSON into the
    // `injections` field. The agent loop wrote the
    // per-user-turn injection manifest here via
    // `update_message_metadata` (see
    // `db::sessions::update_message_metadata`); a
    // `null` / missing / non-array metadata is the
    // "no @relpath tokens" case and is rendered
    // as no hint row. The `action` object's shape
    // is the same wire-format tagged union as
    // the live `FileInjections` event — we
    // narrow with the same `kind` discriminator.
    if (m.metadata !== null && m.metadata !== undefined) {
      const meta = m.metadata as { injections?: unknown };
      if (Array.isArray(meta.injections)) {
        // Defensive: skip entries that don't have
        // the {path, action} shape — DB writes can
        // outlive the schema. Real entries are
        // typed via `InjectionEntry`; we just
        // assign the parsed array directly.
        const entries: InjectionEntry[] = [];
        for (const r of meta.injections) {
          if (
            r &&
            typeof r === "object" &&
            typeof (r as { path?: unknown }).path === "string" &&
            (r as { action?: unknown }).action &&
            typeof (r as { action?: { kind?: unknown } }).action?.kind ===
              "string"
          ) {
            entries.push(r as InjectionEntry);
          }
        }
        if (entries.length > 0) {
          msg.injections = entries;
        }
      }
      // D3 PR3 (2026-06-17): also surface the raw metadata
      // object on the in-memory message so MessageItem can
      // render the "(edited)" label when `metadata.edited_at`
      // is present. The shape is loosely typed (Record<string,
      // unknown>) so future metadata fields don't require
      // touching this rehydrate site. We attach the parsed
      // object verbatim — the same JSON the agent loop
      // persisted via `edit_user_message` (see
      // `.trellis/spec/backend/database-guidelines.md`
      // "Pattern: `edit_user_message`" — `metadata` shape is
      // `{ edited_at, original_content? }`).
      msg.metadata = meta;
    }
    // The `seq` is plumbed through for the F5
    // `update_message_latency` IPC. The streaming path tracks
    // it on `RequestState` instead (the seq is the agent
    // loop's handle, not the controller's).
    msg.seq = m.seq;
    // Group chat (07-29-group-chat, Phase 4 TODO-D3): pass the
    // speaker through verbatim. `m.speaker` is `undefined` /
    // `null` for classic chat / subagent / review rows
    // (pre-Phase 4 behavior) and rehydrates into `msg.speaker`
    // as undefined — the UI's `v-if="message.speaker"` chip
    // condition naturally skips these. For group-chat rows,
    // carries the moderator's `"moderator"` identifier or the
    // participant's user-visible name; the `MessageItem`
    // renders the corresponding chip + accent color.
    if (m.speaker) {
      msg.speaker = m.speaker;
    }
    return msg;
  });
  // Merge user-message tool_results into the previous assistant
  // message for the UI's "done / running" lookup (see chat.ts for
  // the long version of this comment).
  for (let i = 0; i < out.length; i++) {
    const m = out[i];
    if (m.role !== "user" || !m.toolResults?.length) continue;
    for (let j = i - 1; j >= 0; j--) {
      if (out[j].role === "assistant") {
        if (!out[j].toolResults) out[j].toolResults = [];
        out[j].toolResults!.push(...m.toolResults!);
        break;
      }
    }
  }
  // BUG FIX (2013 tool_use orphan, frontend rehydrate side): the
  // backend's `chat` command used to (pre-fix) return on cancel
  // *after* persisting the assistant turn with `tool_use` blocks
  // but *before* persisting the corresponding `user(tool_result)`
  // turn. The DB ended up with an orphan `tool_use` and the next
  // `send()` built a history where `tool_use` had no follow-up
  // `tool_result` — Anthropic API 2013 ("tool call result does
  // not follow tool call"). The backend now persists a synthetic
  // `tool_result` on cancel (see `build_synthetic_tool_result_message`
  // in `app/src-tauri/src/lib.rs`), so *new* orphans stop
  // appearing. This step repairs **historical** orphans sitting
  // in the DB from before that fix.
  //
  // We splice in a synthetic user-role message with one
  // `tool_result` block per orphan `tool_use` id, immediately
  // after the orphan assistant. The merge step above does NOT
  // cover this case: it only moves `tool_result` data from a
  // user message that already has it onto the *preceding*
  // assistant. An orphan `tool_use` is the inverse — an
  // assistant `tool_use` with no following user `tool_result`
  // at all.
  //
  // Reverse scan so the splice-in's index shift doesn't
  // affect the next iteration (splicing at `i + 1` shifts
  // `i + 1` to `i + 2`, but the loop is going down so we
  // won't visit `i + 2` again).
  for (let i = out.length - 1; i >= 0; i--) {
    const m = out[i];
    if (m.role !== "assistant" || !m.toolCalls?.length) continue;
    // Set of `tool_use_id`s already paired with a `tool_result`,
    // either by the merge step (results copied onto this
    // assistant from a later user message) or by the *next*
    // message in the post-merge array carrying its own
    // `toolResults`. Both sources are checked because the
    // merge step *copies* (does not move) toolResults, so
    // a user message that the merge step drained for a
    // *different* preceding assistant can still have its
    // own (now-empty after merge) toolResults field — but
    // for our purposes the post-merge view of the assistant
    // plus the immediate next message's toolResults covers
    // every "did the wire get a result" question.
    const coveredIds = new Set<string>();
    for (const tr of m.toolResults ?? []) coveredIds.add(tr.toolUseId);
    const next = i + 1 < out.length ? out[i + 1] : null;
    if (next && next.role === "user") {
      for (const tr of next.toolResults ?? []) coveredIds.add(tr.toolUseId);
    }
    const orphanCalls = m.toolCalls.filter((tc) => !coveredIds.has(tc.id));
    if (orphanCalls.length === 0) continue;
    const syntheticMsg: ChatMessage = {
      // Distinct id so subsequent `send()`s that build a fresh
      // `userMsg` / `assistantMsg` placeholder don't collide
      // with this synthetic. The `id` is internal to the
      // store / `controller` filter logic — it never reaches
      // the LLM wire.
      id: `${m.id}-orphan-repair`,
      role: "user",
      content: "",
      toolResults: orphanCalls.map((tc) => ({
        toolUseId: tc.id,
        // Same wording as `build_synthetic_tool_result_message`
        // in `lib.rs` so the LLM sees a consistent shape on
        // the live-cancel and the historical-repair paths.
        // English + tool name (per PRD ADR-lite decision).
        content: `Tool execution was interrupted: the user stopped the request or the session was cancelled before the tool could run. The tool ${tc.name} did not run.`,
        isError: true,
      })),
    };
    out.splice(i + 1, 0, syntheticMsg);
    // Mirror the merge step's UI-grouping behavior: push the
    // synthetic toolResults onto the assistant message so the
    // UI's "tool just finished" lookup on the assistant
    // message surface the synthetic results too. Mirrors
    // `out[j].toolResults!.push(...m.toolResults!)` in the
    // merge loop above.
    if (!m.toolResults) m.toolResults = [];
    m.toolResults.push(
      ...syntheticMsg.toolResults!.map((tr) => ({
        toolUseId: tr.toolUseId,
        content: tr.content,
        isError: tr.isError,
      })),
    );
  }
  // After the merge step, the four "deep payload" arrays on every
  // message (toolCalls / toolResults / thinkingBlocks /
  // redactedThinkingData) are immutable for the lifetime of this
  // message — they were built from the DB once, and nothing in
  // this store will ever push into them again. Mark them raw so
  // the reactive Map's deep-proxy does not wrap them (and the
  // ToolCallInfo / ThinkingBlockInfo items inside them) on every
  // access. For a 5000-message session this is the difference
  // between ~10k proxy operations at first render and zero.
  //
  // We do NOT markRaw the message itself, the `content` string, or
  // the `streaming` / `error` fields — those are the per-message
  // mutables that still need reactive updates (see the streaming
  // path below for the parallel markRaw that fires when a fresh
  // message's stream ends).
  for (const m of out) {
    if (m.toolCalls) markRaw(m.toolCalls);
    if (m.toolResults) markRaw(m.toolResults);
    if (m.thinkingBlocks) markRaw(m.thinkingBlocks);
    if (m.redactedThinkingData) markRaw(m.redactedThinkingData);
    // B2 PR3: `injections` is also immutable post-rehydrate
    // — the live `FileInjections` event patches the
    // user message *during* the request, not after a
    // reload. Marking it raw skips the deep proxy wrap
    // for the array and its entries (the cost is small
    // per turn but adds up for sessions with many
    // @file mentions across many turns).
    if (m.injections) markRaw(m.injections);
  }
  return out;
}
