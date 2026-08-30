<script setup lang="ts">
// GeneralTab — Settings「通用」分类(2026-08-29 settings-shell 重构)。
//
// 收编两个此前没有 UI 入口的后端开关:轮次完成 toast 通知
// (`turn_complete_notify_enabled`,F6)与定时任务总调度 kill switch
// (`scheduled_tasks_enabled`,F2,关掉后任务可建但不触发)。读值来自
// `useConfigStore`(app 启动时经 get_app_config 拉取,fail-open 缺省
// 开);写入走 store 的 setter(set_app_config_flag,后端白名单校验)。
//
// P3b(2026-08-31, task 08-31-a2-p3b-sandbox-executor):新增执行期
// 沙盒 kill switch(`sandbox_enabled`,同款布尔白名单通道)、能力探测
// 徽标(`sandbox_capability` 只读派生,生效/已回退)与「额外可写目录」
// 列表编辑(写通道 `set_app_config_list`,评审 W1;生效清单 = 本列表
// + 后端并入的 ~/.cargo 默认项)。
//
// 写入策略:pending 期间 switch 保持点击后的目标态(乐观)但禁点;
// 失败回拨到旧值并 toast。switch 样式复用 ScheduledTasksTab 的
// role="switch" 手搓药丸(36×20 + 滑块),该形态已是项目内开关惯例。

import { reactive, ref } from "vue";
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
  {
    key: "sandboxEnabled",
    title: "只读命令沙盒",
    description:
      "只读档 shell 命令(LS、git diff 等)在 Landlock+seccomp 沙盒下执行:可写范围限定在项目目录、/tmp 与应用输出目录,且禁止联网。关闭后恢复旧行为。",
    value: () => config.sandboxEnabled,
    set: (on) => config.setSandboxEnabled(on),
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

// ----- P3b:额外可写目录列表编辑(评审 W1 写通道) -----

/** In-flight flag for list writes (button disabled + optimistic
 *  rollback on error, same policy as the switches above). */
const listPending = ref(false);
const newListEntry = ref("");

function removeExtraWritable(idx: number): void {
  if (listPending.value) return;
  const next = config.sandboxExtraWritable.filter((_, i) => i !== idx);
  listPending.value = true;
  config
    .setSandboxExtraWritable(next)
    .catch((e) => projects.showToast(`设置失败：${extractErrorMessage(e)}`, "error"))
    .finally(() => {
      listPending.value = false;
    });
}

function addExtraWritable(): void {
  const entry = newListEntry.value.trim();
  if (!entry || listPending.value) return;
  if (config.sandboxExtraWritable.includes(entry)) {
    newListEntry.value = "";
    return;
  }
  listPending.value = true;
  config
    .setSandboxExtraWritable([...config.sandboxExtraWritable, entry])
    .then(() => {
      newListEntry.value = "";
    })
    .catch((e) => projects.showToast(`设置失败：${extractErrorMessage(e)}`, "error"))
    .finally(() => {
      listPending.value = false;
    });
}
</script>

<template>
  <div class="general-tab">
    <ul class="general-tab__list">
      <li v-for="row in rows" :key="row.key" class="general-tab__row">
        <div class="general-tab__text">
          <span class="general-tab__title">
            {{ row.title }}
            <span
              v-if="row.key === 'sandboxEnabled' && config.sandboxCapability !== null"
              class="general-tab__cap"
              :class="{
                'general-tab__cap--ok': config.sandboxCapability,
                'general-tab__cap--off': !config.sandboxCapability,
              }"
            >
              {{ config.sandboxCapability ? "沙盒生效" : "已回退(不沙盒)" }}
            </span>
          </span>
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

    <!-- P3b:额外可写目录。生效清单由后端 get_app_config 返回(含
         ~/.cargo 默认项);这里只编辑用户追加的部分,移除按钮对
         默认项隐藏(~/.cargo 是后端并入,不在编辑列表里,自然不可删)。 -->
    <div class="general-tab__extra">
      <span class="general-tab__title">沙盒额外可写目录</span>
      <span class="general-tab__desc">
        在沙盒可写范围(项目目录、/tmp、应用输出、~/.cargo)之外,允许只读命令写入的目录。
      </span>
      <ul v-if="config.sandboxExtraWritable.length" class="general-tab__extra-list">
        <li v-for="(entry, idx) in config.sandboxExtraWritable" :key="entry" class="general-tab__extra-item">
          <code class="general-tab__extra-path">{{ entry }}</code>
          <button
            type="button"
            class="general-tab__extra-remove"
            :aria-label="`移除 ${entry}`"
            :disabled="listPending"
            @click="removeExtraWritable(idx)"
          >
            ✕
          </button>
        </li>
      </ul>
      <div class="general-tab__extra-add">
        <input
          v-model="newListEntry"
          type="text"
          class="general-tab__extra-input"
          placeholder="追加目录,如 /opt/build-cache(支持 ~)"
          aria-label="追加沙盒可写目录"
          @keydown.enter.prevent="addExtraWritable"
        />
        <button
          type="button"
          class="general-tab__extra-addbtn"
          :disabled="listPending || !newListEntry.trim()"
          @click="addExtraWritable"
        >
          添加
        </button>
      </div>
    </div>
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

/* P3b:能力探测徽标(生效/已回退)+ 额外可写目录编辑 */
.general-tab__cap {
  display: inline-block;
  margin-left: var(--space-2);
  padding: 1px var(--space-2);
  border-radius: 999px;
  font-size: var(--text-xs);
  font-weight: var(--weight-medium);
  vertical-align: middle;
}

.general-tab__cap--ok {
  background: var(--color-bg-inset);
  color: var(--color-text-secondary);
}

.general-tab__cap--off {
  background: var(--color-warning-bg, var(--color-bg-inset));
  color: var(--color-warning-text, var(--color-text-secondary));
}

.general-tab__extra {
  display: flex;
  flex-direction: column;
  gap: var(--space-1);
  padding: var(--space-3) 0;
}

.general-tab__extra-list {
  list-style: none;
  margin: var(--space-2) 0 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: var(--space-1);
}

.general-tab__extra-item {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--space-2);
}

.general-tab__extra-path {
  font-size: var(--text-sm);
  color: var(--color-text-primary);
  overflow-wrap: anywhere;
}

.general-tab__extra-remove {
  flex-shrink: 0;
  border: 0;
  background: transparent;
  color: var(--color-text-secondary);
  cursor: pointer;
  padding: 0 var(--space-2);
  font-size: var(--text-sm);
}

.general-tab__extra-remove:disabled {
  opacity: 0.5;
  cursor: default;
}

.general-tab__extra-add {
  display: flex;
  gap: var(--space-2);
  margin-top: var(--space-2);
}

.general-tab__extra-input {
  flex: 1;
  min-width: 0;
}

.general-tab__extra-addbtn {
  flex-shrink: 0;
}

/* 移动端:全局 `.settings-modal button { min-height: 44px }` 会把
   药丸撑成大方块;与 ScheduledTasksTab 同款压回视觉尺寸(DEC-6
   chip 例外,触控目标由整行承担)。 */
@media (max-width: 767px) {
  .general-tab__switch {
    min-width: 0;
    min-height: 0;
  }

  .general-tab__extra-remove {
    min-width: 0;
    min-height: 0;
  }
}
</style>
