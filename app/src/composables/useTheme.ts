// useTheme — 界面主题(经典 / 激进)的前端本地偏好。
//
// 主题体系:classic = style.css 的 @theme 默认值(html 不带 data-theme,
// 渲染与历史上字节一致);aggressive = theme-aggressive.css 里
// `:root[data-theme="aggressive"]` 的整组 token 覆盖(锐角、可见网格、
// volt 荧光单色 accent)。所有组件都消费 var(--color-*),因此换肤是
// 纯 token 层的事,组件零改动。
//
// 状态放模块级 ref(单例):Sidebar 的 footer 快速切换钮与将来其他
// 入口共享同一响应式状态。apply 在首次 useTheme() 时同步执行一次
// (幂等),main.ts 在 mount 前调用以避免首帧闪面。
//
// 持久化走 localStorage(纯前端偏好,不进后端 app_config —— 那张表
// 的布尔白名单通道装不下三态字符串,也不想为实验主题动后端)。

import { ref } from "vue";

export type ThemeName = "classic" | "aggressive";

const STORAGE_KEY = "everlasting.theme";

/** 未存储时的默认主题。实验期默认 aggressive(任务要求"尝试更激进");
 *  一键可切回 classic,切换即写 localStorage,此后跟随用户选择。 */
const DEFAULT_THEME: ThemeName = "aggressive";

function readStored(): ThemeName {
  try {
    const v = localStorage.getItem(STORAGE_KEY);
    if (v === "classic" || v === "aggressive") return v;
  } catch {
    /* localStorage 不可用(私隐模式等)→ 落默认值 */
  }
  return DEFAULT_THEME;
}

const theme = ref<ThemeName>(readStored());

/** 把当前主题落到 <html> 上。classic 刻意**删除**属性而不是写
 *  data-theme="classic":style.css 的 @theme 值就是 classic 本体,
 *  不带属性可保证 classic 渲染路径与历史完全一致(零覆盖参与)。 */
function applyToDom(next: ThemeName): void {
  if (typeof document === "undefined") return;
  if (next === "aggressive") {
    document.documentElement.dataset.theme = "aggressive";
  } else {
    delete document.documentElement.dataset.theme;
  }
}

/** 首次调用即把存储值应用到 DOM(幂等)。 */
export function useTheme() {
  applyToDom(theme.value);

  function setTheme(next: ThemeName): void {
    theme.value = next;
    applyToDom(next);
    try {
      localStorage.setItem(STORAGE_KEY, next);
    } catch {
      /* 写失败不致命:本次会话内主题仍生效 */
    }
  }

  function toggleTheme(): void {
    setTheme(theme.value === "aggressive" ? "classic" : "aggressive");
  }

  return { theme, setTheme, toggleTheme };
}
