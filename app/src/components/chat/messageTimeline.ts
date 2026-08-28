// 时间轴渲染与 speaker chip 纯函数(拆分自 MessageItem.vue,
// 08-07-large-file-splitting)。入参 message,不读组件作用域。

import type { ChatMessage, ThinkingBlockInfo } from "../../stores/chat.types";
import { renderMarkdown } from "../../utils/markdown";
import { colorTagForName } from "../../utils/colorTag";

// ---------------------------------------------------------------------------
// 交错思考(interleaved thinking): 按 LLM 真实流式到达顺序排列
// thinking + text + tool_use 块的渲染时间轴。核心诉求 —— 让思考/文本/工具
// 按真实流序穿插(Claude.ai/Cursor 形态),而非旧的"思考扎堆顶 + 工具扎堆中"。
//
// 数据源优先级:
//   1. `message.contentBlocks`(reload 后由 rehydrate 从 DB content 数组按
//      原序透传;实时态由 streamController 就地 mutate —— 见 chat_loop.rs
//      ordered_blocks)。
//   2. 回退到分桶数组(thinkingBlocks + toolCalls + content)的固定顺序 ——
//      兼容旧消息(无 contentBlocks)。
//
// tool_use 在 timeline 内渲染 ToolCallCard(配对 getToolResult + 4 个 resolver
// 卡片),实现"工具穿插在思考/文本之间"。每个 thinking 块独立成渲染点,
// text 块各自 markdown 渲染。
//
// 与 msg__bubble / msg__tools 的关系: 走 contentBlocks 时间轴时(useTimeline
// 为真),文本 + 工具都由时间轴渲染,旧的 msg__bubble(文本)和 msg__tools
// (工具区)在 useTimeline 时隐藏(避免重复)。回退路径下两者仍旧行为。
// ---------------------------------------------------------------------------
export type TimelineItem =
  | { kind: "thinking"; blocks: ThinkingBlockInfo[] }
  | { kind: "text"; text: string; html: string }
  | { kind: "tool_use"; id: string; name: string; input: Record<string, unknown> };

export function buildTimeline(
  message: ChatMessage,
  fallbackHtml: string,
): TimelineItem[] {
  const m = message;
  if (m.contentBlocks && m.contentBlocks.length > 0) {
    const out: TimelineItem[] = [];
    for (const b of m.contentBlocks) {
      if (b.kind === "thinking") {
        // ContentBlockView(thinking) → ThinkingBlockInfo(去 kind)。
        out.push({
          kind: "thinking",
          blocks: [{ text: b.text, signature: b.signature }],
        });
      } else if (b.kind === "text" && b.text) {
        out.push({ kind: "text", text: b.text, html: renderMarkdown(b.text) });
      } else if (b.kind === "tool_use") {
        out.push({ kind: "tool_use", id: b.id, name: b.name, input: b.input });
      }
      // redacted_thinking / tool_result 不进 timeline(redacted 走顶部计数行;
      // tool_result 在 wire 上属 user-role,assistant contentBlocks 不含)。
    }
    return out;
  }
  // 回退: 分桶固定顺序(thinking 在前, text 在后)。与改造前观感一致。
  const out: TimelineItem[] = [];
  if (m.thinkingBlocks && m.thinkingBlocks.length) {
    out.push({ kind: "thinking", blocks: m.thinkingBlocks });
  }
  if (m.content) {
    out.push({ kind: "text", text: m.content, html: fallbackHtml });
  }
  return out;
}

/** 是否走 contentBlocks 时间轴(true → 文本由时间轴渲染,
 *  msg__bubble 只留 cursor/edited)。仅在 reload 后且有 contentBlocks
 *  时为真;实时流式态/旧消息为 false(走回退 + msg__bubble)。 */
export function shouldUseTimeline(message: ChatMessage): boolean {
  return (
    !!message.contentBlocks &&
    message.contentBlocks.length > 0 &&
    message.role === "assistant"
  );
}

// -----------------------------------------------------------------
// Group chat (07-29-group-chat, Phase 4 Step 4 TODO-F1/F2/F3):
// speaker chip rendering. The originating speaker is carried
// in `message.speaker` (round-tripped from `messages.speaker`
// column via `rehydrateMessages`). `undefined` for classic chat
// / subagent / review rows → no chip. `Some("moderator")` for
// the moderator's turns. `Some(<participant.name>)` for each
// participant's turns. The chip is rendered only on assistant
// rows (user rows are human by definition in any session type).
// -----------------------------------------------------------------
export function speakerLabelOf(message: ChatMessage): string {
  const s = message.speaker;
  if (!s) return "";
  if (s === "moderator") return "主持人";
  return s;
}

// Hash the speaker name into a palette index for the chip
// accent. The hash is a stable per-name function (so the same
// participant always gets the same color across reloads +
// sessions). Moderator gets a fixed "neutral" treatment (not
// palette-derived) so its role is visually distinct from the
// participants — the user knows who's arbitrating.
export function speakerAccentOf(message: ChatMessage): string {
  const s = message.speaker;
  if (!s) return ""; // v-if guard — not rendered
  if (s === "moderator") return "neutral";
  // djb2 hash → 0..7 palette index(实现在 utils/colorTag 共享)
  return `palette-${colorTagForName(s)}`;
}

export function showSpeakerChipFor(message: ChatMessage): boolean {
  return !!message.speaker && message.role === "assistant";
}