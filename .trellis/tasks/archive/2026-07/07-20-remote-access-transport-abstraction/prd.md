# 远程访问 Phase 1:前端 Transport 抽象 + 后端 emit 散点收敛

> Parent: [07-20-remote-access-multi-channel](../07-20-remote-access-multi-channel/)
> 实施路线: [docs/REMOTE-ACCESS-ROADMAP.md Phase 1](../../../docs/REMOTE-ACCESS-ROADMAP.md#phase-1transport-抽象--emit-散点收敛2-3-天)

## Goal

把 Tauri IPC(`invoke` / `listen`)收敛到 `Transport` 接口背后,**Tauri 版行为零变化**;同时把后端 6 处直调 `app.emit` 的散点收到 sink 抽象,为 Phase 2(daemon 拆分 + 换 HTTP transport)扫清障碍。

## Background

- 现状:21 个非测试文件 + 22 个测试文件直接 import `@tauri-apps/api/(core|event)`;后端 10 个 emit 事件中 7 个经 `AppHandleSink`,**6 处直调 `app.emit` 绕过 sink**。
- 详见 [RESEARCH §1.2b emit 散点](../../../docs/REMOTE-ACCESS-RESEARCH.md#12b-emit-事件全集10-个与抽象现状) 和 [§1.5 前端耦合点](../../../docs/REMOTE-ACCESS-RESEARCH.md#15-前端-transport-耦合点)。

## Requirements

### 前端

- **R1** 新增 `app/src/transport/` 模块:`Transport` interface + `tauriTransport` 实现 + `httpTransport` stub(Phase 2 填充)。
- **R2** 21 个非测试文件的 `import { invoke, listen } from '@tauri-apps/...'` 全部改为 `import { transport } from '@/transport'`,调用点改为 `transport.invoke(...)` / `transport.listen(...)`。**机械替换,无逻辑变化**。
- **R3** 22 个 `*.test.ts` 文件的 mock 同步改造:`vi.mock('@tauri-apps/api/core')` → `vi.mock('@/transport', ...)`。
- **R4** `listen` 接口语义设计:httpTransport 内部维护单个 EventSource + 事件分发表,对外保持 `listen(event, handler)` 签名不变(方案 B,见 RESEARCH §4.2)。**Tauri 端 streamController 的 requestId 分发逻辑零改动**。
- **R5** TitleBar.vue 的 `getCurrentWindow` + `plugin-os` **不改**(窗口 API 不在 transport 抽象范围,浏览器降级由 Phase 2 处理)。

### 后端

- **R6** 收敛 6 处 emit 散点(见 design.md / implement.md 的处理方案表):
  - `agent/chat.rs:187` pre-flight error 直调 → 走 sink
  - `agent/helpers.rs:160,170` → **删除 helper 函数**,调用方用 sink
  - `agent/subagent/sink.rs:279,698` + `dispatch.rs:1192` → 抽象为 `SubagentEventSink` trait(**保留 collector 双通道语义**)
  - `state.rs:317` `projects:refreshed` → 保留(Phase 2 daemon 化时处理)
- **R7** subagent 路径**不能简单合并到 `AppHandleSink`**——subagent 事件注入走 collector 路径,与父 agent loop 的 sink 是两套语义。只抽象类型,不改语义。

## Acceptance Criteria

### 自动化验证

- [ ] `cd app && pnpm vue-tsc --noEmit` 通过
- [ ] `cd app && pnpm vitest run` 全绿(22 个测试文件 mock 改造后)
- [ ] `cd app/src-tauri && cargo test` 全绿(后端散点收敛后)
- [ ] `cd app/src-tauri && cargo clippy` 无新增 warning

### grep 验证(确认收敛完成)

- [ ] 前端:`grep -rn "from '@tauri-apps/api/core'" app/src --include="*.ts" --include="*.vue" | grep -v '\.test\.' | grep -v 'transport/'` 结果为空
- [ ] 后端:`grep -rn '\.emit(' app/src-tauri/src/agent/ | grep -v 'test'` 所有 emit 都经 sink trait 或 `SubagentEventSink` trait(无裸 `app.emit`)

### 行为零变化(手动 smoke test,`pnpm tauri dev`)

- [ ] 发送一条消息,看到流式 delta(chat-event 经 sink)
- [ ] 触发 permission:ask,点允许,agent 继续(oneshot resolve 正常)
- [ ] 触发 ask_user_question,回答后 agent 继续
- [ ] 切换 session,LRU 缓存正常
- [ ] subagent drawer 看到完整事件流(subagent:event / subagent:finished —— **验证 P1.3 subagent 散点收敛**)
- [ ] subagent 内触发 permission:ask,弹窗正常(验证 `subagent/sink.rs:698` 散点)
- [ ] 首次启动,项目列表 backfill 后刷新(projects:refreshed —— 保留不动,验证未破坏)
- [ ] agent error 时前端收到错误事件(验证 `chat.rs:187` 散点收敛)

## Dependencies

- **无前置依赖**(Phase 1 是起点)。
- **后置**:[07-20-remote-access-daemon-split](../07-20-remote-access-daemon-split/) 依赖本 task 完成(emit 散点收敛 + transport interface 是 Phase 2 的前置条件)。

## Risks

- **R-1 测试 mock 改造量**:22 个测试文件,工作量大但机械。缓解:批量 sed + 逐个验证。
- **R-2 subagent 散点收敛误伤**:subagent collector 双通道语义微妙,抽象 trait 时易破坏。缓解:先写 subagent 事件流的集成测试(用 mock provider 跑一轮 subagent),作为回归基线,再动 trait 抽象。

## Notes

- 本 task **不实现 httpTransport 的真实逻辑**(只 stub),httpTransport 实现是 Phase 2 的事。
- 本 task **不处理 `pick_project_dir` 的浏览器降级 UX**(那是 Phase 2 的 GUI 改造范围)。
