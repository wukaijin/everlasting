// useErrorBus — 全局错误总线(A5 错误处理完善,2026-07-02;R1 接入
// reka-ui Toast,2026-07-17;09-21 全局兜底分类收口,任务
// 09-21-error-bus-category-recovery)。
//
// 统一收口 invoke() 的 IPC 错误:parseAppCommandError 把 Tauri/HTTP
// rejection(AppCommandError 形状对象,含 TransportError / JSON 字符串)
// 容错解析成 AppCommandError,push 进全局 errors 数组,按 category 路由
// 分发。09-21 起 `handle()` 对不可识别输入不再静默、也不再误标,按来源
// 分级留痕(见 handle 注释);裸 string 不再强标「Server」—— 本地字符串
// 借「服务端错误」之名弹用户正是 ResizeObserver 误报案的原病。
//
// 为什么用模块级单例 ref(而非 Pinia store):errors 列表小、无需持久化、
// 跨组件共享用模块单例足够(与 useKeyboard 的 window-listener 单例模式
// 同构)。errors 上限 50 FIFO,防长会话 Server/Network 风暴无限增长。
//
// R1 (2026-07-17) 路由分发:`routeByCategory` 现把 4 类
// (Auth/RateLimit/Server/Network) 推给 `useToast()` composable,InvalidRequest
// 保留 `console.warn`(本地错误不打扰用户,见 prd.md R1 决策表)。主链路
// 36 处 IPC 错误展示不动 — `routeByCategory` 实际只服务 main.ts 全局兜底
// 1 个调用点(research/05)。
//
// 与后端契约对齐:AppCommandError 的字段名(camelCase)与 Rust
// `app/src-tauri/src/error.rs` 的 `#[serde(rename_all = "camelCase")]` 一致。
// category 用 PascalCase(与 Rust ErrorCategory variant 名一致),类型
// 单一事实源是 `./error.ts` 的 `AppErrorCategory`(09-21 评审裁决:
// transport 与本文件只导入不本地定义);本文件的 `ErrorCategory` 导出
// 保留为其别名(历史消费面)。

import { ref, readonly } from "vue";
import { useToast } from "../composables/useToast";
import { categoryToastKey, type AppErrorCategory } from "./error";

/** PascalCase 五值 category,单一事实源在 `./error.ts`(别名导出保留
 *  历史引用;新增消费方建议直接 import `AppErrorCategory`)。 */
export type ErrorCategory = AppErrorCategory;

const VALID_CATEGORIES: ReadonlySet<ErrorCategory> = new Set([
  "Auth",
  "RateLimit",
  "InvalidRequest",
  "Server",
  "Network",
]);

export interface AppCommandError {
  category: ErrorCategory;
  kind: string;
  message: string;
  retryable: boolean;
  requestId?: string;
}

// 上限 FIFO:防长会话 Server/Network 风暴让 errors 数组无限增长。
// 单条 dismiss / TTL 过期策略推到 toast UI follow-up。
const MAX_ERRORS = 50;
const errors = ref<AppCommandError[]>([]);

export function useErrorBus() {
  const push = (err: AppCommandError) => {
    errors.value.push(err);
    if (errors.value.length > MAX_ERRORS) {
      // 丢最旧,保留最近 MAX_ERRORS 条。
      errors.value.splice(0, errors.value.length - MAX_ERRORS);
    }
    routeByCategory(err);
  };

  /** 入口:把 `invoke().catch(e => useErrorBus().handle(e))` 与 main.ts
   *  全局监听的未知错误分级收纳(09-21 收口,design D1):
   *
   *  ① 结构化错误(AppCommandError 形状对象,含 TransportError)→
   *     push + 按 category 路由。**必须先于 instanceof Error 判断** ——
   *     TransportError 也是 Error 实例,顺序错会被 ④ 吞掉(接缝集成
   *     用例守门,useErrorBus.test.ts)。
   *  ② 良性浏览器噪音(ResizeObserver loop 等,string 形态)→
   *     console.debug,不进总线不 toast(误报案原路径)。
   *  ③ 其他裸 string → console.warn,不 push FIFO、不 toast
   *     (评审裁决:关死「裸 string → Server toast」的口子)。
   *  ④ 其他 Error 实例(运行时 JS 错误)→ console.error,不 toast ——
   *     未捕运行时错误几乎不可由用户行动修复,弹 toast 只会复刻
   *     「噪音训练用户忽略弹窗」;但绝不无声(此前是静默丢弃)。
   *     其中 name==="TransportError" 单独打标 `[errorBus:transport-
   *     shape-miss]`:正常应被 ① 识别,落到此说明构造收窄被打破,
   *     console 里可直接发现(design D2.3 观测兜底)。
   *  ⑤ 其余(null/number/形状不合法对象)维持静默丢弃(与旧行为一致)。
   */
  const handle = (e: unknown) => {
    const err = parseAppCommandError(e);
    if (err) {
      push(err);
      return;
    }
    if (typeof e === "string") {
      if (isBenignBrowserNoise(e)) {
        console.debug("[errorBus:benign-noise]", e);
        return;
      }
      console.warn("[errorBus:uncaught-string]", e);
      return;
    }
    if (e instanceof Error) {
      if (e.name === "TransportError") {
        console.error("[errorBus:transport-shape-miss]", e);
      } else {
        console.error("[errorBus:uncaught]", e);
      }
    }
  };

  const clear = () => {
    errors.value.splice(0, errors.value.length);
  };

  return {
    errors: readonly(errors),
    push,
    handle,
    clear,
  };
}

/** 已知良性浏览器噪音(不进 errorBus、不 toast;console.debug 留痕)。
 *  **白名单式前缀匹配** —— 只封确切已知的噪音(ResizeObserver loop 的
 *  两个已知变体:completed with undelivered notifications / limit
 *  exceeded),不搞宽松 includes,防误杀真错误。消费方:main.ts window
 *  error 监听入口(event.error 为空时先过这里)+ handle() 的 string
 *  分支。 */
export function isBenignBrowserNoise(msg: string): boolean {
  return msg.startsWith("ResizeObserver loop");
}

/** AppCommandError 形状门(category/kind/message/retryable 四字段 +
 *  category 五值域)。导出供 transport 测试做构造收窄不变量断言
 *  (TransportError 实例恒过本门,http.test.ts)。 */
export function isAppCommandError(x: unknown): x is AppCommandError {
  if (typeof x !== "object" || x === null) return false;
  const o = x as Record<string, unknown>;
  return (
    typeof o.category === "string" &&
    // category 值域校验:防恰好含 4 字段的普通对象/JSON 被误判。
    VALID_CATEGORIES.has(o.category as ErrorCategory) &&
    typeof o.kind === "string" &&
    typeof o.message === "string" &&
    typeof o.retryable === "boolean"
  );
}

/**
 * 容错解析 IPC 错误为 AppCommandError。兼容 2 种输入(09-21 收窄):
 * 1. AppCommandError 形状对象 —— Tauri 序列化 rejection / daemon HTTP
 *    body / TransportError 实例(D2.2 起它本身即此形状)。
 * 2. JSON 字符串 —— 老链路 / 手动 wrap 的 JSON,parse 出形状才认。
 * 返回 `null` 表示无法识别。**裸 string 不再降级 Server/Unknown**
 * (原强标会让本地噪音弹假「服务端错误」);调用方按来源分级留痕,
 * `extractErrorMessage` 的输出逐字节不变(落空后走 string/Error 分支)。
 */
export function parseAppCommandError(e: unknown): AppCommandError | null {
  if (typeof e === "object" && e !== null && isAppCommandError(e)) {
    return e as AppCommandError;
  }
  if (typeof e === "string") {
    try {
      const parsed: unknown = JSON.parse(e);
      if (isAppCommandError(parsed)) return parsed as AppCommandError;
    } catch {
      // 非 JSON:返回 null(调用方 console.warn,不再借 Server 之名)。
    }
    return null;
  }
  return null;
}

/** 从未知错误提取中文消息。前端错误显示的统一入口,兼容:
 * 1. `AppCommandError` 对象(后端 IPC 错误)→ `message`
 * 2. `Error` 实例(本地 JS 错误)→ `e.message`
 * 3. 原始 string → 原样
 * 4. 其他 → "(未知错误)"
 *
 * A5(2026-07-02):后端 command 改返 `AppCommandError` 对象后,直接 `String(e)`
 * 会显示 `[object Object]`;本 helper 让前端所有错误显示点统一兼容结构化错误 +
 * 裸 Error + 裸字符串,替换散落的 `String(e)` / `e instanceof Error ? e.message : String(e)`。
 *
 * 09-21 收窄不变量(AC4):parseAppCommandError 不再对裸 string 降级
 * 构造,但落空后走下方 string/Error 分支,**所有输入的输出逐字节不变**
 * —— 30+ 调用点零改动即兼容。 */
export function extractErrorMessage(e: unknown): string {
  const parsed = parseAppCommandError(e);
  if (parsed) return parsed.message;
  if (e instanceof Error) return e.message;
  if (typeof e === "string") return e;
  return "(未知错误)";
}

/** 从未知错误读 category(design D2.4,给未来不接 UI):形状识别通过
 *  (AppCommandError 对象 / TransportError 实例 / JSON 字符串)→
 *  category 字段;否则 null。供 catch 点 / 后续重试按钮接线取分类;
 *  本任务只交付函数 + 测试,不改任何展示面。 */
export function extractErrorCategory(e: unknown): AppErrorCategory | null {
  const parsed = parseAppCommandError(e);
  return parsed ? parsed.category : null;
}

// 5 类路由分发。
//
// R1 (2026-07-17) 升级:4 类 (Auth/RateLimit/Server/Network) 走
// `useToast().show(...)`(reka-ui Toast 弹窗,在 AppShell.vue 顶层挂载)。
// InvalidRequest 保留 `console.warn`(原状) —— 它是「本地错误不打扰」,
// 调用点若有表单字段下内联渲染需求由调用点自己 watch errors 决定,
// scope B 不接全局 toast。
//
// `kind` / `retryable` 不送 toast(用户看不到的诊断字段);后续如果想要
// "Server with retry 按钮在 toast 上"是从 useToast 加 retry 字段,不在这里。
//
// `title` 字段对 route 分发的 toast 是冗余(`category` 已经决定 4 类
// prefix 的中文短标题),所以这里传 `description: err.message`,
// `ToastProvider.vue` 内部按 category 取 title(避免 route 写两份)。
function routeByCategory(err: AppCommandError): void {
  const key = categoryToastKey(err.category);
  if (key === null) {
    // InvalidRequest or unknown: console.warn 原状(spec 设计如此)。
    showInlineError(err);
    return;
  }
  const { show } = useToast();
  show({
    category: key,
    title: key, // ToastProvider 按此取中文 prefix;同值冗余但保留契约
    description: err.message,
  });
}

function showInlineError(err: AppCommandError): void {
  // InvalidRequest 由调用点 watch errors 决定内联渲染(表单字段下),不全局 toast。
  console.warn(`[errorBus:InvalidRequest] ${err.message}`, err);
}
