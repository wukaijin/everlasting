// useMobileNav — 移动端抽屉导航开关(S5 08-11-mobile-adaptation, Step 2-4)。
//
// 设计动机:
// - 桌面三栏(AppHeader 项目tabs + Sidebar 会话列表 + main 对话)在手机宽度
//   挤死;S5 改抽屉式 —— Sidebar 移动端变全屏 overlay,AppHeader 加汉堡按钮触发。
// - 状态需被 AppHeader(读写 toggle)和 Sidebar(读 open + close)共享。
//
// 设计原则(与 useToast / useErrorBus / useKeyboard 同构):
// - **module-level 单例**(不是 provide/inject):SPA 无 SSR,模块级 ref 跨组件
//   共享同一份状态;AppHeader / Sidebar 直接调 useMobileNav() 拿同一 ref。
// - 桌面下 open 状态被 CSS 忽略(Sidebar 桌面始终常驻,fixed 定位只在
//   @media (max-width:767px) 生效),所以这个状态在桌面不影响布局。
//
// 接入位置:AppShell(遮罩 @click close) / AppHeader(汉堡 toggle) /
// Sidebar(open class + 选 session 自动 close)。

import { ref, type Ref } from "vue";

const mobileNavOpen = ref(false);

export interface UseMobileNavReturn {
  mobileNavOpen: Readonly<Ref<boolean>>;
  open: () => void;
  close: () => void;
  toggle: () => void;
}

export function useMobileNav(): UseMobileNavReturn {
  function open(): void {
    mobileNavOpen.value = true;
  }
  function close(): void {
    mobileNavOpen.value = false;
  }
  function toggle(): void {
    mobileNavOpen.value = !mobileNavOpen.value;
  }
  return {
    mobileNavOpen: mobileNavOpen as Readonly<Ref<boolean>>,
    open,
    close,
    toggle,
  };
}
