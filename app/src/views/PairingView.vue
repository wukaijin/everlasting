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
// MVP input: a single text field with auto-uppercase + a 6-char maxlength.
// A 6-box OTP layout is left for a later polish pass (the design notes
// MVP is fine with one input).

import { computed, ref } from "vue";
import { useRouter } from "vue-router";
import { usePairingStore } from "../stores/pairing";

const router = useRouter();
const pairing = usePairingStore();

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
    // Success: token is now in localStorage (pwa-remote mode active) +
    // the SSE stream was reset. Navigate to the node picker; the guard
    // will let us through now that hasDeviceToken() is true.
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
          class="pairing-card__btn"
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
  line-height: 1.6;
  text-align: center;
}

.pairing-card__subtitle strong {
  color: var(--color-text-primary);
  font-weight: var(--weight-medium);
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

.pairing-card__input {
  padding: 8px 10px;
  background: var(--color-bg-app);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-sm);
  color: var(--color-text-primary);
  font-size: var(--text-base);
  width: 100%;
  box-sizing: border-box;
}

.pairing-card__input:focus {
  outline: none;
  border-color: var(--color-accent);
}

.pairing-card__code-input {
  padding: 10px;
  background: var(--color-bg-app);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-sm);
  color: var(--color-accent);
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
  outline: none;
  border-color: var(--color-accent);
}

.pairing-card__btn {
  padding: 9px 16px;
  background: var(--color-accent);
  color: #fff;
  border: 1px solid var(--color-accent);
  border-radius: var(--radius-sm);
  font-size: var(--text-base);
  font-weight: var(--weight-semibold);
  cursor: pointer;
  transition: background var(--duration-base) var(--ease-out);
}

.pairing-card__btn:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}

.pairing-card__btn:hover:not(:disabled) {
  background: var(--color-accent-hover);
}

.pairing-card__error {
  margin: 0;
  padding: 6px 10px;
  font-size: var(--text-sm);
  line-height: 1.5;
  color: var(--color-tool-error);
  background: color-mix(in srgb, var(--color-tool-error) 8%, transparent);
  border-left: 3px solid var(--color-tool-error);
  border-radius: var(--radius-sm);
}
</style>
