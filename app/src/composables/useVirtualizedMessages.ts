// useVirtualizedMessages — N4 PR1(任务 09-19-n4-render-virtualization,
// design §1/§2):MessageList 虚拟化的**集中装配点**。store 投影 →
// buildRunGroups → flattenRunGroups → useVirtualizer,连同 §2 迁移表的
// 全部锚定动作(jumpToBottom / pending CH8-2a / reload F4 / session
// 切换 / mount 落底 / data-seq 定位 / force-follow)都住在这一个文件 ——
// PR0 spike 证伪时的手写补丁落点就是这里(单文件,评审要求)。
//
// PR0 spike 三条硬约束(implement.md PR0 段,virtual-core 3.17.11 实证):
//   1. `followOnAppend: true` ≡ 'auto'(core 把 true 映射为 behavior:
//      'auto',同受 isAtEnd 门控)——「强制跟滚」不存在。force-follow
//      手写:watch append + forceFollowActive → scrollToEnd(下方
//      force-follow watch)。
//   2. Vue 适配层只在 options / scrollElement 变化时调 `_willUpdate()`
//      (React 适配层每 render 都调)→ 纯 resize(流式末项 grow,无
//      append)的钉底写入被 clamp 后,`_retryClampedAdjustment` 没有
//      重试入口,实测稳定差 ~300px。补偿:onUpdated 每 render 补一次
//      `_willUpdate()`(spike 页腿④实证钉底恢复)。
//   3. 程序化滚动一律走库 API(scrollToIndex / scrollToEnd / scrollBy),
//      禁止 el.scrollTop 直写 —— 直写与库事件驱动的内部 offset 同步
//      竞态,后续 isAtEnd / wasAtEnd 判定读到滞后值而失效。
import { computed, nextTick, onMounted, onScopeDispose, onUpdated, ref, watch } from "vue";
import type { Ref } from "vue";
import { useVirtualizer } from "@tanstack/vue-virtual";
// virtual-core 不在 app 的直接依赖里(pnpm 严格布局),类型经
// vue-virtual 的 `export *` re-export 取(Virtualizer 实例类型)。
import type { VirtualItem, Virtualizer } from "@tanstack/vue-virtual";
import { useChatStore } from "../stores/chat";
import { useQuestionCardsStore } from "../stores/questionCards";
import type { ChatMessage } from "../stores/chat.types";
import {
  buildRunGroups,
  flattenRunGroups,
} from "../utils/messageFormat";
import type { FlatItem } from "../utils/messageFormat";

// ---------------------------------------------------------------------------
// 锚定决策纯函数(design §2,PR0 勘误后形态)
// ---------------------------------------------------------------------------

/** 回底阈值(design §2 迁移表第一行):虚拟化 options 的
 *  scrollEndThreshold(库内 append 跟滚的 isAtEnd 门)与 onScroll 的
 *  按钮显隐 / force-follow 退出判定共用同一常量 —— 同源承诺落在常量上。 */
export const SCROLL_END_THRESHOLD = 80;

// ---- N4 PR3 动画/flash 常量(design §4)--------------------------------
/** data-seq flash 类的驻留时长(AC4 后半):动画本体 1400ms(CH12-1b
 *  主窗口形态:accent 22% → transparent,1400ms ease-out 单次)+ 100ms
 *  清除缓冲。 */
export const SEARCH_FLASH_MS = 1500;
/** run-enter-active 过渡窗(旧 TransitionGroup 参数迁移:
 *  opacity/transform var(--duration-slow)=240ms + 释放判定缓冲)。 */
export const RUN_ENTER_ACTIVE_MS = 300;

/** followModeOf 的完整决策:库的 followOnAppend 选项值 + 是否需要
 *  手写 force-follow。返回对象而非裸值,因为 spike 实证后「强制跟滚」
 *  不再是 followOnAppend 的一个取值(true ≡ 'auto'),而是 composable
 *  的第二条路径 —— 决策函数必须把两条路径一起钉住,单测才有意义。 */
export interface AnchorDecision {
  /** 传给 useVirtualizer 的 followOnAppend。'auto' = 视口在末端
   *  (scrollEndThreshold 内)才跟 append;false = 完全不跟。 */
  followOnAppend: false | "auto";
  /** 手写 force-follow(append 时无条件 scrollToEnd,绕过 isAtEnd
   *  门)。流式且 store.forceFollowActive 时为真。 */
  forceFollow: boolean;
}

/** 锚定决策(design §2 迁移表第一行 + PR0 勘误):
 *  - 非流式:库不跟(回底/定位走显式命令路径)。
 *  - 流式:库 'auto'(isAtEnd 门内跟 append);force-follow 由手写
 *    路径承担 —— 库的 `true` 与 'auto' 同效,不存在强制形态。 */
export function followModeOf(o: {
  isStreaming: boolean;
  forceFollow: boolean;
}): AnchorDecision {
  if (!o.isStreaming) return { followOnAppend: false, forceFollow: false };
  return { followOnAppend: "auto", forceFollow: o.forceFollow };
}

// ---------------------------------------------------------------------------
// estimateSize 实测回归式(design §3;PR1 三态粗估 → PR2 用 10k fixture
// 的逐窗实测数据拟合,数字支撑 = 临时探针全列表采样 + 子元素解剖
// (.msg__markdown 折行 / 气泡 padding / rocard 卡体分别量取);静默后
// 复测为准 —— 首见采样会把 markdown 异步填充中的骨架高误当行高)
// ---------------------------------------------------------------------------

/** markdown 行高(.msg__markdown 实测 22px/行,61/81/401/4800 字符
 *  四锚点回归)。 */
const EST_TEXT_PER_LINE = 22;
/** 折行宽度:实测 ~88 字符/行(e2e Desktop Chrome 1280 视口)。 */
const EST_CHARS_PER_LINE = 88;
/** 气泡座高:md 折行外的恒定部分(bubble padding 20 + msg padding 8)。 */
const EST_TEXT_BASE = 28;
/** user 气泡比 assistant 同长文本实测高 ~16px(154 vs 142 @401c)。 */
const EST_USER_EXTRA = 16;
/** 一张工具卡:紧凑 rocard 卡体实测 26px(read 族 1 行卡;通用
 *  tool-card 更高 —— 偏小仅它自己收敛,无累积)。 */
const EST_TOOL_CARD = 26;
/** 折叠思考块(collapsed details 实测 ~28px/块;redacted 同级)。 */
const EST_THINKING_BLOCK = 28;
/** ghost user 残根:tool_result 已合并进 assistant 卡渲染,本行实测
 *  仅 ~6px。 */
const EST_GHOST_USER = 6;
/** 兜底下限(估 0 会让 spacer 塌掉;流式占位行实测 ~50px,测后收敛)。 */
const EST_MIN_ROW = 6;

/** 按 FlatItem 形态的实测回归估高(update_checklist 等虚拟工具不剔除
 *  —— 测后即收敛(measureElement 持久化))。 */
export function estimateMessageHeight(m: ChatMessage): number {
  let h = 0;
  // 文本长度:m.content(live 流式态)优先;reload 后 timeline 形态的
  // 行 m.content 为空、文本在 contentBlocks 的 text 块里(实测:带卡的
  // assistant 行 text 块 ~80 字符渲染 42px 气泡 —— 不计会系统性低估)。
  let textLen = m.content.length;
  if (!textLen && m.contentBlocks) {
    for (const b of m.contentBlocks) {
      if (b.kind === "text") textLen += b.text.length;
    }
  }
  if (textLen > 0) {
    h += Math.ceil(textLen / EST_CHARS_PER_LINE) * EST_TEXT_PER_LINE;
    h += EST_TEXT_BASE;
    if (m.role === "user") h += EST_USER_EXTRA;
  }
  h += (m.toolCalls?.length ?? 0) * EST_TOOL_CARD;
  // ghost user 行的 toolResults 是 rehydrate 合并的副本(渲染在
  // assistant 的卡里),本行只剩残根 —— 不按卡计。
  if (m.role === "user" && (m.toolResults?.length ?? 0) > 0) {
    h += EST_GHOST_USER;
  }
  h += (m.thinkingBlocks?.length ?? 0) * EST_THINKING_BLOCK;
  h += (m.redactedThinkingData?.length ?? 0) * EST_THINKING_BLOCK;
  return Math.max(h, EST_MIN_ROW);
}

/** 可见性谓词的「单调核心」(PR2 拆分):除 `error` 外的全部可见性
 *  字段。这些字段只增不减(content 只追加、四个结构数组只 push),
 *  所以一条消息一旦核心可见就永远可见 —— 单调性是下方缓存的前提。
 *  (唯一已知的非单调路径:retryChat 原位清空结构数组并剥 ERROR_MARKER
 *  —— 此时该消息是尾行,tailSig 翻转触发重判,见下。) */
function isCoreVisible(m: ChatMessage): boolean {
  return !!(
    m.content ||
    m.toolCalls?.length ||
    m.toolResults?.length ||
    (m.thinkingBlocks && m.thinkingBlocks.length > 0) ||
    (m.redactedThinkingData && m.redactedThinkingData.length > 0)
  );
}

/** 完整可见性 = 单调核心 ∨ error(error 非单调:retry start 会原位
 *  清除;A5+ 语义 = 重试恢复后错误行消失)。导出仅供测试对照缓存链
 *  与原谓词语义一致。 */
export function isVisible(m: ChatMessage): boolean {
  return isCoreVisible(m) || !!m.error;
}

/** 单调核心可见性缓存(PR2,f4 降本主刀):
 *
 *  PR1 形态 `store.messages.filter(isVisible)` 让 flatItems 这条
 *  computed 依赖**每条消息的全部可见性字段** —— 流式 delta 原位追加
 *  content 即失效整条链,visibleMessages→buildRunGroups→flatten 以
 *  O(n) 每 delta 全量重算(F1 实测主线程 ~20-25ms/delta × 20 delta,
 *  f4@10k 859ms 的主要构成;design §9-6 U4 预判坐实)。
 *
 *  拆法:可见性输入分两类 ——
 *  - **单调核心**(上:content 非空 + 四个结构数组长度):一条消息
 *    一旦为真永不为真。按消息对象缓进 WeakMap 后,filter 主循环对
 *    已可见行**零字段读取**(WeakMap.get 不进依赖),流式增长不再
 *    订阅 content。
 *  - **非单调的 error + 新尾行的核心字段翻转**:全部收敛进 tailSig
 *    (下方)——尾行是流式期唯一会翻转可见性的行(占位行 push 时
 *    不可见,首 delta 后可见;tool/thinking/error 事件也都落在尾行)。
 *
 *  tailSig 只读尾行的六个可见性输入(content 仅取空/非空位),取值
 *  在纯文本 delta 期间恒定 —— Vue computed 的值稳定语义(core 3.4+,
 *  本 app vue 3.5)使 delta 不再向下游传播;翻转事件(首 delta /
 *  tool:call / thinking 块 / error)照常触发重判。 */
const coreVisibleCache = new WeakMap<ChatMessage, boolean>();

/** Composable 公开面:MessageList.vue 只做模板(虚拟项 wrapper + 回底
 *  按钮),全部锚定语义在这里。 */
export interface VirtualizedMessages {
  /** 打平后的渲染行(virtualizer count 的数据源;message.seq 供
   *  data-seq 落 attr)。 */
  flatItems: Ref<FlatItem<ChatMessage>[]>;
  /** 当前渲染窗口(可见 + overscan)。由本 composable 计算并返回:
   *  D4 enter 判据(新 run 是否「在可见窗口内」)要消费它,单点持有
   *  避免组件与 composable 各算一份。 */
  virtualItems: Ref<VirtualItem[]>;
  /** 库实例(Vue 适配层返回的 shallowRef)。 */
  virtualizer: Ref<Virtualizer<HTMLElement, Element>>;
  /** 视口是否钉在末端(scrollEndThreshold=80 同源)—— 回底按钮显隐。 */
  isAtBottom: Ref<boolean>;
  /** data-seq flash(AC4 后半,N4 PR3):命中消息的 message.id(字符串
   *  化;wire id 可能是数字)。由 pendingScrollSeq 消费置值,
   *  SEARCH_FLASH_MS 后自动清零;MessageList 据此在 wrapper 挂
   *  `.search-hit` 类。 */
  flashKey: Ref<string | null>;
  /** D4 新 run 划入状态(N4 PR3):`from` = from+active 类同挂(初始
   *  绘制),双 rAF 后转 `active`(仅过渡声明,动画进行中),过渡窗结束
   *  回 null。null = 无动画。MessageList 据此挂 `run-enter-from` /
   *  `run-enter-active` 类。 */
  enterRow: Ref<{ key: string; phase: "from" | "active" } | null>;
  /** 滚动容器上转发(scroll handler 只管按钮显隐 + force-follow 退出)。 */
  onScroll: () => void;
  /** 回底按钮:流式瞬跳 + 重挂 force-follow;idle 平滑。 */
  jumpToBottom: () => void;
}

export function useVirtualizedMessages(
  scrollEl: Ref<HTMLElement | null>,
): VirtualizedMessages {
  const store = useChatStore();
  const questionCardsStore = useQuestionCardsStore();

  // ---- 尾行结构签名(可见性翻转的 reactive 闸门,见上方注记)---------
  // 六个可见性输入(content 只取空/非空位,长度取数值,error 取布尔)
  // 拼成短字符串:纯文本 delta 期间 content 位不变、长度不变 → 取值
  // 恒定 → computed 值稳定 → 不向 visibleMessages 传播。翻转事件
  // (首 delta / tool:call push / thinking 块 / error 置清)改值即传播。
  const tailSig = computed(() => {
    const msgs = store.messages;
    const m = msgs[msgs.length - 1];
    if (!m) return "";
    return `${m.content ? 1 : 0}|${m.toolCalls?.length ?? 0}|${
      m.toolResults?.length ?? 0
    }|${m.thinkingBlocks?.length ?? 0}|${m.redactedThinkingData?.length ?? 0}|${
      m.error ? 1 : 0
    }`;
  });

  // 缓存链的运行态(每 composable 实例一份;重判只在结构变化时发生,
  // 输出数组引用复用使下游 computed 在纯 delta 期间完全静默)。
  let lastTailSig: string | null = null;
  let lastMsgs: ChatMessage[] | null = null;
  let lastMsgsLen = -1;
  let lastVisible: ChatMessage[] = [];

  const visibleMessages = computed<ChatMessage[]>(() => {
    const msgs = store.messages;
    const sig = tailSig.value;
    // 重判触发面(三者任一):尾行签名翻转(可见性翻转事件)、消息
    // 数组换引用(session 切换 / reload 整体替换)、长度变化(append /
    // clear)。纯文本 delta:签名值恒定 + 引用/长度不变 → 直接复用
    // 上次的输出数组(同引用 → buildRunGroups/flatten 不重跑)。
    const sigChanged = sig !== lastTailSig;
    const structuralChange =
      sigChanged || msgs !== lastMsgs || msgs.length !== lastMsgsLen;
    lastTailSig = sig;
    lastMsgs = msgs;
    lastMsgsLen = msgs.length;
    if (!structuralChange) return lastVisible;

    // 重判:缓存未命中的新行 + 签名翻转时的尾行(翻转可能是「换了个
    // 新尾对象」—— cache miss 已覆盖 —— 或「同对象字段翻转」——这里
    // 补判;对同一对象两判其一,不重复)。
    const tailIdx = msgs.length - 1;
    const out: ChatMessage[] = [];
    for (let i = 0; i < msgs.length; i += 1) {
      const m = msgs[i]!;
      let v = coreVisibleCache.get(m);
      if (v === undefined || (sigChanged && i === tailIdx)) {
        v = isCoreVisible(m);
        coreVisibleCache.set(m, v);
      }
      // error 非单调(可能被 retry start 清除):非核心可见行每次重判
      // 都现读一次,依赖保持活跃 —— 清除事件能触发重判(A5+ 行消失)。
      if (!v && m.error) v = true;
      if (v) out.push(m);
    }
    lastVisible = out;
    return out;
  });

  const flatItems = computed<FlatItem<ChatMessage>[]>(() =>
    flattenRunGroups(buildRunGroups(visibleMessages.value)),
  );

  // §2 迁移表:动态 options(spike 实证 setOptions 透传成立)。
  const anchor = computed(() =>
    followModeOf({
      isStreaming: store.isCurrentSessionStreaming,
      forceFollow: store.forceFollowActive,
    }),
  );

  const virtualizer = useVirtualizer(
    computed(() => ({
      count: flatItems.value.length,
      getScrollElement: () => scrollEl.value,
      // 稳定持久 key:测量缓存按 id 存,session 内重滚零重测;
      // anchorTo:'end' 的 append/锚定判定也依赖它。
      getItemKey: (i: number) => flatItems.value[i]!.message.id,
      estimateSize: (i: number) => estimateMessageHeight(flatItems.value[i]!.message),
      // overscan 3(PR2 从 spike 初值 5 收敛):库默认 1;3 = 每侧 3 行
      // 余量,滚动事件同步渲染下无露白(e2e 快滚 + 慢滚用例绿),同时
      // 把瞬跳帧的窗口重挂常数(f2)压 ~15%。PR1 的 5 是保守初值。
      overscan: 3,
      // 聊天流模式:末项增长钉底 + 旧项 prepend 稳定(design §2)。
      anchorTo: "end" as const,
      followOnAppend: anchor.value.followOnAppend,
      // 现有 80px near-bottom 阈值同源迁移。
      scrollEndThreshold: SCROLL_END_THRESHOLD,
      // PR2 正式回退位(design §8 PR2 / §9-3):库对「测量尺寸变化 →
      // 滚动校正」的决策回调。undefined = 库默认策略(virtual-core
      // measure:首测补折上方全部、重测只补完全越线且非回滚方向)——
      // PR1 探针(e2e 两「不跳屏」用例)实证默认策略正确,维持 undefined。
      // 仅当实测出现校正异常(跳屏 / 钉底抖动 / 回滚方向抖动)时,在此
      // 提供 (item, delta, instance) => boolean 覆盖 —— 这是第一调节
      // 旋钮,不许为绕问题去 patch 库或改选项外的行为。
      shouldAdjustScrollPositionOnItemSizeChange: undefined,
      // useCachedMeasurements 有意不开(默认 false):它让 measureElement
      // 直接回缓存尺寸、不读 DOM rect,会废掉 ResizeObserver 驱动的
      // 流式增长测量 —— 动态高度是本场景的核心需求。spinner 重挂载的
      // 测量缓存问题(rebuild 后 estimate 重新起步)是 PR2 面。
    })),
  );

  // ---- PR0 spike 约束 2:React-parity 的 per-render _willUpdate --------
  // MessageList 每 render(DOM 常数,增量极小)后补调一次;纯 resize 的
  // clamp 补写由此获得入口。见文件头约束 2。
  onUpdated(() => {
    virtualizer.value?._willUpdate();
  });

  // ---- 渲染窗口(PR3 从 MessageList 上移:enter 判据单点消费)----------
  const virtualItems = computed<VirtualItem[]>(() =>
    virtualizer.value.getVirtualItems(),
  );

  // ---- 回底按钮显隐 + force-follow 退出(§2 第一行)--------------------
  const isAtBottom = ref(true);
  function onScroll(): void {
    const el = scrollEl.value;
    if (!el) return;
    // 判定式与旧 isNearBottom 同式(DOM 读数,scroll 事件同步当下即可
    // 得正确值)。不走 virtualizer.isAtEnd():库的内部 offset 在它自己的
    // scroll 监听器里更新,而本 handler(Vue @scroll)可能先于它触发,
    // 读到滞后值 —— 按钮显隐会慢一拍(实测 e2e)。读数不是写,spike
    // 约束 3(禁 scrollTop 直写)不涉及;库内 append/resize 跟滚的
    // isAtEnd 门仍由库自己(经 scrollEndThreshold 同一常量)判定。
    const near =
      el.scrollHeight - el.scrollTop - el.clientHeight < SCROLL_END_THRESHOLD;
    isAtBottom.value = near;
    if (store.forceFollowActive && !near) {
      store.forceFollowActive = false;
    }
  }

  // ---- jumpToBottom(§2 第七行)----------------------------------------
  function jumpToBottom(): void {
    isAtBottom.value = true;
    const v = virtualizer.value;
    if (!v) return;
    if (store.isCurrentSessionStreaming) {
      // 流式:瞬跳(behavior auto = instant)并重挂 force-follow ——
      // smooth 会在高频 delta 下叠动画卡顿(现实现同判据)。
      store.forceFollowActive = true;
      v.scrollToEnd({ behavior: "auto" });
    } else {
      v.scrollToEnd({ behavior: "smooth" });
    }
  }

  // ---- 落底原语(mount / session 切换 / reload / pending 共用)----------
  function scrollToLatest(): void {
    const v = virtualizer.value;
    if (!v || flatItems.value.length === 0) return;
    // 约束 3:走库 API。scrollToEnd = 滚到总尺寸末端;测量未落地时用
    // estimate 总高,目标即稳(虚拟化下无 mount churn,单次定位即稳)。
    v.scrollToEnd({ behavior: "auto" });
  }

  // ---- 手写 force-follow(spike 约束 1)--------------------------------
  // append(forceFollowActive && 流式)时无条件跟,绕过库的 isAtEnd 门。
  // 流式中末项 grow(非 append)的钉底由 anchorTo:'end' 的 wasAtEnd
  // 路径承担(force-follow 期间用户必然在门内)。
  watch(
    () => flatItems.value.length,
    (n, prev) => {
      if (n <= (prev ?? 0)) return;
      if (!store.forceFollowActive || !store.isCurrentSessionStreaming) return;
      void nextTick().then(() => {
        virtualizer.value?.scrollToEnd({ behavior: "auto" });
      });
    },
  );

  // ---- mount / session 切换落底(§2 第四行)----------------------------
  // stickToBottomUntilStable 退役:虚拟化下 DOM 数量常数,无多帧 churn,
  // 单次定位即稳(design §2「净简化」)。ChatPanel spinner v-if 重挂载
  // 时 onMounted 就是切换路径;watch 兜住不重挂的 session 变化。
  onMounted(() => {
    void nextTick().then(scrollToLatest);
  });
  watch(
    () => store.currentSessionId,
    (newId, oldId) => {
      if (newId === oldId) return;
      isAtBottom.value = true;
      void nextTick().then(scrollToLatest);
    },
  );

  // ---- reload F4(§2 第六行)--------------------------------------------
  watch(
    () => store.scrollAfterReload,
    () => {
      void nextTick().then(scrollToLatest);
    },
  );

  // ---- pending CH8-2a 强制回底(§2 第五行;契约见
  //      spec/frontend/chat/message-list-and-markdown.md §4)---------------
  const currentPendingInteraction = computed(() => {
    const sid = store.currentSessionId;
    return (sid && questionCardsStore.getPending(sid)) || null;
  });
  watch(currentPendingInteraction, (now, before) => {
    if (!now || before) return; // 仅 null→some;some→some 不重复(契约)
    isAtBottom.value = true;
    scrollToLatest();
  });

  // ---- data-seq 滚到命中 + flash 高亮(AC4 前半 PR1 / 后半 PR3)--------
  // 虚拟化后离屏消息不在 DOM,SearchModal 的 querySelector 直查静默
  // no-op —— 改走 store 命令(照 scrollAfterReload 先例):SearchModal
  // 写 store.pendingScrollSeq,这里消费后清零(同 seq 再次下令可再触发)。
  // flash(后半):命中行短暂高亮,主窗口形态复刻 CH12-1b 的 WAAPI 视觉
  // (accent 22% → transparent,1400ms)—— 由 flashKey 驱动 MessageList
  // 的 `.search-hit` 类,CSS keyframes 承载(几何无效应,不涉 D4 白名单)。
  const flashKey = ref<string | null>(null);
  let flashTimer: ReturnType<typeof setTimeout> | null = null;
  watch(
    () => store.pendingScrollSeq,
    async (seq) => {
      if (seq == null) return;
      await nextTick();
      const idx = flatItems.value.findIndex((f) => f.message.seq === seq);
      if (idx >= 0) {
        virtualizer.value?.scrollToIndex(idx, { align: "center" });
        // wrapper 类绑定按 message.id(getItemKey 同源);id 可能是数字,
        // 统一字符串化。重复下令以最后一次为准。
        flashKey.value = String(flatItems.value[idx]!.message.id);
        if (flashTimer) clearTimeout(flashTimer);
        flashTimer = setTimeout(() => {
          flashKey.value = null;
          flashTimer = null;
        }, SEARCH_FLASH_MS);
      }
      store.pendingScrollSeq = null;
    },
    // immediate:命令可能落在 MessageList 尚未挂载的窗口(spinner 重挂
    // 竞态),挂载时补消费;常态值 null 即 no-op。
    { immediate: true },
  );

  // ---- D4:新 run 划入动画(仅 opacity + translateX,白名单)-----------
  // 触发判据(design §4):该 key 上一渲染窗口不存在 && append 于末端 &&
  // 在可见窗口内。流式中追加的 assistant turn 归入已有 run(runFirst=
  // false)不触发;session 切换 / reloadAfterFinalize 的整体替换
  // (store.messages 数组换引用)只记录基线不动画 —— 它们的进场由容器
  // fade-in 承担,双动画重叠属过度动效。
  //
  // from 态挂在**组首行的 .msg 子根**(wrapper 自身带 inline translateY
  // 定位,transform 不可占用);双 rAF 后释放进 transition,与 Vue
  // TransitionGroup 内部同式。禁 scale / height:动画中间帧的测量值经
  // measureElement 按 getItemKey 写入持久缓存,几何属性会把中间尺寸固化
  // 成 session 内永久空隙(评审 D4 白名单约束)。
  const enterRow = ref<{ key: string; phase: "from" | "active" } | null>(null);
  let enterReleaseTimer: ReturnType<typeof setTimeout> | null = null;
  // 基线在 setup 即取当前值(watch 源的求值基线同一时刻建立):热挂载
  // (session 已加载再挂 MessageList)后的首个 append 不是「整体替换」,
  // 若留 null 会被 replaced 守卫误吞。切换/reload 的换引用判定不受影响
  // —— 它们发生在 setup 之后,新旧引用必然不同。
  let prevMsgsRef: ChatMessage[] | null = store.messages;
  let prevFlatLen = 0;
  let prevWindowKeys = new Set<string>();

  function triggerRunEnter(key: string): void {
    if (enterReleaseTimer) {
      clearTimeout(enterReleaseTimer);
      enterReleaseTimer = null;
    }
    enterRow.value = { key, phase: "from" };
    // 双 rAF:保证 from 态(0/24px)至少绘制一帧,释放才有过渡可放。
    requestAnimationFrame(() => {
      requestAnimationFrame(() => {
        if (enterRow.value?.key !== key) return;
        enterRow.value = { key, phase: "active" };
        enterReleaseTimer = setTimeout(() => {
          enterRow.value = null;
          enterReleaseTimer = null;
        }, RUN_ENTER_ACTIVE_MS);
      });
    });
  }

  watch(virtualItems, (items) => {
    const msgs = store.messages;
    const flat = flatItems.value;
    const keys = new Set<string>();
    for (const vi of items) keys.add(String(vi.key));
    // 上窗快照先取(「新 key」判定的基准),四项基线判定完成后统一更新。
    const prevKeys = prevWindowKeys;
    const replaced = msgs !== prevMsgsRef;
    const grew = flat.length > prevFlatLen;
    const head = flat.length > 0 ? flat[flat.length - 1] : undefined;
    prevMsgsRef = msgs;
    prevFlatLen = flat.length;
    prevWindowKeys = keys;
    if (replaced || !grew) return;
    // append 于末端 + 组首;新 key = 上窗不存在;可见 = 在当前渲染窗口。
    if (!head?.runFirst) return;
    const key = String(head.message.id);
    if (!keys.has(key)) return;
    if (prevKeys.has(key)) return;
    triggerRunEnter(key);
  });

  onScopeDispose(() => {
    if (flashTimer) clearTimeout(flashTimer);
    if (enterReleaseTimer) clearTimeout(enterReleaseTimer);
  });

  return {
    flatItems,
    virtualItems,
    virtualizer,
    isAtBottom,
    flashKey,
    enterRow,
    onScroll,
    jumpToBottom,
  };
}
