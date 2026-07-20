# Research: chat 错误 UI 现状与"重试按钮"锚点选型

- **Query**: chat 消息卡片 / tool 卡片 / subagent 抽屉里错误 UI 的现状如何?重试按钮要嵌在哪?
- **Scope**: internal (chat 组件树)
- **Date**: 2026-07-17

## Findings

### Files Found

| File Path | Description |
|---|---|
| `app/src/components/chat/MessageItem.vue` | 单条 chat 消息容器;错误渲染走 `<MessageItemFooter>` 插槽 |
| `app/src/components/chat/MessageItemFooter.vue` | 错误行 + F5 latency chip(2026-06-23 split,纯展示) |
| `app/src/components/chat/MessageItemEdit.vue` | 编辑模式 UI,自带 inline 错误(不在 footer 范围) |
| `app/src/components/chat/ToolCallCard.vue` | 单个 tool 调用 + result;错误通过 `result.isError` + `tool-card--error` 类显示 |
| `app/src/components/chat/ToolCallHeader.vue` | tool card 头部(状态 icon + 名称 + 时长),纯展示 |
| `app/src/components/chat/SubagentDrawer.vue` | worker 抽屉;`status === "error"` 时挂 `<SubagentDrawerErrorCard>` |
| `app/src/components/chat/SubagentDrawerErrorCard.vue` | worker error card(纯展示,只有 errorMessage 字符串) |
| `app/src/components/chat/MessageActionsMenu.vue` | user 消息 ⋯ 菜单(Edit / Resend / Copy),user-only |
| `app/src/components/chat/SubagentDrawerHeader.vue` | worker 头部(status badge + banner) |

### Code Patterns

#### Pattern 1: 消息级错误 = `<MessageItemFooter>` 内联红字行

`app/src/components/chat/MessageItem.vue:1145-1151, 1262-1268`(两处 footer 挂载点):

```vue
<!-- tool calls 路径(footer 挂在 msg__tools 内,贴着最后一张 tool card) -->
<MessageItemFooter
  v-if="!showBubble && !isEditingThisMessage"
  :role="message.role"
  :streaming="!!message.streaming"
  :latency="message.latency"
  :error="message.error"
/>

<!-- 气泡路径(footer 挂在 li 底部) -->
<MessageItemFooter
  v-if="!visibleToolCalls.length || showBubble"
  :role="message.role"
  :streaming="!!message.streaming"
  :latency="message.latency"
  :error="message.error"
/>
```

`app/src/components/chat/MessageItemFooter.vue:60-69, 123-138`(error 行渲染):

```ts
const props = withDefaults(
  defineProps<{
    role: "user" | "assistant";
    streaming: boolean;
    latency?: { ttfbMs?: number; genMs?: number; totalMs?: number };
    /** Per-message error. Renders a small red row above the
     *  latency chip (or in place of it, if no latency is
     *  available). Missing for non-error rows. */
    error?: { message: string; category?: string };
  }>(),
  { /* ... */ },
);
```

```vue
<div
  v-if="error"
  class="msg__error"
  role="alert"
  data-testid="msg-error-row"
>
  <Icon name="warn" :size="12" icon-class="msg__error-icon" />
  {{ error.message }}
</div>
```

- **错误文案**:纯文本(仅 `error.message`),无 icon 已实装。
- **`category` 字段已接 prop 但模板不渲染**(`MessageItemFooter.vue:63` 类型声明,模板不读)— 给 retry 按钮预留了位但没用。
- **`role="alert"`**:无障碍语义正确。
- **现有测试**: `MessageItemFooter.test.ts` 已存在(`app/src/components/chat/MessageItemFooter.test.ts`,prd §5 索引)。

`app/src/components/chat/MessageItemFooter.vue:178-190`(.msg__error 样式):

```css
.msg__error {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  margin-top: 4px;
  padding: 0 14px;
  font-size: var(--text-sm);
  color: var(--color-tool-error);
}
.msg__error-icon {
  flex-shrink: 0;
}
```

#### Pattern 2: 编辑模式错误 = `<MessageItemEdit>` 内独立错误行(不在 footer 范围)

`app/src/components/chat/MessageItemEdit.vue:164-174`:

```vue
<div
  v-if="errorMessage"
  class="msg__editor-error"
  role="alert"
  data-testid="msg-editor-error"
>
  <Icon name="warn" :size="12" icon-class="msg__editor-error-icon" />
  {{ errorMessage }}
</div>
```

- **完全独立**于 MessageItemFooter;`errorMessage` 是字符串 prop,parent(`MessageItem.vue:757`)持有 `editError: Ref<string | null>`,IPC 失败时写入。
- **样式**:`.msg__editor-error`(MessageItemEdit.vue:248-258)— red tinted background + 1px error border + 8px padding。
- **不影响 scope B**:editor 错误是 IPC 局部失败(editMessage),不是 chat stream 的 LlmError,不属于 retryable 范畴。

#### Pattern 3: Tool 错误 = `<ToolCallCard>` `result.isError` 翻红 + 文案自带

`app/src/components/chat/ToolCallCard.vue:60-87`:

```ts
const props = defineProps<{
  call: ToolCallInfo;
  result?: ToolResultInfo;
}>();

const accent = computed(() => {
  if (props.result?.isError) return "var(--color-tool-error)";
  return toolAccentVar(props.call.name);
});

const isError = computed(() => !!props.result?.isError);
const hasResult = computed(() => !!props.result);

const statusText = computed<string>(() => {
  if (isError.value) return "error";
  if (hasResult.value) return "done";
  return "running…";
});
```

`ToolOutputBody` 渲染:`ToolCallCard.vue:704-709`:

```vue
<ToolOutputBody
  v-if="!isDispatchSubagent && result"
  :content="result.content"
  :is-error="result.isError"
  :duration-ms="result.durationMs"
/>
```

`ToolCallCard.vue:713-733`(.tool-card + .tool-card--error 样式):

```css
.tool-card {
  background: var(--color-bg-surface);
  border: 1px solid var(--color-bg-border);
  border-left: 3px solid var(--color-text-muted);
  border-radius: var(--radius-md);
  padding: 8px 12px;
  /* ... */
}
.tool-card--error {
  border-color: var(--color-tool-error);
  background: var(--color-bg-elevated);
}
```

- **错误是 tool 的 result 状态**,**不是 chat 流错误**;`isError` 来自后端 tool_result.is_error(`toolResult.isError` 在 `ToolResultInfo` interface)。
- **没有 retry 按钮**;也没有"哪个 user prompt 触发的 tool_use" 的引用(LLM 已经在 stream 内决定用 tool,前端只能重新发 user prompt 让 LLM 决定)。
- **scope B 不需要碰 tool card 错误**:tool 错误是 LLM 决策的结果,用户不应该直接 retry tool — 重发 user prompt(走 resendMessage)才是正确语义。tool card 自身的 retry 按钮价值低。

#### Pattern 4: Subagent 抽屉错误 = banner + 独立 error card

`app/src/components/chat/SubagentDrawer.vue:425-443`(banner):

```ts
const bannerText = computed<{ kind: "error" | "warning"; text: string } | null>(() => {
  if (!run.value) return null;
  if (status.value === "error") {
    const summary = run.value.summary;
    if (summary && summary.length > 0) {
      const truncated = summary.length > 80 ? summary.slice(0, 80) + "…" : summary;
      return { kind: "error", text: `Worker exited with error: ${truncated}` };
    }
    return { kind: "error", text: `Worker exited unexpectedly${statusDisplay.value.suffix}` };
  }
  // ... cancelled / incomplete 分支
});
```

`SubagentDrawer.vue:716-727`(error card 挂载):

```vue
<SubagentDrawerErrorCard
  v-if="status === 'error' && errorMessage !== null"
  :error-message="errorMessage"
/>
```

`app/src/components/chat/SubagentDrawerErrorCard.vue:32-41`(渲染):

```vue
<div class="subagent-drawer__error-card" role="alert">
  <div class="subagent-drawer__error-header">
    <span class="subagent-drawer__error-icon">
      <Icon name="shield-x" :size="14" />
    </span>
    <span class="subagent-drawer__error-title">Worker error</span>
  </div>
  <p class="subagent-drawer__error-message">{{ errorMessage }}</p>
</div>
```

- **`errorMessage` 是 worker run 自己的 finalText/summary/error payload**,与 chat stream 错误是**两条独立通道**(`SubagentBufferSink` 不 forward `ChatEvent::Error`,见 `types.rs:469-477` 设计 D4)。
- **scope B 不动**:subagent 抽屉的 worker retry 语义不明确(worker 是被 dispatch 的 subagent,重试等于重新 dispatch,需要 LLM 决策);prd §3 标"scope B 不做中文化",只确认渲染点不变。

#### Pattern 5: 已存在的 user message Resend 入口(可借鉴入口,但不是 chat retry)

`app/src/components/chat/MessageActionsMenu.vue:249-265`:

```vue
<DropdownMenuItem
  class="msg-actions__item"
  :disabled="!canResend()"
  data-testid="msg-actions-resend"
  @select="onResend"
>
  <Icon name="refresh" :size="14" icon-class="msg-actions__item-icon" />
  <span>重发</span>
  <span v-if="role !== 'user'" class="msg-actions__item-hint">仅 user 消息</span>
</DropdownMenuItem>
```

- **入口位置**:`user` 消息 ⋯ 菜单第 2 项,user-only,streaming 时禁用。
- **重发对象**:`user` message 内容(不变,加 `resendSeq` flag);**不是 chat 流错误重试**。
- **可借鉴**:同样的 `refresh` icon + "重发" 文案 + hover affordance;**不要照搬**:`MessageActionsMenu.onResend` 调 `chatStore.resendMessage`(user content + resendSeq),**而 chat retry** 应该调 `controller.startRequest({ history, ... })`(同 history + 无 resendSeq)。

### External References

- `.trellis/spec/frontend/reka-ui-usage.md:401-481` — "Pattern: Tooltip for hover affordances"(`ChatInput.vue` 的 token-usage chip,delay-duration 150ms)— 可作为 retry button 的 hover/tooltip pattern 参考。
- `.trellis/spec/frontend/popover-pattern.md:706-799` — `ConfirmDialog` pattern;scope B 若做"确认 retry"二次确认可参考,但本轮建议**直接 retry 无确认**(用户已主动点)。
- `.trellis/spec/backend/error-handling.md:206-264` — Agent Loop Error Paths;`ChatEvent::Error` terminal,`ERROR_MARKER = "[生成出错中断]"` 加到 partial turn text — 配合 retry 按钮语义,retry 应该**清掉 ERROR_MARKER**(`controller.refresh` 重新 rehydrate 从 DB)。

### Related Specs

- `.trellis/spec/backend/error-handling.md` — Error terminal / partial turn persist / `ERROR_MARKER` 文案契约。

## Caveats / Not Found

1. **3 个候选锚点各有局限**:(a) `MessageItemFooter` 改动小但侵入"已有组件";(b) `ToolCallCard` 语义不对(tool 错误不应直接重试,应让 LLM 决策);(c) 新建独立组件侵入小但跨多组件散开,需要更多 glue。
2. **`MessageItemFooter` 已经预留 `category?: string` prop 但模板不用** — scope B 加 retry 按钮的**最低侵入**路径就是渲染这个字段(从 category 派生 retryable + 加一个 button)。
3. **错误渲染点总计 4 处**(msg footer / edit error / tool card / drawer error card),**scope B 只动 msg footer**(其他 3 处语义都不对:edit error 是 IPC 局部,tool error 是 LLM 决策,drawer error 是 worker 子任务)。
4. **`error.message` 含 `ERROR_MARKER = "[生成出错中断]"`**(`error-handling.md:228`)— 用户看到的文案尾部会有这个 prefix,scope B retry 后 controller 会重新 reload from DB,新 row 不带 marker(已 fix)。
5. **`SubagentDrawer.vue:792-807` 的 incomplete chip 缺失**(prd §2 缺口)在 scope B 不修(纯 subagent 局部)。
6. **`audit()` 行在 retry 时未生成** — D3 PR3 的 `resend_message` audit 是 user message resend;chat stream retry 应不需要新 audit(同一个 user prompt 重发,backend 无 audit 钩子)。若要审计,scope B 加 audit kind(`retry_chat`?)属扩范围,建议 OOS。

## 给 design.md 的输入

错误 UI 渲染点共 4 处(MessageItemFooter / MessageItemEdit / ToolCallCard / SubagentDrawerErrorCard),scope B **只动 MessageItemFooter**(其余 3 处语义不对:edit 错误是 IPC 局部、tool 错误是 LLM 决策、drawer 错误是 worker 子任务)。**首选方案:在 `MessageItemFooter` 的 `.msg__error` 行右侧加 inline `↻ 重试` 按钮**(visible when `error.category` ∈ {server, network, rate_limit}),点击调新的 `chatStore.retryLastError(messageSeq)` helper(本质 = cancel 旧流 + `controller.startRequest({ history })`)。**改动量 ~40 行**:footer prop 加 `retryable?: boolean` + 模板 button + click handler + 1 个 `chatStore.retryLastError` 方法(参考 `resendMessage` 实现但不带 `resendSeq` + 用原 history 而非新 history)。**不新建独立组件**,**不碰 ToolCallCard / SubagentDrawer / MessageItemEdit**,**不改 wire**(category 已够推断 retryable)。**复用**:`MessageActionsMenu` 的 refresh icon + 文案风格 + 已有"重发"按钮的 hover/tooltip pattern。