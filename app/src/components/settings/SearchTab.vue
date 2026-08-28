<script setup lang="ts">
// SearchTab — Settings tab for the web_search tool configuration
// (F4, task `08-25-web-search-tool` WP3)。
//
// 一个表单节:provider 下拉(auto/tavily/ddg)+ Tavily key 密码框
// (masked placeholder,留空 = 不改)+ 「清除已存 key」动作。样式镜像
// RemoteTab(reka Label + 原生 input + .btn 家族 + error banner + toast);
// provider 下拉走 reka-ui SelectRoot(2026-08-28 统一,ProvidersTab/
// ModelForm/SubagentsTab 同款 —— 原生 <select> 的 OS 弹层与其它 tab
// 的暗色 reka 弹层不一致)。
//
// key 三态映射见 stores/webSearch.ts 头注释:留空保存不动 key;清除
// 动作传空串删行(切回 auto 后不会静默复活 Tavily)。

import { onMounted, ref } from "vue";
import {
  Label,
  SelectRoot,
  SelectTrigger,
  SelectValue,
  SelectIcon,
  SelectPortal,
  SelectContent,
  SelectViewport,
  SelectItem,
  SelectItemText,
} from "reka-ui";
import Icon from "../Icon.vue";
import { useWebSearchStore } from "../../stores/webSearch";
import { useProjectsStore } from "../../stores/projects";
import { extractErrorMessage } from "../../utils/useErrorBus";

const webSearch = useWebSearchStore();
const projects = useProjectsStore();

const form = ref({ provider: "auto", tavilyApiKey: "" });
const saving = ref(false);
const clearing = ref(false);
const error = ref<string | null>(null);

const keyPlaceholder = () =>
  webSearch.config?.tavilyKeyMasked
    ? `当前 ${webSearch.config.tavilyKeyMasked} · 留空 = 不改`
    : "tvly-...(Tavily 免费注册即得,1000 次/月)";

async function save() {
  error.value = null;
  saving.value = true;
  try {
    const key = form.value.tavilyApiKey.trim();
    // 留空 = 不动(undefined → 后端 None);非空才传(明文仅此一跳,
    // 后端收到即 AEAD 加密落盘,GET 永不回明文)。
    await webSearch.save(form.value.provider, key === "" ? undefined : key);
    form.value.tavilyApiKey = "";
    projects.showToast("Web 搜索配置已保存", "info");
  } catch (e) {
    const msg = extractErrorMessage(e);
    error.value = msg;
    projects.showToast(`保存 Web 搜索配置失败：${msg}`, "error");
  } finally {
    saving.value = false;
  }
}

async function clearKey() {
  error.value = null;
  clearing.value = true;
  try {
    await webSearch.clearKey();
    form.value.tavilyApiKey = "";
    projects.showToast("已清除 Tavily key(auto 将回落 DuckDuckGo)", "info");
  } catch (e) {
    const msg = extractErrorMessage(e);
    error.value = msg;
    projects.showToast(`清除 Tavily key 失败：${msg}`, "error");
  } finally {
    clearing.value = false;
  }
}

onMounted(async () => {
  try {
    await webSearch.load();
    if (webSearch.config) form.value.provider = webSearch.config.provider;
  } catch (e) {
    error.value = extractErrorMessage(e);
  }
});
</script>

<template>
  <div class="search-tab">
    <p class="search-tab__intro">
      配置 agent 的 web_search 工具后端。Tavily(需 key,免费 1000
      次/月)结果质量最好;DuckDuckGo 零配置兜底,但共享出口 IP 易被
      软限流。正文抓取始终走既有 web_fetch,不受此处影响。
    </p>

    <section class="search-tab__section">
      <h3 class="search-tab__section-title">Web 搜索</h3>
      <div class="search-tab__form">
        <Label class="search-tab__field">
          <span class="search-tab__label">Provider</span>
          <SelectRoot v-model="form.provider">
            <SelectTrigger class="search-tab__trigger" aria-label="Provider">
              <SelectValue />
              <SelectIcon class="search-tab__trigger-icon">
                <Icon name="chevron-down" :size="12" />
              </SelectIcon>
            </SelectTrigger>
            <SelectPortal>
              <SelectContent
                class="search-tab__dropdown"
                position="popper"
                :side-offset="4"
              >
                <SelectViewport class="search-tab__dropdown-viewport">
                  <SelectItem value="auto" class="search-tab__option">
                    <SelectItemText>auto(有 Tavily key 用 Tavily,否则 DDG)</SelectItemText>
                  </SelectItem>
                  <SelectItem value="tavily" class="search-tab__option">
                    <SelectItemText>tavily</SelectItemText>
                  </SelectItem>
                  <SelectItem value="ddg" class="search-tab__option">
                    <SelectItemText>ddg(DuckDuckGo,零配置)</SelectItemText>
                  </SelectItem>
                </SelectViewport>
              </SelectContent>
            </SelectPortal>
          </SelectRoot>
        </Label>

        <Label class="search-tab__field">
          <span class="search-tab__label">Tavily API Key</span>
          <input
            v-model="form.tavilyApiKey"
            type="password"
            class="search-tab__input"
            :placeholder="keyPlaceholder()"
            autocomplete="off"
            spellcheck="false"
          />
        </Label>
        <p class="search-tab__hint">
          key 加密存储在本机,不会回显明文。选 tavily 前需先填 key。
        </p>

        <div class="search-tab__form-actions">
          <button
            v-if="webSearch.config?.tavilyKeySet"
            type="button"
            class="search-tab__btn btn btn--ghost"
            :disabled="clearing || saving"
            @click="clearKey"
          >
            {{ clearing ? "清除中…" : "清除已存 key" }}
          </button>
          <button
            type="button"
            class="search-tab__btn btn btn--primary"
            :disabled="saving || clearing"
            @click="save"
          >
            {{ saving ? "保存中…" : "保存" }}
          </button>
        </div>
        <p
          v-if="webSearch.config?.tavilyKeySet"
          class="search-tab__hint"
        >
          清除后 auto 模式将回落到 DuckDuckGo,已存的 Tavily key 会被删除。
        </p>
        <p
          v-if="error"
          class="search-tab__error"
          role="alert"
        >
          {{ error }}
        </p>
      </div>
    </section>
  </div>
</template>

<style scoped>
.search-tab {
  display: flex;
  flex-direction: column;
  gap: 16px;
}

.search-tab__intro {
  margin: 0 0 4px 0;
  font-size: var(--text-sm);
  color: var(--color-text-secondary);
  line-height: 1.6;
}

.search-tab__section {
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.search-tab__section-title {
  margin: 0;
  font-size: var(--text-sm);
  font-weight: var(--weight-semibold);
  color: var(--color-text-primary);
}

/* --- form(mirrors RemoteTab / ProvidersTab)--- */

.search-tab__form {
  display: flex;
  flex-direction: column;
  gap: 12px;
  padding: 12px;
  background: var(--color-bg-elevated);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-md);
}

.search-tab__field {
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.search-tab__label {
  font-size: var(--text-xs);
  font-weight: var(--weight-medium);
  color: var(--color-text-secondary);
}

.search-tab__input {
  padding: 6px 10px;
  background: var(--color-bg-app);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-sm);
  color: var(--color-text-primary);
  font-size: var(--text-sm);
  width: 100%;
  box-sizing: border-box;
}

.search-tab__input:focus {
  outline: none;
  border-color: var(--color-accent);
}

/* --- reka Select(ScheduledTasksTab 同款;trigger 字号与本表单
   input 一致(text-sm),弹层 option 与全局各下拉一致(text-base))--- */

.search-tab__trigger {
  display: inline-flex;
  align-items: center;
  justify-content: space-between;
  gap: 6px;
  padding: 6px 10px;
  background: var(--color-bg-app);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-sm);
  color: var(--color-text-primary);
  font-size: var(--text-sm);
  font-family: inherit;
  width: 100%;
  box-sizing: border-box;
  cursor: pointer;
  transition: border-color var(--duration-base) var(--ease-out);
}

.search-tab__trigger:hover {
  border-color: var(--color-accent-muted);
}

.search-tab__trigger[data-state="open"] {
  border-color: var(--color-accent);
}

.search-tab__trigger-icon {
  color: var(--color-text-muted);
  display: inline-flex;
  align-items: center;
  flex-shrink: 0;
}

/* Portal children —— SelectPortal teleport 到 body,规范要求 :deep()。 */
:deep(.search-tab__dropdown) {
  position: fixed;
  background: var(--color-bg-surface);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-md);
  box-shadow: var(--shadow-md);
  min-width: var(--reka-select-trigger-width, 240px);
  width: var(--reka-select-trigger-width);
  z-index: var(--z-over-modal) !important;
  overflow: hidden;
}

:deep(.search-tab__dropdown-viewport) {
  padding: 4px;
}

:deep(.search-tab__option) {
  display: flex;
  align-items: center;
  padding: 6px 10px;
  font-size: var(--text-base);
  color: var(--color-text-primary);
  border-radius: var(--radius-sm);
  cursor: pointer;
  user-select: none;
}

:deep(.search-tab__option[data-highlighted]) {
  background: var(--color-bg-elevated);
  color: var(--color-text-primary);
}

:deep(.search-tab__option[data-state="checked"]) {
  color: var(--color-accent-text);
}

.search-tab__form-actions {
  display: flex;
  gap: 8px;
  justify-content: flex-end;
}

/* 按钮样式由全局 .btn 家族承载(primary / ghost);此处仅保留家族
   不拥有的字重。 */
.search-tab__btn {
  font-weight: var(--weight-medium);
}

.search-tab__hint {
  margin: 0;
  font-size: var(--text-xs);
  color: var(--color-text-secondary);
  line-height: 1.5;
}

/* --- error banner(mirrors RemoteTab)--- */

.search-tab__error {
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
