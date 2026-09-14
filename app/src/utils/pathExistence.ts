// pathExistence — 本地路径链接的存在性确认(2026-09-14)。
//
// 问题:linkify(`utils/markdown.ts`)按正则把聊天/工具输出里的本地路径
// 渲染成可点击链接,但 LLM 会写**不存在**的路径(幻觉 / "将要生成"的
// 产物),这些死链接长期高亮误导点击。
//
// 方案与抖动权衡(核心决策,改动前先读):
//   - **乐观渲染,确认缺失才降级**。渲染是同步纯函数、存在性是异步事实,
//     两者不可能原子。乐观序 = 未知按存在渲染(锚点照出),stat 404 回来
//     后重渲染降级纯文本。这样**存在的文件(常态)永不闪烁**;缺失路径
//     只有一次性短暂高亮(本地 daemon 一个 stat 往返,典型 <50ms)。
//     反序(悲观:确认存在才渲染)会让常态路径从纯文本闪成链接,SSE
//     流式场景抖动面大得多,弃。
//   - **SSE 零阻塞**:渲染路径只做 Map 查询(乐观默认),fetch 全在渲染
//     外异步发起;`createDebouncedRenderer` 的 50ms 节流渲染管线不变,
//     存在性结果经订阅回调触发一次补偿重渲染(仅当文本确实含该路径,
//     且无 pending 节流定时器 —— 节流帧会自然带上新结果)。
//   - **缓存与去重**:键 = 按 cwd 解析后的绝对/`~/` 形态(与点击解析
//     `resolveImagePath` 同源,基准一致)。positive 永久缓存(文件消失
//     点击走弹层既有错误态);negative 15s TTL —— 重查静默进行,consult
//     不区分 TTL 新旧(陈旧 negative 仍按缺失渲染),文件被补建后
//     重查翻 positive 才升回链接,全程无闪烁。
//   - **只信 200/404**:400(白名单外/相对路径)、5xx、网络失败 → 未知,
//     不写缓存,保持乐观链接(点击有弹层错误态兜底)。
//
// 消费方接线:
//   - computed 消费方(ToolOutputBody 的 linkifyPlainText 等)免费获得
//     重算 —— `entries` 是 reactive Map,`pathKnownMissing` 的 `get`
//     在 computed 上下文里被依赖追踪,结果落缓存即失效重算。
//   - `createDebouncedRenderer`(setTimeout 上下文,无 effect scope)显式
//     订阅 `onPathsResolved`,回调带 raw 路径数组(微任务合并),渲染器
//     用 `text.includes(raw)` 过滤无关消息,零串扰。
//
// 测试隔离:vitest(`import.meta.env.MODE === "test"`)下默认禁网 ——
// 各组件/markdown 测试的乐观路径不会真发 fetch(本机 daemon 在跑时
// 404/200 会注入真实结果,破坏确定性)。`pathExistence.test.ts` 用
// `__enableNetworkForTests` + `vi.stubGlobal("fetch")` 打开真链路;
// 其余测试用 `setExistenceForTests` 直播种缓存。

import { getActivePinia } from "pinia";
import { reactive } from "vue";
import { useChatStore } from "../stores/chat";
import { resolveImagePath, statUrl } from "./imageUrl";

/** negative 缓存 TTL:15s 内不重查;到点后渲染时静默重查(自愈补建)。 */
const MISSING_TTL_MS = 15_000;

interface ExistenceEntry {
  exists: boolean;
  checkedAt: number;
}

/** resolved 路径 → 存在性。reactive:computed 消费方经 pathKnownMissing
 * 的读自动依赖追踪,结果落缓存即失效重算。 */
const entries = reactive(new Map<string, ExistenceEntry>());
/** 去重:同一路径的在途请求只有一班。 */
const inFlight = new Set<string>();

type PathsResolvedListener = (rawPaths: string[]) => void;
const listeners = new Set<PathsResolvedListener>();
/** 微任务合并:同一 tick 内落地的多条结果只广播一拍,渲染器端单次重渲染。 */
let notifyQueue: string[] | null = null;

function notify(rawPath: string): void {
  if (notifyQueue === null) {
    notifyQueue = [rawPath];
    queueMicrotask(() => {
      const batch = notifyQueue ?? [];
      notifyQueue = null;
      for (const l of listeners) l(batch);
    });
  } else if (!notifyQueue.includes(rawPath)) {
    notifyQueue.push(rawPath);
  }
}

/** 订阅存在性结果落地。回调参数是 raw 路径(渲染文本里出现的原文),
 *  消费方自行按文本过滤。返回退订函数。 */
export function onPathsResolved(listener: PathsResolvedListener): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

/** raw 路径 → 缓存键。绝对/`~/` 原样;相对路径借会话 cwd 解析(与点击
 *  时刻的 `resolveImagePath` 同源同基准)。解析不出(无 pinia 的单测
 *  直调、cwd 未知)→ null = 不猜,调用方按"未知"处理。 */
function resolveKey(raw: string): string | null {
  if (raw.startsWith("/") || raw.startsWith("~/")) return raw;
  if (!getActivePinia()) return null;
  const resolved = resolveImagePath(raw, useChatStore().currentCwd);
  return resolved.startsWith("/") || resolved.startsWith("~/") ? resolved : null;
}

/** consult:该路径是否**已确认**不存在。未知(无缓存/解析不出)一律
 *  false —— 乐观渲染的默认态。 */
export function pathKnownMissing(raw: string): boolean {
  const key = resolveKey(raw);
  if (!key) return false;
  const e = entries.get(key);
  return e !== undefined && !e.exists;
}

/** schedule:对未知/过期缓存发起 stat 确认。幂等且可重入 —— 渲染每帧
 *  都会对文本里的每个路径调用,内部靠缓存 + inFlight 去重,常态零请求。 */
export function schedulePathCheck(raw: string): void {
  const key = resolveKey(raw);
  if (!key || inFlight.has(key)) return;
  const e = entries.get(key);
  if (e && (e.exists || Date.now() - e.checkedAt < MISSING_TTL_MS)) return;
  inFlight.add(key);
  void doCheck(key, raw).finally(() => inFlight.delete(key));
}

async function doCheck(key: string, raw: string): Promise<void> {
  if (import.meta.env.MODE === "test" && !networkEnabledForTests) return;
  let exists: boolean;
  try {
    const res = await fetch(statUrl(key));
    if (res.status === 200) {
      exists = true;
    } else if (res.status === 404) {
      // 哨兵 body 区分两类 404:真 stat 的"文件不存在" vs 陈旧 daemon
      // (vite 热更前端、daemon 未重启)路由 fallback 的 404 —— 后者按
      // 未知处理保持乐观,否则整个 09-13 链接功能会被静默杀光。字面量
      // 与 `daemon/routes/files.rs` stat_file 成对持有,改动须两侧对照。
      if ((await res.text()) !== "stat: file not found") return;
      exists = false;
    } else {
      // 400(白名单外/相对路径)/ 5xx → 未知:不写缓存,保持乐观链接。
      return;
    }
  } catch {
    return; // daemon 不可达 → 未知,同样保持乐观
  }
  const prev = entries.get(key);
  entries.set(key, { exists, checkedAt: Date.now() });
  // 值未变(404 刷新 TTL)不广播 —— 否则陈旧 negative 的 15s 周期重查
  // 会给含该路径的消息带来无意义的重渲染。
  if (!prev || prev.exists !== exists) notify(raw);
}

// --- 测试 seam(仅 *_ForTests 命名空间,勿在产品代码调用)-----------------

let networkEnabledForTests = false;

/** 打开真实 fetch 链路(vitest 默认禁网;配合 vi.stubGlobal("fetch"))。 */
export function __enableNetworkForTests(): void {
  networkEnabledForTests = true;
}

/** 直播种缓存并按值变化广播(渲染管线的补偿重渲染也可借此驱动)。 */
export function setExistenceForTests(raw: string, exists: boolean): void {
  const key = resolveKey(raw);
  if (!key) return;
  const prev = entries.get(key);
  entries.set(key, { exists, checkedAt: Date.now() });
  if (!prev || prev.exists !== exists) notify(raw);
}

/** 测试隔离:清缓存与在途集合。 */
export function resetExistenceForTests(): void {
  entries.clear();
  inFlight.clear();
}
