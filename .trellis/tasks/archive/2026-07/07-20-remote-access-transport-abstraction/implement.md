# Implement:Transport 抽象 + emit 散点收敛

> 配套 prd.md(R1-R7)+ design.md(契约)。
> 实施路线对应 [ROADMAP Phase 1](../../../docs/REMOTE-ACCESS-ROADMAP.md#phase-1transport-抽象--emit-散点收敛2-3-天) 的 P1.1 / P1.2 / P1.3。

## 执行顺序

严格串行:**P1.1 → P1.2 → P1.3**。每个 step 完成后跑验证,绿了再进下一步。Tauri 版在每个 step 后都必须可用。

---

## P1.1 Transport interface 定义(0.5 天)

**目标**:新增 `app/src/transport/` 模块,只定义接口 + tauriTransport 实现 + httpTransport stub。**不改任何调用点**。

### Checklist

- [ ] **P1.1.1** 新增 `app/src/transport/types.ts`:`Transport` interface(`invoke` + `listen`)。契约见 design.md §1.1。
- [ ] **P1.1.2** 新增 `app/src/transport/tauri.ts`:`tauriTransport` 实现(转发 `@tauri-apps/api/core` 的 `invoke` + `@tauri-apps/api/event` 的 `listen`)。见 design.md §1.3。
- [ ] **P1.1.3** 新增 `app/src/transport/http.ts`:`httpTransport` stub(throw `not implemented`)。见 design.md §1.4。
- [ ] **P1.1.4** 新增 `app/src/transport/index.ts`:`isTauri()` 判断 + 默认 export。见 design.md §1.5。
- [ ] **P1.1.5** 新增 `app/src/transport/transport.test.ts`:验证 `tauriTransport.invoke` 正确转发到 `@tauri-apps/api/core` 的 `invoke`(mock Tauri API);验证 `tauriTransport.listen` 回调收到的是 `payload` 而非 `Event<T>`。

### 验证

```bash
cd app
pnpm vue-tsc --noEmit          # 类型检查
pnpm vitest run src/transport/ # transport 模块单测
pnpm tauri dev                 # 手动确认:现有功能完全不受影响(因为还没改调用点)
```

### 回滚点

P1.1 完成后 commit:`feat(transport): add Transport interface + tauriTransport + http stub`。若后续 P1.2 出问题,可独立 revert。

---

## P1.2 TauriTransport + 21 文件迁移 + 22 测试 mock(1-1.5 天)

**目标**:21 个非测试文件改用 `transport.invoke/listen`,22 个测试文件 mock 同步改造。**机械替换,无逻辑变化**。

### Checklist

- [ ] **P1.2.1**(可选)抽测试辅助 `app/src/test/helpers/mockTransport.ts`(`createMockTransport` + `mockTransportInstance`)。decision.md §5 Q3 澄清后决定。
- [ ] **P1.2.2** 迁移 stores(13 个):streamController / chat / permissions / projects / subagentRuns / audit / config / memory / models / permissionGrants / providers / subagents / traceStore。按 design.md §2.1 模式替换。
- [ ] **P1.2.3** 迁移 utils(5 个):toolModeChange / toolQuestion / toolTaskStateTransition / uiDiffApply / useErrorBus。
- [ ] **P1.2.4** 迁移 components(4 个):ChatInput.vue / ModelSelect.vue / AskUserQuestionCard.vue / ModelsTab.vue。
- [ ] **P1.2.5** **不改** TitleBar.vue(window/os API 不在范围)。
- [ ] **P1.2.6** **不改** main.ts(仅 import 类型)。
- [ ] **P1.2.7** 改造 22 个测试文件的 mock:批量替换 `vi.mock('@tauri-apps/api/core', ...)` → `vi.mock('@/transport', ...)`,import 改为 `import { transport } from '@/transport'`,调用断言改为 `transport.invoke`。

### 验证

```bash
cd app
pnpm vue-tsc --noEmit
pnpm vitest run                # 全套,22 测试文件 mock 改造后必须全绿

# grep 确认无残留(除 transport 模块本身和 TitleBar)
grep -rn "from '@tauri-apps/api/core'" app/src --include="*.ts" --include="*.vue" \
  | grep -v '\.test\.' | grep -v 'transport/'
# 期望:空

grep -rn "from '@tauri-apps/api/event'" app/src --include="*.ts" --include="*.vue" \
  | grep -v '\.test\.' | grep -v 'transport/'
# 期望:空

# 行为零变化 smoke test
pnpm tauri dev
# 走 prd.md Acceptance Criteria 的手动 smoke test 清单(8 项)
```

### 回滚点

P1.2 完成后 commit:`refactor(transport): migrate 21 files + 22 tests to Transport interface`。

---

## P1.3 后端 emit 散点收敛(0.5-1 天)

**目标**:收敛 6 处直调 `app.emit` 到 sink 抽象,为 Phase 2 换 transport 扫清障碍。

### Checklist

- [ ] **P1.3.1**(前置)grep 确认 `helpers.rs:160,170` 的 `emit_chat_event` / `emit_tool_result` 调用点,判定是否死代码(design.md §5 Q1)。
  ```bash
  grep -rn 'emit_chat_event\|emit_tool_result' app/src-tauri/src/
  ```
- [ ] **P1.3.2** 收 `agent/chat.rs:187`:pre-flight error 路径改走 `AppHandleSink`。该路径已经有 `app: AppHandle` 参数,构造一个 sink 或复用已有的。
- [ ] **P1.3.3** 处理 `agent/helpers.rs:160,170`:
  - 若 P1.3.1 确认死代码 → 删除函数
  - 若有调用 → 改为转发到 sink(签名改为接受 `&dyn ChatEventSink`)
- [ ] **P1.3.4** 新增 `SubagentEventSink` trait + `AppHandleSubagentSink` 实现(design.md §3.3)。**先写集成测试作为回归基线**:
  - [ ] 新增 `app/src-tauri/src/agent/subagent/sink_integration_test.rs`(或现有 test 模块):用 mock provider 跑一轮 subagent,断言 `subagent:event` / `subagent:finished` / worker 内 `permission:ask` 事件序列。**先跑一次确认绿**(基线),再动 trait。
- [ ] **P1.3.5** subagent 3 处散点(`subagent/sink.rs:279,698` + `dispatch.rs:1192`)改为持有 `Arc<dyn SubagentEventSink>` 调 trait 方法。生产注入 `AppHandleSubagentSink`。
- [ ] **P1.3.6** `state.rs:317` `projects:refreshed` **保留不动**(Phase 2 处理),加注释 `// TODO(Phase 2): route through SystemEventSink when daemonizing`。

### 验证

```bash
cd app/src-tauri
cargo test                       # 全套绿
cargo clippy                     # 无新增 warning

# grep 确认散点收敛(无裸 app.emit in agent/)
grep -rn '\.emit(' src/agent/ | grep -v test | grep -v 'AppHandleSink\|SubagentEventSink\|sink\.'
# 期望:只剩 projects:refreshed(在 state.rs 不在 agent/)+ 允许的 sink 调用

# subagent 回归测试(P1.3.4 写的集成测试)
cargo test subagent_sink_integration

# 行为零变化 smoke test(重点验证 subagent 场景)
cd ../..
pnpm tauri dev
# 走 prd.md Acceptance Criteria 的手动 smoke test 清单,重点:
#   □ subagent drawer 看到完整事件流(subagent:event / subagent:finished)
#   □ subagent 内触发 permission:ask,弹窗正常
```

### 回滚点

P1.3 完成后 commit:`refactor(agent): consolidate 6 emit bypass points into sink traits`。

---

## 最终验收(P1.1 + P1.2 + P1.3 全部完成后)

### 自动化

```bash
cd app && pnpm vue-tsc --noEmit && pnpm vitest run
cd src-tauri && cargo test && cargo clippy
```

### grep 双重确认

```bash
# 前端:无非 transport 模块的 @tauri-apps/api/core import(TitleBar 的 window/plugin-os 除外)
grep -rn "from '@tauri-apps/" app/src --include="*.ts" --include="*.vue" \
  | grep -v '\.test\.' | grep -v 'transport/' | grep -v 'TitleBar'
# 期望:空

# 后端:agent/ 内无裸 app.emit(只剩 sink trait 调用)
grep -rn '\.emit(' app/src-tauri/src/agent/ | grep -v test
# 期望:全是 sink.method() 调用,无 app.emit(
```

### 手动 smoke test(`pnpm tauri dev`)

走 prd.md Acceptance Criteria 的 8 项清单,**全部勾选**。

### 完成 → 进入 Phase 3

- [ ] Phase 3.3 spec update:若散点收敛 / Transport 抽象有可复用的 pattern,写入 `.trellis/spec/`(如 `frontend/transport-pattern.md` / `backend/event-sink-trait.md`)
- [ ] Phase 3.4 commit:P1.1 / P1.2 / P1.3 三个 commit 已分别落地,此处只需确认无遗漏
- [ ] `/trellis:finish-work` archive 本 task

---

## Review Gates

- **P1.1 后**:transport 模块单测绿 + `pnpm tauri dev` 正常 → 进 P1.2
- **P1.2 后**:全套 vitest 绿 + grep 确认无残留 + 8 项 smoke test → 进 P1.3
- **P1.3 后**:cargo test 绿 + grep 确认散点收敛 + subagent 集成测试绿 + smoke test 重点 subagent 场景 → 进 Phase 3
