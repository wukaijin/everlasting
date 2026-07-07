# 执行计划:跨 session 待处理交互 UI 提醒

> 顺序执行;每步完成后跑该步的验证再进下一步。全部为前端改动,无 Rust。

## 前置(已完成)
- [x] 探索确认数据层就绪、缺口在展示层(prd/design 已记录 file:line)。

## B1 — 扩展 toast 支持 sessionId(C 档基础)
- [ ] `app/src/stores/projects.ts`:`ToastMessage` interface 加 `sessionId?: string`。
- [ ] `showToast(message, kind = "info", opts?: { sessionId?: string })`:把 `opts?.sessionId` 写入 `toast.value`。
- [ ] 验证:`grep -n "showToast(" app/src` 所有现有调用点(应均不传第三参)→ 行为不变。
- **门**:无(纯加可选字段)。

## B2 — AppShell toast 点击跳转(C 档)
- [ ] `app/src/components/layout/AppShell.vue`:toast 的 `@click` 改为 `onToastClick`,函数内:若 `toast.sessionId && chatStore.sessions.some(s => s.id === toast.sessionId)` → `chatStore.switchSession(toast.sessionId)`;然后 `dismissToast`。
- [ ] import `useChatStore`(若未引)。
- [ ] 验证:无 sessionId 的 toast(项目操作)走 else → `dismissToast`,行为同前。
- **门**:`vue-tsc --noEmit` 过。

## B3 — streamController handler 加 toast 触发(C 档)
- [ ] `app/src/stores/streamController.ts`:`handleModeChangeRequest`(`:1448`)与 `handleToolQuestion`(`:1422`)在 `addPending` 之后:
  ```
  if (payload.session_id !== <currentSessionId>) {
    const title = chatStore.sessions.find(s => s.id === payload.session_id)?.title;
    const who = title ? `「${title}」` : "另一项目的会话";
    const msg = <kind===mode_change> ? `${who} 申请切换到 ${payload.target_mode} 模式` : `${who} 有问题等你回答`;
    projectsStore.showToast(msg, "info", { sessionId: payload.session_id });
  }
  ```
- [ ] currentSessionId 取值:`useChatStore().currentSessionId`(确认 store 暴露该字段)。
- [ ] 验证:当前 session 的 pending 不弹 toast;非当前才弹。
- **门**:vitest(streamController 相关)过。

## B4 — SessionList 角标(A 档)
- [ ] `app/src/components/SessionList.vue`:
  - import `useQuestionCardsStore`、`REQUEST_MODE_CHANGE_TOOL_NAME` 不需要(直接读 kind)。
  - `const qc = useQuestionCardsStore()`。
  - 在两处 `session-item__title-row`(flat 模式 `:434` 区 + grouped 模式 `:519` 区),`session-item__pending-approval` 之后加:
    ```
    <span v-if="qc.getPending(s.id)" class="session-item__pending-interaction" :title="...">
      <Icon :name="qc.getPending(s.id)!.kind === 'mode_change' ? 'refresh' : 'info'" :size="12" />
    </span>
    ```
  - mode_change 配色:可加 `:style` 按 `target_mode` 映射 color,或一个 modifier class。
- [ ] CSS:复用 `pulseDot` 动画;`flex-shrink:0`。
- [ ] 验证:compact 密度下 pending-approval + pending-interaction 并列不溢出。
- **门**:vitest(SessionList 渲染测试)+ `vue-tsc`。

## B5 — AppHeader 顶栏徽章(B 档)
- [ ] 新建 `app/src/components/layout/PendingBadge.vue`(或在 AppHeader 内联):`count = computed(() => qc.pendingBySession.size)`,`v-if="count > 0"`,脉冲 chip + 数字 + title。
- [ ] 点击:取当前 project 内最近 pending session(`chatStore.sessions` 过滤 `qc.getPending(s.id)` 存在 → 按 `ts` 最大)→ `switchSession`;无则 no-op。
- [ ] 挂载:`app/src/components/layout/AppHeader.vue` 的 `<TitleBar>` slot 内(`<HiddenProjectsMenu />` 之后)。CSS 推到右侧(design §4.2)。
- [ ] 验证:`count=0` 不渲染;`count≥1` 显示脉冲。
- **门**:vitest + `vue-tsc`。

## B6 — 测试
- [ ] `app/src/stores/projects.test.ts` 或新 `streamController` 测试:`showToast` 带 sessionId 写入;handler 在非当前 session 触发 toast、当前 session 不触发。
- [ ] SessionList 测试(参考现有 `SessionList` 测试范式):mock `useQuestionCardsStore.getPending` 返回 mode_change / question / undefined → 角标渲染/图标/隐藏。
- [ ] AppShell 或 PendingBadge 测试:count computed、点击跳转(同 project 切 / 跨 project 不切)。
- **门**:`cd app && pnpm test` 全绿。

## B7 — 类型 + 构建门
- [ ] `cd app && pnpm build`(`vue-tsc --noEmit` + `vite build`)全绿。
- **门**:零 type error。

## B8 — 手测(可选,建议)
- [ ] `cd app && pnpm tauri dev`:开两个 session(同 project + 跨 project),在非当前 session 触发 `request_mode_change` / `ask_user_question`(可临时让 LLM 调,或 mock event):
  - 同 project:A 角标 + B 徽章 + C toast 都出现;toast 点击切过去;允许/拒绝后全部消失。
  - 跨 project:sidebar 看不到目标 session,但 B 徽章有数字 + C toast 弹(文案"另一项目的会话");toast 点击不跳。

## 验证命令汇总
```bash
cd app && pnpm test                      # vitest
cd app && pnpm build                     # vue-tsc --noEmit + vite build
# 无 Rust 改动,不需要 cargo check/test
```

## 风险点 / 回滚
- **toast 点击 handler 改动影响所有 toast**(B2):无 sessionId 的 toast 必须仍只 dismiss → 测试覆盖。
- **TitleBar slot 徽章 CSS**(B5):可能撞窗口控件/drag region → 备选 absolute 定位(design §4.2)。
- **compact 密度角标溢出**(B4):3 图标并列 → 实测调整。
- **回滚**:单 PR revert,5 前端文件 + 测试,无后端/DB/IPC 残留。
