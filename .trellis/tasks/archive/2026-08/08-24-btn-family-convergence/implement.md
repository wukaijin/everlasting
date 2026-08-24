# Implement — Button CSS 家族收敛

> 执行顺序对应 design.md「批次与验证策略」;每 WP 一个 commit,commit 前
> 跑当批验证。全程在 `app/` 下操作,样式主战场 `app/src/style.css` + 69 个组件。

## 顺序清单

### WP1 家族落地(纯增量)

- [ ] style.css 新增 `.btn` 家族区(基类 + 7 variant + 2 size + pill/circle/icon
      修饰),按 design.md API 契约与注释体例;放在 focus-visible 基线之后
- [ ] `.trellis/spec/frontend/design-tokens.md` 回写「Button family」节
      (照 z-index ladder 一节体例:表 + 微特例说明 + grep 审计命令)
- [ ] 验证:pnpm test + vue-tsc 零错(零迁移,应全绿)

### WP2 modal/confirm 簇(12 文件)

- [ ] ConfirmDialog(danger/secondary/warning 三变体)、YoloConfirmModal、
      DeleteWorktreeConfirm、DeleteModelConfirm(裸 rgba 红 ×1 在此)
- [ ] RuntimeMemoryModal(danger-soft + refresh)、GroupChatConfigModal、
      MemoryModal、DiffModal、AuditLogModal、MarkdownDetailModal、
      SettingsModal、PermissionGrantsModal
- [ ] 验证:test + vue-tsc + ui-review.sh --screenshots-only(modal 弹窗截图)

### WP3 settings 簇(6 文件)

- [ ] ModelForm(--primary/--secondary 三变体)、ProvidersTab(8 按钮 5 变体,
      裸 rgba 红 ×1)、ModelsTab、RemoteTab、ModelRow(裸 rgba ×1)
- [ ] PluginSelect:**修 `--color-bg-overlay` 失效引用 ×2**(473/559 行,
      按视觉意图换 `--color-bg-elevated`,迁移前先截图确认现状渲染)
- [ ] 验证:同 WP2

### WP4 chat 簇(17 文件)

- [ ] ChatPanel 四兄弟(memory/audit/grants/trace:muted 本体 + accent-muted hover)
      + ChatInput(send/stop `--circle`,呼吸动画保留本地 + staged-remove)
- [ ] AskUserQuestionCard、PermissionAskBody(--always primary)、
      RequestModeChangeCard(--yolo danger / plan=tool-read 特例注释)、
      RequestTaskStateTransitionCard
- [ ] MessageItemEdit、MessageItemFooter(danger-soft)、ToolCallCard、
      EditFileCard、WorktreeChip(tint)、ChecklistCard(ball `--circle`)
- [ ] SubagentDrawer(jump-latest `--pill` + FAB 阴影特例保留)、MessageList
      (scroll-to-bottom `--circle` + FAB 阴影特例)、SearchHistoryCard
      (裸 transition ×1)、WorkerAskBanner / PendingBadge(`--pill`)
- [ ] 验证:同 WP2(重点:ChatInput send/stop 单独截图)

### WP5 布局导航簇(17 文件)

- [ ] Sidebar、TitleBar(close hover 红实底 → ghost + 本地覆写 + 注释)、
      ProjectTabs(裸 radius ×2 / 裸 font ×1 / 裸 transition ×4 / tab close
      hover 红实度)、SessionList(delete hover 红实底 + palette-dot 特例保留)
- [ ] ModeSelect、ModelSelect、TriggerMenu、HiddenProjectsMenu、SearchModal
      (8 按钮 + chip `--pill`)、ReviewMatrix(裸 radius ×1)、TracePanel、
      DrawerSection、DrawerPromptCard、NodeListView(retry outline 红边特例
      + node-card 特例保留)、PairingView(lg CTA)、EmptyProjectState
      (lg CTA + --ease-spring press 特例保留)
- [ ] 验证:同 WP2

### WP6 长尾 + 债清扫 + 终盘审计

- [ ] 剩余零散文件迁完(对照盘点 69 文件清单逐一勾销)
- [ ] 债清扫:disabled opacity 五档归一(0.4/0.55/0.6/0.7 → 0.5)、
      裸 rgba 红(DiffPrimitive 家族外若还有)、`var(--radius-sm, 4px)`
      fallback 噪音 ×2、裸 font-size/radius/transition 清零
- [ ] 终盘 grep 审计(复跑盘点命令,证据存 research/):
      按钮作用域内 `background:`/`padding:`/`border-radius:`/`font-size:`/
      `transition:` 的非家族残留 = F 节特例清单且各有注释
- [ ] 全量 `scripts/ui-review.sh`(VLM 评审)+ 前后对照图存 research/
- [ ] 键盘 Tab 抽查:迁移后按钮 focus-visible 环仍在(headless 探针或手动)

## 验证命令速查

```bash
cd app && pnpm test                      # 1175+ 前端测试
cd app && pnpm build                     # vue-tsc + vite
scripts/ui-review.sh --screenshots-only  # 批间:省 VLM quota
scripts/ui-review.sh                     # WP6:全量 VLM 评审
# 终盘审计(按钮作用域残留,示例):
cd app/src && grep -rn "background: var(--color-accent)" --include="*.vue" . \
  | grep -v "\.btn"   # 期望:仅特例清单命中
```

## 风险文件(重点盯)

| 文件 | 风险 |
|---|---|
| `components/chat/ChatInput.vue` | send/stop 圆形 + 呼吸动画与形状耦合;staged-remove |
| `components/SessionList.vue` | 样式最多;delete hover 红实底特例;palette-dot 特例 |
| `components/settings/ProvidersTab.vue` | 8 按钮 5 变体 + 裸 rgba |
| `components/chat/ChatPanel.vue` | 四兄弟 muted→tint hover 语义归类 |
| `views/PairingView.vue` / `components/chat/EmptyProjectState.vue` | lg CTA + spring press 特例 |

## 回滚点

- 每 WP 独立 commit;任何批次视觉不可接受,单 revert 该 WP。
- WP1 纯增量可独立回滚(无消费方时零影响)。

## 完成前置(finish-work 检查)

- [ ] prd.md AC1-AC4 全过,证据(grep 输出 / 截图对照 / 测试结果)存 research/
- [ ] design-tokens.md Button family 节与 style.css 实现一致
- [ ] 归档前把「ui-prim 生成式 UI 家族是否消费 .btn」写回 BACKLOG 附录 A 候选
