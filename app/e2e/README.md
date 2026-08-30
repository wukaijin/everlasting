# e2e — 浏览器交互回归(Playwright)

> RULE-TEST-001 的落地层:守护 jsdom 测不到的交互类回归 —— 真实键盘 /
> 指针 trusted input、跨组件滚动/焦点联动、outside-click/浮层层叠。
> 与 vitest(jsdom,`src/**/*.test.ts`)分层不重叠;分层判据总表见
> design.md §5(PR3 落位 `.trellis/spec/frontend/browser-regression.md`)。

## 运行

```bash
cd app && pnpm test:e2e        # playwright test(vite dev 1422 自动编排)
pnpm exec playwright install chromium   # 首次/换版本后装浏览器
```

- **端口**:vite dev 跑 `1422`(CLI `--port` 覆盖 vite.config.ts 硬编码的
  1420/strictPort;playwright.config.ts 的 `webServer.port` 只管等哪个端口)。
  本地 `reuseExistingServer` 开着 —— 手动 `pnpm dev --port 1422` 后跑测试可
  复用同实例调试。1422 被占用时的行为分两种:占用方应 HTTP → 本地会**误复用**
  (boot 对着错服务器超时,先查端口占用);占用方不应 HTTP 或 CI(reuse 关闭)
  → Playwright fork 的 vite strictPort fail-loud,属预期。
- **WSL 系统依赖**:本机已具备跑 headless Chromium 的系统库(ui-review 的
  scratch playwright-core 先例),无需额外 `playwright install-deps`。新
  Chromium build 若报缺库再补:`pnpm exec playwright install-deps chromium`。
- 与 ui-review 互不污染:那边是 scratch 目录里的独立 `playwright-core`;
  这边是 `app/` 的 devDependency `@playwright/test`。两者共用
  `~/.cache/ms-playwright` 下按 build 版本隔离的浏览器目录。

## harness 用法(fixtures.ts,design §2.3 契约)

```ts
import { expect } from "@playwright/test";
import { EDITOR, test } from "./fixtures";

test("…", async ({ page, boot, mockCmd, stream, reqs, waitForCmd }) => {
  mockCmd("agent", "chat", { status: "started" }); // 覆盖默认表(可选)
  await boot();            // goto(/)+ 等 app mount(route/initScript 已先装好)
  await page.click(EDITOR);
  await page.keyboard.type("hi");
  await page.keyboard.press("Enter");
  const chat = await waitForCmd("agent", "chat");   // 网络观察断言
  expect(chat.body).toMatchObject({ messages: [{ role: "user", content: "hi" }] });
  stream.emit("chat-event", { request_id: "…", kind: "delta", text: "…", session_id: "…" });
});
```

- `boot(path = "/")`:`/` 经 router redirect 到 `/chat`。localStorage 保持空
  → browser-local 直连模式(无 proxy 前缀、无配对门)。
- `mockCmd(domain, cmd, payload)`:payload 是 daemon `Json<T>` **原样**
  (无 envelope;`null` = JSON null);请求体顶层键已被
  `transformArgsTopLevel` 转成 **snake_case**(断言按 snake_case)。
- `mockHealth(health)`:覆盖 `GET /api/v1/health`(mount 硬门)。默认形状
  `daemonId` + `apiVersions:["v1"]`,**不得**带 `remoteId`(会被 router
  判成 remote 上下文踢去 `/pairing`)。
- `reqs`:dispatcher 观察到的**全部** `/api/v1/**` 请求(含 miss),按序
  记录;`waitForCmd` 是其轮询封装。
- `stream.emit(name, payload)`:fake EventSource 推事件。事件名照抄
  http.ts listen 分发表:`chat-event` / `tool:call` / `tool:result` /
  `tool:question` / `mode:change:request` / `task:state:transition:request` /
  `subagent:event` / `permission:ask` / `projects:refreshed` /
  `stream-resync`。payload 形状读对应 store 的泛型(streamEvents.ts /
  streamController.ts)。**刻意不含重连逻辑**(被排除的被测面)。
  妙用:带 `session_id` 的 `chat-event start` 会被跨客户端认领
  (adoptForeignRequest)—— 不走前端发送路径即可把会话置成流式态。

### 默认 mock 注册表(boot 前已装;mockCmd 随时覆盖)

| key(`domain/cmd`) | 应答 | 消费方 |
|---|---|---|
| `providers/list_providers` | `[]` | config.load |
| `providers/list_models` | `[]` | config.load |
| `providers/get_default_model` | `null` | config.load |
| `config/get_home_dir` | `"/home/e2e"` | config.load |
| `config/get_app_config` | `{turnCompleteNotifyEnabled:true, scheduledTasksEnabled:true}` | config.load |
| `projects/list_projects` | 1 个项目(`e2e-project`) | ChatWindow 选中 → ChatPanel 渲染 |
| `projects/list_hidden_projects` | `[]` | HiddenProjectsMenu onMounted |
| `scheduled_tasks/list_scheduled_tasks` | `[]` | AppShell onMounted |
| `sessions/list_sessions` | `[]` | chat watcher(空 = 保持无 session → 发送走懒建) |
| `sessions/create_session` | session row(`id:"e2e-session-1"`) | createNewSession |
| `sessions/load_session` | `null` | ensureLoaded(fresh session 无历史) |
| `message_queue/list_queued_messages` | `[]` | ChatPanel watch currentSessionId |
| `question/get_pending_interaction` | `null` | ensureLoaded 权威 pending 拉平 |
| `agent/chat` | `{status:"started"}` | startRequest(F1 ChatAcceptance) |
| `permissions/list_turn_traces` | `[]` | startRequest → traceStore.loadHistory |
| `permissions/list_session_audit_events` | `[]` | 同上 |
| `usage/usage_window` | 空 report(camelCase) | ChatInputTokenUsage mount |

未注册的请求 → **500 fail-loud**(防漏 mock 经 vite `/api` proxy 静默漏到
真 daemon :7456)。新增用例若要打新 cmd,必须先 `mockCmd` 注册。

## 选择器约定 + data-testid 登记表

选择器优先**既有稳定 class hook**(ui-review.sh SELECTORS 先例);只在没有
稳定钩子时才加 `data-testid`,且必须在本表登记(用途 + 引入任务)。
生产代码 diff 约束:除登记过的 `data-testid` 外 `app/src` 零改动。

当前登记(全部为**既有**生产 testid,本流水线未新增):

| data-testid | 组件 | 用途 | 引入 |
|---|---|---|---|
| `grant-revoke-confirm` | ConfirmDialog(PermissionGrantsModal 内) | 撤销确认弹窗定位;取消/确认按钮仍走 `.confirm-modal__btn--cancel/--danger` | CH7-4(2026-08-29,先于本流水线) |

既有 class hook 常量(fixtures.ts 顶部):`EDITOR` / `EDITOR_LINE` /
`SEND_BUTTON`;spec 内稳定 hook:`ul.messages`(滚动容器)、
`.scroll-to-bottom`、`.ask-card`、`.chat-input__row--streaming`、
`.f1-queued-chip`、`.toast.toast--warn`、`.chat-panel__grants-btn`、
`.grant-modal`、`.grant-item(--revoke/--tool)`、`.confirm-backdrop`。

## 已知陷阱(踩过的)

- **boot 顺序**:route 与 addInitScript 必须先于 goto(health 握手发生在
  main.ts mount 前)。world fixture 的 setup 已保证,不要在 boot 后再装 route。
- **CodeMirror 行断言**:真实行 = `.cm-line` 数量(软换行不拆行);编辑器
  清空后 `.cm-line` 文本非空 —— 占位文本是 `.cm-placeholder` widget,判空看
  占位符可见性。
- **vite dev 跨域**:dev 模式 `daemonBase()` = `http://localhost:7456`,页面
  源 1422 → fulfill 的响应带 `Access-Control-Allow-Origin: *`;Chromium 在
  Playwright 拦截下不发 OPTIONS preflight,dispatcher 仍兜了 OPTIONS 分支。
- **验收三件套**:`pnpm test:e2e && pnpm test && pnpm build` 全绿(vitest
  include 仅 `src/**`,e2e 目录天然隔离;vue-tsc 不查 e2e —— Playwright
  自带转译)。
