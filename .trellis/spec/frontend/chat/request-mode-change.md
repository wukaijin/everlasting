# request_mode_change inline card (B6+ A, 2026-07-07)

> **Source**: extracted from `frontend/chat.md` §"request_mode_change inline card (B6+ A, 2026-07-07)" (2026-08-10 doc-split task).

## request_mode_change inline card (B6+ A, 2026-07-07)

`request_mode_change` tool 的前端载体是**inline message card**,**不**是
modal / portal / overlay(同 `AskUserQuestionCard` 红线)。**唯一例外**:
Yolo 路径走既有的 `pendingYoloConfirm` modal(2026-06-13 落地,user
主动切 Yolo 路径)做二次确认,LLM 申请 Yolo 路径**完全沿用**该 modal
避免重复实现。后端契约详见
[tool-contract/11-request-mode-change.md §request_mode_change](../../backend/tool-contract/11-request-mode-change.md);
权限层 IPC 链见
[permission-layer.md §5c](../../backend/permission-layer.md)。

### 组件与红线

- **组件**:`<RequestModeChangeCard>`(新建,3 状态机 `pending` / `allowed`
  / `cancelled`)。
- **挂载点**:`MessageList` 的 `visibleToolCalls` v-for 内,作为
  `<ToolCallCard>` 的 sibling 挂(同 `<AskUserQuestionCard>` /
  `<UiCard>` 模式)。**不**portal / **不**overlay / **不**reka-ui
  Dialog —— 跟 tool_call 卡片同层(由 `MessageItem.vue` 路由分发,
  `tool_name === "request_mode_change"` → `<RequestModeChangeCard>`,
  其余仍走 `<ToolCallCard>`)。
- **Yolo 二次 modal 例外**:`pendingYoloConfirm` modal 是 portal 到
  body 的 reka-ui Dialog(z-index 1100,跟其他 modal 共享),沿用
  `useChatStore.requestSetMode` 的现有实现,**不新写 modal 组件**。

### 视觉规格

- **Header chip**:目标 mode 名字("切换到 Edit" / "切换到 Plan" /
  "切换到 Yolo") + 状态色(plan = cyan / edit = accent / yolo = red,
  映射 `Mode` 枚举 → design token)。
- **Reason 文本**(LLM 给的 `reason`,≤500 字符,可选):卡片副标题,
  纯文本 wrap,**不** markdown 渲染(避免 prompt injection + 减少
  XSS 风险;mode 切换理由不需要富文本)。
- **Action row(pending 态)**:**允许** + **拒绝** 两按钮(等宽并列,
  允许在左)。允许按钮颜色 = 目标 mode 颜色(plan 蓝 / edit 灰 /
  yolo 红) — 视觉强提示高风险。
- **Allowed 态**:`已切换到 [mode]` status pill + `prev_mode` →
  `new_mode` 对比小字。
- **Cancelled 态**:`已拒绝` status pill,LLM 自决(从 tool_result
  看到 `cancelled_by_user: true`)。
- **Noop 态**:由后端不弹 card 处理(LLM 申请切到当前 mode 时
  tool 立即返回 `{"noop": true, ...}`,不渲染 card,UI 透明)。

### Yolo 二次 modal 流(双 IPC)

card 上"允许"按钮 → 不直接调 `resolve_mode_change` IPC,先 dispatch
到 `useChatStore.requestSetMode(sid, "yolo")` 触发既有
`pendingYoloConfirm` modal:

```
1. user 点 card "允许" (target = yolo)
   ↓
2. <RequestModeChangeCard> emit "allowed" → MessageItem handler
   ↓
3. handler 调 useChatStore.requestSetMode(sid, "yolo")
   ↓
4. useChatStore 设 pendingYoloConfirm = true
   ↓
5. 弹 Yolo 二次 modal(显示 "切换到 Yolo 将跳过所有用户确认,
   仅硬 kill list 仍生效")
   ↓
6a. user 点 modal "确认" → confirmYolo action
    → set_session_mode IPC(sid, "yolo")       [IPC A — 落库 + audit "mode_changed"]
    → set_session_mode handler 检测 card dispatch 路径
    → 额外 record_audit("mode_change_allowed", ...)
    → resolve_mode_change IPC(allow=true)     [IPC B — 解 oneshot]
    → agent loop oneshot 解除
6b. user 点 modal "取消" → cancelYolo action
    → resolve_mode_change IPC(allow=false)
    → record_audit("mode_change_denied", { reason: "yolo_cancelled_confirm" })
    → tool_result = {"cancelled_by_user": true, ...}
7. (root guard) is_running_as_root=true 时,modal "确认"按钮 disabled +
   红字 "Cannot enable Yolo as root";点击无效 → 走 6b 路径
   (audit reason: "yolo_root_guard")
```

非 Yolo 路径(target = edit / plan)不走 4-7,直接调
`resolve_mode_change IPC(allow=true)`(单 IPC)。

### 数据流契约

- **wire shape**:`ModeChangePayload` 走 snake_case 序列化(共享 struct
  豁免 —— `#[serde(rename_all = "snake_case")]`,跟
  `ToolQuestionPayload` 同款,跟顶层 Tauri arg 的 auto-camel 区分)。
  前端 `getPendingInteraction` IPC 解析时也按 snake_case 拿
  `target_mode` / `current_mode` / `tool_use_id` / `session_id` /
  `reason` / `ts`。
- **store 归属**:`pendingBySession: reactive(new Map<sid, PendingInteraction>)`
  在 `useQuestionCardsStore`(沿用 ask_user_question 的 store,**不**新
  建 store;state-management.md:131-139 跨切面领域状态归 feature store
  硬规则),`kind: "question" | "mode_change"` 区分。同 session 单
  pending gate(LLM 第二次 request_mode_change 时 `QuestionStore`
  报 `AlreadyPending`,前端 `getPendingInteraction` 仍能查到第一次)。
- **session 切换保留**:同 ask_user_question — session A 挂 pending
  → 切到 B → 切回 A → card 仍可答(通过 `getPendingInteraction` 重
  新查询)。
- **mode chip 同步**:`resolve_mode_change(allow=true)` IPC 返回
  `SessionRow`(对齐 `set_session_mode` 既有 IPC 形态),前端 invoke
  后直接 `setCurrentSession(row)`,`ModeSelect` chip 立即反映新
  mode(不需要 reload 整个 session)。Yolo 二段路径
  `set_session_mode` 自身也返回 `SessionRow`,`confirmYolo` 拿 row
  后 `setCurrentSession(row)` 一次,IPC B(解 oneshot)仅后端消费
  不需前端再 setSessionMode(避免重复)。

### MessageItem dispatch(`tool_name` 路由)

```vue
<template v-for="tc in visibleToolCalls" :key="tc.id">
  <ToolCallCard :call="tc" :result="..." />
  <AskUserQuestionCard v-if="askCardPropsFor(tc) !== undefined" v-bind="askCardPropsFor(tc)!" />
  <RequestModeChangeCard v-if="tc.name === 'request_mode_change'" v-bind="requestModeChangeCardPropsFor(tc)!" />
  <UiCard v-if="tc.name === USE_UI_TOOL_NAME" :call="tc" />
</template>
```

加新 tool = 改 `MessageItem` template 一行 v-if + 新组件。`MessageItem`
保持纯展示 / 路由,业务逻辑在 `useChatStore` /
`useQuestionCardsStore`。

### Tests required

`RequestModeChangeCard.test.ts`(23 个,3 状态 × 3 target_mode + 边界):

| Test | 断言 |
|---|---|
| `renders_pending_state_with_target_mode_header` | pending 态渲染 target mode 名 + 状态色 |
| `renders_allowed_state_with_prev_to_new_pill` | allowed 态渲染 `已切换到 [mode]` pill + 对比 |
| `renders_cancelled_state_with_denied_pill` | cancelled 态渲染 `已拒绝` pill |
| `renders_reason_when_provided` | reason 文本显示 |
| `hides_reason_when_absent` | reason=None 时副标题不渲染 |
| `truncates_reason_at_500_chars` | 501 chars 截断(防御 LLM 超长) |
| `allow_button_color_matches_target_mode` | allow 按钮 class 跟 target mode 颜色映射 |
| `allow_button_emits_allowed_with_target_mode` | 点击 emit `allowed` + target mode |
| `deny_button_emits_denied` | 点击 emit `denied` |
| `yolo_allow_dispatches_to_request_set_mode_not_resolve` | Yolo 路径不直接调 `resolveModeChange`,调 `requestSetMode(sid, "yolo")` |
| `non_yolo_allow_dispatches_to_resolve_mode_change` | edit/plan 路径直接调 `resolveModeChange(allow=true)` |
| `cancelled_state_disables_buttons` | cancelled 态按钮 disabled(防双击) |
| `allowed_state_disables_buttons` | allowed 态按钮 disabled |
| `no_portal_no_overlay_no_dialog_outside_yolo_path` | DOM 里无 portal/overlay,只有 Yolo 二次 modal 走 `pendingYoloConfirm`(root 节点) |
| `mode_color_mapping_cyan_accent_red` | plan=cyan / edit=accent / yolo=red 3 套 CSS 变量 |
| `reason_text_no_markdown_render` | reason 含 `**` / `#` 字符时不解析为 markdown |
| `noop_state_not_rendered` | noop 路径由后端处理,前端无 noop 态测试 |
| `card_is_sibling_of_tool_call_card` | DOM 顺序: ToolCallCard → RequestModeChangeCard(无 modal 中介) |
| `yolo_cancelled_confirm_returns_denied_path` | modal 取消 → 走 denied IPC(audit reason: yolo_cancelled_confirm) |
| `yolo_root_guard_disables_confirm_button` | root 守卫触发时 modal "确认" disabled + 红字 |
| `session_switch_preserves_pending_card` | session A pending → 切 B → 切回 A → card 仍 pending(可答) |
| `mode_chip_updates_immediately_on_resolve` | IPC 返 SessionRow → setCurrentSession 同步 → ModeSelect chip 切到新 mode |
| `duplicate_resolve_is_noop` | 第二次调 resolve(rid 已 resolve)→ store 静默忽略(防双 audit) |

`useQuestionCardsStore.resolveModeChange`(2 个)+ `useQuestionCardsStore
.getPendingInteraction` IPC binding(2 个)+ `useChatStore.requestSetMode`
Yolo 路径已有 `chatMode.test.ts` 覆盖(零回归)。

### Common Mistakes / Gotchas

- **新写 Yolo 二次 modal 组件**:沿用 `useChatStore.pendingYoloConfirm` 既
  有 modal,只改 `<RequestModeChangeCard>` 的 allowed emit handler 调
  `requestSetMode(sid, "yolo")` 触发。
- **allowed 路径直接调 `resolve_mode_change` 不查 mode**:Yolo 路径必须
  先 dispatch 到 `requestSetMode` 走 modal,非 Yolo 路径才直接调
  `resolveModeChange`。在 `RequestModeChangeCard` emit handler 处分支
  `if (targetMode === 'yolo') requestSetMode(); else resolveModeChange(allow=true)`。
- **tool_result 回灌 reason**:tool_result 走 `{"allowed": true, "prev_mode", "new_mode"}`
  / `{"cancelled_by_user": true, reason?}` / `{"noop": true, "current_mode"}` 三种
  形态,**不**回灌 LLM 申请时的 reason(避免 prompt 膨胀;audit 是
  审计,tool_result 是 LLM 决策输入,职责分离)。
- **resolve_mode_change IPC 返回 `SessionRow` 而非 `void`**:对齐
  `set_session_mode` IPC 既有形态,前端 `invoke` 拿回 row 直接
  `setCurrentSession(row)`,**不**调 `loadSession(sid)` 重拉(防 N+1
  查询 + 闪烁)。
- **Yolo 二段路径 IPC 顺序**:`set_session_mode` 落库 + audit →
  `resolve_mode_change` 解 oneshot;反过来 agent loop 会先收到 Allow
  但 DB 未落库,出现不一致(design §7.2 风险段)。
- **noop 路径不渲染 card**:LLM 申请切到当前 mode 时后端不挂 store、
  不发 IPC,前端不渲染 card,UI 透明;测试无 noop 态断言(后端 short-circuit
  保证)。
- **mode 颜色用 design token**:`plan` = `--color-mode-plan` (cyan),
  `edit` = `--color-mode-edit` (accent),`yolo` = `--color-mode-yolo`
  (red),不写死 hex;跟 `ModeSelect` chip 同 token 保持视觉一致。
