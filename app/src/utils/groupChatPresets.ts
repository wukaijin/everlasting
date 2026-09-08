// groupChatPresets — 共享 preset 消费逻辑(gce-m4c Step 5,纯搬家自
// ScheduledTasksTab.vue)。
//
// 单一事实源 = scripts/group-chat-presets.json(M1 script 的
// `composePresets` 消费同一份)。本模块负责:
//   1. preset JSON 的 TS 形状(personas / persona_common / presets);
//   2. `GC_PRESETS` 加载(vite JSON import;文件在 app/ 外:vite dev 需
//      server.fs.allow——vite.config.ts 已配,build/vitest 不受
//      dev-server 限制);
//   3. `resolveModelRef(models, ref)` — preset 的模型引用(名字或
//      UUID)→ 目录 UUID 的两趟解析;
//   4. `composePersonaMd(kind)` — persona kind → 完整 persona_md
//      (镜像 M1 `composePresets`:边界文本 + "\n\n" + persona_common;
//      提交时逐字带过,前端不加工)。
//
// 消费方:ScheduledTasksTab(M4a 定时审议)+ GroupChatConfigModal
// (gce-m4c 弹窗 preset 卡)。两处 persona 展开与模型解析必须逐字同形,
// 因此收敛在此,禁止复制粘贴。

import groupChatPresetsJson from "../../../scripts/group-chat-presets.json";

/** preset 配方的运行时形状(与 M1 `composePresets` 消费的 JSON 同构;
 *  JSON import 的字面量类型按 key 收窄,动态取档需放宽成 Record)。 */
export interface GcPresetDef {
  description: string;
  moderator_model: string;
  participants: { name: string; model: string; persona: string }[];
}

export interface GcPresetsJson {
  persona_common: string;
  personas: Record<string, string>;
  presets: Record<string, GcPresetDef>;
}

export const GC_PRESETS = groupChatPresetsJson as unknown as GcPresetsJson;

/** persona kind → 完整 persona_md(边界文本 + "\n\n" + 公共纪律)。
 *  缺 kind 返回 null(正常不可能:JSON 单源固定四 kind;防御转提交
 *  错误)。 */
export function composePersonaMd(kind: string): string | null {
  const base = GC_PRESETS.personas[kind];
  return base === undefined ? null : `${base}\n\n${GC_PRESETS.persona_common}`;
}

/** `resolveModelRef` 可消费的最小模型形状(models store 的
 *  `ModelWithProvider` 的结构子集;调用方直接传 `models.models ?? []`)。 */
export interface ModelRefLike {
  id: string;
  modelName: string;
  displayName: string;
}

/** 模型引用(名字或 UUID)→ 目录 UUID。镜像 M1 `normalizeModelRef` 的
 *  两趟语义(UUID → 精确 modelName/displayName → 大小写不敏感;目录里
 *  存在「glm-5.3 的 modelName == GLM-5.3-Flash 的 displayName」的真实
 *  撞车,单趟 lowercase 会随数组序漂移)。查不到返回 null(提交时转
 *  用户可读错误——预设引用的模型必须真实存在,后端 catalog 预检同样
 *  拦截)。models 数组显式入参:调用方各自持有 models store 引用。 */
export function resolveModelRef(
  models: readonly ModelRefLike[],
  ref: string,
): string | null {
  if (!ref) return null;
  const byId = models.find((m) => m.id === ref);
  if (byId) return byId.id;
  const exact = models.find((m) => m.modelName === ref || m.displayName === ref);
  if (exact) return exact.id;
  const lower = ref.toLowerCase();
  const ci = models.find(
    (m) =>
      (m.modelName || "").toLowerCase() === lower ||
      (m.displayName || "").toLowerCase() === lower,
  );
  return ci?.id ?? null;
}
