# PRD — BUGLIST 功能池交互打磨批次(CH8-2 / CH4-3 / CH3-2 / CH7-4)

> 来源:`docs/BUGLIST.md` §5 功能建议池(D 类,2026-08-29 WebUI 黑盒全量测试产出)。
> 本批次收尾 §5 中 4 条小交互打磨;做完后 §5 仅剩 CH12-2(独立小功能,另行立任务)。
> 四项均为前端小改动,互相独立,单项可独立验收。Lightweight 任务,PRD-only。

---

## CH8-2 提问卡滚动可见性 + 聊天回答并存提示

**背景**:
- pending interaction(`ask_user_question` / `loop_intervention` / `turn_limit_softcap` / `mode_change` / `task_state_transition`)的卡片挂载在消息流末尾(`MessageItem` 锚定)或 ChatPanel 浮动卡(合成 id 变体)。`MessageList` 只在 near-bottom(80px 阈值)或 force-follow 时自动跟底;**用户上滚读历史时,阻塞式提问卡落在视口外,无任何提示** —— loop 被阻塞等用户,用户不知情。
- 另一侧:流式中用户发消息走 F1 排队,但 pending 询问仍在等**卡片**提交;用户可能以为输入框回答就能解锁 Agent,实际消息排队而 loop 仍阻塞在 QuestionStore oneshot 上。

**需求**:
- R1(滚动可见性):`MessageList` watch 当前 session 的 pending interaction(`questionCardsStore.getPending(currentSessionId)`),**从无到有的跃迁**(null → non-null)时强制回底(`isAtBottom = true` + 瞬时 `scrollToBottom(false)`),保证阻塞卡进入视口。pending 内容不变、some → some(如切换到另一个本就有 pending 的 session,重载路径 `scrollAfterReload` 本就回底)不重复触发。
- R2(并存提示):`chatSendActions.send()` 在 `queueingClassic` 分支(经典 session 流式排队发送)且当前 session 存在 pending interaction 时,warn toast 提示(文案自定,需传达两点:消息已排队;Agent 正阻塞等待提问卡提交)。

**验收**:
- AC1: 流式中上滚读历史 → agent 发起阻塞询问 → 视图自动回底,提问卡可见。
- AC2: pending 存在时经输入框发送 → toast 出现,消息正常入队,卡片状态不受影响。
- AC3: 无 pending 时排队发送 → 无该 toast(不回归)。
- AC4: 单测覆盖 MessageList 回底 watcher(null→some 触发;some→some 不触发)+ send 排队 + pending 的 toast 断言。

---

## CH4-3 助手消息菜单禁用项灰态视觉加强

**背景**:`MessageActionsMenu` 对非 user 消息渲染禁用的「编辑/重发」项(reka `data-disabled`),当前只有 `color: var(--color-text-muted)` + `cursor: not-allowed`,与启用项(`--color-text-primary`)对比弱,黑盒测试据此误判「禁用项点了没反应/看不出不可点」。

**需求**:
- R:`[data-disabled]` 菜单项整体降低不透明度(约 0.55,叠加现有 muted 色),保持 `not-allowed`;highlight 态不给禁用项上背景。改动限 `MessageActionsMenu.vue` scoped CSS,桌面 + 移动端 44px 行高块不受影响。

**验收**:
- AC1: 非 user 消息菜单中「编辑/重发」视觉明显弱于「复制」。
- AC2: user 消息菜单三项启用态渲染不变。

---

## CH3-2 隐藏项目「重新打开」后下拉自动收起

**背景**:`HiddenProjectsMenu`(reka DropdownMenu)在有多个隐藏项目时,点某行「重新打开」后菜单保持展开,列表仍显示已恢复的项目(陈旧数据),需手动收起。

**需求**:
- R:`DropdownMenuRoot` 绑 `v-model:open`;`onUnhide` 成功后置 `open = false`(无论剩余数量;剩 0 时现有 `v-if` 卸载本就收起)。不影响既有「只点按钮不点整行」的事件绑定策略。

**验收**:
- AC1: 2+ 隐藏项目时点其中一个「重新打开」→ 菜单立即收起,badge 计数 -1。
- AC2: 最后一个隐藏项目恢复 → 触发按钮消失(现状保持)。

---

## CH7-4 放行管理「撤销」加确认

**背景**:`PermissionGrantsModal` 的「撤销」一键直达 DB 删除(设计 D1 即时生效),无确认;放行条目是安全相关配置,误触代价不对称。

**需求**:
- R:撤销点击 → `ConfirmDialog`(danger 变体,复用通用组件;先例 RuntimeMemoryModal/MemoryPreview:组件直接放进 reka `DialogContent` 内,靠 DialogContent stacking context 自然覆盖,无需 z-index 特技)展示该行 tool 名 + matchKind 中文标签(整工具/前缀/路径)+ matchValue;确认后走既有 `store.revoke(row)`;取消/Esc/backdrop 关闭不产生任何变更。store 与 `PermissionGrantItem` 零改动(仍 emit `revoke`,语义变为「请求撤销」)。

**验收**:
- AC1: 点撤销 → 弹确认并正确展示行内容;确认 → 行消失(revoke 生效)。
- AC2: 取消 → 无任何 IPC,列表不变。
- AC3: 单测覆盖确认流(确认触发 revoke、取消不触发)。

---

## 全局约束

- 全部改动限 `app/src` 前端;后端零 diff。
- 遵循 design-tokens(不新增 `--color-*` token)、`.btn` 家族约定、spec `test-environment.md` 的 jsdom/fake-timers 坑。
- 完成后 `pnpm test` 全绿 + `vue-tsc` 干净。
- 完成后 `docs/BUGLIST.md` §5 对应条目标 ✅(CH12-2 保留)。
