// useToast — 全局 toast 通知(A5 错误处理完善 / scope B,2026-07-17)。
//
// 设计动机:
// - `useErrorBus.routeByCategory` 5 个 stub 已就位但全 `console.warn`,scope B
//   R1 把它升级为 reka-ui 2.9.9 Toast primitive(本文件 + `ToastProvider.vue`)。
// - 主链路 36 处 IPC 错误展示不动(`useErrorBus.ts:127-180` 只 1 个调用点
//   `main.ts:17-27` 全局兜底,见 research/05-useErrorBus-stub-callsites.md)。
//
// 设计原则:
// - **module-level 单例**(不是 composable 注入):与 `useErrorBus` / `useKeyboard`
//   同构,`main.ts` 全局兜底不需 setup,non-Vue 上下文能调用。
// - **4 类 category**:Auth / RateLimit / Server / Network。InvalidRequest
//   不走 toast(原 `routeByCategory` 走 console.warn,保留)。
// - **queue 上限 3 + dedupe 5s**:max 3 是经验值(突发 3 类同时到也够看);
//   5s dedupe 挡"同一抖动"重复戳;overflow FIFO(老的先走,因新错误更重要)。
// - **TTL 5s 自动消失**:重用户可点击 × 手关(reka-ui `ToastClose` 自动 wire)。
//
// 接入位置:`ToastProvider.vue` + `AppShell.vue`(F3 挂载)。

import { ref, type Ref } from "vue";

const MAX_CONCURRENT = 3;
const DEDUPE_WINDOW = 5000;
const DEFAULT_TTL = 5000;

export type ToastCategory = "Auth" | "RateLimit" | "Server" | "Network";

export interface Toast {
  id: string;
  category: ToastCategory;
  title: string;
  description?: string;
  createdAt: number;
  ttl: number;
}

export interface ShowToastInput {
  category: ToastCategory;
  title: string;
  description?: string;
  ttl?: number;
}

export interface UseToastReturn {
  // toasts 暴露为 readonly 类型提示,运行时不变(与现有 useErrorBus.errors
  // readonly 模式一致 —— 见 `utils/useErrorBus.ts:46-47`);`.value` 仍可读,
  // 外部消费只能通过返回的 show / dismiss / clear 改写。
  toasts: Readonly<Ref<Toast[]>>;
  show: (input: ShowToastInput) => string | null;
  dismiss: (id: string) => void;
  clear: () => void;
}

const toasts = ref<Toast[]>([]);

// 每个 toast 的 ttl 定时器 id。Key = toast.id。`dismiss` 取消并删。
const ttlTimers = new Map<string, ReturnType<typeof setTimeout>>();

function genId(): string {
  // 同步生成 uuid-ish id(millis + 8 随机字符)。Math.random 已足够 dedupe
  // 用(碰撞概率 ~1e-12);真正 UUID v4 留 server 端。
  return (
    Date.now().toString(36) +
    "-" +
    Math.random().toString(36).slice(2, 10)
  );
}

function clearTtlTimer(id: string): void {
  const t = ttlTimers.get(id);
  if (t !== undefined) {
    clearTimeout(t);
    ttlTimers.delete(id);
  }
}

export function useToast(): UseToastReturn {
  const show = (input: ShowToastInput): string | null => {
    const now = Date.now();
    // dedupe:同 (category, title, description) 在 DEDUPE_WINDOW 内只弹 1 次。
    // description 缺失时按字符串 "" 比对(语义:description 一样 → 同一条)。
    const dedupeDesc = input.description ?? "";
    for (const t of toasts.value) {
      const sameTitle = t.title === input.title;
      const sameDesc = (t.description ?? "") === dedupeDesc;
      const inWindow = now - t.createdAt < DEDUPE_WINDOW;
      if (t.category === input.category && sameTitle && sameDesc && inWindow) {
        // 已被同 toast 收纳,直接吞本次。
        return null;
      }
    }
    const id = genId();
    const ttl = input.ttl ?? DEFAULT_TTL;
    const toast: Toast = {
      id,
      category: input.category,
      title: input.title,
      createdAt: now,
      ttl,
      ...(input.description !== undefined
        ? { description: input.description }
        : {}),
    };
    toasts.value.push(toast);
    // overflow:FIFO 删最早的(老的先走;新错误比老错误更重要)。
    while (toasts.value.length > MAX_CONCURRENT) {
      const removed = toasts.value.shift();
      if (removed) clearTtlTimer(removed.id);
    }
    // ttl 自动消失。
    const timer = setTimeout(() => {
      dismiss(id);
    }, ttl);
    ttlTimers.set(id, timer);
    return id;
  };

  const dismiss = (id: string): void => {
    clearTtlTimer(id);
    const idx = toasts.value.findIndex((t) => t.id === id);
    if (idx >= 0) toasts.value.splice(idx, 1);
  };

  const clear = (): void => {
    for (const id of ttlTimers.keys()) clearTtlTimer(id);
    toasts.value.splice(0, toasts.value.length);
  };

  return {
    toasts: toasts as Readonly<Ref<Toast[]>>,
    show,
    dismiss,
    clear,
  };
}

// 仅测试用(避免 TTL 定时器在 vitest `vi.useFakeTimers()` 里卡死进程),
// 生产代码不应调。`clear` 已覆盖;`dismissAllTimers` 用于单测 afterEach。
export function _useToastInternal_clearAllTimers(): void {
  for (const id of ttlTimers.keys()) clearTtlTimer(id);
  ttlTimers.clear();
}
