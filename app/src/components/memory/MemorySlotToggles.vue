<script setup lang="ts">
// MemorySlotToggles — 4 槽位记忆植入开关(2026-09-10 hard switch PR2,
// 任务 09-10-memory-everlasting-md-hard-switch;评审 P0 #4)。
//
// scope="user" 渲染用户层两开关(MemoryTab / Settings「全局」),
// scope="project" 渲染项目层两开关(ProjectMemoryTab / Settings「项目」)。
// 经 config store 的 get_app_config / set_app_config_flag 通道读写,
// key 与后端 `memory::flags::KEY_*` 白名单一一对应。
//
// 生效语义提示(文案内):主循环下一会话生效(D2 freeze 机制强制),
// worker 下一 dispatch 生效 —— 开关不是即时打断,是注入口径变更。
//
// 开关药丸复用 GeneralTab 的 role="switch" 手搓形态(36×20 + 滑块,
// 项目内开关惯例);写入策略同款:pending 禁点,失败回拨 + toast。

import { computed, reactive } from "vue";
import { useConfigStore } from "../../stores/config";
import { useProjectsStore } from "../../stores/projects";
import { extractErrorMessage } from "../../utils/useErrorBus";

const props = defineProps<{
  scope: "user" | "project";
}>();

const config = useConfigStore();
const projects = useProjectsStore();

interface ToggleRow {
  key: string;
  title: string;
  description: string;
  value: () => boolean;
  set: (on: boolean) => Promise<void>;
}

const rows = computed<ToggleRow[]>(() => {
  if (props.scope === "user") {
    return [
      {
        key: "memoryUserEverlasting",
        title: "注入 User EVERLASTING.md",
        description:
          "关闭后 ~/.config/everlasting/EVERLASTING.md 不再注入会话(主循环下一会话生效,子代理下一次派发生效)。",
        value: () => config.memoryUserEverlastingEnabled,
        set: (on) => config.setMemoryUserEverlastingEnabled(on),
      },
      {
        key: "memoryUserAgents",
        title: "注入 User AGENTS.md",
        description:
          "关闭后 ~/.config/everlasting/AGENTS.md 不再注入会话(主循环下一会话生效,子代理下一次派发生效)。",
        value: () => config.memoryUserAgentsEnabled,
        set: (on) => config.setMemoryUserAgentsEnabled(on),
      },
    ];
  }
  return [
    {
      key: "memoryProjectEverlasting",
      title: "注入 Project EVERLASTING.md",
      description:
        "关闭后项目根的 EVERLASTING.md 不再注入会话(主循环下一会话生效,子代理下一次派发生效)。",
      value: () => config.memoryProjectEverlastingEnabled,
      set: (on) => config.setMemoryProjectEverlastingEnabled(on),
    },
    {
      key: "memoryProjectAgents",
      title: "注入 Project AGENTS.md",
      description:
        "关闭后项目根的 AGENTS.md 不再注入会话(主循环下一会话生效,子代理下一次派发生效)。",
      value: () => config.memoryProjectAgentsEnabled,
      set: (on) => config.setMemoryProjectAgentsEnabled(on),
    },
  ];
});

const pending = reactive<Record<string, boolean>>({});

async function onToggle(row: ToggleRow): Promise<void> {
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
  <div class="slot-toggles" :data-testid="`memory-slot-toggles-${scope}`">
    <span class="slot-toggles__heading">指令注入开关</span>
    <ul class="slot-toggles__list">
      <li v-for="row in rows" :key="row.key" class="slot-toggles__row">
        <div class="slot-toggles__text">
          <span class="slot-toggles__title">{{ row.title }}</span>
          <span class="slot-toggles__desc">{{ row.description }}</span>
        </div>
        <button
          type="button"
          role="switch"
          :aria-checked="row.value()"
          :aria-label="row.title"
          class="slot-toggles__switch"
          :class="{ 'slot-toggles__switch--on': row.value() }"
          :disabled="!!pending[row.key]"
          @click="onToggle(row)"
        >
          <span class="slot-toggles__switch-knob" />
        </button>
      </li>
    </ul>
  </div>
</template>

<style scoped>
.slot-toggles {
  display: flex;
  flex-direction: column;
  gap: 8px;
  padding: 12px;
  border: 1px solid var(--color-bg-border-strong);
  border-radius: var(--radius-md);
}

.slot-toggles__heading {
  font-size: var(--text-sm);
  font-weight: 600;
  color: var(--color-text-primary);
}

.slot-toggles__list {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.slot-toggles__row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
}

.slot-toggles__text {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
}

.slot-toggles__title {
  font-size: var(--text-sm);
  color: var(--color-text-primary);
}

.slot-toggles__desc {
  font-size: var(--text-xs);
  color: var(--color-text-secondary);
  line-height: var(--leading-normal);
}

/* 开关药丸:GeneralTab 同款 role="switch" 形态(36×20 + 14px 滑块)。 */
.slot-toggles__switch {
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

.slot-toggles__switch--on {
  background: var(--color-accent);
  border-color: var(--color-accent);
}

.slot-toggles__switch:disabled {
  opacity: 0.5;
  cursor: default;
}

.slot-toggles__switch-knob {
  position: absolute;
  top: 2px;
  left: 2px;
  width: 14px;
  height: 14px;
  border-radius: 50%;
  background: var(--color-text-primary);
  transition: transform var(--duration-base) var(--ease-out);
}

.slot-toggles__switch--on .slot-toggles__switch-knob {
  transform: translateX(16px);
  background: var(--color-text-on-accent);
}
</style>
