<script setup lang="ts">
// App root — router outlet (S4 Step 2, design §5.2) + the transport-layer
// 401 handler registration (S4 Step 5, design §6.2 / P2-1).
//
// The previous AppShell + ChatWindow + streamController lifecycle moved
// 1:1 into `views/ChatView.vue` (mounted on the /chat route). The SSE
// stream is thus scoped to the chat view rather than always-on.
//
// 401 handling (P2-1): the errorBus only catches *uncaught* promise
// rejections, but every existing invoke() call site systematically
// try/catch+swallows errors (ProvidersTab, etc.) — a 401 there would
// never reach the errorBus and would silently fail. So the 401
// interception lives in `transport/http.ts` (the single choke point all
// app commands pass through), which drops the CURRENT node's token
// (08-26 multi-node: other pairings survive) + tears down the SSE
// stream + invokes the `onAuthFailed` callback registered here. We
// wire that callback to /nodes while other pairings remain, else
// /pairing — a revoked token only bounces the user all the way back to
// pairing when it was the last one. This only fires when a device
// token was present (i.e. pwa-remote mode); daemon/Tauri never carry a
// token, so the registration is inert there.
import { onMounted, onUnmounted } from "vue";
import { router } from "./router";
import { setOnAuthFailed } from "./transport/http";
import { hasPairedNode } from "./transport/auth";

onMounted(() => {
  setOnAuthFailed(() => {
    void router.push(hasPairedNode() ? "/nodes" : "/pairing");
  });
});

onUnmounted(() => {
  // Clear the callback if the root ever unmounts (it doesn't in this
  // SPA, but the cleanup keeps the registration symmetric and avoids a
  // dangling closure in tests that mount/unmount the app).
  setOnAuthFailed(null);
});
</script>

<template>
  <!--
    08-14 ux-polish-r1 WP4 4.1(评审 D1):路由切换 fade 过渡。
    - v-slot + <Transition mode="out-in">:旧视图先淡出、新视图再淡入,
      避免两个视图同时半透明叠层(ChatView 的 AppShell 是全屏布局,
      in-out 同帧叠加会穿帮)。
    - 三个 view(ChatView/NodeListView/PairingView)均为单根节点,
      Transition 可用;`:is="Component"` 切换即触发。
    - 过渡类 .route-fade-* 定义在全局 style.css(--duration-base +
      --ease-decelerate);prefers-reduced-motion 由 style.css 顶层的
      `* { transition-duration: 0.01ms !important }` 兜底 —— 该选择器
      覆盖任何元素上携带 .route-fade-*-active 的 transition,验证过写法。
  -->
  <router-view v-slot="{ Component }">
    <Transition name="route-fade" mode="out-in">
      <component :is="Component" />
    </Transition>
  </router-view>
</template>
