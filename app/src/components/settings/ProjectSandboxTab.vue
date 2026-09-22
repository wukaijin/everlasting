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
import { useProjectsStore, type NetStateInfo, type ShellGrantInfo } from "../../stores/projects";
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

// -- 网络档(09-21-sandbox-net-bindonly,R4/R8)---------------------------

const netState = ref<NetStateInfo | null>(null);
const netLoading = ref(false);
const netPortsInput = ref("");
const netWorktreeInput = ref("");

async function loadNetState(): Promise<void> {
  if (!props.projectId) {
    netState.value = null;
    return;
  }
  netLoading.value = true;
  try {
    netState.value = await projects.getProjectNetState(props.projectId);
  } catch (e) {
    projects.showToast(`读取网络档失败：${extractErrorMessage(e)}`, "error");
    netState.value = null;
  } finally {
    netLoading.value = false;
  }
}

watch(
  () => props.projectId,
  (id) => {
    netWorktreeInput.value =
      projects.projectById(id)?.path ?? "";
    netPortsInput.value = "";
    void loadNetState();
  },
  { immediate: true },
);

/** 网络档显示值:block(含缺省)/ bind_only / allow_all(只读展示)。 */
const netTier = computed<"block" | "bind_only" | "allow_all">(() => {
  const t = netState.value?.tier ?? currentProject.value?.sandbox_net ?? null;
  if (!t) return "block";
  if (t === "allow_all") return "allow_all";
  if (t.startsWith("bind_only")) return "bind_only";
  return "block";
});

/** 最新已确认快照(项目级默认显示/切档时引用)。 */
const latestSnapshot = computed(
  () => netState.value?.snapshots[0] ?? null,
);
const pendingProposals = computed(() =>
  (netState.value?.proposals ?? []).filter((p) => p.status === "pending"),
);
/** 文案③:平台不支持(BindOnly 执法需 Landlock ABI ≥4)→ 禁写。 */
const netSupported = computed(() => netState.value?.bind_only_supported ?? false);

async function onNetSelect(value: "block" | "bind_only"): Promise<void> {
  if (!props.projectId || netLoading.value || value === netTier.value) return;
  if (value === "bind_only" && !netSupported.value) return;
  netLoading.value = true;
  try {
    if (value === "block") {
      await projects.setProjectSandboxNet(props.projectId, "block");
    } else {
      // 切到 BindOnly 需要有已确认快照(授权真源);无快照时提示走
      // 下方确认流(confirm 成功后自动落 bind_only 档)。
      if (!latestSnapshot.value) {
        projects.showToast(
          "请先在下方确认端口快照,确认后自动切换到「仅放行监听」档。",
          "error",
        );
        return;
      }
      await projects.setProjectSandboxNet(
        props.projectId,
        `bind_only:${latestSnapshot.value.ports}`,
      );
    }
    await loadNetState();
  } catch (e) {
    projects.showToast(`设置失败：${extractErrorMessage(e)}`, "error");
    await loadNetState();
  } finally {
    netLoading.value = false;
  }
}

function parsePortsInput(raw: string): number[] | null {
  const ports = raw
    .split(/[,，\s]+/)
    .filter(Boolean)
    .map((t) => Number(t));
  if (
    ports.length === 0 ||
    ports.some((p) => !Number.isInteger(p) || p < 1 || p > 65535)
  ) {
    return null;
  }
  return Array.from(new Set(ports));
}

async function onConfirmSnapshot(): Promise<void> {
  if (!props.projectId) return;
  const ports = parsePortsInput(netPortsInput.value);
  if (!ports) {
    projects.showToast("端口格式:1-65535,逗号分隔(如 3000,3001)", "error");
    return;
  }
  const worktree = netWorktreeInput.value.trim() ||
    currentProject.value?.path ||
    "";
  if (!worktree) {
    projects.showToast("需要 worktree 路径(默认项目根目录)", "error");
    return;
  }
  netLoading.value = true;
  try {
    netState.value = await projects.confirmNetSnapshot(
      props.projectId,
      worktree,
      ports,
    );
    netPortsInput.value = "";
    await projects.loadProjects();
  } catch (e) {
    projects.showToast(`确认失败：${extractErrorMessage(e)}`, "error");
  } finally {
    netLoading.value = false;
  }
}

async function onRejectProposal(worktreeKey: string): Promise<void> {
  if (!props.projectId) return;
  netLoading.value = true;
  try {
    netState.value = await projects.rejectNetProposal(
      props.projectId,
      worktreeKey,
    );
  } catch (e) {
    projects.showToast(`操作失败：${extractErrorMessage(e)}`, "error");
  } finally {
    netLoading.value = false;
  }
}

async function onAcceptProposal(ports: string, worktreeKey: string): Promise<void> {
  const parsed = parsePortsInput(ports);
  if (!parsed) {
    projects.showToast("建议端口无法解析", "error");
    return;
  }
  netLoading.value = true;
  try {
    netState.value = await projects.confirmNetSnapshot(
      props.projectId!,
      worktreeKey,
      parsed,
    );
    await projects.loadProjects();
  } catch (e) {
    projects.showToast(`确认失败：${extractErrorMessage(e)}`, "error");
  } finally {
    netLoading.value = false;
  }
}

// -- 免沙箱命令授权(09-21-durable-prefix-grant,R2/R4)-------------------

/** 项目级 durable 前缀授权列表。批准语义:该前缀命令在本项目
 *  可免沙箱启动(沙箱档)/ 免审批弹卡(off 档),跨 session 与
 *  daemon 重启;按 worktree 键控(隔离 worker 不继承)。 */
const shellGrants = ref<ShellGrantInfo[]>([]);
const grantsLoading = ref(false);

async function loadShellGrants(): Promise<void> {
  if (!props.projectId) {
    shellGrants.value = [];
    return;
  }
  grantsLoading.value = true;
  try {
    shellGrants.value = await projects.listProjectShellGrants(props.projectId);
  } catch (e) {
    projects.showToast(`读取授权列表失败：${extractErrorMessage(e)}`, "error");
    shellGrants.value = [];
  } finally {
    grantsLoading.value = false;
  }
}

async function onRevokeGrant(worktreeKey: string, prefixTokens: string): Promise<void> {
  if (!props.projectId) return;
  grantsLoading.value = true;
  try {
    await projects.revokeProjectShellGrant(props.projectId, worktreeKey, prefixTokens);
    projects.showToast(`已撤销「${prefixTokens}」的免沙箱授权`, "info");
    await loadShellGrants();
  } catch (e) {
    projects.showToast(`撤销失败：${extractErrorMessage(e)}`, "error");
  } finally {
    grantsLoading.value = false;
  }
}

// 项目切换即重拉授权列表(immediate 与 netState 的 watch 分离:
// 本段的 ref 声明晚于那个 watch,挂在一起会在 immediate 执行时
// 触发 TDZ —— `Cannot access 'grantsLoading' before initialization`)。
watch(
  () => props.projectId,
  () => {
    void loadShellGrants();
  },
  { immediate: true },
);

/** v-model 先行(乐观):radio 点击即改本地选中;写失败回拨到
 *  项目当前档位并 toast(与开关行「乐观 + 失败回拨」同款策略)。 */
async function onSelect(value: Policy): Promise<void> {  const current = projects.projectById(props.projectId)?.sandbox_policy ?? "readwrite";
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

      <!-- 网络档(09-21-sandbox-net-bindonly) -->
      <h4 class="project-sandbox-tab__net-title">网络策略</h4>
      <p
        v-if="!netSupported"
        class="project-sandbox-tab__net-unsupported"
        data-testid="net-unsupported"
      >
        本平台不生效:BindOnly 需要 Landlock ABI ≥4(内核 ≥6.7)的
        TCP 端口规则;当前内核不支持,选择后运行时将自动降级为「全部断网」。
      </p>
      <div
        role="radiogroup"
        aria-label="项目网络策略"
        class="project-sandbox-tab__group"
      >
        <label class="project-sandbox-tab__option" :class="{ 'project-sandbox-tab__option--active': netTier === 'block' }">
          <input
            type="radio"
            name="project-sandbox-net"
            value="block"
            :checked="netTier === 'block'"
            :disabled="netLoading"
            class="project-sandbox-tab__radio"
            @change="onNetSelect('block')"
          />
          <span class="project-sandbox-tab__option-title">全部断网(默认)</span>
          <span class="project-sandbox-tab__option-desc">
            禁止创建 INET socket:无出网、无监听(dev server 起不来)。
            这是现状语义。
          </span>
        </label>
        <label
          class="project-sandbox-tab__option"
          :class="{ 'project-sandbox-tab__option--active': netTier === 'bind_only' }"
        >
          <input
            type="radio"
            name="project-sandbox-net"
            value="bind_only"
            :checked="netTier === 'bind_only'"
            :disabled="netLoading || !netSupported"
            class="project-sandbox-tab__radio"
            @change="onNetSelect('bind_only')"
          />
          <span class="project-sandbox-tab__option-title">仅放行监听(BindOnly)</span>
          <span class="project-sandbox-tab__option-desc" data-testid="net-bindonly-desc">
            只放行已确认快照端口上的 TCP 监听,出网仅限 80/443 与快照端口。
            注意:放行端口=数据可外发面,保密性保护不适用;UDP/DNS 在本档
            不受控(仅 TCP 受管);文件写入仍按上方文件档约束。
          </span>
        </label>
        <label class="project-sandbox-tab__option project-sandbox-tab__option--disabled" title="暂未开放">
          <input type="radio" name="project-sandbox-net" value="allow_all" disabled class="project-sandbox-tab__radio" />
          <span class="project-sandbox-tab__option-title">全部放行(挂账)</span>
          <span class="project-sandbox-tab__option-desc">
            本期不提供(等同 off 级信任;待 capability token 机制前置后开放)。
          </span>
        </label>
      </div>

      <!-- 快照管理 -->
      <div class="project-sandbox-tab__snapshots">
        <h5 class="project-sandbox-tab__snapshots-title">端口快照(授权真源)</h5>
        <p class="project-sandbox-tab__hint">
          生效端口集 = 你确认后的快照(按 worktree 路径键控,换分支需重新
          确认)。模型/清单提议仅为建议,经你确认才生效。
        </p>
        <ul v-if="netState?.snapshots.length" class="project-sandbox-tab__snapshot-list" data-testid="net-snapshot-list">
          <li v-for="snap in netState.snapshots" :key="snap.worktree_key">
            <code>{{ snap.ports }}</code> @ {{ snap.worktree_key }}
            <span class="project-sandbox-tab__muted">({{ snap.confirmed_by }})</span>
          </li>
        </ul>
        <p v-else class="project-sandbox-tab__muted">暂无快照。</p>

        <div
          v-if="pendingProposals.length"
          class="project-sandbox-tab__proposals"
          data-testid="net-proposal-list"
        >
          <h6>待确认的端口建议</h6>
          <div v-for="pr in pendingProposals" :key="pr.worktree_key" class="project-sandbox-tab__proposal-row">
            <span>
              建议 <code>{{ pr.ports }}</code> @ {{ pr.worktree_key }}
              <span class="project-sandbox-tab__muted">({{ pr.source }})</span>
            </span>
            <span class="project-sandbox-tab__proposal-actions">
              <button
                type="button"
                :disabled="netLoading"
                data-testid="net-accept-proposal"
                @click="onAcceptProposal(pr.ports, pr.worktree_key)"
              >确认</button>
              <button
                type="button"
                :disabled="netLoading"
                data-testid="net-reject-proposal"
                @click="onRejectProposal(pr.worktree_key)"
              >拒绝</button>
            </span>
          </div>
        </div>

        <div class="project-sandbox-tab__confirm-row">
          <input
            v-model="netWorktreeInput"
            type="text"
            placeholder="worktree 路径(默认项目根目录)"
            class="project-sandbox-tab__input"
            data-testid="net-worktree-input"
          />
          <input
            v-model="netPortsInput"
            type="text"
            placeholder="端口,如 3000,3001"
            class="project-sandbox-tab__input"
            data-testid="net-ports-input"
          />
          <button
            type="button"
            :disabled="netLoading || !netSupported"
            data-testid="net-confirm"
            @click="onConfirmSnapshot"
          >确认快照</button>
        </div>
      </div>
      <!-- 免沙箱命令授权(09-21-durable-prefix-grant) -->
      <div class="project-sandbox-tab__snapshots">
        <h5 class="project-sandbox-tab__snapshots-title">免沙箱命令授权</h5>
        <p class="project-sandbox-tab__hint">
          命令被沙盒拦截后审批卡上点「始终允许」会记住该命令前缀(按
          worktree 键控):此后同前缀命令直接免沙箱启动(文件+网络全开)
          / 免审批弹卡,跨会话与守护进程重启有效。撤销后恢复沙盒。
        </p>
        <ul v-if="shellGrants.length" class="project-sandbox-tab__snapshot-list" data-testid="shell-grant-list">
          <li v-for="g in shellGrants" :key="`${g.worktreeKey}|${g.prefixTokens}`">
            <code>{{ g.prefixTokens }}</code> @ {{ g.worktreeKey }}
            <span class="project-sandbox-tab__muted">({{ g.toolName }})</span>
            <button
              type="button"
              class="project-sandbox-tab__grant-revoke"
              :disabled="grantsLoading"
              data-testid="shell-grant-revoke"
              @click="onRevokeGrant(g.worktreeKey, g.prefixTokens)"
            >撤销</button>
          </li>
        </ul>
        <p v-else class="project-sandbox-tab__muted" data-testid="shell-grant-empty">暂无授权。</p>
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

.project-sandbox-tab__net-title,
.project-sandbox-tab__snapshots-title {
  margin: var(--space-2) 0 0;
  font-size: var(--text-base);
  font-weight: var(--weight-medium);
  color: var(--color-text-primary);
}

.project-sandbox-tab__net-unsupported {
  margin: 0;
  padding: var(--space-2) var(--space-3);
  border: 1px solid var(--color-warning, #c7902b);
  border-radius: var(--radius-md, 8px);
  font-size: var(--text-sm);
  color: var(--color-warning, #c7902b);
}

.project-sandbox-tab__option--disabled {
  opacity: 0.55;
  cursor: not-allowed;
}

.project-sandbox-tab__snapshot-list {
  margin: 0;
  padding-left: var(--space-5);
  font-size: var(--text-sm);
}

.project-sandbox-tab__muted {
  color: var(--color-text-muted);
}

.project-sandbox-tab__grant-revoke {
  margin-left: var(--space-2);
  padding: 0 var(--space-2);
  font-size: var(--text-sm);
}

.project-sandbox-tab__proposal-row {
  display: flex;
  justify-content: space-between;
  gap: var(--space-2);
  align-items: center;
  padding: var(--space-2) 0;
  font-size: var(--text-sm);
  border-bottom: 1px solid var(--color-bg-border);
}

.project-sandbox-tab__confirm-row {
  display: flex;
  gap: var(--space-2);
  flex-wrap: wrap;
}

.project-sandbox-tab__input {
  flex: 1 1 12rem;
  min-width: 0;
}

/* 移动端:设置弹窗的 44px 最小触控高度对 radio 无意义,压回自然尺寸。 */
@media (max-width: 767px) {
  .project-sandbox-tab__radio {
    min-width: 0;
    min-height: 0;
  }
}
</style>
