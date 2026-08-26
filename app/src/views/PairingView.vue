<script setup lang="ts">
// PairingView — mobile / remote-browser pairing code entry (S4 Step 4,
// design §5.3 / §6.1).
//
// Shown when the SPA is served by the remote daemon and no device token
// is present yet (router guard, design §5.1). The user types the 6-digit
// code displayed on the PC's Settings → Remote tab; on submit the
// pairing store POSTs to the remote's `/api/v1/pairing/redeem` (direct
// fetch, D4), persists the returned device token, and navigates to
// /nodes.
//
// 08-26-multi-node-pairing: pairings accumulate per node, so this view
// stays useful with existing pairings — "前往选择 →" links to /nodes
// (e.g. user came here to add a second PC but changed their mind).
//
// MVP input: a single text field with auto-uppercase + a 6-char maxlength.
// A 6-box OTP layout is left for a later polish pass (the design notes
// MVP is fine with one input).

import { computed, ref } from "vue";
import { useRouter } from "vue-router";
import { usePairingStore } from "../stores/pairing";
import { hasPairedNode } from "../transport/auth";

const router = useRouter();
const pairing = usePairingStore();
// 已有配对时才显示"前往选择"入口;挂载时判定一次即可(redeem 成功后
// 本视图即被导航离开)。
const hasExistingPairs = hasPairedNode();

function goNodes() {
  void router.push("/nodes");
}

const code = ref("");
const deviceName = ref("");
const pairingInProgress = ref(false);
const errorMessage = ref<string | null>(null);

/** Auto-uppercase + digits/letters only, trimmed to 6 chars. The remote
 *  generates 6-digit numeric codes (pairing.rs), but we accept alphanum
 *  defensively in case the code alphabet changes upstream. */
function onCodeInput(e: Event) {
  const v = (e.target as HTMLInputElement).value
    .toUpperCase()
    .replace(/[^A-Z0-9]/g, "")
    .slice(0, 6);
  code.value = v;
  // Clear the error as soon as the user edits the code.
  errorMessage.value = null;
}

const canSubmit = computed(
  () => code.value.length === 6 && !pairingInProgress.value,
);

async function submit() {
  if (!canSubmit.value) return;
  pairingInProgress.value = true;
  errorMessage.value = null;
  try {
    await pairing.redeem(code.value, deviceName.value.trim());
    // Success: the node's token is now in the auth map (pwa-remote mode
    // active) + the SSE stream was reset. Navigate to the node picker;
    // the guard will let us through now that hasPairedNode() is true.
    void router.push("/nodes");
  } catch (e) {
    // The store throws Error instances with user-facing Chinese
    // messages; extractErrorMessage would also work, but these are
    // plain Errors so .message is the message.
    errorMessage.value = e instanceof Error ? e.message : "(未知错误)";
  } finally {
    pairingInProgress.value = false;
  }
}
</script>

<template>
  <div class="pairing-view">
    <div class="pairing-card">
      <h1 class="pairing-card__title">配对你的设备</h1>
      <p class="pairing-card__subtitle">
        在 PC 的 <strong>Settings → Remote → 生成配对码</strong> 获取配对码。
      </p>

      <form class="pairing-card__form" @submit.prevent="submit">
        <label class="pairing-card__field">
          <span class="pairing-card__label">配对码</span>
          <input
            :value="code"
            type="text"
            inputmode="text"
            autocomplete="one-time-code"
            class="pairing-card__code-input"
            placeholder="ABCDEF"
            maxlength="6"
            spellcheck="false"
            autofocus
            @input="onCodeInput"
          />
        </label>

        <label class="pairing-card__field">
          <span class="pairing-card__label">设备名（可选）</span>
          <input
            v-model="deviceName"
            type="text"
            class="pairing-card__input"
            placeholder="我的手机"
            maxlength="64"
            autocomplete="off"
          />
        </label>

        <button
          type="submit"
          class="pairing-card__btn btn btn--primary btn--lg"
          :disabled="!canSubmit"
        >
          {{ pairingInProgress ? "配对中…" : "配对" }}
        </button>

        <p
          v-if="errorMessage"
          class="pairing-card__error"
          role="alert"
        >
          {{ errorMessage }}
        </p>
      </form>

      <button
        v-if="hasExistingPairs"
        type="button"
        class="pairing-card__goto-nodes"
        @click="goNodes"
      >
        已配对设备？前往选择 →
      </button>
    </div>
  </div>
</template>

<style scoped>
.pairing-view {
  position: fixed;
  inset: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  background: var(--color-bg-app);
  padding: 1rem;
  box-sizing: border-box;
}

.pairing-card {
  width: 100%;
  max-width: 360px;
  background: var(--color-bg-surface);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-lg);
  box-shadow: var(--shadow-xl);
  padding: 24px 20px;
  display: flex;
  flex-direction: column;
  gap: 16px;
}

.pairing-card__title {
  margin: 0;
  font-size: 1.25rem;
  font-weight: var(--weight-semibold);
  color: var(--color-text-primary);
  text-align: center;
}

.pairing-card__subtitle {
  margin: 0;
  font-size: var(--text-sm);
  color: var(--color-text-secondary);
  line-height: var(--leading-relaxed);
  text-align: center;
}

.pairing-card__subtitle strong {
  /* 08-21 ui-review:PC 端路径是配对页唯一需要照做的操作指引,
     secondary→primary 灰阶差在小字号下读不出强调;换 accent-text
     (design-tokens.md "accent as ink",6.71:1 AA)+ semibold,
     用户扫一眼就能定位路径。 */
  color: var(--color-accent-text);
  font-weight: var(--weight-semibold);
}

.pairing-card__form {
  display: flex;
  flex-direction: column;
  gap: 14px;
}

.pairing-card__field {
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.pairing-card__label {
  font-size: var(--text-xs);
  font-weight: var(--weight-medium);
  color: var(--color-text-secondary);
}

/* 08-14 ux-polish-r1 WP1 1.5(评审 A5):两个输入框统一为同一视觉规格 ——
   同 padding / border / radius / 背景 / focus 表现(border-accent +
   --shadow-ring 焦点环,对齐全局表单焦点约定)。配对码保留 mono + 大字号
   + accent 作为 hero 强调(功能性差异),边框规格与设备名完全一致。 */
.pairing-card__input {
  padding: 10px;
  background: var(--color-bg-app);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-sm);
  color: var(--color-text-primary);
  font-size: var(--text-base);
  width: 100%;
  box-sizing: border-box;
}

.pairing-card__input:focus {
  /* 自有焦点替代::focus 换 accent 边框(见下),无需 UA 默认环 */
  outline: none;
  border-color: var(--color-accent);
  box-shadow: var(--shadow-ring);
}

/* 占位符统一降一档 muted(评审 A5:占位符与已输入状态难区分 —— 原来继承
   输入框 color,配对码占位符是 accent、几乎与已输入态同亮)。muted 后
   "空的"与"填了"一眼可辨。 */
.pairing-card__input::placeholder,
.pairing-card__code-input::placeholder {
  color: var(--color-text-muted);
  opacity: 1;
}

.pairing-card__code-input {
  padding: 10px;
  background: var(--color-bg-app);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-sm);
  color: var(--color-accent-text);
  font-family: var(--font-mono);
  font-size: 1.75rem;
  font-weight: var(--weight-bold);
  letter-spacing: 0.4em;
  text-align: center;
  text-transform: uppercase;
  width: 100%;
  box-sizing: border-box;
  padding-left: calc(10px + 0.4em); /* offset trailing letter-spacing */
}

.pairing-card__code-input:focus {
  /* 自有焦点替代::focus 换 accent 边框(见下),无需 UA 默认环 */
  outline: none;
  border-color: var(--color-accent);
  box-shadow: var(--shadow-ring);
}

/* 配对 CTA 由 .btn--primary--lg 承载;9px→10px 落家族档。 */
.pairing-card__btn {
  font-weight: var(--weight-semibold);
}

.pairing-card__error {
  margin: 0;
  padding: 6px 10px;
  font-size: var(--text-sm);
  line-height: var(--leading-normal);
  color: var(--color-tool-error-text);
  background: color-mix(in srgb, var(--color-tool-error) 8%, transparent);
  border-left: 3px solid var(--color-tool-error);
  border-radius: var(--radius-sm);
}

/* 已有配对时的次级出口:文字链形态(accent 作墨,design-tokens
 * "accent as ink"),弱于主 CTA。 */
.pairing-card__goto-nodes {
  border: none;
  padding: 0;
  background: none;
  font-size: var(--text-sm);
  color: var(--color-accent-text);
  cursor: pointer;
  text-align: center;
}

.pairing-card__goto-nodes:hover {
  text-decoration: underline;
}
</style>
