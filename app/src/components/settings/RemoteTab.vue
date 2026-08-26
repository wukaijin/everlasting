<script setup lang="ts">
// RemoteTab — Settings tab for the PC-side remote tunnel configuration
// (S4 Step 3, design §3).
//
// Sections, thin wrappers over the daemon IPCs wired in S2
// (commands/{config,pairing}.rs + CMD_TO_DOMAIN):
//
//   1. Config    — `get_remote_config` / `set_remote_config`
//   2. Status    — `get_tunnel_status` (polled every 2s while mounted)
//   3. Pairing   — `generate_pairing_code` + 60s countdown
//   4. Node info — 生效 `nodeId`（status 轮询）+ 自定义编辑
//                  （`set_tunnel_node_id`，08-26-custom-node-id）
//
// P3-1 (design §3.1, 08-26 解除): the node info area edits BOTH the custom
// `nodeId`（同 hostname 两台机器各设一个自定义 id 即可在 remote 侧消歧
// 互踢）and the `displayName`（`set_tunnel_display_name`，同日增补——
// S4 当时"PC 端够不着 displayName"的约束已由本任务解除；status 快照仍
// 不带 displayName，自定义值经 `get_remote_config` 回显）。
//
// Style mirrors ProvidersTab (reka-ui `Label` + native `<input>`) and
// SubagentsTab (per-row error banner + toast on failure).

import { computed, onMounted, onUnmounted, ref } from "vue";
import { Label } from "reka-ui";
import { useRemoteConfigStore } from "../../stores/remoteConfig";
import { useProjectsStore } from "../../stores/projects";
import { extractErrorMessage } from "../../utils/useErrorBus";

const remoteConfig = useRemoteConfigStore();
const projects = useProjectsStore();

// --- config form (mirrors ProvidersTab's reactive form) ---
const form = ref({ remoteUrl: "", sharedSecret: "" });
const saving = ref(false);
const configError = ref<string | null>(null);

// --- custom node id form (08-26-custom-node-id) ---
const nodeIdForm = ref("");
const nodeIdSaving = ref(false);
const nodeIdError = ref<string | null>(null);

// --- custom display name form (08-26 增补) ---
const displayNameForm = ref("");
const displayNameSaving = ref(false);
const displayNameError = ref<string | null>(null);

// --- pairing code countdown ---
const codeError = ref<string | null>(null);
const generating = ref(false);
/** Whole seconds remaining until the displayed code expires. `-1` while
 *  no code is active (drives the countdown render + the "expired" hint). */
const remainingSec = ref(0);
let countdownTimer: ReturnType<typeof setInterval> | null = null;

// --- status poll ---
let statusTimer: ReturnType<typeof setInterval> | null = null;

/** Aggregate connection state for the status badge (design §3). The
 *  four states map to distinct colors so the user can tell at a glance
 *  whether the tunnel is up, retrying, unconfigured, or auth-broken. */
type ConnState = "unconfigured" | "connected" | "reconnecting" | "auth_failed";

const connState = computed<ConnState>(() => {
  const s = remoteConfig.status;
  if (!s) return "unconfigured";
  if (s.connected) return "connected";
  // lastError === "auth_failed" is the daemon's signal that the remote
  // rejected the shared secret (tunnel client handshake). Surfaced as
  // its own red state so the user knows to fix the secret, not wait.
  if (s.lastError === "auth_failed") return "auth_failed";
  return "reconnecting";
});

const connLabel = computed(() => {
  switch (connState.value) {
    case "connected":
      return "已连接";
    case "reconnecting":
      return "重连中";
    case "auth_failed":
      return "认证失败";
    case "unconfigured":
    default:
      return "未配置";
  }
});

/** True when the save button should be enabled. Empty remote_url means
 *  "disable the tunnel" (set_remote_config accepts empty string), which
 *  is a valid action, so we don't require a non-empty URL to save. */
const canSave = computed(() => !saving.value);

async function save() {
  configError.value = null;
  saving.value = true;
  try {
    await remoteConfig.save(form.value.remoteUrl.trim(), form.value.sharedSecret);
    projects.showToast("Remote 配置已保存", "info");
    // Refresh status immediately so the badge reflects the new config
    // without waiting for the next 2s tick.
    await remoteConfig.refreshStatus();
  } catch (e) {
    const msg = extractErrorMessage(e);
    configError.value = msg;
    projects.showToast(`保存 Remote 配置失败：${msg}`, "error");
  } finally {
    saving.value = false;
  }
}

/** 保存自定义 node_id（空串 = 清除，回自动派生）。非法值由 daemon 校验
 *  拒绝（InvalidRequest），inline 显示后端中文消息。 */
async function saveNodeId() {
  nodeIdError.value = null;
  nodeIdSaving.value = true;
  try {
    await remoteConfig.saveNodeId(nodeIdForm.value.trim());
    projects.showToast("Node ID 已保存", "info");
  } catch (e) {
    const msg = extractErrorMessage(e);
    nodeIdError.value = msg;
    projects.showToast(`保存 Node ID 失败：${msg}`, "error");
  } finally {
    nodeIdSaving.value = false;
  }
}

/** 保存自定义显示名（空串 = 清除，回默认 hostname）。空白 / 超 64 字符
 *  由 daemon 校验拒绝（InvalidRequest），inline 显示后端中文消息。 */
async function saveDisplayName() {
  displayNameError.value = null;
  displayNameSaving.value = true;
  try {
    await remoteConfig.saveDisplayName(displayNameForm.value.trim());
    projects.showToast("显示名已保存", "info");
  } catch (e) {
    const msg = extractErrorMessage(e);
    displayNameError.value = msg;
    projects.showToast(`保存显示名失败：${msg}`, "error");
  } finally {
    displayNameSaving.value = false;
  }
}

async function generateCode() {
  codeError.value = null;
  generating.value = true;
  try {
    await remoteConfig.generateCode();
    startCountdown();
  } catch (e) {
    const msg = extractErrorMessage(e);
    codeError.value = msg;
    projects.showToast(`生成配对码失败：${msg}`, "error");
  } finally {
    generating.value = false;
  }
}

function startCountdown() {
  stopCountdown();
  const pc = remoteConfig.pairingCode;
  if (!pc) return;
  remainingSec.value = Math.max(
    0,
    Math.ceil((pc.expiresAt - Date.now()) / 1000),
  );
  countdownTimer = setInterval(() => {
    remainingSec.value = Math.max(
      0,
      Math.ceil((pc.expiresAt - Date.now()) / 1000),
    );
    if (remainingSec.value <= 0) {
      // Code expired — clear it + prompt the user to regenerate.
      stopCountdown();
      remoteConfig.pairingCode = null;
      codeError.value = "配对码已过期，请重新生成";
    }
  }, 1000);
}

function stopCountdown() {
  if (countdownTimer) {
    clearInterval(countdownTimer);
    countdownTimer = null;
  }
}

onMounted(async () => {
  // Load the persisted config so the form pre-fills, then start the
  // status poll. The poll is the only "live" signal the user has that
  // the tunnel is up (the daemon reconnects silently; without polling
  // the badge would be a stale snapshot).
  try {
    await remoteConfig.load();
    if (remoteConfig.config) {
      form.value.remoteUrl = remoteConfig.config.remoteUrl;
      form.value.sharedSecret = remoteConfig.config.sharedSecret;
    }
    // 自定义 node_id / 显示名预填（null / 未配置 remote = 空 = 自动/默认）。
    nodeIdForm.value = remoteConfig.config?.nodeId ?? "";
    displayNameForm.value = remoteConfig.config?.displayName ?? "";
  } catch (e) {
    configError.value = extractErrorMessage(e);
  }
  await remoteConfig.refreshStatus().catch(() => {
    // Status poll failures are non-fatal — the badge just stays on the
    // last snapshot. Don't spam a toast every 2s on a flaky daemon.
  });
  statusTimer = setInterval(() => {
    void remoteConfig.refreshStatus().catch(() => {});
  }, 2000);
});

onUnmounted(() => {
  if (statusTimer) {
    clearInterval(statusTimer);
    statusTimer = null;
  }
  stopCountdown();
});
</script>

<template>
  <div class="remote-tab">
    <p class="remote-tab__intro">
      配置 remote 服务器地址与共享密钥，让手机或其他浏览器经 remote 隧道连接
      本机。保存后 daemon 会自动重连；状态区实时反映连接情况。
    </p>

    <!-- 1. Config -->
    <section class="remote-tab__section">
      <h3 class="remote-tab__section-title">配置</h3>
      <div class="remote-tab__form">
        <Label class="remote-tab__field">
          <span class="remote-tab__label">Remote URL (wss://)</span>
          <input
            v-model="form.remoteUrl"
            type="text"
            class="remote-tab__input"
            placeholder="wss://remote.example.com"
            autocomplete="off"
            spellcheck="false"
          />
        </Label>
        <Label class="remote-tab__field">
          <span class="remote-tab__label">Shared Secret</span>
          <input
            v-model="form.sharedSecret"
            type="password"
            class="remote-tab__input"
            placeholder="与 remote 共享的密钥"
            autocomplete="off"
            spellcheck="false"
          />
        </Label>
        <div class="remote-tab__form-actions">
          <button
            type="button"
            class="remote-tab__btn remote-tab__btn--primary btn btn--primary"
            :disabled="!canSave"
            @click="save"
          >
            {{ saving ? "保存中…" : "保存" }}
          </button>
        </div>
        <p
          v-if="configError"
          class="remote-tab__error"
          role="alert"
        >
          {{ configError }}
        </p>
      </div>
    </section>

    <!-- 2. Connection status -->
    <section class="remote-tab__section">
      <h3 class="remote-tab__section-title">连接状态</h3>
      <div
        class="remote-tab__status"
        :data-state="connState"
      >
        <span class="remote-tab__status-dot" />
        <span class="remote-tab__status-label">{{ connLabel }}</span>
        <span
          v-if="remoteConfig.status?.remoteUrl"
          class="remote-tab__status-url"
        >
          {{ remoteConfig.status.remoteUrl }}
        </span>
      </div>
      <p
        v-if="connState === 'auth_failed'"
        class="remote-tab__error"
        role="alert"
      >
        共享密钥与 remote 不匹配，请检查后重新保存配置。
      </p>
    </section>

    <!-- 3. Pairing code -->
    <section class="remote-tab__section">
      <h3 class="remote-tab__section-title">配对码</h3>
      <div class="remote-tab__pairing">
        <button
          type="button"
          class="remote-tab__btn remote-tab__btn--primary btn btn--primary"
          :disabled="generating || connState === 'unconfigured'"
          @click="generateCode"
        >
          {{ generating ? "生成中…" : "生成配对码" }}
        </button>
        <div
          v-if="remoteConfig.pairingCode"
          class="remote-tab__code-display"
        >
          <span class="remote-tab__code">{{ remoteConfig.pairingCode.code }}</span>
          <span class="remote-tab__countdown">{{ remainingSec }}s</span>
        </div>
        <p
          v-if="remoteConfig.pairingCode"
          class="remote-tab__hint"
        >
          在手机上打开
          <code>{{ remoteConfig.status?.remoteUrl || form.remoteUrl || "remote 域名" }}</code>
          并输入此配对码。
        </p>
        <p
          v-if="codeError"
          class="remote-tab__error"
          role="alert"
        >
          {{ codeError }}
        </p>
      </div>
    </section>

    <!-- 4. Node info（生效 nodeId 展示 + 自定义编辑，08-26-custom-node-id） -->
    <section class="remote-tab__section">
      <h3 class="remote-tab__section-title">节点信息</h3>
      <div class="remote-tab__node-info">
        <span class="remote-tab__label">Node ID</span>
        <code class="remote-tab__node-id">{{
          remoteConfig.status?.nodeId || "—"
        }}</code>
      </div>
      <div class="remote-tab__form">
        <Label class="remote-tab__field">
          <span class="remote-tab__label">自定义 Node ID</span>
          <input
            v-model="nodeIdForm"
            type="text"
            class="remote-tab__input"
            placeholder="留空 = 自动（hostname 派生）"
            autocomplete="off"
            spellcheck="false"
          />
        </Label>
        <div class="remote-tab__form-actions">
          <button
            type="button"
            class="remote-tab__btn remote-tab__btn--primary btn btn--primary"
            :disabled="nodeIdSaving"
            @click="saveNodeId"
          >
            {{ nodeIdSaving ? "保存中…" : "保存" }}
          </button>
        </div>
        <p
          v-if="nodeIdError"
          class="remote-tab__error"
          role="alert"
        >
          {{ nodeIdError }}
        </p>
        <p class="remote-tab__hint">
          两台机器 hostname 相同时会在 remote 侧互相踢线，可分别设置不同的
          ID 消歧；仅限小写字母、数字和连字符。修改后已配对设备绑定的是
          旧 ID，需重新配对。
        </p>
      </div>
      <div class="remote-tab__form">
        <Label class="remote-tab__field">
          <span class="remote-tab__label">显示名</span>
          <input
            v-model="displayNameForm"
            type="text"
            class="remote-tab__input"
            placeholder="留空 = 自动（hostname）"
            autocomplete="off"
            spellcheck="false"
          />
        </Label>
        <div class="remote-tab__form-actions">
          <button
            type="button"
            class="remote-tab__btn remote-tab__btn--primary btn btn--primary"
            :disabled="displayNameSaving"
            @click="saveDisplayName"
          >
            {{ displayNameSaving ? "保存中…" : "保存" }}
          </button>
        </div>
        <p
          v-if="displayNameError"
          class="remote-tab__error"
          role="alert"
        >
          {{ displayNameError }}
        </p>
        <p class="remote-tab__hint">
          这是手机 App / 远程节点列表里显示的名字，支持中文，最长 64 个
          字符。修改后隧道会以新名字重连。
        </p>
      </div>
    </section>
  </div>
</template>

<style scoped>
.remote-tab {
  display: flex;
  flex-direction: column;
  gap: 16px;
}

.remote-tab__intro {
  margin: 0 0 4px 0;
  font-size: var(--text-sm);
  color: var(--color-text-secondary);
  line-height: 1.6;
}

.remote-tab__section {
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.remote-tab__section-title {
  margin: 0;
  font-size: var(--text-sm);
  font-weight: var(--weight-semibold);
  color: var(--color-text-primary);
}

/* --- form (mirrors ProvidersTab) --- */

.remote-tab__form {
  display: flex;
  flex-direction: column;
  gap: 12px;
  padding: 12px;
  background: var(--color-bg-elevated);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-md);
}

.remote-tab__field {
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.remote-tab__label {
  font-size: var(--text-xs);
  font-weight: var(--weight-medium);
  color: var(--color-text-secondary);
}

.remote-tab__input {
  padding: 6px 10px;
  background: var(--color-bg-app);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-sm);
  color: var(--color-text-primary);
  font-size: var(--text-sm);
  width: 100%;
  box-sizing: border-box;
}

.remote-tab__input:focus {
  /* 自有焦点替代::focus 换 accent 边框(见下),无需 UA 默认环 */
  outline: none;
  border-color: var(--color-accent);
}

.remote-tab__form-actions {
  display: flex;
  gap: 8px;
  justify-content: flex-end;
}

/* --- buttons (mirrors ProvidersTab) --- */

/* 按钮样式由全局 .btn 家族承载(primary);此处仅保留家族不拥有的
   字重。 */
.remote-tab__btn {
  font-weight: var(--weight-medium);
}

/* --- status badge --- */

.remote-tab__status {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 12px;
  background: var(--color-bg-elevated);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-md);
  font-size: var(--text-sm);
}

.remote-tab__status-dot {
  width: 10px;
  height: 10px;
  border-radius: 50%;
  flex-shrink: 0;
  /* default dot color per state (driven by [data-state] below) */
}

.remote-tab__status[data-state="connected"] .remote-tab__status-dot {
  background: #22c55e;
}
.remote-tab__status[data-state="reconnecting"] .remote-tab__status-dot {
  background: #eab308;
}
.remote-tab__status[data-state="auth_failed"] .remote-tab__status-dot {
  background: var(--color-tool-error);
}
.remote-tab__status[data-state="unconfigured"] .remote-tab__status-dot {
  background: var(--color-text-muted);
  opacity: 0.5;
}

.remote-tab__status-label {
  font-weight: var(--weight-semibold);
  color: var(--color-text-primary);
}

.remote-tab__status-url {
  margin-left: auto;
  font-family: var(--font-mono);
  font-size: var(--text-xs);
  color: var(--color-text-muted);
  max-width: 60%;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

/* --- pairing code --- */

.remote-tab__pairing {
  display: flex;
  flex-direction: column;
  gap: 8px;
  padding: 12px;
  background: var(--color-bg-elevated);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-md);
}

.remote-tab__code-display {
  display: flex;
  align-items: baseline;
  gap: 12px;
}

.remote-tab__code {
  font-family: var(--font-mono);
  font-size: 2rem;
  font-weight: var(--weight-bold);
  letter-spacing: 0.4em;
  color: var(--color-accent-text);
  padding-left: 0.4em; /* offset letter-spacing on the last char */
}

.remote-tab__countdown {
  font-family: var(--font-mono);
  font-size: var(--text-sm);
  color: var(--color-text-muted);
}

.remote-tab__hint {
  margin: 0;
  font-size: var(--text-xs);
  color: var(--color-text-secondary);
  line-height: 1.5;
}

.remote-tab__hint code {
  font-family: var(--font-mono);
  font-size: var(--text-xs);
  color: var(--color-accent-text);
  background: var(--color-bg-app);
  border: 1px solid var(--color-bg-border);
  border-radius: 3px;
  padding: 0 4px;
  word-break: break-all;
}

/* --- node info --- */

.remote-tab__node-info {
  display: flex;
  flex-direction: column;
  gap: 4px;
  padding: 12px;
  background: var(--color-bg-elevated);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-md);
}

.remote-tab__node-id {
  font-family: var(--font-mono);
  font-size: var(--text-sm);
  color: var(--color-text-primary);
  word-break: break-all;
}

/* --- error banner (mirrors SubagentsTab) --- */

.remote-tab__error {
  margin: 0;
  font-size: var(--text-xs);
  line-height: 1.5;
  padding: 4px 8px;
  border-radius: var(--radius-sm);
  color: var(--color-tool-error-text);
  background: color-mix(in srgb, var(--color-tool-error) 8%, transparent);
  border-left: 2px solid var(--color-tool-error);
}
</style>
