import { defineStore } from "pinia";
import { ref, computed, watch } from "vue";
import { transport } from "../transport";

import { useProvidersStore } from "./providers";
import { useModelsStore } from "./models";

/** localStorage key for the last active project id. Restored on app
 *  start (Q1 / PROPOSAL §5.5). The value is a project UUID; if it
 *  doesn't match any loaded project on start, the chat store's
 *  watcher falls back to the first visible project. */
const LAST_ACTIVE_PROJECT_KEY = "everlasting.lastActiveProjectId";
const LAST_SESSION_KEY_PREFIX = "everlasting.lastSession_";

export const useConfigStore = defineStore("config", () => {
  const loaded = ref(false);

  // PR3 (BACKLOG §5.1 follow-up): the home directory is fetched once
  // on app start and cached here so the chat panel header can
  // shorten the cwd display (`/home/carlos/code/foo` -> `~/code/foo`).
  // `null` means "not yet loaded" or "load failed" — in either case
  // the helper `simplifyPath` returns the original path unchanged,
  // so the UI is safe to render before this resolves.
  const homeDir = ref<string | null>(null);

  // Persisted across sessions via localStorage. Loaded synchronously
  // at store creation so it's available before the chat store's
  // watcher fires its first run.
  const lastActiveProjectId = ref<string | null>(readLastActive());

  // -----------------------------------------------------------------------
  // Backward-compatible computed properties (derived from the catalog).
  // Existing components (Settings status badges, etc.) still read these.
  // Step 5 will clean up all call sites and remove these fields.
  // -----------------------------------------------------------------------

  /** The display name of the current default model, or `""` if none.
   *  Note: Pinia auto-unwraps refs on the store proxy, so
   *  `useModelsStore().defaultModel` is already `ModelWithProvider | null`
   *  (not a ComputedRef). We read it directly without `.value`. */
  const model = computed<string>(() => {
    const modelsStore = useModelsStore();
    return modelsStore.defaultModel?.displayName ?? "";
  });

  /** The base URL of the default model's provider, or `""` if none. */
  const baseUrl = computed<string>(() => {
    const modelsStore = useModelsStore();
    const dm = modelsStore.defaultModel;
    if (!dm) return "";
    const provider = useProvidersStore().byId(dm.providerId);
    return provider?.baseUrl ?? "";
  });

  /** True when a default model exists AND its provider has an api_key
   *  set (hasKey). Drives the Settings tab's "(api key 未设置)" hint
   *  and the warn styling. RULE-D-001: 后端只回传 hasKey 布尔. */
  const configured = computed<boolean>(() => {
    const modelsStore = useModelsStore();
    const dm = modelsStore.defaultModel;
    if (!dm) return false;
    const provider = useProvidersStore().byId(dm.providerId);
    return !!provider?.hasKey;
  });

  function readLastActive(): string | null {
    try {
      return window.localStorage.getItem(LAST_ACTIVE_PROJECT_KEY);
    } catch {
      return null;
    }
  }

  function writeLastActive(id: string | null): void {
    try {
      if (id) {
        window.localStorage.setItem(LAST_ACTIVE_PROJECT_KEY, id);
      } else {
        window.localStorage.removeItem(LAST_ACTIVE_PROJECT_KEY);
      }
    } catch {
      // localStorage may be unavailable (private mode, etc.) — fail
      // silently; the in-memory value is still correct.
    }
  }

  // F1: per-project last active session persistence.
  function readLastSession(projectId: string): string | null {
    try {
      return window.localStorage.getItem(LAST_SESSION_KEY_PREFIX + projectId);
    } catch {
      return null;
    }
  }

  function writeLastSession(projectId: string, sessionId: string | null): void {
    try {
      if (sessionId) {
        window.localStorage.setItem(LAST_SESSION_KEY_PREFIX + projectId, sessionId);
      } else {
        window.localStorage.removeItem(LAST_SESSION_KEY_PREFIX + projectId);
      }
    } catch {
      // fail silently
    }
  }

  // Persist on every change. The chat store updates
  // `lastActiveProjectId` whenever the user switches tabs.
  watch(lastActiveProjectId, (id) => {
    writeLastActive(id);
  });

  // F6 异步 agent 任务(2026-08-27):跨 session 轮次完成 toast 总开关。
  // 缺省 true(fail-open,与后端 `turn_complete_notify_enabled` 读法一致);
  // 仅当后端 app_config 显式存了 "false" 才关。加载失败维持 true ——
  // 通知是观测层增强,配置读失败不应静默吞掉它。
  const turnCompleteNotify = ref(true);

  // F2 定时任务(2026-08-28):全局调度 kill switch 的展示值(读法与
  // 后端调度循环一致,fail-open 缺省开)。仅「定时任务」面板的状态行
  // 消费;关掉时不做硬拦截(与后端语义一致:可建任务,只是不触发)。
  const scheduledTasksEnabled = ref(true);

  // P3b 执行期沙盒(2026-08-31, task 08-31-a2-p3b-sandbox-executor):
  // kill switch 展示值(fail-open 缺省开)、额外可写目录生效清单
  // (含后端并入的 ~/.cargo 默认项)与能力探测只读派生值(null =
  // 尚未加载/旧 daemon 无该字段,设置面此时不显示徽标)。
  // RULE-SBX-002(P3c):生效清单降级为**展示层**;编辑走 raw 列表
  // (下方 sandboxExtraWritableRaw),否则「移除 ~/.cargo 后又被后端
  // 并入 → 复活」。
  const sandboxEnabled = ref(true);
  const sandboxExtraWritable = ref<string[]>([]);
  const sandboxExtraWritableRaw = ref<string[]>([]);
  const sandboxCapability = ref<boolean | null>(null);

  // F3 磁盘治理(2026-09-03, task 09-03-f3-disk-governance):每日磁盘
  // 回收节拍 kill switch 与有主 outputs 按龄回收开关的展示值(读法与
  // 后端 fail-open 一致:仅字面 "false" 关)。节拍开关 false = 自动
  // 回收停,但手动「立即清理」仍可用(PR3 DiskTab 消费)。
  const diskGovernorEnabled = ref(true);
  const outputsAgeCleanupEnabled = ref(true);

  // 问询永不超时(2026-09-03, task 09-03-ask-no-timeout):全局 enable
  // 开关(fail-closed,缺省 false —— 与上述 kill-switch 的 fail-open
  // 方向相反,对齐后端 `ask_no_timeout` 读法:仅字面 "true" 开)。开 =
  // 权限审批卡与轮数上限「继续?」软卡不再自动超时收尾,一直挂着等
  // 用户响应。permissions.ts 据此不 arm 本地 120s 计时器。
  const askNoTimeout = ref(false);

  async function load() {
    // Load providers + models from the catalog (replaces the old
    // `get_llm_config` env path). Store references are obtained at
    // runtime (inside the function body) to avoid Pinia circular
    // dependency issues during setup.
    const providersStore = useProvidersStore();
    const modelsStore = useModelsStore();

    await Promise.all([providersStore.load(), modelsStore.load()]);

    // PR3: home_dir is a best-effort cache for display. A failure
    // (rare — sandboxed container without `$HOME`) is logged but
    // never propagates; the UI degrades to rendering the full
    // cwd path. We deliberately do NOT roll this into the same
    // `try` as the catalog: a missing provider/api_key would
    // otherwise mask the home-dir load.
    try {
      homeDir.value = await transport.invoke<string | null>("get_home_dir");
    } catch (e) {
      console.error("failed to load home dir:", e);
      homeDir.value = null;
    } finally {
      loaded.value = true;
    }

    // F6: app_config 开关面,同样 best-effort(旧 daemon 无此命令时
    // 维持缺省 true)。
    try {
      const appConfig = await transport.invoke<{
        turnCompleteNotifyEnabled: boolean;
        scheduledTasksEnabled?: boolean;
        sandboxEnabled?: boolean;
        sandboxExtraWritable?: string[];
        sandboxExtraWritableRaw?: string[];
        sandboxCapability?: boolean;
        diskGovernorEnabled?: boolean;
        outputsAgeCleanupEnabled?: boolean;
        askNoTimeout?: boolean;
      }>("get_app_config");
      turnCompleteNotify.value = appConfig.turnCompleteNotifyEnabled !== false;
      // F2:additive 字段(旧 daemon 缺省 true)。
      scheduledTasksEnabled.value = appConfig.scheduledTasksEnabled !== false;
      // P3b:additive 三字段(旧 daemon 缺省 true / [] / null)。
      sandboxEnabled.value = appConfig.sandboxEnabled !== false;
      sandboxExtraWritable.value = appConfig.sandboxExtraWritable ?? [];
      // P3c(RULE-SBX-002):raw 编辑清单,旧 daemon 缺省 []。
      sandboxExtraWritableRaw.value = appConfig.sandboxExtraWritableRaw ?? [];
      sandboxCapability.value = appConfig.sandboxCapability ?? null;
      // F3:additive 两字段(旧 daemon 缺省 true)。
      diskGovernorEnabled.value = appConfig.diskGovernorEnabled !== false;
      outputsAgeCleanupEnabled.value = appConfig.outputsAgeCleanupEnabled !== false;
      // ask_no_timeout:enable 语义 fail-closed —— 仅字面 true 开,
      // 旧 daemon / 未存缺省 false(与 kill-switch 的 !== false 相反)。
      askNoTimeout.value = appConfig.askNoTimeout === true;
    } catch (e) {
      console.warn("get_app_config unavailable, keep toast default on:", e);
    }
  }

  // -----------------------------------------------------------------------
  // Settings「通用」开关写入口(2026-08-29 settings-shell)。写成功后才
  // 更新本地 ref(失败时调用方 toast,本地值保持 DB 现状);key 常量与
  // 后端 `SETTABLE_APP_FLAGS` 白名单一一对应。
  // -----------------------------------------------------------------------

  /** Toggle the per-turn completion toast (app_config
   *  `turn_complete_notify_enabled`). Throws on transport error —
   *  the caller keeps the switch on the pre-toggle value. */
  async function setTurnCompleteNotify(on: boolean): Promise<void> {
    await transport.invoke("set_app_config_flag", {
      key: "turn_complete_notify_enabled",
      value: on,
    });
    turnCompleteNotify.value = on;
  }

  /** Toggle the scheduled-tasks global kill switch (app_config
   *  `scheduled_tasks_enabled`). Fail-open semantics upstream:
   *  only a literal `"false"` disables the scheduler tick. */
  async function setScheduledTasksEnabled(on: boolean): Promise<void> {
    await transport.invoke("set_app_config_flag", {
      key: "scheduled_tasks_enabled",
      value: on,
    });
    scheduledTasksEnabled.value = on;
  }

  /** Toggle the sandbox kill switch (app_config
   *  `sandbox_enabled`, P3b R6/D1). Fail-open upstream: only a
   *  literal `"false"` disables sandboxing. */
  async function setSandboxEnabled(on: boolean): Promise<void> {
    await transport.invoke("set_app_config_flag", {
      key: "sandbox_enabled",
      value: on,
    });
    sandboxEnabled.value = on;
  }

  /** Toggle the disk-governor daily-beat kill switch (app_config
   *  `disk_governor_enabled`, F3). Fail-open upstream; `false` stops
   *  the automatic beat but the manual "clean up now" entry stays
   *  available (AC9). */
  async function setDiskGovernorEnabled(on: boolean): Promise<void> {
    await transport.invoke("set_app_config_flag", {
      key: "disk_governor_enabled",
      value: on,
    });
    diskGovernorEnabled.value = on;
  }

  /** Toggle the owned-outputs age-based recycling switch (app_config
   *  `outputs_age_cleanup_enabled`, F3). Fail-open upstream; orphan
   *  buckets and `_no_session` are NOT governed by this switch. */
  async function setOutputsAgeCleanupEnabled(on: boolean): Promise<void> {
    await transport.invoke("set_app_config_flag", {
      key: "outputs_age_cleanup_enabled",
      value: on,
    });
    outputsAgeCleanupEnabled.value = on;
  }

  /** Toggle the global no-timeout switch (app_config
   *  `ask_no_timeout`, 2026-09-03 task 09-03-ask-no-timeout).
   *  Enable semantics — fail-closed upstream: only a literal
   *  `"true"` turns it on. When on, permission asks + the
   *  turn-limit softcap never auto-settle (frontend arms no
   *  120s timer either). */
  async function setAskNoTimeout(on: boolean): Promise<void> {
    await transport.invoke("set_app_config_flag", {
      key: "ask_no_timeout",
      value: on,
    });
    askNoTimeout.value = on;
  }

  /** Persist the RAW extra-writable list (RULE-SBX-002, P3c): the
   *  editable list is exactly what lands in app_config — the `~/.cargo`
   *  default is NOT part of it (the backend merges it in at read
   *  time; the effective `sandboxExtraWritable` ref is display-only
   *  and refreshes via `load()`). */
  async function setSandboxExtraWritableRaw(list: string[]): Promise<void> {
    await transport.invoke("set_app_config_list", {
      key: "sandbox_extra_writable",
      value: list,
    });
    sandboxExtraWritableRaw.value = list;
  }

  return {
    model,
    baseUrl,
    configured,
    loaded,
    homeDir,
    turnCompleteNotify,
    scheduledTasksEnabled,
    sandboxEnabled,
    sandboxExtraWritable,
    sandboxExtraWritableRaw,
    sandboxCapability,
    diskGovernorEnabled,
    outputsAgeCleanupEnabled,
    askNoTimeout,
    lastActiveProjectId,
    readLastSession,
    writeLastSession,
    setTurnCompleteNotify,
    setScheduledTasksEnabled,
    setSandboxEnabled,
    setDiskGovernorEnabled,
    setOutputsAgeCleanupEnabled,
    setAskNoTimeout,
    setSandboxExtraWritableRaw,
    load,
  };
});
