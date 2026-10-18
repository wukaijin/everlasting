// modelMetadata — 模型探测 + 在线能力匹配的前端薄封装
// (2026-10-19, task `10-19-merge-provider-model-settings`)。
//
// 两条 daemon IPC(域 `providers`,见
// `app/src-tauri/src/daemon/routes/providers.rs`):
//   - `probe_provider_models`  → 供应商模型列表(带在线能力富化)
//   - `lookup_model_metadata`  → 按模型名查 models.dev 候选
//
// 两个响应都是「业务失败也 200」的形状(`success:false` + `error`),
// 与 `test_model` 同族契约 —— 调用方用 `success` 判断,不必 try/catch
// 预期内的网络/目录失败(真正的传输失败仍会 reject)。

import { transport } from "../transport";

/** models.dev 目录条目的归一化投影(与 Rust `ModelSpec` 对齐)。 */
export interface ModelSpec {
  providerKey: string;
  providerName: string;
  modelId: string;
  /** 目录可读名(如 "Claude Sonnet 4.5 (latest)")。 */
  name: string;
  contextWindow: number | null;
  /** 最大输出 token(= 本应用 models.max_tokens 的语义)。 */
  maxOutputTokens: number | null;
  supportsVision: boolean;
  supportsThinking: boolean;
  /** reasoning_options 里 type=effort 的取值(无则空数组)。 */
  thinkingLevels: string[];
  releaseDate: string | null;
}

export interface LookupMetadataResult {
  success: boolean;
  source?: string;
  /** 目录快照时间(ISO 字符串)。 */
  fetchedAt?: string;
  cached?: boolean;
  /** 网络失败降级到过期缓存时为 true。 */
  stale?: boolean;
  matches: ModelSpec[];
  error: string | null;
}

export interface ProbedModel {
  /** 供应商返回的 id(落库即 models.model_name)。 */
  modelName: string;
  displayName: string;
  /** 在线目录命中的能力;未命中为 null。 */
  spec: ModelSpec | null;
}

export interface ProbeModelsResult {
  success: boolean;
  latencyMs: number;
  models: ProbedModel[];
  truncated: boolean;
  error: string | null;
  metadataSource?: string;
  metadataFetchedAt?: string | null;
  metadataCached?: boolean | null;
  metadataStale?: boolean | null;
  /** 目录不可用的原因(列表仍可用,只是缺能力字段)。 */
  metadataWarning: string | null;
}

/** 按模型名查在线目录(providerId 只作匹配排序线索,不参与过滤)。 */
export function lookupModelMetadata(
  modelName: string,
  providerId?: string | null,
): Promise<LookupMetadataResult> {
  return transport.invoke<LookupMetadataResult>("lookup_model_metadata", {
    modelName,
    ...(providerId ? { providerId } : {}),
  });
}

/** 拉一次供应商的模型列表端点。 */
export function probeProviderModels(providerId: string): Promise<ProbeModelsResult> {
  return transport.invoke<ProbeModelsResult>("probe_provider_models", { providerId });
}

/**
 * 从目录给出的思考挡位里挑一个本应用表单能表达的值。
 *
 * 顺序:原值仍合法 → `high` → `medium` → `low` → 首个合法值 → null。
 * 词表按协议分叉(`openai_responses` 只认 minimal|low|medium|high,
 * 其余协议 low|medium|high|xhigh|max);目录里出现词表外的值时退回
 * `high`,避免写进一个请求会 400 的值。
 */
export function pickThinkingEffort(
  spec: ModelSpec,
  protocol: string,
  current?: string | null,
): string | null {
  const allowed =
    protocol === "openai_responses"
      ? ["minimal", "low", "medium", "high"]
      : ["low", "medium", "high", "xhigh", "max"];
  const levels = spec.thinkingLevels.filter((v) => allowed.includes(v));
  if (current && levels.includes(current)) return current;
  for (const pref of ["high", "medium", "low"]) {
    if (levels.includes(pref)) return pref;
  }
  return levels[0] ?? null;
}

/** 目录条目 → 表单/落库用的能力字段(纯函数,单测覆盖)。 */
export interface ModelCapabilities {
  contextWindow: number;
  /** null = 不覆盖全局默认(max_tokens 留空)。 */
  maxTokens: number | null;
  supportsThinking: boolean;
  supportsImages: boolean;
  /** null/"none" = 不显式发送思考挡位。 */
  thinkingEffort: string | null;
}

/**
 * 把目录条目折算成模型能力字段。
 *
 * - 上下文窗口:目录缺失时沿用 `fallbackContext`(默认 200k,与
 *   `test_model` / 表单一贯的缺省口径一致)。
 * - 最大输出:目录的 `limit.output` 就是本应用 `models.max_tokens`
 *   的语义(单轮输出上限)。
 * - 思考:目录 `reasoning` 为真才开;挡位按协议词表挑选,挑不出就
 *   留 `null`(后端回落全局默认 high)。
 */
export function capabilitiesFromSpec(
  spec: ModelSpec,
  protocol: string,
  fallbackContext = 200_000,
): ModelCapabilities {
  return {
    contextWindow: spec.contextWindow ?? fallbackContext,
    maxTokens: spec.maxOutputTokens ?? null,
    supportsThinking: spec.supportsThinking,
    supportsImages: spec.supportsVision,
    thinkingEffort: spec.supportsThinking
      ? pickThinkingEffort(spec, protocol)
      : null,
  };
}

/** 上下文窗口的紧凑展示(200000 → 200k;1000000 → 1M)。 */
export function formatTokens(n: number | null | undefined): string {
  if (n == null || n <= 0) return "—";
  if (n >= 1_000_000) {
    const m = n / 1_000_000;
    return `${Number.isInteger(m) ? m : m.toFixed(1)}M`;
  }
  if (n >= 1000) {
    const k = n / 1000;
    return `${Number.isInteger(k) ? k : k.toFixed(1)}k`;
  }
  return String(n);
}

/** 一条候选的能力摘要标签(UI 徽标用)。 */
export function specTags(spec: ModelSpec): string[] {
  const tags: string[] = [];
  if (spec.contextWindow) tags.push(`ctx ${formatTokens(spec.contextWindow)}`);
  if (spec.maxOutputTokens) tags.push(`out ${formatTokens(spec.maxOutputTokens)}`);
  if (spec.supportsVision) tags.push("vision");
  if (spec.supportsThinking) tags.push("thinking");
  return tags;
}
