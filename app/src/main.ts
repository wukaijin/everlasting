import { createApp } from "vue";
import { createPinia } from "pinia";
import App from "./App.vue";
import "./style.css";
import { router } from "./router";
import { useErrorBus, isBenignBrowserNoise } from "./utils/useErrorBus";
import { transport } from "./transport";
import { tauriTransport } from "./transport/tauri";
import { awaitDaemonHealthy, type DaemonHealth } from "./transport/health";
import { useTheme } from "./composables/useTheme";

const app = createApp(App);
app.use(createPinia());
// 主题在 mount 前落到 <html>(读 localStorage,默认 aggressive):
// mount 后首帧就是目标主题,无闪面。幂等,Sidebar 里再调用无害。
useTheme();
// NOTE(router 时序):app.use(router) **不**放这——它会在 module load 时触发
// initial navigation(async microtask),在 bootstrap 的 `await awaitDaemonHealthy()`
// 期间 resolve,此时 __DAEMON_HEALTH__ 还没设 → isRemoteContext() 误判 false →
// 手机进 /chat 不跳 /pairing(S4 bug,E2E 暴露)。router 改到 bootstrap 内、health
// 设后 mount 前注册,确保 initial navigation 读到 __DAEMON_HEALTH__。

// A5(2026-07-02)全局未捕错误器;2026-09-21 分级收口(任务
// 09-21-error-bus-category-recovery)。按错误来源分级,不再是
// 「一律入总线」:
//   - 结构化错误(AppCommandError 形状对象,含 TransportError)→ 入
//     错误总线,按真实 category 路由(Auth/RateLimit/Server/Network
//     toast,InvalidRequest console.warn);
//   - 良性浏览器噪音(isBenignBrowserNoise:ResizeObserver loop 等已知
//     前缀)→ 入口直接 console.debug 丢弃,不进总线不 toast(它们只有
//     message 没有 error 对象,曾因此被误标「服务端错误」弹窗);
//   - 裸 string / 其他 Error 实例(本地运行时错误)→ console
//     三级留痕(warn / error),不入总线不 toast —— 不再借「服务端
//     错误」之名误报,也不再静默丢失(详见 useErrorBus.ts handle 注释)。
// 现有 fire-and-forget .catch(record_tool_duration / update_message_latency /
// permissions 超时 deny)故意 swallow,不触发本监听(它们已 .catch)。
if (typeof window !== "undefined") {
  const { handle } = useErrorBus();
  window.addEventListener("error", (event) => {
    // 良性噪音(ResizeObserver loop)以 window.onerror 形态抛出:只有
    // event.message、event.error 为空 —— 入口先过滤(handle 的 string
    // 分支是同函数的第二道,防其他路径混入)。
    if (!event.error && isBenignBrowserNoise(event.message)) {
      console.debug("[errorBus:benign-noise]", event.message);
      return;
    }
    // event.error 是 Error 对象(或 undefined);fallback 到 event.message(string)。
    handle(event.error ?? event.message);
  });
  window.addEventListener("unhandledrejection", (event) => {
    // event.reason 是 rejection 原因(AppCommandError 对象 / Error / string)。
    handle(event.reason);
  });
}

// R1.2(09-21 收口):接管 Vue 默认错误 handler。组件生命周期/异步链里
// 的错误经默认 handler 只 console.warn 且部分形态到不了 window.onerror
// —— 显式 console.error 兜底防静默。不 toast:运行时错误几乎不可由用户
// 行动修复,与 errorBus 的 Error 分支同一判据(可见但不打扰)。
app.config.errorHandler = (err, _instance, info) => {
  console.error("[vue:errorHandler]", info, err);
};

// P2.4 D3.4: 在 `app.mount` 前等 daemon 健康(Q5 分层校验)。
// httpTransport 是默认(P2.4 D3.1),若 daemon 未就绪 GUI 完全无功能,
// 故 fail-loud:超时/协议不匹配 → 渲染全屏错误覆盖层,不静默降级。
// `?transport=tauri` 逃生模式下无 daemon,跳过握手(Rust 侧 Full 模式直连 IPC)。
//
// 暴露 handshake 结果到 window 供 App.vue 启动诊断 + 测试断言用。
async function bootstrap(): Promise<void> {
  if (transport === tauriTransport) {
    // 逃生模式:Rust Full GUI 模式,无 sidecar,直接挂载。
    app.use(router);
    app.mount("#app");
    return;
  }

  try {
    const health = await awaitDaemonHealthy();
    (window as unknown as { __DAEMON_HEALTH__?: DaemonHealth }).__DAEMON_HEALTH__ =
      health;
    // router 在 health 设后注册——initial navigation(mount 触发)此时能读到
    // __DAEMON_HEALTH__,isRemoteContext() 正确判 remote → 跳 /pairing。
    app.use(router);
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
