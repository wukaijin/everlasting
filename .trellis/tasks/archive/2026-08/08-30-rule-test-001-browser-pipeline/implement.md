# Implement — 浏览器交互回归流水线

> 前置:prd.md(D1–D3 定案)+ design.md 已评审。按序执行;每步有独立验证点,PR1 后任意步可独立回滚。

## PR1 基建(fixtures + config + 首条 spec)

- [ ] 1. `app/` 加 devDependency `@playwright/test`(版本取当前 stable;`pnpm add -D @playwright/test`),`package.json` 加 `"test:e2e": "playwright test"`。**不动** `pnpm test`(vitest 语义保持)。
- [ ] 2. 本地装浏览器:`pnpm exec playwright install chromium`(WSL 若缺系统库补 `playwright install-deps chromium`,记录进 HACKING)。
- [ ] 3. 写 `app/playwright.config.ts`:`testDir: "./e2e"`;`webServer: { command: "pnpm dev --port 1422", port: 1422, reuseExistingServer: !process.env.CI }`(vite.config.ts 硬编码 1420/strictPort,CLI `--port` 才能覆盖;playwright `port` 字段只管等哪个端口);`retries: process.env.CI ? 1 : 0`;`use.baseURL` 指向 webServer;`fullyParallel: true` 前先确认 fixture 的 route/emit 无跨 test 串扰(每 test 独立 page 即安全)。
- [ ] 4. 写 `app/e2e/fixtures.ts`(design §2.3 契约):单条 catch-all `page.route('**/api/v1/**')` dispatcher(注册表查找,miss → 500 fail-loud,防漏 mock 经 vite `/api` proxy 漏到真 daemon)+ `mockHealth` / `mockCmd` / fake EventSource(`stream.emit`)/ `boot`。wire 事实(design §2.3 已考):**无 envelope**——成功 `Json<T>` 原样透传(空 body → null),错误 `TransportError(status, {kind?, message?, request_id?})`;**请求体顶层键 camelCase→snake_case**(`transformArgsTopLevel`,http.ts:270-289)——注册匹配与断言按 snake_case。`mockHealth` 形状读 `app/src/transport/health.ts` 的 `DaemonHealth`。
- [ ] 5. 写 `app/e2e/chat-input-keys.spec.ts`(试点①):Shift+Enter 换行不发送;Enter 发送(断言 create_session/chat POST 被观察到)。选择器优先既有 class;不足再加 `data-testid` 并登记到 spec 文档「testid 登记表」段。
- [ ] 6. 验证:`cd app && pnpm test:e2e` 全绿;`pnpm test`(vitest)确认未捡到 e2e 目录、数量不降;`pnpm build` 绿。

**回滚点 PR1**:全部为新增文件 + package.json 两处;revert 即净。

## PR2 试点②③ + CI 接线

- [ ] 7. `app/e2e/question-card-scroll.spec.ts`(试点②):mock list_messages 种子消息 → 上滚 → `stream.emit("tool:question", …)` → 断言强制回底;pending 期间发送 → 断言排队并存提示。事件名与 payload 形状读 `app/src/stores/streamEvents.ts`(`tool:question` 通道,:1078/:1126)/ `questionCards` store。
- [ ] 8. `app/e2e/permission-revoke-confirm.spec.ts`(试点③):放行列表 mock → 撤销 → ConfirmDialog 出现;取消 → 无 revoke 网络请求;确认 → revoke 请求 + 行移除。cmd 名与 envelope 读 permissions store / routes。
- [ ] 9. `.github/workflows/ci.yml` frontend job 追加:actions/cache(`~/.cache/ms-playwright`,key 含版本)→ `pnpm exec playwright install --with-deps chromium` → `pnpm test:e2e`(design §4 顺序,位于 `pnpm build` 后)。
- [ ] 10. 验证:`pnpm test:e2e` 3 spec 全绿;本地模拟 CI:`rm -rf node_modules && pnpm install --frozen-lockfile && pnpm test:e2e`(可选:确认 frozen-lockfile 与新 devDep 一致)。

**回滚点 PR2**:CI 步骤独立可撤;两条 spec 文件独立可删。

## PR3 流程固化 + 收尾(R4/R6)

- [ ] 11. 写 `.trellis/spec/frontend/browser-regression.md`(英文):分层判据表(design §5)+ 新增用例 checklist(fixture 用法、确定性准入、选择器策略、testid 登记表)+ 已知陷阱(boot 顺序、fake EventSource 边界、WSL 依赖)。
- [ ] 12. `.trellis/spec/frontend/index.md` Guidelines Index 表加一行。
- [ ] 13. `docs/HACKING-wsl.md` 补「浏览器交互回归」本地运行条目(install-deps 坑、与 ui-review 的 ms-playwright 共存说明)。
- [ ] 14. 收尾账务:DEBT.md 删 RULE-TEST-001 并更新优先级分布表(2→1);BUGLIST §4 CH5-1/CH14-1 结论补注「浏览器层已可自动守护(后续同类不再需人工复核)」、§6 核对未覆盖项;ROADMAP 第三档 RULE-TEST-001 相关表述微调(如有)。
- [ ] 15. 全量回归:`cd app && pnpm test && pnpm build`;后端零改动无需 cargo。

**回滚点 PR3**:纯文档;单独 revert 无影响。

## 质量门(Phase 2.2 final pass)

- `cd app && pnpm test` / `pnpm build` / `pnpm test:e2e` 三绿。
- `app/src` 生产代码 diff 应≈0(仅允许登记过的 `data-testid`);若超界,回 design §6 检查。
- CI 语义检查:frontend job 步骤顺序、cache key、blocking 生效(无 continue-on-error)。

## 风险与注意

- **无 envelope + snake_case**(评审 P2 实证):mockCmd 应答是 `Json<T>` 原样,不存在 `{ok, data}` 之类的壳;请求体经 `transformArgsTopLevel` 顶层键转 snake_case——试点① 断言 Enter 发送的 POST、试点③ 断言 revoke 请求体,匹配键一律 snake_case。
- **vite dev `/api` proxy**(评审 P1 连带发现):dev server 把 `/api` 代理到 `localhost:7456`(vite.config.ts)——catch-all fail-loud route 是必须项,漏 mock 的请求否则会静默打到真 daemon。
- CodeMirror 换行断言:读 `chatInputCodeMirror.ts` 确认 Shift+Enter 走浏览器默认行为(BUGLIST CH5-1 考证),断言看 editor DOM 内容而非 store;Enter 发送断言走网络观察,别依赖合成事件语义。
- 试点② 的滚动断言用 `scrollTop`/`scrollTo` 行为值,勿用截图;jsdom 里 scrollIntoView 是 mock 的,这正是本层存在的意义。
- fake EventSource 别实现重连逻辑——那是被刻意排除的被测面,实现了反而掩盖回归。
- PR3 写 spec 文档时用**准确事件名**(如 `tool:question`,streamEvents.ts),不写泛化「question 事件」(评审 P3)。
