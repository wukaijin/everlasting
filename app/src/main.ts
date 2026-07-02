import { createApp } from "vue";
import { createPinia } from "pinia";
import App from "./App.vue";
import "./style.css";
import { useErrorBus } from "./utils/useErrorBus";

const app = createApp(App);
app.use(createPinia());

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

app.mount("#app");
