<script setup lang="ts">
// ChatView — the existing chat experience (AppShell + ChatWindow), now
// reachable via the `/chat` route (S4 Step 2, design §5.5).
//
// Lifted 1:1 from the pre-router `App.vue`: the global SSE listener
// lifecycle is owned by this view (onMounted start / onUnmounted stop),
// so the stream only runs while the user is actually in chat. Pairing
// and node-list views don't need an active SSE tunnel (design §5.2 —
// avoids wasting the proxied connection + permissions outside chat).
import { onMounted, onUnmounted } from "vue";
import AppShell from "../components/layout/AppShell.vue";
import ChatWindow from "../components/ChatWindow.vue";
import { useStreamControllerStore } from "../stores/streamController";

const streamController = useStreamControllerStore();

onMounted(() => {
  void streamController.start();
});

onUnmounted(() => {
  streamController.stop();
});
</script>

<template>
  <AppShell>
    <ChatWindow />
  </AppShell>
</template>
