// N9 F1 前端渲染基准(任务 09-19-n9-perf-benchmark,design §5)。
//
// 与回归门禁(e2e/*.spec.ts / playwright.config.ts)完全隔离:独立目录 +
// 独立 config + npm script `bench:fe`;CI 的 test:e2e 与本目录零触碰。
// 依赖方向收口:只依赖 e2e/fixtures.ts 的 world(route-mock + fake
// EventSource),不自建 mock 面(防种子形状与真实 wire 漂移)。
//
// 统计口径:median + P75 + max(8 run;「P99」名不实,评审结论 #11)。
// 报告落 out/bench-fe/<ts>/report.json(out/ 不进 git)。

import { defineConfig, devices } from "@playwright/test";

export default defineConfig({
  testDir: "./bench",
  testMatch: "**/*.bench.ts",
  // 测量稳定性:串行 worker;数字抖动,绝不进 CI blocking 门禁(仅
  // CI 编译门做 `--list` 级收集检查)。
  workers: 1,
  fullyParallel: false,
  timeout: 180_000,
  retries: 0,
  use: {
    baseURL: "http://localhost:1422",
    ...devices["Desktop Chrome"],
  },
  webServer: {
    command: "pnpm dev --port 1422",
    port: 1422,
    reuseExistingServer: !process.env.CI,
  },
});
