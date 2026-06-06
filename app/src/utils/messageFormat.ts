// Pure helpers for rendering message-shaped data. Extracted from
// ChatWindow.vue during the D3 decomposition so MessageItem /
// ThinkingBlock / ToolCallCard can import them without dragging in
// the ChatWindow component or Pinia stores.

import type {
  ToolCallInfo,
  ToolResultInfo,
  ThinkingBlockInfo,
} from "../stores/chat";

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

/** Concatenated thinking text for display. Multiple blocks
 *  (interleaved thinking) are joined with a blank line so they read
 *  as separate reasoning phases. */
export function thinkingDisplayText(
  blocks: ThinkingBlockInfo[] | undefined,
): string {
  if (!blocks || blocks.length === 0) return "";
  return blocks.map((b) => b.text).join("\n\n");
}

/** Rough token estimate for the thinking header. Claude counts
 *  tokens closer to ~3.5 chars/token; we use length/4 as a
 *  conservative upper bound so the label is at least an order of
 *  magnitude right. */
export function estimateThinkingTokens(
  blocks: ThinkingBlockInfo[] | undefined,
): number {
  if (!blocks || blocks.length === 0) return 0;
  const totalChars = blocks.reduce((n, b) => n + b.text.length, 0);
  return Math.max(1, Math.round(totalChars / 4));
}

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
    default:
      return "wrench";
  }
}
