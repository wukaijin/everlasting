<script setup lang="ts">
// CliTab — Settings「集成」分类(CLI (evl)):宿主机 evl CLI 检测 + 一键安装。
//
// 两段式:
//   1. 环境检测:detect_evl 渲染 Node / evl 两行状态(安装形态 chip +
//      版本 + 路径),进入 tab 自动拉一次;「重新检测」按钮。检测在
//      daemon 进程执行 —— 状态反映的是 **daemon 宿主机**(remote 场景
//      即远端机器)。
//   2. 安装 / 更新:notInstalled → 「安装」;managed 且版本 ≠ 内置 →
//      「更新」;external(用户自己 pnpm link / 手动放置)→ 不提供
//      覆盖安装,只显示说明。
// 安装 = daemon 内嵌文件写出 `{data_dir}/cli/` + symlink
// `~/.local/bin/evl`(不覆盖非托管文件)。按钮直调 IPC 不经全局
// store —— 状态仅本 tab 消费(DiskTab 同款)。

import { computed, onMounted, ref } from "vue";
import { transport } from "../../transport";
import { useProjectsStore } from "../../stores/projects";
import { extractErrorMessage } from "../../utils/useErrorBus";

const projects = useProjectsStore();

/** 后端 EvlCliStatusPayload 的 camelCase wire 形态。 */
interface EvlNodeStatus {
  found: boolean;
  version?: string | null;
  ok: boolean;
  reason?: string | null;
}

interface EvlStatus {
  state: "notInstalled" | "managed" | "external";
  path?: string | null;
  version?: string | null;
  onPath: boolean;
}

interface EvlCliStatusPayload {
  bundledVersion: string;
  node: EvlNodeStatus;
  evl: EvlStatus;
  localBinDir: string;
  localBinOnPath: boolean;
}

const status = ref<EvlCliStatusPayload | null>(null);
const loading = ref(false);
const installing = ref(false);

const STATE_LABEL: Record<EvlStatus["state"], string> = {
  notInstalled: "未安装",
  managed: "已安装 · 应用内置",
  external: "已安装 · 外部",
};

/** 可安装:未装 + Node 满足。 */
const canInstall = computed(
  () =>
    !!status.value &&
    status.value.evl.state === "notInstalled" &&
    status.value.node.ok,
);

/** 可更新:托管安装且版本 ≠ 内置(同源版本号,不等即落后/漂移)。 */
const canUpdate = computed(
  () =>
    !!status.value &&
    status.value.evl.state === "managed" &&
    status.value.evl.version !== status.value.bundledVersion,
);

async function refresh(): Promise<void> {
  if (loading.value) return;
  loading.value = true;
  try {
    status.value = await transport.invoke<EvlCliStatusPayload>("detect_evl");
  } catch (e) {
    projects.showToast(`检测失败:${extractErrorMessage(e)}`, "error");
  } finally {
    loading.value = false;
  }
}

async function install(): Promise<void> {
  if (installing.value) return;
  installing.value = true;
  try {
    status.value = await transport.invoke<EvlCliStatusPayload>("install_evl");
    projects.showToast("evl CLI 安装完成", "info");
  } catch (e) {
    projects.showToast(`安装失败:${extractErrorMessage(e)}`, "error");
  } finally {
    installing.value = false;
  }
}

onMounted(refresh);
</script>

<template>
  <div class="cli-tab">
    <!-- 第 1 段:环境检测 -->
    <section class="cli-tab__section">
      <div class="cli-tab__section-head">
        <span class="cli-tab__section-title">环境检测</span>
        <button
          type="button"
          class="btn btn--ghost cli-tab__refresh"
          :disabled="loading"
          @click="refresh"
        >
          {{ loading ? "检测中…" : "重新检测" }}
        </button>
      </div>

      <p v-if="loading && !status" class="cli-tab__hint">正在检测宿主机环境…</p>
      <template v-else-if="status">
        <ul class="cli-tab__status-list">
          <li class="cli-tab__status-row">
            <span class="cli-tab__status-label">Node</span>
            <span v-if="status.node.ok" class="cli-tab__status-value">
              {{ status.node.version ?? "已安装" }}(≥ 20 ✓)
            </span>
            <span v-else class="cli-tab__status-value cli-tab__status-value--warn">
              {{ status.node.reason ?? "不可用" }}
            </span>
          </li>
          <li class="cli-tab__status-row">
            <span class="cli-tab__status-label">evl CLI</span>
            <span class="cli-tab__status-value">
              <span
                class="cli-tab__chip"
                :class="{
                  'cli-tab__chip--ok': status.evl.state !== 'notInstalled',
                  'cli-tab__chip--muted': status.evl.state === 'notInstalled',
                }"
              >
                {{ STATE_LABEL[status.evl.state] }}
              </span>
              <span v-if="status.evl.version" class="cli-tab__version">
                {{ status.evl.version }}
              </span>
            </span>
          </li>
          <li v-if="status.evl.path" class="cli-tab__status-row">
            <span class="cli-tab__status-label">路径</span>
            <span class="cli-tab__status-value cli-tab__path">{{ status.evl.path }}</span>
          </li>
          <li class="cli-tab__status-row">
            <span class="cli-tab__status-label">内置版本</span>
            <span class="cli-tab__status-value">{{ status.bundledVersion }}(随应用更新)</span>
          </li>
        </ul>

        <p v-if="!status.node.ok" class="cli-tab__hint" role="alert">
          evl CLI 需要 Node ≥ 20,请先安装 / 升级 Node 再使用一键安装。
        </p>
        <p
          v-if="status.evl.state === 'managed' && !status.evl.onPath"
          class="cli-tab__hint"
          role="alert"
        >
          已安装但当前未在 PATH 上直接可见:请确认 {{ status.localBinDir }} 在你的 shell
          PATH 中(通常重开终端即生效)。
        </p>
        <p v-if="status.evl.state === 'external'" class="cli-tab__hint">
          检测到非本应用安装的 evl({{ status.evl.path ?? "路径未知"
          }}),不做覆盖;如需改用应用内置版,请先移除该文件再回到此页安装。
        </p>
        <p class="cli-tab__hint cli-tab__hint--dim">
          检测以 daemon 运行环境为准(remote 场景即远端宿主机)。
        </p>
      </template>
      <p v-else class="cli-tab__hint">暂无数据,点击「重新检测」。</p>
    </section>

    <!-- 第 2 段:安装 / 更新 -->
    <section class="cli-tab__section">
      <span class="cli-tab__section-title">安装</span>
      <p class="cli-tab__desc">
        将应用内置的 evl CLI 写入数据目录并在 {{ status?.localBinDir ?? "~/.local/bin" }}
        创建 evl 命令(不覆盖已存在的外部文件;重复安装即为更新)。
      </p>
      <button
        v-if="canInstall || canUpdate"
        type="button"
        class="btn btn--primary cli-tab__install"
        :disabled="installing"
        @click="install"
      >
        {{
          installing
            ? "安装中…"
            : canUpdate
              ? `更新到内置版本 ${status?.bundledVersion}`
              : "安装 evl CLI"
        }}
      </button>
      <p v-if="status?.evl.state === 'managed' && !canUpdate && !installing" class="cli-tab__hint">
        已是最新(内置 {{ status.bundledVersion }})。
      </p>
      <p v-if="status?.evl.state === 'managed'" class="cli-tab__hint cli-tab__hint--dim">
        移除:rm {{ status.evl.path ?? `${status.localBinDir}/evl` }} 并删除数据目录下的
        cli/ 文件夹。
      </p>
    </section>
  </div>
</template>

<style scoped>
.cli-tab {
  display: flex;
  flex-direction: column;
  gap: var(--space-4);
}

.cli-tab__section {
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
}

.cli-tab__section + .cli-tab__section {
  border-top: 1px solid var(--color-bg-border);
  padding-top: var(--space-4);
}

.cli-tab__section-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--space-2);
}

.cli-tab__section-title {
  font-size: var(--text-base);
  font-weight: var(--weight-medium);
  color: var(--color-text-primary);
}

.cli-tab__refresh {
  flex-shrink: 0;
}

.cli-tab__status-list {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
}

.cli-tab__status-row {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  gap: var(--space-4);
  padding: var(--space-2) 0;
  border-bottom: 1px solid var(--color-bg-border);
}

.cli-tab__status-row:last-child {
  border-bottom: 0;
}

.cli-tab__status-label {
  flex-shrink: 0;
  font-size: var(--text-sm);
  color: var(--color-text-secondary);
}

.cli-tab__status-value {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: var(--space-2);
  min-width: 0;
  font-size: var(--text-sm);
  color: var(--color-text-primary);
  text-align: right;
}

.cli-tab__status-value--warn {
  color: var(--color-text-primary);
  font-weight: var(--weight-medium);
}

.cli-tab__path {
  font-family: var(--font-mono, monospace);
  font-size: var(--text-sm);
  color: var(--color-text-secondary);
  overflow-wrap: anywhere;
}

.cli-tab__chip {
  flex-shrink: 0;
  padding: 1px var(--space-2);
  border-radius: var(--radius-sm, 4px);
  border: 1px solid var(--color-bg-border-strong);
  font-size: var(--text-sm);
  color: var(--color-text-secondary);
}

.cli-tab__chip--ok {
  border-color: var(--color-accent);
  color: var(--color-accent);
}

.cli-tab__chip--muted {
  opacity: 0.8;
}

.cli-tab__version {
  font-variant-numeric: tabular-nums;
  color: var(--color-text-secondary);
}

.cli-tab__desc {
  font-size: var(--text-sm);
  color: var(--color-text-secondary);
  line-height: var(--leading-normal);
}

.cli-tab__hint {
  font-size: var(--text-sm);
  color: var(--color-text-secondary);
  line-height: var(--leading-normal);
}

.cli-tab__hint--dim {
  opacity: 0.75;
}

.cli-tab__install {
  align-self: flex-start;
}
</style>
