import { createApp } from "vue";
import { createPinia } from "pinia";
import App from "./App.vue";
import "./style.css";
import { router } from "./router";
import { useErrorBus } from "./utils/useErrorBus";
import { transport } from "./transport";
import { tauriTransport } from "./transport/tauri";
import { awaitDaemonHealthy, type DaemonHealth } from "./transport/health";

const app = createApp(App);
app.use(createPinia());
app.use(router);

// A5(2026-07-02)全局未捕错误器:`invoke()` 未 `.catch` 的 rejection + 任意
// 运行时 JS 错误,统一入错误总线。`parseAppCommandError` 容错 3 种输入
// (AppCommandError 对象 / JSON 字符串 / 原始 string),所以无论后端返回
// 结构化错误还是老链路 String rejection,都能被收纳 + 按 category 路由。
// 防静默 —— 任何漏掉的 invoke .catch 或运行时错误都进 errorBus,不丢失。
// 现有 fire-and-forget .catch(record_tool_duration / update_message_latency /
// permissions 超时 deny)故意 swallow,不触发本监听(它们已 .catch)。
if (typeof window !== "undefined") {
  const { handle } = useErrorBus();
  window.addEventListener("error", (event) => {
    // event.error 是 Error 对象(或 undefined);fallback 到 event.message(string)。
    handle(event.error ?? event.message);
  });
  window.addEventListener("unhandledrejection", (event) => {
    // event.reason 是 rejection 原因(AppCommandError 对象 / Error / string)。
    handle(event.reason);
  });
}

// P2.4 D3.4: 在 `app.mount` 前等 daemon 健康(Q5 分层校验)。
// httpTransport 是默认(P2.4 D3.1),若 daemon 未就绪 GUI 完全无功能,
// 故 fail-loud:超时/协议不匹配 → 渲染全屏错误覆盖层,不静默降级。
// `?transport=tauri` 逃生模式下无 daemon,跳过握手(Rust 侧 Full 模式直连 IPC)。
//
// 暴露 handshake 结果到 window 供 App.vue 启动诊断 + 测试断言用。
async function bootstrap(): Promise<void> {
  if (transport === tauriTransport) {
    // 逃生模式:Rust Full GUI 模式,无 sidecar,直接挂载。
    app.mount("#app");
    return;
  }

  try {
    const health = await awaitDaemonHealthy();
    (window as unknown as { __DAEMON_HEALTH__?: DaemonHealth }).__DAEMON_HEALTH__ =
      health;
    app.mount("#app");
  } catch (e) {
    // Fail-loud:渲染全屏错误覆盖层。不 mount app(避免半渲染无功能 UI)。
    renderFatalOverlay(e instanceof Error ? e.message : String(e));
  }
}

/** 渲染 daemon 不可用时的全屏错误覆盖层(fail-loud)。
 *  替代 mount app —— 用户看到明确错误 + 排查步骤,而非空白/卡死 UI。 */
function renderFatalOverlay(message: string): void {
  const root = document.getElementById("app");
  if (!root) return;
  root.innerHTML = `
    <div style="position:fixed;inset:0;display:flex;align-items:center;justify-content:center;background:#1a1a1a;color:#e5e5e5;font-family:system-ui,sans-serif;padding:2rem;">
      <div style="max-width:640px;">
        <h1 style="font-size:1.25rem;margin:0 0 1rem;color:#f87171;">Everlasting daemon 不可用</h1>
        <pre style="white-space:pre-wrap;font-size:0.875rem;line-height:1.5;color:#d4d4d4;">${escapeHtml(message)}</pre>
        <p style="margin-top:1.5rem;font-size:0.8125rem;color:#a3a3a3;">关闭此窗口后重试,或在 URL 加 <code style="background:#333;padding:0 0.25rem;">?transport=tauri</code> 走 Full 模式逃生。</p>
      </div>
    </div>`;
}

function escapeHtml(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;");
}

void bootstrap();
