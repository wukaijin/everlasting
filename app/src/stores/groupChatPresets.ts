// useGroupChatPresetsStore — Pinia store for user group-chat presets
// (Settings →「群聊预设」tab;GCE-P1, task `09-12-gc-preset-settings`).
//
// 职责三层:
//   1. `load()` / `ensureLoaded()` — 拉用户预设全量(`list_group_chat_presets`,
//      后端 ORDER BY name 稳定序)。消费方(Settings tab onMounted、
//      ScheduledTasksTab、GroupChatConfigModal open)统一走 `ensureLoaded`,
//      未加载才拉,幂等(design §4.2)。
//   2. `create` / `update` / `remove` — 写操作包装:update / remove 挂行级
//      spinner(finally 必删,防"点击后忘关"卡死;subagents.spinnerByName
//      同款),成功后重拉列表保持 name 序(canonical 序在服务端)。
//   3. `mergedPresets` — 合并视图(design §4.1):内置四档
//      (scripts/group-chat-presets.json,只读,键序 = JSON 声明序)在前,
//      用户行按 name 字典序追加。**关键机制**:用户行的模型引用是
//      models.id UUID,直接塞进 `GcPresetDef` 的 model 字段 ——
//      `resolveModelRef` 第一趟就是 byId 精确匹配,既有解析 / 预填 /
//      禁用警告链路对内置档和用户档逐字同形,零新分支。内置 key
//      (review/fe_review/arch/retro)与用户 key(行 id UUID)域不相交,
//      同一 Record 不会撞键。
//
// wire 形状:`GcPresetRow` camelCase(Rust `#[serde(rename_all =
// "camelCase")]`,subagents 域同惯例)。**请求**:顶层 key camelCase
// (transport 扳 snake),嵌套 participants 元素保持 camelCase
// `{name, modelId, persona}` —— 嵌套值不经 transport 转换,Rust 侧按
// camelCase 反序列化(routes oneshot 测试锁死该形状)。
import { defineStore } from "pinia";
import { computed, reactive, ref } from "vue";
import { transport } from "../transport";
import { extractErrorMessage } from "../utils/useErrorBus";
import { GC_PRESETS, type GcPresetDef } from "../utils/groupChatPresets";

/** One participant of a user preset row(wire 形状,镜像 Rust
 *  `GcPresetParticipant`)。`modelId` 是 models.id UUID;`persona` 是
 *  五种内置 kind 之一(arch/product/backend/frontend/outsider,白名单
 *  校验在 commands 层)。 */
export interface GcPresetParticipantRow {
  name: string;
  modelId: string;
  persona: string;
}

/** One row of `list_group_chat_presets`(wire camelCase,镜像 Rust
 *  `GcPresetRow`;drift = cross-layer bug)。 */
export interface GcPresetRow {
  id: string;
  name: string;
  description: string;
  /** models.id UUID。 */
  moderatorModelId: string;
  participants: GcPresetParticipantRow[];
  /** RFC 3339。 */
  createdAt: string;
  /** RFC 3339。 */
  updatedAt: string;
}

/** `create` / `update` 的表单载荷。顶层 camelCase(transport 扳
 *  snake);participants 元素保持 camelCase(嵌套不转换)。 */
export interface GcPresetInput {
  name: string;
  description: string;
  moderatorModelId: string;
  participants: GcPresetParticipantRow[];
}

/** 合并视图条目:`GcPresetDef` + 出处标记。消费方(ScheduledTasksTab /
 *  GroupChatConfigModal)把 `key` 当 preset 键、`builtin` 当「自定义」
 *  徽标判据;其余字段与内置档同形。 */
export interface MergedGcPreset extends GcPresetDef {
  /** 合并键:内置 = JSON key;用户 = 行 id(UUID)。 */
  key: string;
  builtin: boolean;
  /** 展示名:内置 = key(现状),用户 = 行 name(下拉 / 卡片标签)。 */
  name: string;
}

export const useGroupChatPresetsStore = defineStore("groupChatPresets", () => {
  // -----------------------------------------------------------------------
  // Reactive state
  // -----------------------------------------------------------------------

  /** 用户预设行(后端 ORDER BY name;getter 里再排一次防未来后端
   *  改序 reshuffle UI —— SubagentsTab sortedRows 同款防御)。 */
  const rows = ref<GcPresetRow[]>([]);

  /** `true` after the first `load()` resolves. 消费方据此决定是否拉取
   *  (`ensureLoaded` 的判据;失败保持 false,下次交互重试)。 */
  const loaded = ref(false);

  /** 行级 spinner:`id` ∈ set = 该行有写操作在途。`finally` 里 delete,
   *  spinner 永不粘手(subagents.spinnerByName 同款)。 */
  const spinnerById = reactive(new Set<string>());

  // -----------------------------------------------------------------------
  // mergedPresets(核心 getter)
  // -----------------------------------------------------------------------

  /** 内置在前(JSON 声明序)+ 用户按 name 序追加的合并视图。每次依赖
   *  变更重建(行数个位数级,无需增量)。 */
  const mergedPresets = computed<Record<string, MergedGcPreset>>(() => {
    const out: Record<string, MergedGcPreset> = {};
    for (const [key, def] of Object.entries(GC_PRESETS.presets)) {
      out[key] = { ...def, key, builtin: true, name: key };
    }
    const sorted = [...rows.value].sort((a, b) => a.name.localeCompare(b.name));
    for (const row of sorted) {
      out[row.id] = {
        description: row.description,
        // UUID 直进 model 字段:resolveModelRef 第一趟 byId 精确命中,
        // 既有解析 / 预填 / 警告链路零分支复用(见模块头注)。
        moderator_model: row.moderatorModelId,
        participants: row.participants.map((p) => ({
          name: p.name,
          model: p.modelId,
          persona: p.persona,
        })),
        key: row.id,
        builtin: false,
        name: row.name,
      };
    }
    return out;
  });

  // -----------------------------------------------------------------------
  // Actions
  // -----------------------------------------------------------------------

  /** 拉全量用户预设(整表替换)。 */
  async function load(): Promise<void> {
    // `?? []`:daemon 恒回数组;空体 / mock 未 stub 的场景降级空表,
    // 别让 undefined 流进 mergedPresets 的排序。
    const list = await transport.invoke<GcPresetRow[]>(
      "list_group_chat_presets",
      {},
    );
    rows.value = list ?? [];
    loaded.value = true;
  }

  /** 未加载才拉(幂等)。ScheduledTasksTab / GroupChatConfigModal 的
   *  轻量入口 —— 打开即拉一次,已加载零开销(design §4.2)。 */
  async function ensureLoaded(): Promise<void> {
    if (!loaded.value) await load();
  }

  /** 新建用户预设,返回服务端生成的行(id UUID)。校验在服务端
   *  (单一事实源);前端预校验只为即时反馈(tab 组件内)。 */
  async function create(input: GcPresetInput): Promise<GcPresetRow> {
    try {
      const row = await transport.invoke<GcPresetRow>("create_group_chat_preset", {
        name: input.name,
        description: input.description,
        moderatorModelId: input.moderatorModelId,
        participants: input.participants,
      });
      await load();
      return row;
    } catch (e) {
      throw new Error(extractErrorMessage(e));
    }
  }

  /** 全量 patch(表单整体提交语义,无部分更新)。 */
  async function update(id: string, input: GcPresetInput): Promise<GcPresetRow> {
    if (spinnerById.has(id)) {
      throw new Error("该预设已有操作在进行中");
    }
    spinnerById.add(id);
    try {
      const row = await transport.invoke<GcPresetRow>("update_group_chat_preset", {
        id,
        name: input.name,
        description: input.description,
        moderatorModelId: input.moderatorModelId,
        participants: input.participants,
      });
      await load();
      return row;
    } catch (e) {
      throw new Error(extractErrorMessage(e));
    } finally {
      spinnerById.delete(id);
    }
  }

  /** 硬删(服务端幂等:不存在也 ok —— 快照语义下已建任务自包含,
   *  删预设不悬空,design §5)。 */
  async function remove(id: string): Promise<void> {
    if (spinnerById.has(id)) {
      throw new Error("该预设已有操作在进行中");
    }
    spinnerById.add(id);
    try {
      await transport.invoke("delete_group_chat_preset", { id });
      await load();
    } catch (e) {
      throw new Error(extractErrorMessage(e));
    } finally {
      spinnerById.delete(id);
    }
  }

  return {
    rows,
    loaded,
    spinnerById,
    mergedPresets,
    load,
    ensureLoaded,
    create,
    update,
    remove,
  };
});
