<script setup lang="ts">
// NodeListView — mobile / remote-browser node picker (S4 Step 5,
// design §5.4).
//
// The home view once paired: lists the PCs bound to the current device
// token (GET /api/v1/nodes via the nodes store). The user taps an online
// node to enter the full chat experience (/chat). Offline nodes show a
// transient "该 PC 离线" notice instead of navigating (the proxied
// connection would just hang / fail).
//
// Logout clears the device token and routes back to /pairing.
//
// The view is mounted by <router-view /> at the app root, OUTSIDE the
// AppShell — so the projects-store toast (rendered in the Sidebar) is
// not visible here. The offline-node notice is a small local ephemeral
// banner scoped to this view.

import { onMounted, ref } from "vue";
import { useRouter } from "vue-router";
import { useNodesStore, type NodeInfo } from "../stores/nodes";
import { extractErrorMessage } from "../utils/useErrorBus";

const router = useRouter();
const nodes = useNodesStore();

const loadError = ref<string | null>(null);
/** Transient notice for non-navigating taps (offline node). Auto-clears. */
const notice = ref<string | null>(null);
let noticeTimer: ReturnType<typeof setTimeout> | null = null;

/** Format `lastSeenAt` (epoch ms) as a Chinese relative time string.
 *  Coarse granularity (分钟/小时/天) is enough for "is this PC alive
 *  recently". No new dependency (Intl.RelativeTimeFormat would need
 *  locale-aware pluralization glue; a hand-rolled ladder is simpler
 *  for the 3 buckets we care about). */
function relativeTime(epochMs: number): string {
  const diffMs = Date.now() - epochMs;
  if (diffMs < 60_000) return "刚刚";
  const min = Math.floor(diffMs / 60_000);
  if (min < 60) return `${min} 分钟前`;
  const hr = Math.floor(min / 60);
  if (hr < 24) return `${hr} 小时前`;
  const day = Math.floor(hr / 24);
  return `${day} 天前`;
}

function showNotice(msg: string) {
  notice.value = msg;
  if (noticeTimer) clearTimeout(noticeTimer);
  noticeTimer = setTimeout(() => {
    notice.value = null;
    noticeTimer = null;
  }, 3000);
}

function onClickNode(node: NodeInfo) {
  if (node.status === "online") {
    nodes.selectNode(node.nodeId);
    void router.push("/chat");
  } else {
    // Offline: the proxied WSS to that node is down, so entering chat
    // would just stall. Surface a notice instead of a dead screen.
    showNotice("该 PC 离线，无法连接。");
  }
}

async function refresh() {
  loadError.value = null;
  try {
    await nodes.loadNodes();
  } catch (e) {
    loadError.value = extractErrorMessage(e);
  }
}

function logout() {
  nodes.logout();
  void router.push("/pairing");
}

onMounted(refresh);
</script>

<template>
  <div class="node-list-view">
    <header class="node-list-view__header">
      <h1 class="node-list-view__title">选择设备</h1>
      <button
        type="button"
        class="node-list-view__logout btn btn--outline"
        @click="logout"
      >
        登出
      </button>
    </header>

    <div class="node-list-view__body">
      <Transition name="node-list-view__notice">
        <p
          v-if="notice"
          class="node-list-view__notice"
          role="status"
        >
          {{ notice }}
        </p>
      </Transition>

      <p
        v-if="loadError"
        class="node-list-view__error"
        role="alert"
      >
        {{ loadError }}
        <button
          type="button"
          class="node-list-view__retry btn btn--outline btn--sm"
          @click="refresh"
        >
          重试
        </button>
      </p>

      <div v-if="nodes.loading && !nodes.loaded" class="node-list-view__loading">
        加载中…
      </div>

      <ul
        v-else-if="nodes.nodes.length > 0"
        class="node-list-view__list"
      >
        <li
          v-for="node in nodes.nodes"
          :key="node.nodeId"
        >
          <button
            type="button"
            class="node-card"
            :class="{ 'node-card--offline': node.status !== 'online' }"
            @click="onClickNode(node)"
          >
            <span
              class="node-card__dot"
              :data-status="node.status"
            />
            <span class="node-card__main">
              <span class="node-card__name">{{ node.displayName || node.nodeId }}</span>
              <span class="node-card__meta">
                {{ node.status === "online" ? "在线" : `离线 · ${relativeTime(node.lastSeenAt)}` }}
              </span>
            </span>
          </button>
        </li>
      </ul>

      <div v-else-if="nodes.loaded" class="node-list-view__empty">
        没有已配对的设备。请在 PC 上生成配对码并重新配对。
      </div>
    </div>
  </div>
</template>

<style scoped>
.node-list-view {
  position: fixed;
  inset: 0;
  display: flex;
  flex-direction: column;
  background: var(--color-bg-app);
}

.node-list-view__header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 12px 16px;
  border-bottom: 1px solid var(--color-bg-border);
  background: var(--color-bg-elevated);
  flex-shrink: 0;
}

.node-list-view__title {
  margin: 0;
  font-size: var(--text-base);
  font-weight: var(--weight-semibold);
  color: var(--color-text-primary);
}

/* logout/retry 由全局 .btn 家族承载(outline / outline sm);retry 是错误
 * 重试语义,叠 tool-error 红边红字覆写。 */
.node-list-view__retry {
  flex-shrink: 0;
  border-color: var(--color-tool-error);
  color: var(--color-tool-error-text);
}

/* node-card 是卡片式设备选择钮(卡片语义非按钮家族形态),特例保留本地样式。 */

.node-list-view__body {
  flex: 1;
  overflow-y: auto;
  padding: 16px;
  display: flex;
  flex-direction: column;
  gap: 12px;
  max-width: 560px;
  width: 100%;
  margin: 0 auto;
  box-sizing: border-box;
}

.node-list-view__notice {
  margin: 0;
  padding: 8px 12px;
  font-size: var(--text-sm);
  color: var(--color-text-primary);
  background: color-mix(in srgb, var(--color-accent) 12%, transparent);
  border: 1px solid var(--color-accent-muted, var(--color-accent));
  border-radius: var(--radius-sm);
}

.node-list-view__error {
  margin: 0;
  padding: 8px 12px;
  font-size: var(--text-sm);
  color: var(--color-tool-error-text);
  background: color-mix(in srgb, var(--color-tool-error) 8%, transparent);
  border-left: 3px solid var(--color-tool-error);
  border-radius: var(--radius-sm);
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
}

/* logout/retry 由全局 .btn 家族承载(outline / outline sm);retry 是错误
 * 重试语义,叠 tool-error 红边红字覆写。 */
.node-list-view__retry {
  flex-shrink: 0;
  border-color: var(--color-tool-error);
  color: var(--color-tool-error-text);
}

/* node-card 是卡片式设备选择钮(卡片语义非按钮家族形态),特例保留本地样式。 */
.node-list-view__loading,
.node-list-view__empty {
  padding: 32px 16px;
  text-align: center;
  color: var(--color-text-muted);
  font-size: var(--text-sm);
}

.node-list-view__list {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.node-card {
  display: flex;
  align-items: center;
  gap: 12px;
  width: 100%;
  padding: 14px 16px;
  background: var(--color-bg-surface);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-md);
  cursor: pointer;
  text-align: left;
  font-family: inherit;
  transition:
    border-color var(--duration-base) var(--ease-out),
    background var(--duration-base) var(--ease-out);
}

.node-card:hover {
  border-color: var(--color-accent-muted, var(--color-accent));
  background: var(--color-bg-elevated);
}

.node-card--offline {
  cursor: default;
  opacity: 0.75;
}

.node-card--offline:hover {
  border-color: var(--color-bg-border);
  background: var(--color-bg-surface);
}

.node-card__dot {
  width: 10px;
  height: 10px;
  border-radius: 50%;
  flex-shrink: 0;
}

.node-card__dot[data-status="online"] {
  background: #22c55e;
}

.node-card__dot[data-status="offline"] {
  background: var(--color-text-muted);
  opacity: 0.5;
}

.node-card__main {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
}

.node-card__name {
  font-size: var(--text-base);
  font-weight: var(--weight-medium);
  color: var(--color-text-primary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.node-card__meta {
  font-size: var(--text-xs);
  color: var(--color-text-muted);
}

/* Notice enter/leave transition */
.node-list-view__notice-enter-active,
.node-list-view__notice-leave-active {
  transition:
    opacity var(--duration-base) var(--ease-out),
    transform var(--duration-base) var(--ease-out);
}

.node-list-view__notice-enter-from,
.node-list-view__notice-leave-to {
  opacity: 0;
  transform: translateY(-4px);
}
</style>
