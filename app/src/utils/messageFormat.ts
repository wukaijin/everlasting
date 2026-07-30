// Pure helpers for rendering message-shaped data. Extracted from
// ChatWindow.vue during the D3 decomposition so MessageItem /
// ThinkingBlock / ToolCallCard can import them without dragging in
// the ChatWindow component or Pinia stores.

import type {
  ToolCallInfo,
  ToolResultInfo,
  ThinkingBlockInfo,
} from "../stores/chat.types";

/** Pretty-print a tool call's input for display in the card. */
export function formatToolInput(tc: ToolCallInfo): string {
  return JSON.stringify(tc.input, null, 2);
}

/** Cap a tool result's rendered output to keep cards compact. The
 *  reader sees the first `max` chars plus a "more chars" suffix. */
export function truncateOutput(s: string, max = 500): string {
  if (s.length <= max) return s;
  return s.slice(0, max) + `… (${s.length - max} more chars)`;
}

/** Step 4 follow-up: the agent loop wraps every tool result in a
 *  JSON envelope `{ "result": "<legacy string>", "cwd": "<path>" }`
 *  so the LLM can see which on-disk cwd the tool ran against (REQ-16).
 *  The envelope is the LLM-facing contract — it round-trips through
 *  the DB and the outbound `toPayloadContent` so the model has cwd
 *  context on the next turn. The UI, however, doesn't want to
 *  render the raw envelope; it wants the original tool output
 *  string.
 *
 *  This helper is the bridge. It tries to parse `content` as the
 *  envelope shape and returns `result` if it matches; otherwise it
 *  returns the raw content unchanged (forward- and backward-compat
 *  with pre-follow-up sessions whose tool_result blocks are plain
 *  strings). The DB payload is left untouched — only the rendered
 *  string changes. */
export function extractToolResultDisplay(content: string): string {
  if (!content) return content;
  // Fast path: not even JSON-looking, skip the parse.
  if (content[0] !== "{") return content;
  try {
    const parsed = JSON.parse(content);
    if (
      parsed &&
      typeof parsed === "object" &&
      typeof parsed.result === "string" &&
      typeof parsed.cwd === "string"
    ) {
      return parsed.result;
    }
  } catch {
    // not JSON, fall through
  }
  return content;
}

/** Concatenated thinking text for display. Multiple blocks
 *  (interleaved thinking) are joined with a blank line so they read
 *  as separate reasoning phases. */
export function thinkingDisplayText(
  blocks: ThinkingBlockInfo[] | undefined,
): string {
  if (!blocks || blocks.length === 0) return "";
  return blocks.map((b) => b.text).join("\n\n");
}

// F5 follow-up: `estimateThinkingTokens` used to live here and
// was rendered in the ThinkingBlock header as "Thought for X
// tokens". Replaced with a wall-clock duration captured by the
// streaming `streamController` (see `RequestState.thinkingStartedAt`
// / `thinkingDurationMs`) — the user's "did this take a long
// time?" question is answered by time, not content size. The
// helper is removed because nothing imports it; if a future
// feature needs a token estimate for some other reason
// (cost-cap copy, etc.), reintroduce it then.

/** Find the matching tool_result for a given tool_use id on a
 *  message. The store's rehydrate path attaches user-message
 *  tool_results to the assistant message for UI grouping, so the
 *  lookup stays local to a single message. */
export function getToolResult(
  m: { toolResults?: ToolResultInfo[] },
  callId: string,
): ToolResultInfo | undefined {
  return m.toolResults?.find((r) => r.toolUseId === callId);
}

/** 交错思考(interleaved thinking): 判定一条消息是否是"真·用户输入"
 *  (开启一个新 agent run 的起点),用于 MessageList 的 `renderGroups`
 *  分组。判据(设计文档 §3.4):
 *    - role 必须是 user。
 *    - **不是** ghost user(tool_result 行):rehydrate 的 merge step 把
 *      user 行的 toolResults 复制到前一个 assistant 后,user 行自身的
 *      toolResults 仍在,所以 ghost user 带 toolResults。
 *    - **不是** orphan-repair synthetic:id 后缀 `-orphan-repair`
 *      (rehydrate 的 orphan-repair 步骤 splice 进来的合成消息)。
 *
 *  其余消息(assistant turn / ghost user / orphan-repair)归入当前 run。
 *  误判最坏后果只是"多分几个 run 气泡"(渲染层回退,不丢数据)。
 *
 *  只需 `id` / `role` / `toolResults` 字段,便于对任意 ChatMessage-like
 *  对象判定(分组、测试均可复用)。 */
export function isRealUserTurnStart(m: {
  id: string;
  role: "user" | "assistant";
  toolResults?: unknown[];
}): boolean {
  if (m.role !== "user") return false;
  // ghost user(tool_result):merge step 复制非移动,user 行仍带 toolResults。
  if (m.toolResults && m.toolResults.length > 0) return false;
  // orphan-repair synthetic:id 后缀。
  if (m.id.endsWith("-orphan-repair")) return false;
  return true;
}

/** Map a tool name to the CSS custom property that holds its
 *  accent color (the 3px left bar on a ToolCallCard). The tool list
 *  is closed for MVP (read_file / write_file / shell) so a plain
 *  switch reads cleaner than a registry. */
export function toolAccentVar(toolName: string): string {
  switch (toolName) {
    case "read_file":
      return "var(--color-tool-read)";
    case "write_file":
      return "var(--color-tool-write)";
    case "shell":
      return "var(--color-tool-shell)";
    default:
      return "var(--color-text-muted)";
  }
}

/** Map a tool name to an icon name (key in the Icon component's
 *  registry) shown in the card header. Defaults to a generic wrench
 *  for unknown tools so the UI never blanks out when a new tool lands
 *  before its icon is wired. */
export function toolIcon(toolName: string): string {
  switch (toolName) {
    case "read_file":
      return "document";
    case "write_file":
      return "pencil";
    case "shell":
      return "command-line";
    case "dispatch_subagent":
      // Worker subagent — `brain` carries the agent connotation
      // (this card spawns a worker agent; `wrench` is the generic
      // fallback for unknown tools and reads wrong here).
      return "brain";
    default:
      return "wrench";
  }
}
