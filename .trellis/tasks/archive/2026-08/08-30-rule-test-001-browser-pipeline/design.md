# Design — 浏览器交互回归流水线

> 对应 prd.md R1–R5。三项核心定案:runner、安装位置、环境策略。

## 1. Runner 选型:Playwright(R1)

| 候选 | 结论 | 理由 |
|---|---|---|
| **Playwright(`@playwright/test`)** | ✅ 定案 | ① 仓库先例:ui-review 已验证 playwright-core + ms-playwright Chromium 在本机 WSL2 可跑;② test-runner 内建 auto-wait / web-first expect / webServer 编排 / retry / trace,恰好命中「合成事件不触发浏览器默认行为」痛点——Playwright 走 CDP trusted input,CodeMirror 能吃到真实 keydown;③ CI 侧 `playwright install --with-deps chromium` 在 ubuntu-latest 一键备齐;④ 社区主流,后续铺用例的资料面最广 |
| webdriverio | ✗ | 无仓库先例;auto-wait / webServer / trace 需手工组装的体验明显更糙 |
| Cypress | ✗ | 自有 runner 架构(非 W3C 协议),对 EventSource 控制、多 tab、出站 fetch 拦截的表达力弱于 route 模型;引入第二套浏览器引擎无先例 |

**边界**:ui-review.sh 的 scratch playwright-core 模式**保持不动**(视觉截图用途,独立版本演进);本任务给 `app/` 加 `@playwright/test` devDependency,两者共用 `~/.cache/ms-playwright` 下按 build 版本隔离的浏览器目录,互不干扰。

## 2. 环境策略:route-mock + 受控流(R2)

### 2.1 总体形态

```
Playwright (chromium, trusted input)
  │
  ├── vite dev server (webServer 编排;dev 不注册 SW,无缓存干扰)
  │
  ├── fetch 层:page.route("**/api/v1/**") 全拦截 → fixture 注册表应答
  │     (真 fetch、真网络栈、确定性 JSON;未注册路由 → 500 fail-loud)
  │
  └── SSE 层:addInitScript 注入受控 fake EventSource(window.EventSource 替身)
        测试内 window.__stream.emit(eventName, payload) 程序化推事件
```

### 2.2 为什么 route-mock 而不是真 daemon

- **被测面诚实**:本层守护的是「UI 交互 + store 联动 + 渲染行为」,不是 wire 契约——后者 Rust `--test e2e` 已有基线(RULE-TEST-003),SSE 重连语义有 spec + 专项测试。两层职责正交,不重复。
- **确定性 = D2 blocking 前提**:无 Rust 编译依赖、无端口占用、无 LLM、无 DB 种子;frontend job 内自洽。
- **先例一致**:jsdom 层已有 canonical transport mock(spec `test-environment.md`),浏览器层是同一思路的 browser 版,心智连续。

### 2.3 fixture 契约(canonical harness)

`app/e2e/fixtures.ts` 扩展 playwright `test`,统一词表:

| helper | 语义 |
|---|---|
| `mockCmd(domain, cmd, payload)` | 注册 `POST /api/v1/{domain}/{cmd}` 应答。**无 envelope**:成功响应是 daemon `Json<T>` 原样透传(空 body → null,http.ts:403-407),错误才包 `TransportError(status, {kind?, message?, request_id?})`。**请求体顶层键经 camelCase→snake_case 转换**(`transformArgsTopLevel`,http.ts:270-289)——注册表匹配与请求体断言一律按 snake_case |
| `mockHealth()` | `GET /api/v1/health` 返回 `{daemonId, daemonVersion, apiVersions:["v1"]}`(health.ts 硬门,缺失会全屏错误 overlay 不 mount) |
| `stream.emit(name, payload)` | fake EventSource 推事件;事件名与 payload 形状照抄 `http.ts` listen 分发表(如 `tool:question`,streamEvents.ts 消费侧同名) |
| `boot(path?)` | install mocks + initScripts → `page.goto` → 等 app mount |

实现形态:**单条 catch-all `page.route('**/api/v1/**')` dispatcher**——handler 内查注册表,mix 则 **500 fail-loud**。fail-loud 是**必须**而非建议:vite dev 自带 `/api → localhost:7456` proxy(vite.config.ts server.proxy),漏拦截的请求会经 proxy 静默漏到真 daemon,掩盖缺陷。

boot 顺序约束:route 与 addInitScript 必须**先于 goto** 注册(health 握手发生在 main.ts mount 前);localStorage 保持空(无 token → browser-local 直连,无 proxy 前缀,免配对门)。

**fake EventSource 最小实现**:`addEventListener(name, handler)` + `close()` + 测试侧 emit 时构造真实 `MessageEvent` dispatch。原生重连语义**不在**被测面(由 Rust e2e / stream-resync spec 覆盖)。

**备选已否决**:真分块 SSE(route.fulfill 持 ReadableStream controller)——mid-test 写入时序 flaky、controller 贯穿 fixture 复杂度高,收益只有「过了原生 SSE parser」,不值。

### 2.4 选择 vite dev 而非 dist+preview

- dev server 不注册 SW(vite.config.ts 明示),避开 autoUpdate SW 抢缓存;preview 跑生产 SW 需要额外禁用手段。
- 交互回归对 bundle 形态不敏感;vue-tsc 类型门已由 `pnpm build` 步骤守住,无需重复。
- webServer 编排:**`{ command: "pnpm dev --port 1422", port: 1422, reuseExistingServer: !CI }`**。注意 vite.config.ts:121-123 硬编码 `port: 1420, strictPort: true`,playwright 的 `port` 字段只决定**等哪个端口**、不会改 vite 绑定——必须用 vite CLI `--port 1422` 覆盖(CLI 优先于 config);strictPort 语义保留,1422 被占时 fail-loud 而非漂移端口,正是测试想要的确定性。1422 同时避开 1420(用户日常 dev)与 7456(daemon)。

## 3. 试点用例映射(D3)

| spec 文件 | 盲区类别 | 关键断言 | 主要 mock |
|---|---|---|---|
| `e2e/chat-input-keys.spec.ts` | 键盘(CH5-1) | Shift+Enter 插入换行且不发送;Enter 发送(观察到 create_session/send POST) | mockCmd chat 域;stream request/done 事件 |
| `e2e/question-card-scroll.spec.ts` | 滚动+store 联动(CH8-2) | 上滚状态下 stream 推 `tool:question` 事件 → 强制回底;pending 期间发送 → 排队并存提示 | list_messages 种子;stream.emit `tool:question` |
| `e2e/permission-revoke-confirm.spec.ts` | 指针+弹窗(CH7-4) | 撤销 → ConfirmDialog 出现;取消 → 无 revoke 请求;确认 → revoke 请求 + 行消失 | permissions 列表 + revoke cmd |

选择器策略:**优先既有稳定 class hook**(ui-review SELECTORS 先例),无稳定钩子才加 `data-testid`,加必挂 spec 文档登记——避免大面积测试属性入侵组件。

## 4. CI 接线(R5)

frontend job 追加(位于 `pnpm build` 之后):

```
- actions/cache: ~/.cache/ms-playwright (key 含 playwright 版本)
- pnpm exec playwright install --with-deps chromium   (缓存 miss 时 ~1min)
- pnpm test:e2e                                        (3 spec,重试 1 次)
```

预算:热缓存增量 ≈ 1.5–2min;`playwright.config.ts` `retries: process.env.CI ? 1 : 0`,workers 保持默认(D2:不稳定的用例宁可 local-only,不让 retry 掩盖)。

## 5. 分层判据(R4,进 spec 文档)

| 层 | 守护什么 | 例子 |
|---|---|---|
| vitest(jsdom) | 默认层:组件 props/事件、store 状态机、纯函数、transport 单元 | 现有 ~1287 例 |
| **browser spec(playwright)** | jsdom 证伪过的类别:真实键盘/指针 trusted event、跨组件滚动/焦点联动、outside-click/浮层层叠、拖拽、剪贴板 | 本任务 3 条试点 |
| Rust `--lib` | 后端逻辑 | ~2014 例 |
| Rust `--test e2e` | wire 契约(路由/序列化/SSE 语义) | RULE-TEST-003 基线 |
| turn-smoke.sh | 真 LLM 单轮 live 链路 | agent loop 改动后 |
| ui-review.sh | 视觉观感(VLM 评审),非交互 | 样式改动后 |

## 6. 兼容与回滚

- **零生产代码侵入**:改动面 = `app/package.json`(devDep + `test:e2e` script)、`app/playwright.config.ts`、`app/e2e/`、CI yml 追加步骤、文档。组件侧最多加 `data-testid`(按 §3 策略,逐条登记)。
- **vitest 隔离**:vitest include 仅 `src/**`,`app/e2e/*.spec.ts` 不会被捡;`pnpm test` 语义不变。
- **回滚**:revert 单个 commit 即全部移除;无迁移、无 wire 变更、无 daemon 改动。
- **风险**:WSL 本地 chromium 系统依赖——ui-review 先例已证可跑;若新 build 缺库,`playwright install-deps chromium` 补齐(HACKING 文档记录)。fixture 侧两个易踩坑已固化进 §2.3/§2.4:请求体顶层 snake_case(无 envelope)+ vite dev `/api` proxy 漏/mock 泄漏面(catch-all fail-loud 必须存在)。
