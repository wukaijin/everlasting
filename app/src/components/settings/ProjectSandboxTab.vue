<script setup lang="ts">
// ProjectSandboxTab — Settings「项目」scope → 项目沙盒(P3c,
// task 09-01-a2-p3c-sandbox-ux,design §2)。
//
// 三态 per-project 沙盒策略选择,写通道
// `update_project_sandbox_policy`(daemon route + Tauri command 双端,
// 后端白名单校验 off/readwrite/readonly)。档位语义(PRD D2):
// - off(放行):该项目无沙盒,经典审批路径(P3b 前行为);
// - readwrite(读写,默认):全命令进沙盒,项目内自由读写,边界外
//   收紧(面外写/断网 → 升级审批卡);
// - readonly(只读):硬隔离/审计第三方仓库,worktree 亦不可写。
//
// 不变量提示文案(设计要求显式说明):全局 kill-switch 是 master
// (关 = 全局无沙盒,优先于档位);Yolo 模式恒不沙盒。Plan 模式
// 下 session 级只读面覆盖项目档位。

import { computed, ref, watch } from "vue";
import { useProjectsStore } from "../../stores/projects";
import { extractErrorMessage } from "../../utils/useErrorBus";

const props = defineProps<{
  /** 项目 scope 选择器当前选中的项目 id(null = 无可见项目)。 */
  projectId: string | null;
}>();

const projects = useProjectsStore();

type Policy = "off" | "readwrite" | "readonly";

interface PolicyOption {
  value: Policy;
  title: string;
  description: string;
}

const OPTIONS: PolicyOption[] = [
  {
    value: "off",
    title: "放行",
    description: "该项目不走沙盒,shell 命令走经典审批路径(弹窗确认)。",
  },
  {
    value: "readwrite",
    title: "读写(默认)",
    description:
      "全命令进沙盒:项目目录内自由读写,/tmp 与 ~/.cargo 可写,禁止联网;越界写或联网时弹一次审批卡(可记住)。",
  },
  {
    value: "readonly",
    title: "只读",
    description:
      "硬隔离 / 审计第三方仓库:项目目录也只读(脚本仍可运行),其余同读写档。",
  },
];

const selected = ref<Policy>("readwrite");
const pending = ref(false);

/** 项目切换与 store 刷新(loadProjects / 他处改档)都回同步本地
 *  选中;in-flight 期间不回写(避免覆盖乐观选中)。 */
watch(
  [
    () => props.projectId,
    () => projects.projectById(props.projectId)?.sandbox_policy,
  ],
  () => {
    if (pending.value) return;
    selected.value = projects.projectById(props.projectId)?.sandbox_policy ?? "readwrite";
  },
  { immediate: true },
);

const currentProject = computed(() => projects.projectById(props.projectId));

/** v-model 先行(乐观):radio 点击即改本地选中;写失败回拨到
 *  项目当前档位并 toast(与开关行「乐观 + 失败回拨」同款策略)。 */
async function onSelect(value: Policy): Promise<void> {
  const current = projects.projectById(props.projectId)?.sandbox_policy ?? "readwrite";
  if (!props.projectId || pending.value || value === current) {
    selected.value = current;
    return;
  }
  pending.value = true;
  try {
    await projects.setProjectSandboxPolicy(props.projectId, value);
  } catch (e) {
    selected.value = current;
    projects.showToast(`设置失败：${extractErrorMessage(e)}`, "error");
  } finally {
    pending.value = false;
  }
}
</script>

<template>
  <div class="project-sandbox-tab">
    <template v-if="projectId && currentProject">
      <p class="project-sandbox-tab__hint">
        全局沙盒开关(通用设置)是总闸:关闭时所有项目均不沙盒。Yolo
        模式下沙盒恒不生效。
      </p>
      <div role="radiogroup" aria-label="项目沙盒策略" class="project-sandbox-tab__group">
        <label
          v-for="opt in OPTIONS"
          :key="opt.value"
          class="project-sandbox-tab__option"
          :class="{ 'project-sandbox-tab__option--active': selected === opt.value }"
        >
          <input
            v-model="selected"
            type="radio"
            name="project-sandbox-policy"
            :value="opt.value"
            :disabled="pending"
            class="project-sandbox-tab__radio"
            @change="onSelect(opt.value)"
          />
          <span class="project-sandbox-tab__option-title">{{ opt.title }}</span>
          <span class="project-sandbox-tab__option-desc">{{ opt.description }}</span>
        </label>
      </div>
    </template>
    <p v-else class="project-sandbox-tab__empty">没有可选项目。</p>
  </div>
</template>

<style scoped>
.project-sandbox-tab {
  display: flex;
  flex-direction: column;
  gap: var(--space-3);
}

.project-sandbox-tab__hint {
  margin: 0;
  font-size: var(--text-sm);
  color: var(--color-text-secondary);
  line-height: var(--leading-normal);
}

.project-sandbox-tab__group {
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
}

.project-sandbox-tab__option {
  display: grid;
  grid-template-columns: auto 1fr;
  grid-template-rows: auto auto;
  column-gap: var(--space-2);
  align-items: center;
  padding: var(--space-3);
  border: 1px solid var(--color-bg-border);
  border-radius: var(--radius-md, 8px);
  cursor: pointer;
  transition: border-color var(--duration-base) var(--ease-out);
}

.project-sandbox-tab__option--active {
  border-color: var(--color-accent);
}

.project-sandbox-tab__radio {
  grid-row: 1 / span 2;
  accent-color: var(--color-accent);
}

.project-sandbox-tab__option-title {
  font-size: var(--text-base);
  font-weight: var(--weight-medium);
  color: var(--color-text-primary);
}

.project-sandbox-tab__option-desc {
  grid-column: 2;
  font-size: var(--text-sm);
  color: var(--color-text-secondary);
  line-height: var(--leading-normal);
}

.project-sandbox-tab__empty {
  margin: 0;
  padding: var(--space-4);
  font-size: var(--text-sm);
  color: var(--color-text-muted);
  text-align: center;
}

/* 移动端:设置弹窗的 44px 最小触控高度对 radio 无意义,压回自然尺寸。 */
@media (max-width: 767px) {
  .project-sandbox-tab__radio {
    min-width: 0;
    min-height: 0;
  }
}
</style>
