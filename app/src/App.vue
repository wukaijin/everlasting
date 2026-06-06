<script setup lang="ts">
// App root. Wires the global SSE listener lifecycle via the
// streamController store: register listeners on mount (idempotent
// — start() no-ops after the first call), tear them down on
// unmount. PR2 scaffold; the chat store (chat.ts) is the actual
// consumer of the controller's API, wired in PR3.
import { onMounted, onUnmounted } from "vue";
import AppShell from "./components/layout/AppShell.vue";
import ChatWindow from "./components/ChatWindow.vue";
import { useStreamControllerStore } from "./stores/streamController";

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
