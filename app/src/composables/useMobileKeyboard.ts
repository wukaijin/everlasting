// useMobileKeyboard — iOS 软键盘适配(S5 08-11-mobile-adaptation, Step 6)。
//
// 设计动机(design §4.1 / review P2-2):
// - iOS Safari 软键盘是 overlay,**不改 layout viewport**,dvh 对它无感
//   (dvh 只处理 URL bar 收缩)。软键盘弹起时 ChatInput(在 AppShell 底部)
//   被遮住,PRD 验收第 3 条(软键盘不挡输入框)核心机制。
// - Android Chrome 会 resize layout viewport,本方案对 Android 无害
//   (visualViewport.height ≈ layout viewport.height,变量不变)。
//
// 机制:
// - 监听 window.visualViewport 的 resize/scroll,把 visualViewport.height
//   写到 :root 的 --visual-viewport-height CSS 变量。
// - AppShell 移动端 height: var(--visual-viewport-height, var(--app-height))
//   —— 软键盘弹起时 AppShell 缩到键盘上方,ChatInput 自然不被遮。
//
// 生命周期:调用方(ChatInput)在 setup 调 useMobileKeyboard(),onMounted 加
// 监听 / onUnmounted 移除。桌面 visualViewport 存在但 height 不变,且桌面
// AppShell height 不引用此变量(桌面用 100vh),所以桌面调用无害。

import { onMounted, onUnmounted } from "vue";

function updateViewportHeight(): void {
  const vv = window.visualViewport;
  if (vv) {
    document.documentElement.style.setProperty(
      "--visual-viewport-height",
      `${vv.height}px`,
    );
  }
}

export function useMobileKeyboard(): void {
  onMounted(() => {
    if (typeof window === "undefined" || !window.visualViewport) return;
    window.visualViewport.addEventListener("resize", updateViewportHeight);
    window.visualViewport.addEventListener("scroll", updateViewportHeight);
    updateViewportHeight();
  });

  onUnmounted(() => {
    if (typeof window === "undefined" || !window.visualViewport) return;
    window.visualViewport.removeEventListener("resize", updateViewportHeight);
    window.visualViewport.removeEventListener("scroll", updateViewportHeight);
  });
}
