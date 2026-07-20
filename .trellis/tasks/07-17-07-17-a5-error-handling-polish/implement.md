# A5 错误处理完善 — Implement Plan

> **状态**:draft(2026-07-17)。配套 `prd.md`(scope B)+ `design.md`(架构 + 数据流)。本文件是 F1-F13 ordered checklist + 验证命令 + 风险/回滚点。

## F1-F13 Ordered Checklist

### F1. `useToast()` composable skeleton + types

**目标**:`app/src/composables/useToast.ts` 新文件,~80-120 行。

具体动作:
- 导出 `Toast` / `ToastCategory` / `UseToastReturn` 类型(`design.md §2.1`)
- module-level 单例 `toasts: Ref<Toast[]>` + `show` / `dismiss` / `clear`
- 内部状态:`MAX_CONCURRENT = 3` / `DEDUPE_WINDOW = 5000` / `DEFAULT_TTL = 5000`
- dedupe 逻辑:遍历 `toasts` 找同 `(category, title, description?)` 且 `Date.now() - createdAt < DEDUPE_WINDOW` → return null
- overflow FIFO:超过 MAX → 删最旧(FIFO,不是 LIFO,因为错误从老到新越来越不重要)
- TTL:每次 `show` 注册 `setTimeout(dismiss, ttl)`,`onUnmounted` 清理
- 导出 `useToast(): UseToastReturn`(返回 module-level 单例的 `toasts` ref)

**不进**:
- 不做 i18n(scope B 全中文,沿用项目硬编码中文风格)
- 不做"用户点 × 关闭"按钮(ToastClose 由 reka-ui 自动渲染,handler 走 dismiss)

**回滚点**:整个 F1 文件可删,无副作用。

### F2. `ToastProvider.vue` — reka-ui 2.9.9 Toast 包装

**目标**:`app/src/components/common/ToastProvider.vue` 新文件,~50-80 行。

具体动作:
- 用 reka-ui 2.9.9 primitive:`ToastProvider` (顶层) + `ToastViewport` (右上角) + `ToastPortal` + `ToastRoot` (per toast) + `ToastTitle` + `ToastDescription` + `ToastClose` (× 按钮)
- 模板遍历 `useToast().toasts`,逐个 `<ToastRoot>` 渲染
- 样式:position: fixed / top: 16px / right: 16px / z-index: 9999 / flex column gap 8px
- 4 类 category → 4 类颜色(项目 CSS variable `--color-error` / `--color-warning` / `--color-info` / `--color-success`;如无 `--color-success`,fallback `--color-info`)
- 接入 reka-ui 的 `ToastClose` 自动 dismiss — 不用手写 handler

**回滚点**:`ToastProvider` 不挂载 → 整套 toast 不可见,但 `useToast().show` 不报错,只 DOM 不渲染。

### F3. 挂载 `ToastProvider` 到 `AppShell.vue`

**目标**:`app/src/components/layout/AppShell.vue` 顶层加 `<ToastProvider />` 1 行。

具体动作:
- import + 在 template 顶层(sidebar / chat panel 同级)放 1 个 `<ToastProvider />`
- 验证:`pnpm dev` 启动后,devtools console 跑 `throw new Error('test')` → 屏幕右上 toast 出现

**回滚点**:删这 1 行,无副作用。

### F4. `categoryRetryable()` helper

**目标**:`app/src/utils/error.ts` 新文件或扩展现有,~10 行。

具体动作:
- 导出 `categoryRetryable(category: ErrorCategory): boolean`(`design.md §2.3`)
- 5 类映射:
  - Auth / InvalidRequest → false
  - RateLimit / Server / Network → true
- 注释:与后端 `AppError::retryable()` 默认派生一致(`llm/error.rs:62-70`)

**回滚点**:helper 不删,只改返回(影响 R2 retry button 显隐)。

### F5. `useErrorBus.routeByCategory` 5 stub → `useToast`

**目标**:`app/src/utils/useErrorBus.ts:127-180` 改 5 stub,~30-50 行替换。

具体动作:
- 5 个 category 分支:
  - Auth: `useToast().show({ category: 'Auth', title: '鉴权失败', description: message })`
  - RateLimit: `useToast().show({ category: 'RateLimit', title: '请求过于频繁', description: message })`
  - Server: `useToast().show({ category: 'Server', title: '服务端错误', description: message })`
  - Network: `useToast().show({ category: 'Network', title: '网络问题', description: message })`
  - InvalidRequest: 保留 `console.warn(category, message, cause)`(原状)
- 移除 TODO 注释 `useErrorBus.ts:158` "接 reka-ui toast"
- 同步移除 `useErrorBus.ts:132` 兜底"(未知错误)"(本 PR 不修,留 §3 polish) — 注意:**不动**,仅移除 TODO

**回滚点**:改回 `console.warn` 即可,`useToast` import 还在但无调用。

### F6. `chatStore.retryChat()` skeleton

**目标**:`app/src/stores/chat.ts` 新增 `retryChat(sessionId, messageSeq)` 函数,~70 行。

具体动作:
- 复制 `resendMessage(sessionId, messageId)`(chat.ts:1257-1341)结构
- 关键差异(`design.md §2.4`):
  - 找 session.messages 找 seq = messageSeq 的 message(不用 id)
  - mutate 该 message:`status: 'error' → 'streaming'` / `text: ''` / `error: null` / `toolCalls: []`
  - 重建 messages 上下文:`session.messages.slice(0, messageSeq + 1)`
  - 用 max+1 seq 生成新 requestId(同 resendMessage)
  - invoke `chat` IPC(同 resendMessage)
  - 不写 `resendSeq`(用户级 retry)
  - 不 push 新 placeholder(原地 mutate)

**回滚点**:函数加 1 个 if `false` 兜底即可禁用。

### F7. `MessageItemFooter.vue` — retry button + loading state

**目标**:`app/src/components/chat/MessageItemFooter.vue` 加 ~40 行。

具体动作:
- import `categoryRetryable` from `@/utils/error`
- import `useChatStore` + 用 `const chatStore = useChatStore()`
- 新增 props:无新增(复用现有 message + session 上下文)
- template:在现有错误红字下面加 `<button v-if="categoryRetryable(category)">↻ 重试</button>`
- button 状态:`loading: Ref<boolean>`(local),点击 → `loading = true` + 调 `chatStore.retryChat(sessionId, messageSeq)`
- 复位:watch session activeRequestId(已经有 store getter 暴露);当流 enter → loading = false
- 样式:复用现有 button 风格(参考 MessageActionsMenu 按钮);不引入新 design token

**回滚点**:template 这 1 段 `v-if` 删除,无副作用。

### F8. Tests — `useToast.test.ts`

**目标**:`app/src/composables/useToast.test.ts` 新文件,~80-120 行。

具体动作:
- 覆盖(`design.md §5.3`):
  - `show` 推入新 toast,`toasts.length === 1`
  - `dismiss` 移除指定 id
  - `clear` 清空所有
  - max 3:推 4 个 → 最早的被 FIFO 移除
  - dedupe:同 (category, title, description) 5s 内 2 次 → 第 2 次返回 null
  - ttl:fake timer 推 1 个,advance 5000ms → 自动 dismiss
- 用 vitest `vi.useFakeTimers()` / `vi.advanceTimersByTime()`

### F9. Tests — `categoryRetryable.test.ts`

**目标**:`app/src/utils/error.test.ts` 新文件或扩展,~20 行。

具体动作:
- 5 类 × 期望值:
  - Auth → false
  - RateLimit → true
  - Server → true
  - Network → true
  - InvalidRequest → false

### F10. Tests — `MessageItemFooter.test.ts`

**目标**:`app/src/components/chat/MessageItemFooter.test.ts` 新文件,~60-100 行。

具体动作:
- 覆盖:
  - 5 类 × retryable 显隐(Auth/InvalidRequest 不显示 button,RateLimit/Server/Network 显示)
  - 点击 button → 调 `chatStore.retryChat` 1 次
  - loading 态:点击后 button text 变 "重试中..." + disabled
  - 流 enter 事件 → loading = false
- 用 vitest `@vue/test-utils` `mount` + mock `useChatStore`

### F11. 全量验证

具体动作(顺序跑):
```bash
# 1. 前端类型 + build
cd app && pnpm build
# 期望:vue-tsc 0 err + vite build OK + 0 新增 warning

# 2. 前端测试
cd app && pnpm vitest run
# 期望:全过(863 baseline + 新增 3 测试文件 useToast / categoryRetryable / MessageItemFooter)

# 3. Rust 检查(WSL 注意 PKG_CONFIG_PATH)
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo check
# 期望:0 err(scope B 不改 Rust,只是 sanity)

# 4. 手测
cd app && pnpm tauri dev
# devtools console 跑:
#   - throw new Error('test auth fail') → 期望 toast 出现 "鉴权失败"
#   - Promise.reject({category: 'Network', message: 'timeout'}) → 期望 toast 出现 "网络问题: timeout"
#   - InvalidRequest 类不弹 toast,仅 console.warn
# chat UI:触发 LLM 错误(改 ANTHROPIC_API_KEY 为无效值后发消息) → 期望 MessageItemFooter 出现 "↻ 重试" 按钮
#   - 点击按钮 → 已有卡片原地重试(不 push 新条目)
```

### F12. 文档同步

具体动作:
- `.trellis/spec/backend/error-handling.md` 加 `RULE-A-018` 条目(全局错误 toast 路由,对应 commit 引用,closed 状态,落地 `useErrorBus.routeByCategory` + `useToast` composable)
- `.trellis/spec/frontend/state-management.md` 加 `useToast` composable 模式条目
- 不改 SPEC-DRIFT.md(scope B 不解决 audit.rs:4 docstring 等二阶 drift,留 V3)

### F13. Check + commit + archive

具体动作:
- 派遣 `trellis-check` subagent 跑质量校验
- 修 review 反馈(中等以下问题立即修;若发现需重做 F 步骤,回滚到对应 F)
- 1 commit:`feat(a5): useToast + retryable button(scope B)`,覆盖 F1-F12 全部
- archive task:`python3 .trellis/scripts/task.py archive 07-17-07-17-a5-error-handling-polish`
- journal recap

## 验证命令汇总(完整)

```bash
# 前端
cd app && pnpm build                          # type-check + build
cd app && pnpm vitest run                     # 单测
cd app && pnpm tauri dev                      # 手测

# 后端(sanity,scope B 不改 Rust)
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo check
```

## 风险与回滚点

| 风险 | 概率 | 缓解 | 回滚 |
|---|---|---|---|
| reka-ui 2.9.9 Toast API 与预期不一致 | 低 | F2 单独验证(`pnpm dev` 看 devtools) | F2 改自研 CSS-only toast,其余 F 不变 |
| `useToast` module-level 单例在 SSR / test 隔离问题 | 低 | F8 用 `beforeEach(() => clear())` 隔离 | 改成 composable 注入(改 ~30 行) |
| `retryChat` 与现有流竞争(同时 2 个流) | 中 | F7 watch `activeRequestId` + cancel_chat 顶替 | F6 if false 兜底 |
| 现有 `MessageItemFooter` 测试覆盖未知导致回归 | 中 | F10 单测前先跑 `pnpm vitest run` baseline,确认现有测试不挂 | F7 commit revert(无外部依赖) |
| `useErrorBus.routeByCategory` 修改影响主链路 | **极低**(research/05 确认仅 1 个全局调用点) | F5 改完跑 `pnpm vitest run` + 手测 devtools throw | F5 commit revert |

## Review Gate 清单(implement 完成后,trellis-check 前自查)

- [ ] F1-F12 全过,F13 commit 干净
- [ ] vue-tsc 0 err,新增 3 测试文件全过,无 baseline 回归
- [ ] 手测 4 路径:throw / reject / LLM 错误 → MessageItemFooter / 点击 retry
- [ ] 文档:error-handling.md RULE-A-018 + state-management.md useToast 同步
- [ ] 1 commit + 1 archive + 1 journal

## 估算

- F1-F12 代码 ~330-460 行(含 3 测试文件)
- F13 验证 + 修 review 反馈 ~30-60 分钟
- 1 PR 容量合理
