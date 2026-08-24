# 按钮样式全量盘点(2026-08-24,planning 期子代理 grep/read 实证)

> 供 WP2-6 迁移期逐处对照。基础数据:**167 个 `<button>` / 69 个 .vue 文件**,
> 约 135 条按钮样式规则。token 体系在 `app/src/style.css` `@theme` 块。
> `.btn` / `.btn--*` 类名 src 下零占用。

## 1. 变体家族(规则级)

### A. primary/accent 实底 — 13 处规则
`background: var(--color-accent)`,hover 绝大多数 `--color-accent-hover`:
- `components/search/SearchModal.vue:793`(.search-modal__open-btn)
- `components/settings/ModelForm.vue:458`、`RemoteTab.vue:402`、`ProvidersTab.vue:647`、`ModelsTab.vue:364`(各 --primary)
- `components/chat/AskUserQuestionCard.vue:853`、`PermissionAskBody.vue:404`(--always)、`MessageItemEdit.vue:295`(--save)、`RequestModeChangeCard.vue:582`(--edit)、`RequestTaskStateTransitionCard.vue:427`
- `views/PairingView.vue:248`、`components/chat/EmptyProjectState.vue:175`(empty-state__add)
- `components/chat/ChatInput.vue:1154`(.chat-input__action = send,圆形特例)

### B. danger 实底 — 8 处
`background: var(--color-tool-error)`:
- `components/chat/YoloConfirmModal.vue:333`、`DeleteWorktreeConfirm.vue:228`、`RequestModeChangeCard.vue:597`(--yolo)
- `components/common/ConfirmDialog.vue:215`(--danger)
- `components/layout/TitleBar.vue:335`(close hover)、`components/SessionList.vue:920`(session delete hover)、`components/layout/ProjectTabs.vue:322`(tab close hover)—— 这 3 处是「ghost 本体、hover 变红实底」→ ghost + 本地覆写
- `components/chat/ChatInput.vue:1184`(stop,圆形 + 呼吸动画)

### C. danger 弱红底(hover tint)— 9 处
- 裸 `rgba(239, 68, 68, 0.15)` ×3(漂移):`ProvidersTab.vue:682`、`ModelRow.vue:261`、`DeleteModelConfirm.vue:113`
- token 写法 ×6:`RuntimeMemoryModal.vue:876`、`GroupChatConfigModal.vue:683`、`WorkerMergeControls.vue:332`、`MessageItemFooter.vue:306`、`MemoryPreview.vue:869`、`PermissionGrantItem.vue:165`

### D. ghost / 透明底 — 约 44 处(最大家族)
本体 `transparent`,hover 才有底:
- 全部 modal close 按钮 ×10+:`YoloConfirmModal.vue:226`、`MemoryModal.vue:151`、`DeleteWorktreeConfirm.vue:164`、`DiffModal.vue:138`、`RuntimeMemoryModal.vue:619`、`PermissionGrantsModal.vue:212`、`TracePanel.vue:323`、`AuditLogModal.vue:395`、`MarkdownDetailModal.vue:305`、`ConfirmDialog.vue:165`、`SettingsModal.vue:145`
- 布局/导航:`Sidebar.vue:300/321/350`、`TitleBar.vue:315`、`ProjectTabs.vue:303/330/384`、`ModeSelect.vue:339`、`ModelSelect.vue:228`
- 菜单/列表行:`TriggerMenu.vue:467`、`WorktreeChip.vue:340`、`SearchModal.vue:521/549/669`、`ReviewMatrix.vue:187/223`、`HiddenProjectsMenu.vue:128/251`、`MemoryLayerItem.vue:214`
- settings 表单基底:`ModelForm.vue:445`、`ProvidersTab.vue:636/666`、`ModelRow.vue:239`、`ModelsTab.vue:351`、`DeleteModelConfirm.vue:101`、`RemoteTab.vue:389`
- 其他:`SessionList.vue:902/959`、`SessionSearchInput.vue:185`、`ChatPanel.vue:1317/1550`、`DrawerSection.vue:198`、`DrawerPromptCard.vue:183`、`PluginSelect.vue:357`、`MessageItemFooter.vue:293`、`ChecklistCard.vue:328`、`SubagentDrawer.vue:1103`、`WorkerTurnTraceList.vue:157`、`NodeListView.vue:190`、`EmptyProjectState.vue:204`(text-link 形态)

### E. muted / 弱底 — 约 32 处(第二大家族)
本体 `--color-bg-elevated` / `--color-bg-surface`:
- `ChatPanel.vue:1345/1375/1405/1436`(memory/audit/grants/trace 四兄弟)+ `:1279` chip
- cancel 按钮 ×4:`YoloConfirmModal.vue:324`、`DeleteWorktreeConfirm.vue:219`、`ConfirmDialog.vue:206`、settings 各 --secondary(ModelForm:468、ProvidersTab:657、DeleteModelConfirm:117)
- `RuntimeMemoryModal.vue:847`、`GroupChatConfigModal.vue:858/671`、`ReviewMatrix.vue:269`、`WorkerMergeControls.vue:296`、`AskUserQuestionCard.vue:831`、`PermissionAskBody.vue:394`、`RequestModeChangeCard.vue:555`、`RequestTaskStateTransitionCard.vue:405`、`MessageItemEdit.vue:278`、`TracePanel.vue:296`、`SearchHistoryCard.vue:298`、`TurnTimeline.vue:155`
- 圆形特例:`MessageList.vue:387`(scroll-to-bottom)、`ChatInput.vue:923`(staged-remove)、`ChecklistCard.vue:449`(ball)、`SubagentDrawer.vue:945`(jump-latest pill)
- refresh 类:`MemoryPreview.vue:579`、`MemoryLayerItem.vue:391`、`AuditLogModal.vue:606`、`PermissionGrantsModal.vue:251`、`MessageActionsMenu.vue:335`、`ChatInputTokenUsage.vue:894`(token-save)、`ChatInputLatencyPopover.vue:212`

### F. accent-muted 色调家族 — 约 19 处
本体即 accent-muted ×4:`EditFileCard.vue:307`、`WorktreeChip.vue:299`、`ToolCallCard.vue:759`、`ConfirmDialog.vue:225`(--warning);hover 变 accent-muted ×15:`ChatPanel` 四按钮 hover(1355/1385/1415/1446)、`EmptyProjectState.vue:346`、`ProjectTabs.vue:343/394`、`MemoryPreview.vue:590`、`MemoryLayerItem.vue:402`、`HiddenProjectsMenu.vue:139`、`PermissionGrantsModal.vue:264`、`AuditLogModal.vue:616`、`ChatInputLatencyPopover.vue:224/230`、`MessageList.vue:396`

### G. outline / 描边主导 — 3-4 处
- `views/NodeListView.vue:246`(retry:border tool-error、透明底)
- `components/chat/primitives/DiffPrimitive.vue:274-299`、`CodeBlockPrimitive.vue:82-89`、`ButtonPrimitive.vue:147-158`(ui-prim 家族,Out of Scope)

### H. 其他特殊底(特例)
`RequestModeChangeCard.vue:590`(plan = tool-read 实底)、`WorkerMergeControls.vue:323`(merge hover = tool-write 12%)、`PluginSelect.vue:485`(toggle-pill 开关)、`SessionList.vue:1036`(palette-dot)、`NodeListView.vue:271`(node-card 卡片按钮)、`PendingBadge.vue:87`(accent 18% 药丸)、`SearchModal.vue:562`(chip--active accent 16%)、`ReviewMatrix.vue:233`(tab--active bg-selected)、`MessageItemEdit.vue:301/306`(save hover = accent 85% 混黑、disabled = accent 50%)

## 2. 尺寸阶梯

**padding 分布**(48 种写法):`0` ×22、`4px`(icon) ×14、`5px 12px` ×8、`6px 10px`/`12px 16px` 各 ×7、`4px 8px`/`2px 8px`/`10px 16px` 各 ×6、`4px 12px` ×4、`6px 8px`/`6px 12px`/`2px 10px` 各 ×3、其余孤值(`9px 16px`、`11px 22px`、`2px 9px`、`0 1px` 等)各 1-2。
奇数 padding(5px/9px/11px)共 12 处。

**font-size**(92 处):text-xs 38 / text-sm 37(82%)/ text-base 7 / text-md 4 / text-2xs 3;裸值:`11px` ×2(DiffPrimitive.vue:283、CodeBlockPrimitive.vue:89)、`10px`(ChatInput.vue:926)、`13px`(ProjectTabs.vue:220)。

## 3. 形状(按钮作用域 ~103 处 radius)

- `var(--radius-sm)` 73 处(~71%);`var(--radius-md)` 真按钮仅 1 个(`SessionList.vue:961` .session-more__btn)
- `50%` 圆形真按钮 5 个:`ChatInput.vue:1152`(send/stop)、`ChatInput.vue:922`(staged-remove)、`MessageList.vue:383`(scroll-to-bottom)、`ChecklistCard.vue:445`(ball)、`SessionList.vue:1039`(palette-dot)
- `999px` 药丸真按钮 4 个:`PendingBadge.vue:86`、`WorkerAskBanner.vue:95`、`SubagentDrawer.vue:943`(jump-latest)、`SearchModal.vue:551`(chip)
- 裸值:`3px` ×3(ProjectTabs.vue:302、ReviewMatrix.vue:191、Sidebar.vue:361)、`4px` ×1(ProjectTabs.vue:386)
- **修正:ChatInputTokenUsage.vue:825 的 `border-radius: 1px` 在 `.chat-input__token-bar-col`(token 进度条分段)上,非按钮**

## 4. 热点文件 TOP10(65/167 = 39%)

ProvidersTab(8)、SearchModal(8)、RuntimeMemoryModal(7)、WorktreeChip(7)、PermissionAskBody(6)、ChatPanel(6)、Sidebar(5)、AuditLogModal(5)、SessionList(5)、ReviewMatrix/GroupChatConfigModal/ProjectTabs(4 并列)。

## 5. disabled 处理

`:disabled` 引用 163 行;样式块 98 个:`cursor: not-allowed` 29 处、opacity 34 处且 5 种值(0.5 ×20、0.6 ×10、0.4 ×2、0.7 ×1、0.55 ×1)。

## 6. transition 惯例

按钮作用域 69 处;主流 `background + color + border-color`(+duration/ease token)×15;裸值漂移仅 ProjectTabs ×4(`0.1s`,220/311/337/390)、SearchHistoryCard:302(`0.15s ease`);CodeBlockPrimitive `.ui-prim__copy` 无 transition。

## 7. 类名碰撞

`class="btn"` / `.btn {` 零命中;.btn 家族可安全引入。ButtonPrimitive.vue 为生成式 UI 渲染器(同名不同物)。

## 8. 孤值/漂移清单(优先级序)

1. 未定义 token:`var(--color-bg-overlay)` ×2 —— PluginSelect.vue:473/559(@theme 无此变量,解析为无效)
2. 裸 rgba 红 ×3:ProvidersTab.vue:682、ModelRow.vue:261、DeleteModelConfirm.vue:113
3. 按钮上裸 radius:`3px` ×3 + `4px` ×1(见 §3)
4. 按钮上裸 font-size:`11px` ×2、`10px`、`13px`(见 §2)
5. fallback 噪音:`var(--radius-sm, 4px)` ×2(DiffPrimitive.vue:280、CodeBlockPrimitive.vue:86)
6. 裸 transition 秒数:ProjectTabs ×4、SearchHistoryCard:302
7. 奇数 padding 12 处(见 §2)
8. 特殊 hover 混色:MessageItemEdit.vue:301(accent 85%+#000)、ChatInput.vue:1205(tool-error 80%+#000)、MemoryPreview.vue:857 系 12% vs 其他 10%
9. 裸 box-shadow(FAB 特例,design-tokens.md 特例表已收录保留):SubagentDrawer.vue:948、MessageList.vue:391、ChecklistCard.vue:453

## 家族归一结论

6 variant + 3 size + 2 shape 覆盖 ~92% 规则;特例 6-8 条保留(F 节/H 节)。
TOP10(39%)为第一批高收益对象;单文件多按钮共享基类(ProvidersTab 5 变体、
ModelForm 3 变体、ConfirmDialog 3 变体)迁移收益最高。
