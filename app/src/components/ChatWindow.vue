<script setup lang="ts">
import { ref, nextTick, watch, computed, onMounted } from "vue";
import { useChatStore, type ToolCallInfo, type ToolResultInfo, type ThinkingBlockInfo } from "../stores/chat";
import { useConfigStore } from "../stores/config";

const store = useChatStore();
const config = useConfigStore();
const input = ref("");
const messagesEl = ref<HTMLElement | null>(null);

onMounted(async () => {
  config.load();
  await store.loadSessions();
});

const isComposing = ref(false);

// IME-safe textarea binding.
function onTextareaInput(e: Event) {
  if (isComposing.value) return;
  input.value = (e.target as HTMLTextAreaElement).value;
}

function onCompositionStart() {
  isComposing.value = true;
}

function onCompositionEnd(_e: CompositionEvent) {
  isComposing.value = false;
  input.value = (_e.target as HTMLTextAreaElement).value;
}

async function scrollToBottom() {
  await nextTick();
  if (messagesEl.value) {
    messagesEl.value.scrollTop = messagesEl.value.scrollHeight;
  }
}

watch(
  () =>
    store.messages
      .map(
        (m) =>
          m.content +
          (m.toolCalls?.length ?? 0) +
          (m.toolResults?.length ?? 0) +
          // Track thinking stream length + redacted count so the chat
          // auto-scrolls while a long thinking block streams in.
          (m.thinkingBlocks?.reduce((n, b) => n + b.text.length, 0) ?? 0) +
          (m.redactedThinkingData?.length ?? 0),
      )
      .join("|"),
  () => scrollToBottom(),
);

watch(
  () => store.messages.length,
  () => scrollToBottom(),
);

async function onSubmit() {
  const text = input.value;
  if (!text.trim() || store.sending) return;
  input.value = "";
  await store.send(text);
}

function onKeydown(e: KeyboardEvent) {
  if (e.key === "Enter" && !e.shiftKey && !isComposing.value) {
    e.preventDefault();
    onSubmit();
  }
}

const hasMessages = computed(() => store.messages.length > 0);

/** Filter out "ghost" messages that exist only to carry tool_result
 *  blocks for the LLM. After rehydration, the user tool_result message
 *  is empty (`text=""`) and has no tool_calls of its own — the actual
 *  tool card lives on the previous assistant message. Hiding them keeps
 *  the chat list clean.
 *
 *  An assistant message with only thinking blocks (no text, no tool
 *  calls, no error) is NOT a ghost — show it so the user can see the
 *  thinking even if the model never produced a visible reply. */
const visibleMessages = computed(() =>
  store.messages.filter(
    (m) =>
      m.content ||
      m.toolCalls?.length ||
      m.error ||
      // Persisted redacted_thinking-only assistant messages (extremely
      // rare but possible if a turn was pure redacted reasoning) should
      // still be visible. Pure "thinking-only" messages are also useful
      // to display when the model thought but didn't answer.
      (m.thinkingBlocks && m.thinkingBlocks.length > 0) ||
      (m.redactedThinkingData && m.redactedThinkingData.length > 0),
  ),
);

function getToolResult(m: { toolResults?: ToolResultInfo[] }, callId: string): ToolResultInfo | undefined {
  return m.toolResults?.find((r) => r.toolUseId === callId);
}

function formatToolInput(tc: ToolCallInfo): string {
  return JSON.stringify(tc.input, null, 2);
}

function truncateOutput(s: string, max = 500): string {
  if (s.length <= max) return s;
  return s.slice(0, max) + `… (${s.length - max} more chars)`;
}

/** Concatenated thinking text for display. Multiple blocks (interleaved
 *  thinking) are joined with a blank line so they read as separate
 *  reasoning phases. */
function thinkingDisplayText(blocks: ThinkingBlockInfo[] | undefined): string {
  if (!blocks || blocks.length === 0) return "";
  return blocks.map((b) => b.text).join("\n\n");
}

/** Rough token estimate for the thinking header. Claude counts tokens
 *  closer to ~3.5 chars/token; we use length/4 as a conservative upper
 *  bound so the label "Thought for N tokens" is at least an order of
 *  magnitude right. */
function estimateThinkingTokens(blocks: ThinkingBlockInfo[] | undefined): number {
  if (!blocks || blocks.length === 0) return 0;
  const totalChars = blocks.reduce((n, b) => n + b.text.length, 0);
  return Math.max(1, Math.round(totalChars / 4));
}

async function onNewSession() {
  if (store.sending) return;
  await store.createNewSession();
}

async function onSwitchSession(id: string) {
  if (id === store.currentSessionId) return;
  await store.switchSession(id);
}

async function onDeleteSession(id: string, e: MouseEvent) {
  e.stopPropagation();
  if (store.sending && id === store.currentSessionId) return;
  if (!confirm("删除此 session 及其所有消息？")) return;
  await store.deleteSession(id);
}
</script>

<template>
  <div class="app">
    <aside class="sidebar">
      <div class="sidebar__header">
        <span class="sidebar__title">Sessions</span>
      </div>
      <button class="sidebar__new" @click="onNewSession">+ 新对话</button>
      <ul class="sidebar__list">
        <li
          v-for="s in store.sessions"
          :key="s.id"
          :class="['session-item', { 'session-item--active': s.id === store.currentSessionId }]"
          @click="onSwitchSession(s.id)"
        >
          <div class="session-item__main">
            <div class="session-item__title">{{ s.title }}</div>
            <div v-if="s.preview" class="session-item__preview">{{ s.preview }}</div>
          </div>
          <button class="session-item__delete" @click="(e) => onDeleteSession(s.id, e)" title="删除">×</button>
        </li>
        <li v-if="store.sessions.length === 0" class="session-empty">
          还没有对话，点上方按钮开始
        </li>
      </ul>
    </aside>

    <section class="content">
      <header class="app__header">
        <h1 class="app__title">Everlasting</h1>
        <span class="app__subtitle">vibe coding workbench · step 3a</span>
      </header>

      <main ref="messagesEl" class="app__main">
        <div v-if="!hasMessages" class="empty">
          <p>输入一句话,跟 LLM 聊聊看 👋</p>
          <p class="empty__hint">中文输入测试 + 流式响应 + 工具调用</p>
        </div>

        <ul v-else class="messages">
          <li
            v-for="m in visibleMessages"
            :key="m.id"
            :class="['msg', `msg--${m.role}`, { 'msg--err': m.error }]"
          >
            <details
              v-if="m.role === 'assistant' && m.thinkingBlocks && m.thinkingBlocks.length"
              class="msg__thinking"
            >
              <summary class="msg__thinking-summary">
                <span class="msg__thinking-icon">💭</span>
                <span>Thought for {{ estimateThinkingTokens(m.thinkingBlocks) }} tokens</span>
                <span v-if="m.thinkingBlocks.length > 1" class="msg__thinking-count">
                  · {{ m.thinkingBlocks.length }} blocks
                </span>
                <span v-if="m.streaming && !m.content" class="msg__thinking-streaming">streaming…</span>
              </summary>
              <pre class="msg__thinking-body">{{ thinkingDisplayText(m.thinkingBlocks) }}</pre>
            </details>

            <div
              v-if="m.redactedThinkingData && m.redactedThinkingData.length"
              class="msg__redacted"
              :title="`${m.redactedThinkingData.length} redacted thinking block(s); preserved verbatim for the LLM but not displayable`"
            >
              🔒 {{ m.redactedThinkingData.length }} redacted thinking block{{ m.redactedThinkingData.length === 1 ? '' : 's' }} (preserved for LLM)
            </div>

            <div
              v-if="m.toolCalls && m.toolCalls.length"
              class="msg__tools"
            >
              <div
                v-for="tc in m.toolCalls"
                :key="tc.id"
                class="tool-card"
                :class="{ 'tool-card--error': getToolResult(m, tc.id)?.isError }"
              >
                <div class="tool-card__header">
                  <span class="tool-card__name">{{ tc.name }}</span>
                  <span class="tool-card__status">
                    {{ getToolResult(m, tc.id) ? '✓ done' : '⏳ running…' }}
                  </span>
                </div>
                <details class="tool-card__details">
                  <summary>input</summary>
                  <pre class="tool-card__pre">{{ formatToolInput(tc) }}</pre>
                </details>
                <details v-if="getToolResult(m, tc.id)" class="tool-card__details" open>
                  <summary>output</summary>
                  <pre class="tool-card__pre">{{ truncateOutput(getToolResult(m, tc.id)!.content) }}</pre>
                </details>
              </div>
            </div>

            <div
              v-if="m.content || (!m.toolCalls?.length && !m.toolResults?.length && !m.thinkingBlocks?.length && !m.redactedThinkingData?.length)"
              class="msg__bubble"
            >
              <span class="msg__text">{{ m.content }}</span>
              <span v-if="m.streaming" class="msg__cursor">▍</span>
            </div>

            <div v-if="m.error" class="msg__error">
              ⚠ {{ m.error.message }}
            </div>
          </li>
        </ul>
      </main>

      <footer class="app__footer">
        <textarea
          :value="input"
          class="input"
          rows="2"
          placeholder="输入消息,Enter 发送,Shift+Enter 换行"
          :disabled="store.sending"
          @input="onTextareaInput"
          @compositionstart="onCompositionStart"
          @compositionend="onCompositionEnd"
          @keydown="onKeydown"
        />
        <button
          class="send"
          :disabled="store.sending || !input.trim()"
          @click="onSubmit"
        >
          {{ store.sending ? "生成中…" : "发送" }}
        </button>
      </footer>

      <div v-if="config.loaded" class="statusbar" :class="{ 'statusbar--warn': !config.configured }">
        <span class="statusbar__dot" />
        <span class="statusbar__model">{{ config.model || "(no model)" }}</span>
        <span class="statusbar__sep">·</span>
        <span class="statusbar__url">{{ config.baseUrl || "(no base_url)" }}</span>
        <span v-if="!config.configured" class="statusbar__hint">ANTHROPIC_API_KEY 未设置</span>
      </div>
    </section>
  </div>
</template>

<style scoped>
.app {
  display: flex;
  height: 100vh;
  background: #fafbfc;
}

/* --- Sidebar --- */

.sidebar {
  width: 260px;
  flex-shrink: 0;
  display: flex;
  flex-direction: column;
  background: #f3f4f6;
  border-right: 1px solid #e5e7eb;
  overflow: hidden;
}

.sidebar__header {
  padding: 14px 16px 8px;
}

.sidebar__title {
  font-size: 12px;
  font-weight: 600;
  color: #6b7280;
  text-transform: uppercase;
  letter-spacing: 0.05em;
}

.sidebar__new {
  margin: 0 12px 8px;
  padding: 8px 12px;
  border: 1px solid #d1d5db;
  border-radius: 6px;
  background: #ffffff;
  color: #1f2328;
  font-size: 13px;
  font-weight: 500;
  cursor: pointer;
  text-align: left;
  transition: background 0.15s;
}

.sidebar__new:hover {
  background: #f9fafb;
  border-color: #9ca3af;
}

.sidebar__list {
  list-style: none;
  margin: 0;
  padding: 0 8px 8px;
  overflow-y: auto;
  flex: 1;
}

.session-item {
  display: flex;
  align-items: flex-start;
  gap: 4px;
  padding: 8px 10px;
  margin-bottom: 2px;
  border-radius: 6px;
  cursor: pointer;
  transition: background 0.1s;
}

.session-item:hover {
  background: #e5e7eb;
}

.session-item--active {
  background: #ffffff;
  border: 1px solid #d1d5db;
}

.session-item--active:hover {
  background: #ffffff;
}

.session-item__main {
  flex: 1;
  min-width: 0;
}

.session-item__title {
  font-size: 13px;
  font-weight: 500;
  color: #1f2328;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.session-item__preview {
  font-size: 11px;
  color: #6b7280;
  margin-top: 2px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.session-item__delete {
  flex-shrink: 0;
  width: 20px;
  height: 20px;
  border: none;
  border-radius: 4px;
  background: transparent;
  color: #9ca3af;
  font-size: 16px;
  line-height: 1;
  cursor: pointer;
  opacity: 0;
  transition: all 0.1s;
}

.session-item:hover .session-item__delete {
  opacity: 1;
}

.session-item__delete:hover {
  background: #fca5a5;
  color: #ffffff;
}

.session-empty {
  padding: 16px 12px;
  font-size: 12px;
  color: #9ca3af;
  text-align: center;
}

/* --- Content (right side) --- */

.content {
  flex: 1;
  display: flex;
  flex-direction: column;
  min-width: 0;
}

.app__header {
  display: flex;
  align-items: baseline;
  gap: 12px;
  padding: 14px 20px;
  border-bottom: 1px solid #e5e7eb;
  background: #ffffff;
}

.app__title {
  margin: 0;
  font-size: 16px;
  font-weight: 600;
  color: #111827;
}

.app__subtitle {
  font-size: 12px;
  color: #6b7280;
}

.app__main {
  flex: 1;
  overflow-y: auto;
  padding: 20px;
}

.empty {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  height: 100%;
  color: #6b7280;
  text-align: center;
}

.empty p {
  margin: 4px 0;
}

.empty__hint {
  font-size: 12px;
  color: #9ca3af;
}

.messages {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 12px;
}

.msg {
  display: flex;
  flex-direction: column;
  max-width: 75%;
}

.msg--user {
  align-self: flex-end;
}

.msg--assistant {
  align-self: flex-start;
}

.msg__bubble {
  padding: 10px 14px;
  border-radius: 12px;
  white-space: pre-wrap;
  word-break: break-word;
  line-height: 1.6;
}

.msg--user .msg__bubble {
  background: #2563eb;
  color: #ffffff;
  border-bottom-right-radius: 2px;
}

.msg--assistant .msg__bubble {
  background: #ffffff;
  color: #1f2328;
  border: 1px solid #e5e7eb;
  border-bottom-left-radius: 2px;
}

.msg--err .msg__bubble {
  border-color: #fca5a5;
  background: #fef2f2;
}

.msg__cursor {
  display: inline-block;
  margin-left: 2px;
  animation: blink 1s steps(1) infinite;
  color: #6b7280;
}

@keyframes blink {
  50% {
    opacity: 0;
  }
}

.msg__error {
  margin-top: 4px;
  padding: 0 14px;
  font-size: 12px;
  color: #b91c1c;
}

.msg__thinking {
  margin-bottom: 6px;
  max-width: 100%;
}

.msg__thinking-summary {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 4px 10px;
  background: #f9fafb;
  border: 1px solid #e5e7eb;
  border-radius: 999px;
  font-size: 11px;
  color: #6b7280;
  cursor: pointer;
  user-select: none;
  list-style: none;
  font-family: ui-monospace, SFMono-Regular, "SF Mono", Consolas, monospace;
  transition: background 0.1s, border-color 0.1s;
}

.msg__thinking-summary::-webkit-details-marker {
  display: none;
}

.msg__thinking-summary:hover {
  background: #f3f4f6;
  border-color: #d1d5db;
}

.msg__thinking[open] .msg__thinking-summary {
  background: #f3f4f6;
  border-color: #d1d5db;
  border-bottom-left-radius: 0;
  border-bottom-right-radius: 0;
  border-bottom-color: transparent;
}

.msg__thinking-icon {
  font-size: 12px;
}

.msg__thinking-count {
  color: #9ca3af;
}

.msg__thinking-streaming {
  margin-left: 2px;
  color: #2563eb;
  font-weight: 500;
}

.msg__thinking-body {
  margin: 0;
  padding: 10px 12px;
  background: #f9fafb;
  border: 1px solid #e5e7eb;
  border-top: none;
  border-radius: 0 0 8px 8px;
  white-space: pre-wrap;
  word-break: break-word;
  font-size: 12px;
  line-height: 1.6;
  color: #374151;
  font-family: ui-monospace, SFMono-Regular, "SF Mono", Consolas, monospace;
  max-height: 360px;
  overflow-y: auto;
}

.msg__redacted {
  margin-bottom: 6px;
  padding: 4px 10px;
  background: #f3f4f6;
  border: 1px dashed #d1d5db;
  border-radius: 6px;
  font-size: 11px;
  color: #6b7280;
  font-family: ui-monospace, SFMono-Regular, "SF Mono", Consolas, monospace;
}

.msg__tools {
  display: flex;
  flex-direction: column;
  gap: 6px;
  margin-top: 8px;
  max-width: 100%;
}

.tool-card {
  background: #f3f4f6;
  border: 1px solid #e5e7eb;
  border-radius: 8px;
  padding: 8px 12px;
  font-size: 12px;
  font-family: ui-monospace, SFMono-Regular, "SF Mono", Consolas, monospace;
}

.tool-card--error {
  border-color: #fca5a5;
  background: #fef2f2;
}

.tool-card__header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
}

.tool-card__name {
  font-weight: 600;
  color: #374151;
}

.tool-card--error .tool-card__name {
  color: #b91c1c;
}

.tool-card__status {
  font-size: 11px;
  color: #6b7280;
}

.tool-card__details {
  margin-top: 4px;
}

.tool-card__details summary {
  cursor: pointer;
  color: #6b7280;
  font-size: 11px;
  user-select: none;
}

.tool-card__pre {
  margin: 4px 0 0;
  padding: 6px 8px;
  background: #ffffff;
  border: 1px solid #e5e7eb;
  border-radius: 4px;
  white-space: pre-wrap;
  word-break: break-all;
  max-height: 200px;
  overflow-y: auto;
  font-size: 11px;
  line-height: 1.4;
  color: #1f2328;
}

.app__footer {
  display: flex;
  gap: 8px;
  padding: 12px 20px 16px;
  border-top: 1px solid #e5e7eb;
  background: #ffffff;
}

.input {
  flex: 1;
  resize: none;
  padding: 10px 12px;
  border: 1px solid #d1d5db;
  border-radius: 8px;
  background: #ffffff;
  outline: none;
  font-family: inherit;
  font-size: 14px;
  line-height: 1.5;
}

.input:focus {
  border-color: #2563eb;
  box-shadow: 0 0 0 3px rgba(37, 99, 235, 0.15);
}

.input:disabled {
  background: #f3f4f6;
  color: #9ca3af;
}

.send {
  padding: 0 18px;
  border: none;
  border-radius: 8px;
  background: #2563eb;
  color: #ffffff;
  font-weight: 500;
  cursor: pointer;
  transition: background 0.15s;
}

.send:hover:not(:disabled) {
  background: #1d4ed8;
}

.send:disabled {
  background: #93c5fd;
  cursor: not-allowed;
}

.statusbar {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 6px 16px;
  background: #f3f4f6;
  border-top: 1px solid #e5e7eb;
  font-size: 11px;
  color: #6b7280;
  font-family: ui-monospace, SFMono-Regular, "SF Mono", Consolas, monospace;
}

.statusbar--warn {
  background: #fef3c7;
  color: #92400e;
}

.statusbar__dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: #10b981;
}

.statusbar--warn .statusbar__dot {
  background: #f59e0b;
}

.statusbar__model {
  font-weight: 500;
}

.statusbar__sep {
  color: #9ca3af;
}

.statusbar__url {
  color: inherit;
  opacity: 0.85;
}

.statusbar__hint {
  margin-left: auto;
  color: #b45309;
  font-weight: 500;
}
</style>
