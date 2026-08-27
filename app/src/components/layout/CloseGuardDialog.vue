<script setup lang="ts">
// CloseGuardDialog — F6 detach 边界守卫(2026-08-27,
// `08-27-f6-async-agent-task`)。仅在 Tauri webview(Thin/Full GUI 壳)
// 注册:窗口关闭请求(X / Alt+F4 / 任务栏 / TitleBar close 按钮)时有
// session 在跑 → 拦截并弹确认「终止并关闭」。Web/PWA **不挂**
// (`isTauriWebview()` 为 false):关标签/杀 App 只断 SSE 订阅,
// standalone daemon 独立存活,任务照跑 —— 两种关闭语义见
// docs/REMOTE-DEPLOY.md「detach 边界」。
//
// busy 口径 = 本端在跑(streamingSessionIds)∪ 服务端 busy
// (list_sessions 的 runtime 信号;含**其他端**发起的轮次与**等闸**
// 轮次 —— Thin 模式关 GUI 杀 sidecar daemon,全部陪葬,必须数全)。
// 同一 session 两态并见只计一次。
//
// 守卫判据刻意是 `isTauriWebview()`(判 window.__TAURI_INTERNALS__)
// 而非 transport 种类:daemon 化后 Tauri 壳内默认也是 httpTransport
// (sidecar),transport 分辨不出壳形态(评审 P2)。
import { onBeforeUnmount, ref } from "vue";
import { useChatStore } from "../../stores/chat";
import { useStreamControllerStore } from "../../stores/streamController";
import { isTauriWebview } from "../../transport/env";
import ConfirmDialog from "../common/ConfirmDialog.vue";

const open = ref(false);
const busyCount = ref(0);

let unlisten: (() => void) | null = null;
// handler 外捕获一次的 window 句柄;确认时复用 destroy()(绕过
// close-requested 二次拦截),不重新 getCurrentWindow(评审 P2 细节)。
let pendingDestroy: (() => Promise<void>) | null = null;

/** 导出供测试:闭合计数(本端 streaming ∪ 服务端 busy,去重)。 */
function countBusy(): number {
  const stream = useStreamControllerStore();
  const chatStore = useChatStore();
  let n = stream.streamingSessionIds.size;
  for (const s of chatStore.sessions) {
    if (s.busy === true && !stream.streamingSessionIds.has(s.id)) n++;
  }
  return n;
}

if (isTauriWebview()) {
  void (async () => {
    const { getCurrentWindow } = await import("@tauri-apps/api/window");
    const win = getCurrentWindow();
    pendingDestroy = () => win.destroy();
    unlisten = await win.onCloseRequested((event) => {
      const n = countBusy();
      if (n === 0) return; // 无在跑:不 preventDefault,放行默认关闭
      event.preventDefault();
      busyCount.value = n;
      open.value = true;
    });
  })().catch((e) => {
    // 注册失败(理论不可达):宁可守卫失效也不阻塞关闭。
    console.error("[CloseGuard] onCloseRequested registration failed", e);
  });
}

function onConfirm() {
  open.value = false;
  void pendingDestroy?.();
}

function onCancel() {
  open.value = false;
}

onBeforeUnmount(() => {
  unlisten?.();
  unlisten = null;
});

defineExpose({ countBusy });
</script>

<template>
  <ConfirmDialog
    :open="open"
    title="仍有会话在后台运行"
    variant="warning"
    confirm-text="终止并关闭"
    @confirm="onConfirm"
    @cancel="onCancel"
  >
    <p class="close-guard__text">
      {{ busyCount }} 个会话仍有在跑的任务。关闭窗口会终止 sidecar
      daemon,这些任务(含排队中的轮次)将被中断;需要后台持续运行请改用
      standalone daemon(浏览器/PWA 访问)。
    </p>
  </ConfirmDialog>
</template>

<style scoped>
.close-guard__text {
  margin: 0;
  font-size: var(--font-size-sm, 0.8125rem);
  line-height: 1.6;
  color: var(--color-text-secondary, #999);
}
</style>
