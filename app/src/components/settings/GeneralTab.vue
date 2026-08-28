<script setup lang="ts">
// GeneralTab — Settings「通用」分类(2026-08-29 settings-shell 重构)。
//
// 收编两个此前没有 UI 入口的后端开关:轮次完成 toast 通知
// (`turn_complete_notify_enabled`,F6)与定时任务总调度 kill switch
// (`scheduled_tasks_enabled`,F2,关掉后任务可建但不触发)。读值来自
// `useConfigStore`(app 启动时经 get_app_config 拉取,fail-open 缺省
// 开);写入走 store 的 setter(set_app_config_flag,后端白名单校验)。
//
// 写入策略:pending 期间 switch 保持点击后的目标态(乐观)但禁点;
// 失败回拨到旧值并 toast。switch 样式复用 ScheduledTasksTab 的
// role="switch" 手搓药丸(36×20 + 滑块),该形态已是项目内开关惯例。

import { reactive } from "vue";
import { useConfigStore } from "../../stores/config";
import { useProjectsStore } from "../../stores/projects";
import { extractErrorMessage } from "../../utils/useErrorBus";

const config = useConfigStore();
const projects = useProjectsStore();

/** Per-switch in-flight state(keyed like the flag names below);
 *  presence = the write is in flight (switch disabled). */
const pending = reactive<Record<string, boolean>>({});

interface FlagRow {
  key: string;
  title: string;
  description: string;
  /** Current store value (reactive read). */
  value: () => boolean;
  /** Persist via the config store setter. */
  set: (on: boolean) => Promise<void>;
}

const rows: FlagRow[] = [
  {
    key: "turnCompleteNotify",
    title: "轮次完成通知",
    description:
      "轮次完成后弹出 toast 提示(含跨 session 的异步任务完成通知)。关闭后仅在应用内静默完成。",
    value: () => config.turnCompleteNotify,
    set: (on) => config.setTurnCompleteNotify(on),
  },
  {
    key: "scheduledTasksEnabled",
    title: "定时任务调度",
    description:
      "定时任务的总开关。关闭后已有任务不再触发(仍可创建与编辑,与后端 fail-open 语义一致)。",
    value: () => config.scheduledTasksEnabled,
    set: (on) => config.setScheduledTasksEnabled(on),
  },
];

async function onToggle(row: FlagRow): Promise<void> {
  if (pending[row.key]) return;
  const target = !row.value();
  pending[row.key] = true;
  try {
    await row.set(target);
  } catch (e) {
    projects.showToast(`设置失败：${extractErrorMessage(e)}`, "error");
  } finally {
    delete pending[row.key];
  }
}
</script>

<template>
  <div class="general-tab">
    <ul class="general-tab__list">
      <li v-for="row in rows" :key="row.key" class="general-tab__row">
        <div class="general-tab__text">
          <span class="general-tab__title">{{ row.title }}</span>
          <span class="general-tab__desc">{{ row.description }}</span>
        </div>
        <button
          type="button"
          role="switch"
          :aria-checked="row.value()"
          :aria-label="row.title"
          class="general-tab__switch"
          :class="{ 'general-tab__switch--on': row.value() }"
          :disabled="!!pending[row.key]"
          @click="onToggle(row)"
        >
          <span class="general-tab__switch-knob" />
        </button>
      </li>
    </ul>
  </div>
</template>

<style scoped>
.general-tab {
  display: flex;
  flex-direction: column;
}

.general-tab__list {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
}

.general-tab__row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--space-4);
  padding: var(--space-3) 0;
  border-bottom: 1px solid var(--color-bg-border);
}

.general-tab__row:last-child {
  border-bottom: 0;
}

.general-tab__text {
  display: flex;
  flex-direction: column;
  gap: var(--space-1);
  min-width: 0;
}

.general-tab__title {
  font-size: var(--text-base);
  font-weight: var(--weight-medium);
  color: var(--color-text-primary);
}

.general-tab__desc {
  font-size: var(--text-sm);
  color: var(--color-text-secondary);
  line-height: var(--leading-normal);
}

/* 开关药丸:复用 ScheduledTasksTab 的 role="switch" 形态
   (36×20 + 14px 滑块),启用态 accent。 */
.general-tab__switch {
  flex-shrink: 0;
  width: 36px;
  height: 20px;
  border-radius: 999px;
  border: 1px solid var(--color-bg-border-strong);
  background: var(--color-bg-app);
  padding: 0;
  position: relative;
  cursor: pointer;
  transition:
    background var(--duration-base) var(--ease-out),
    border-color var(--duration-base) var(--ease-out);
}

.general-tab__switch--on {
  background: var(--color-accent);
  border-color: var(--color-accent);
}

.general-tab__switch:disabled {
  opacity: 0.5;
  cursor: default;
}

.general-tab__switch-knob {
  position: absolute;
  top: 2px;
  left: 2px;
  width: 14px;
  height: 14px;
  border-radius: 50%;
  background: var(--color-text-primary);
  transition: transform var(--duration-base) var(--ease-out);
}

.general-tab__switch--on .general-tab__switch-knob {
  transform: translateX(16px);
  background: var(--color-text-on-accent);
}

/* 移动端:全局 `.settings-modal button { min-height: 44px }` 会把
   药丸撑成大方块;与 ScheduledTasksTab 同款压回视觉尺寸(DEC-6
   chip 例外,触控目标由整行承担)。 */
@media (max-width: 767px) {
  .general-tab__switch {
    min-width: 0;
    min-height: 0;
  }
}
</style>
