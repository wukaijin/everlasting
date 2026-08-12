<script setup lang="ts">
// RemoteTab — Settings tab for the PC-side remote tunnel configuration
// (S4 Step 3, design §3).
//
// Four sections, each a thin wrapper over one of the four daemon IPCs
// wired in S2 (commands/{config,pairing}.rs + CMD_TO_DOMAIN):
//
//   1. Config    — `get_remote_config` / `set_remote_config`
//   2. Status    — `get_tunnel_status` (polled every 2s while mounted)
//   3. Pairing   — `generate_pairing_code` + 60s countdown
//   4. Node info — read-only `nodeId` from the status snapshot
//
// P3-1 (design §3.1): the node info area shows `nodeId` ONLY. The
// `TunnelStatusPayload` (config.rs:152) carries no `displayName`; the
// display name lives in the remote nodes table, unreachable from the PC
// without a Rust change (forbidden by S4 hard constraint).
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
            class="remote-tab__btn remote-tab__btn--primary"
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
          class="remote-tab__btn remote-tab__btn--primary"
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

    <!-- 4. Node info (P3-1: nodeId only, no displayName) -->
    <section class="remote-tab__section">
      <h3 class="remote-tab__section-title">节点信息</h3>
      <div class="remote-tab__node-info">
        <span class="remote-tab__label">Node ID</span>
        <code class="remote-tab__node-id">{{
          remoteConfig.status?.nodeId || "—"
        }}</code>
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
  outline: none;
  border-color: var(--color-accent);
}

.remote-tab__form-actions {
  display: flex;
  gap: 8px;
  justify-content: flex-end;
}

/* --- buttons (mirrors ProvidersTab) --- */

.remote-tab__btn {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 5px 12px;
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-sm);
  font-size: var(--text-sm);
  font-weight: var(--weight-medium);
  cursor: pointer;
  background: transparent;
  color: var(--color-text-secondary);
  transition:
    background var(--duration-base) var(--ease-out),
    color var(--duration-base) var(--ease-out);
}

.remote-tab__btn:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}

.remote-tab__btn--primary {
  background: var(--color-accent);
  color: #fff;
  border-color: var(--color-accent);
}

.remote-tab__btn--primary:hover:not(:disabled) {
  background: var(--color-accent-hover);
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
  color: var(--color-accent);
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
  color: var(--color-accent);
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
  color: var(--color-tool-error);
  background: color-mix(in srgb, var(--color-tool-error) 8%, transparent);
  border-left: 2px solid var(--color-tool-error);
}
</style>
