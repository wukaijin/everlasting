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
// app commands pass through), which clears the token + tears down the
// SSE stream + invokes the `onAuthFailed` callback registered here.
// We wire that callback to `router.push("/pairing")` so a revoked /
// expired token bounces the user back to pairing. This only fires when
// a device token was present (i.e. pwa-remote mode); daemon/Tauri never
// carry a token, so the registration is inert there.
import { onMounted, onUnmounted } from "vue";
import { router } from "./router";
import { setOnAuthFailed } from "./transport/http";

onMounted(() => {
  setOnAuthFailed(() => {
    void router.push("/pairing");
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
  <router-view />
</template>
