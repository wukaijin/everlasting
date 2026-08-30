# 浏览器交互回归流水线(RULE-TEST-001):runner 选型 + 可复用流程

## Goal

建立一套**系统性、可复用**的浏览器交互回归流程:真实 Chromium 里驱动真实前端,守护 jsdom 测不到的交互类回归(真实键盘、指针交互、滚动联动、弹窗层叠),并让「后续 UI 任务如何添加/运行/守护交互回归」成为有 spec、有 CI 位置、有准入判据的标准流程。闭合 DEBT.md `RULE-TEST-001`(P3)。

## Background / 动机(实证)

- BUGLIST 两项因自动化测不出被迫人工真键盘复核:CH5-1(Shift+Enter 换行,CodeMirror contenteditable 不受合成 keydown 影响)、CH14-1(焦点环 Tab 走查)。
- `MarkdownDetailModal.test.ts:383-390` 的 pointerdown-outside 在 jsdom 无法模拟,仅占位守护(RULE-TEST-001 登记原因)。
- 最近一批 P2 修复恰是此类交互:CH8-2 提问卡强制回底、CH7-4 放行撤销确认、CH3-2 下拉收起。
- `scripts/ui-review.sh` 的方法局限已在 AGENTS.md 记录:静态截图看不见 hit area / hover / 动效,是**视觉评审**不是**交互回归**。

## Confirmed Facts(仓库证据)

1. **Playwright 已有先例**:`scripts/ui-review.sh` 用 `playwright-core`(scratch 目录)+ `~/.cache/ms-playwright` Chromium,WSL2 已跑通(仅作视觉截图,无断言体系)。
2. **browser-local 模式 mock 面干净**(模式判定语义见 spec `transport-and-pwa-modes.md`;SSE 单全局 EventSource lazy 创建细节源出 `http.ts:20-29/:289-316` 代码注释,spec 未载):localStorage 无 token → 无 proxy 前缀直连 `POST /api/v1/{domain}/{cmd}`;`GET /api/v1/health` 返回 `daemonId`(camelCase,`apiVersions` 含 `"v1"`)→ 免配对门直达 `/chat`。
3. **transport wire 事实**(http.ts 实证):无 envelope,成功 `Json<T>` 原样透传(空 body → null),错误 `TransportError`;请求体顶层键 camelCase→snake_case(`transformArgsTopLevel`,http.ts:270-289)。
4. **vitest include = `src/**/*.test.ts`**(vitest.config.ts):playwright 用例放 `app/e2e/*.spec.ts` 天然隔离,零冲突。
5. **vite dev server**:不注册 Service Worker(devOptions disabled);`server.port: 1420, strictPort: true` 硬编码(vite.config.ts:121-123,playwright 侧等端口不改绑定,须 CLI `--port` 覆盖);自带 `/api → localhost:7456` proxy(route 拦截在浏览器侧绕过它,但漏 mock 会经 proxy 漏到真 daemon)。
6. **CI frontend job 现状**:pnpm install → vitest → `pnpm build`(vue-tsc gate);无浏览器环节,加 job 步骤即可。
7. 测试分层现状:Rust `--lib` ~2014 + `--test e2e`(wire 契约)、vitest jsdom ~1287、`turn-smoke.sh`(真 LLM 单轮)、`remote-e2e-smoke.mjs`(remote 链路)——**浏览器交互层为空**。

## Decisions(brainstorm 定案)

- **D1 交付形态 = 单任务全链交付**(2026-08-30):R1–R6 一个任务完成;试点仅 3 条用例不铺量;实施中若发现需要大规模铺用例,再拆 child。
- **D2 CI gate = blocking 硬门禁**(2026-08-30):进 frontend job 作 merge 门禁。配套约束:CI 只收确定性用例(route-mock 驱动,无 daemon/无 LLM/无网络依赖)+ Playwright retry 兜底;时序不确定的用例标 local-only 不进 CI。「进 CI」是每条用例的准入标准而非默认。
- **D3 试点用例 = 三类盲区各一条**(2026-08-30):① 键盘类 Shift+Enter 换行 vs Enter 发送(CH5-1 原型);② 滚动+store 联动类 提问卡强制回底 + 排队并存提示(CH8-2,需 mock SSE 流,顺带验证流式驱动基建);③ 指针+弹窗类 放行撤销确认弹窗(CH7-4,pointerdown-outside 语义)。

## Requirements

- **R1 runner 定案**:Playwright(`@playwright/test`)进 `app/` devDependency(CI 需 lockfile 可装;与 ui-review 的 scratch playwright-core 并存互不污染);Chromium 单引擎。决策论证落 design.md。
- **R2 环境策略 = route-mock + 受控流**:测试跑 vite dev server(无 SW),`page.route` mock 全部 `/api/v1/**` fetch,SSE 用注入的受控 fake EventSource(测试内 `emit` 驱动);不依赖 Rust daemon。契约见 design.md。
- **R3 试点用例**:按 D3 三条 spec 落地,全绿且在 CI 确定性准入内。
- **R4 流程固化**:`.trellis/spec/frontend/browser-regression.md`(英文,遵循 spec index 语言约定)——分层判据(什么进浏览器回归 vs vitest vs Rust e2e vs turn-smoke vs ui-review)+ 新增用例步骤约定 + fixture 用法 + 已知陷阱;`docs/HACKING-wsl.md` 补本地运行条目。
- **R5 CI 接线**:frontend job 增 playwright 安装(缓存 `~/.cache/ms-playwright`)+ `pnpm test:e2e` 步骤,blocking。
- **R6 收尾**:DEBT.md 删 RULE-TEST-001;BUGLIST §4(CH5-1/CH14-1 类「自动化测不出」结论补注浏览器层已覆盖)与 §6 未覆盖项核对;ROADMAP 第三档相应微调。

## Acceptance Criteria

- [ ] AC1 `design.md` 存在且含三项定案论证:runner、安装位置、环境策略(mock 边界:什么被真实验证、什么被 mock)。
- [ ] AC2 `cd app && pnpm test:e2e` 在干净环境按 HACKING 文档步骤一键跑通,3 条试点 spec 全绿(含 retry 配置)。
- [ ] AC3 三条试点 spec 分别覆盖键盘 / 滚动联动 / 弹窗指针三类 jsdom 盲区,断言在真实 Chromium 中通过。
- [ ] AC4 spec 文档落位 `.trellis/spec/frontend/browser-regression.md` 且含分层判据表 + 新增用例 checklist;HACKING-wsl 有本地运行说明。
- [ ] AC5 CI frontend job 含 playwright 步骤且 blocking;本地模拟 CI 路径(`pnpm install --frozen-lockfile` 后跑 test:e2e)通过。
- [ ] AC6 DEBT.md RULE-TEST-001 删除,BUGLIST/ROADMAP 相应条目同步;`pnpm test`(vitest)与 `pnpm build` 零回归。

## Out of Scope

- 全量铺开试点之外的用例(后续 UI 任务按新流程顺带/另立)。
- 视觉回归(截图 diff / VLM)——ui-review 流水线已有,不重叠。
- 真 LLM / 真 daemon 参与的浏览器 E2E(turn-smoke.sh / remote-e2e-smoke.mjs 已覆盖)。
- Firefox/WebKit 多引擎矩阵——先 Chromium 单引擎。
- ui-review.sh 改造去复用 @playwright/test(独立 scratch 模式保持不动)。
