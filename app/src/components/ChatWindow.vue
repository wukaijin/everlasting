<script setup lang="ts">
import { ref, nextTick, watch, computed, onMounted } from "vue";
import {
  useChatStore,
  type ToolCallInfo,
  type ToolResultInfo,
  type ThinkingBlockInfo,
} from "../stores/chat";
import { useConfigStore } from "../stores/config";
import { useProjectsStore } from "../stores/projects";
import ProjectTabs from "./ProjectTabs.vue";
import SessionList from "./SessionList.vue";

const store = useChatStore();
const config = useConfigStore();
const projectsStore = useProjectsStore();
const input = ref("");
const messagesEl = ref<HTMLElement | null>(null);

onMounted(async () => {
  config.load();
  await projectsStore.loadProjects();
  // Restore last active project (Q1 / PROPOSAL §5.5). The chat
  // store's watcher in `chat.ts` will load sessions for the
  // selected project; we just need to choose which one.
  const lastId = config.lastActiveProjectId;
  if (lastId && projectsStore.projects.find((p) => p.id === lastId)) {
    projectsStore.currentProjectId = lastId;
  } else if (projectsStore.projects.length > 0) {
    projectsStore.currentProjectId = projectsStore.projects[0].id;
  }
  // If neither condition holds, the empty state will show. The
  // chat store's watcher fires for `currentProjectId = null` and
  // just clears sessions.
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
  if (!projectsStore.currentProjectId) {
    // Defensive: the empty state should make this unreachable,
    // but if some race gets us here, surface a toast instead of
    // silently failing. The PR1 backend's `create_session` would
    // also reject an empty project_id.
    projectsStore.showToast("请先添加项目", "warn");
    return;
  }
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

const showEmptyState = computed<boolean>(
  () => projectsStore.currentProjectId === null,
);

const showSidebar = computed<boolean>(
  () => projectsStore.currentProjectId !== null,
);

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

async function onEmptyAddProject() {
  await projectsStore.addProject();
}

async function onUnhideFromEmpty(id: string) {
  await projectsStore.unhideProject(id);
}

async function loadHiddenFromEmpty() {
  await projectsStore.loadHiddenProjects();
}

// Q5: render the current cwd in the chat header. We display the
// canonical path verbatim — the `~/` shortening referenced in Q5
// requires a home-dir lookup that PR1's backend doesn't expose; the
// deviation is documented in the implement report.
const cwdDisplay = computed<string>(() => {
  const cwd = store.currentCwd;
  return cwd || "";
});

const currentProject = computed(() =>
  projectsStore.projectById(projectsStore.currentProjectId),
);
</script>

<template>
  <div class="app">
    <header class="app__tabs">
      <ProjectTabs :streaming-project-ids="store.streamingProjectIds" />
    </header>

    <div class="app__body">
      <aside v-if="showSidebar" class="sidebar">
        <SessionList />
      </aside>

      <section class="content">
        <!-- Empty state: no project active. Per Q-resolutions, the
             session sidebar is not rendered in this branch; the
             middle of the screen shows a centered "添加项目"
             affordance plus, if any, the "最近隐藏的项目" list
             (Q3 / PROPOSAL §5.3). -->
        <template v-if="showEmptyState">
          <main class="app__main app__main--center">
            <div class="empty">
              <p class="empty__title">还没有项目</p>
              <p class="empty__hint">
                点上方「+ 添加项目」,从文件系统选个目录开始
              </p>
              <button class="empty__add" @click="onEmptyAddProject">
                + 添加项目
              </button>

              <div
                v-if="projectsStore.hiddenProjects.length > 0"
                class="hidden-projects"
              >
                <div class="hidden-projects__sep" />
                <div class="hidden-projects__title">最近隐藏的项目</div>
                <ul class="hidden-projects__list">
                  <li
                    v-for="p in projectsStore.hiddenProjects"
                    :key="p.id"
                    class="hidden-projects__item"
                  >
                    <span class="hidden-projects__name" :title="p.path">
                      <span
                        v-if="p.is_legacy"
                        class="hidden-projects__icon"
                        title="旧数据,自动归入"
                      >📦</span>
                      <span
                        v-else-if="!p.is_git_repo"
                        class="hidden-projects__icon hidden-projects__icon--warn"
                        title="未启用 git 隔离"
                      >⚠️</span>
                      {{ p.name }}
                    </span>
                    <button
                      class="hidden-projects__btn"
                      @click="onUnhideFromEmpty(p.id)"
                    >重新打开</button>
                  </li>
                </ul>
              </div>

              <button
                v-else
                class="empty__load-hidden"
                @click="loadHiddenFromEmpty"
              >
                查看最近隐藏的项目
              </button>
            </div>
          </main>
        </template>

        <!-- Normal state: project + session + chat. -->
        <template v-else>
          <header class="app__header">
            <h1 class="app__title">Everlasting</h1>
            <span class="app__subtitle">vibe coding workbench</span>
            <div
              v-if="cwdDisplay"
              class="cwd"
              :title="cwdDisplay"
            >cwd: {{ cwdDisplay }}</div>
          </header>

          <main ref="messagesEl" class="app__main">
            <div v-if="!hasMessages" class="empty">
              <p>输入一句话,跟 LLM 聊聊看 👋</p>
              <p class="empty__hint">
                中文输入测试 + 流式响应 + 工具调用
              </p>
              <p v-if="currentProject" class="empty__project">
                当前项目: <strong>{{ currentProject.name }}</strong>
                <span v-if="!currentProject.is_git_repo" class="empty__warn">
                  ⚠️ 未启用 git 隔离
                </span>
                <span v-else-if="currentProject.is_legacy" class="empty__warn">
                  📦 旧数据,自动归入
                </span>
              </p>
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
        </template>

        <div v-if="config.loaded" class="statusbar" :class="{ 'statusbar--warn': !config.configured }">
          <span class="statusbar__dot" />
          <span class="statusbar__model">{{ config.model || "(no model)" }}</span>
          <span class="statusbar__sep">·</span>
          <span class="statusbar__url">{{ config.baseUrl || "(no base_url)" }}</span>
          <span v-if="!config.configured" class="statusbar__hint">ANTHROPIC_API_KEY 未设置</span>
        </div>
      </section>
    </div>

    <!-- Toast (Q8v2 / Q2) — minimal fixed bottom-center div. No
         external toast library; the `projects` store owns the
         message queue and timing. -->
    <transition name="toast">
      <div
        v-if="projectsStore.toast"
        :class="['toast', `toast--${projectsStore.toast.kind}`]"
        @click="projectsStore.dismissToast"
      >
        {{ projectsStore.toast.message }}
      </div>
    </transition>
  </div>
</template>

<style scoped>
.app {
  display: flex;
  flex-direction: column;
  height: 100vh;
  background: #fafbfc;
}

.app__tabs {
  flex-shrink: 0;
}

.app__body {
  flex: 1;
  display: flex;
  min-height: 0;
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
  flex-wrap: wrap;
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

.cwd {
  font-size: 11px;
  color: #6b7280;
  font-family: ui-monospace, SFMono-Regular, "SF Mono", Consolas, monospace;
  background: #f3f4f6;
  padding: 2px 8px;
  border-radius: 4px;
  margin-left: auto;
  max-width: 50%;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  direction: rtl; /* keep the tail (path tail) visible when truncated */
  text-align: left;
}

.app__main {
  flex: 1;
  overflow-y: auto;
  padding: 20px;
}

.app__main--center {
  display: flex;
  align-items: center;
  justify-content: center;
}

.empty {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  color: #6b7280;
  text-align: center;
  max-width: 480px;
  padding: 32px 16px;
}

.empty p {
  margin: 4px 0;
}

.empty__title {
  font-size: 18px;
  font-weight: 500;
  color: #1f2328;
  margin: 0 0 8px;
}

.empty__hint {
  font-size: 12px;
  color: #9ca3af;
}

.empty__project {
  font-size: 12px;
  color: #6b7280;
  margin-top: 12px;
}

.empty__warn {
  margin-left: 6px;
  color: #d97706;
  font-size: 11px;
}

.empty__add {
  margin-top: 20px;
  padding: 10px 22px;
  border: 1px solid #d1d5db;
  border-radius: 8px;
  background: #2563eb;
  color: #ffffff;
  font-size: 14px;
  font-weight: 500;
  cursor: pointer;
  transition: background 0.15s, border-color 0.15s;
  font-family: inherit;
}

.empty__add:hover {
  background: #1d4ed8;
  border-color: #1d4ed8;
}

.empty__load-hidden {
  margin-top: 20px;
  padding: 6px 12px;
  background: transparent;
  border: none;
  color: #6b7280;
  font-size: 12px;
  cursor: pointer;
  text-decoration: underline;
  font-family: inherit;
}

.empty__load-hidden:hover {
  color: #2563eb;
}

.hidden-projects {
  width: 100%;
  margin-top: 24px;
  text-align: left;
}

.hidden-projects__sep {
  height: 1px;
  background: #e5e7eb;
  margin: 0 0 16px;
}

.hidden-projects__title {
  font-size: 12px;
  font-weight: 600;
  color: #6b7280;
  text-transform: uppercase;
  letter-spacing: 0.05em;
  margin-bottom: 8px;
}

.hidden-projects__list {
  list-style: none;
  margin: 0;
  padding: 0;
}

.hidden-projects__item {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 8px 10px;
  border: 1px solid #e5e7eb;
  border-radius: 6px;
  background: #ffffff;
  margin-bottom: 6px;
  font-size: 13px;
}

.hidden-projects__name {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.hidden-projects__icon {
  flex-shrink: 0;
  font-size: 12px;
}

.hidden-projects__icon--warn {
  color: #d97706;
}

.hidden-projects__btn {
  flex-shrink: 0;
  margin-left: 8px;
  padding: 4px 10px;
  background: #ffffff;
  border: 1px solid #d1d5db;
  border-radius: 4px;
  color: #2563eb;
  font-size: 12px;
  cursor: pointer;
  transition: background 0.1s;
  font-family: inherit;
}

.hidden-projects__btn:hover {
  background: #f3f4f6;
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
  font-family: inherit;
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

/* --- Toast (Q8v2) --- */

.toast {
  position: fixed;
  bottom: 24px;
  left: 50%;
  transform: translateX(-50%);
  padding: 10px 18px;
  border-radius: 8px;
  background: #1f2328;
  color: #ffffff;
  font-size: 13px;
  box-shadow: 0 4px 12px rgba(0, 0, 0, 0.18);
  cursor: pointer;
  max-width: 80vw;
  z-index: 9999;
}

.toast--warn {
  background: #f59e0b;
  color: #1f2328;
}

.toast--error {
  background: #ef4444;
  color: #ffffff;
}

.toast--info {
  background: #2563eb;
  color: #ffffff;
}

.toast-enter-active,
.toast-leave-active {
  transition: opacity 0.2s, transform 0.2s;
}

.toast-enter-from,
.toast-leave-to {
  opacity: 0;
  transform: translate(-50%, 8px);
}
</style>
