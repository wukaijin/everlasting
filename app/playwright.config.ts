// Playwright 浏览器交互回归(vitest 盲区:真实键盘/指针/滚动联动)。
// 分层判据与新增用例约定见 `.trellis/spec/frontend/browser-regression.md`
// (PR3 落位)与 `e2e/README.md`。
//
// # 为什么跑 vite dev server(而不是 dist + preview)
// dev 不注册 Service Worker(vite.config.ts devOptions.enabled=false),
// 避开 autoUpdate SW 抢缓存;交互回归对 bundle 形态不敏感,vue-tsc
// 类型门由 `pnpm build` 步骤单独守住。
//
// # 端口 1422(约束,勿改单边)
// vite.config.ts 硬编码 `port: 1420, strictPort: true` —— 本文件的
// `webServer.port` 只决定**等哪个端口**,不改 vite 绑定;必须靠 CLI
// `--port 1422` 覆盖(CLI 优先于 config)。strictPort 语义保留:1422
// 被占时 fail-loud 而非漂移端口。1422 同时避开 1420(日常 dev)与
// 7456(daemon)。`e2e/fixtures.ts` 的 mock base 同样依赖此端口。
import { defineConfig, devices } from "@playwright/test";

export default defineConfig({
  testDir: "./e2e",
  // vitest include 仅 `src/**`,e2e/*.spec.ts 天然隔离(两边零冲突)。
  fullyParallel: true,
  // D2(CI blocking 硬门禁):本地 0 retry 不掩盖不稳定;CI retry 1 次
  // 兜底。时序不确定的用例宁可标 local-only,不让 retry 掩盖。
  retries: process.env.CI ? 1 : 0,
  forbidOnly: !!process.env.CI,
  use: {
    baseURL: "http://localhost:1422",
    ...devices["Desktop Chrome"],
  },
  webServer: {
    command: "pnpm dev --port 1422",
    port: 1422,
    // 本地复用已开的 1422 便于调试;CI 必须 fork 自己的实例。
    reuseExistingServer: !process.env.CI,
  },
});
