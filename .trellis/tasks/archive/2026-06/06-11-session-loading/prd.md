# 体验优化: session 记忆 / 滚动 / 删除确认 / loading

> **状态**: F1-F4 已实施，F5（耗时统计）延后到下一个 session。

## Goal

一轮体验打磨，覆盖 4 个 UX 改进点，提升日常使用流畅度。F5（LLM 耗时统计）因涉及 Rust DB migration 工作量较大，延后单独实施。

## Requirements

### F1: 切换项目默认选中上一次 session

- per-project `lastActiveSessionId` 持久化到 localStorage
- 键格式: `everlasting.lastSession_{projectId}`
- 切换项目时，先从 localStorage 读取 lastActiveSessionId，若存在于 session 列表则选中，否则 fallback 到 sessions[0]（当前行为）
- 参照现有 `lastActiveProjectId` 模式（config.ts）
- 涉及: chat.ts `onProjectChange()`, config.ts

### F2: 发送消息后消息区域滚动到底部

**问题**: 发送后只滚一次（差一帧高度），流式回复过程中不再滚动
**方案**:
- 发送消息后进入"跟踪模式"（forceFollowBottom ref），全程滚底
- 用户主动上翻超过 80px 停止自动滚（"用户主动看历史 = 别打扰"）
- stream done 后重置跟踪模式
- 发送时在 nextTick 后额外 scroll 一次，覆盖用户消息 + assistant 占位不在同一帧的问题
- 涉及: MessageList.vue, chat.ts `send()`

### F3: 删除 session 二次确认

**问题**: 原生 `confirm()` 在 Tauri webview 中不可靠（静默吞掉）
**方案**:
- 从 `DeleteWorktreeConfirm.vue` 提取通用 `ConfirmDialog.vue` 组件
- Props: `open`, `title`, `body`（slot 或 string）, `confirmText`, `variant: 'danger' | 'warning'`
- 保留键盘处理（Escape 关闭）、聚焦管理、过渡动画
- 有消息的 session 弹确认，空 session 直接删
- 替换 SessionList.vue 中的原生 `confirm()`（onDelete + contextDelete 两处）
- 顺便改造 `DeleteWorktreeConfirm` 和 `DeleteModelConfirm` 使用通用组件（可选，不改也行）
- 涉及: 新建 `components/common/ConfirmDialog.vue`, 改造 SessionList.vue

### F4: Chat panel loading + 切换 session 时序修复

**问题**: 切换 session 无 loading 反馈；`reloadAfterFinalize` 和 `switchSession` 双 IPC 导致位置抖动
**方案**:
- 切换 session 时在消息区中央显示小 spinner
- 引入 `sessionLoading` ref（chat.ts），switchSession 开始设 true，ensureLoaded 完成后设 false
- 修 `switchSession()` 双 IPC: 合并 ensureLoaded + token usage 的 load_session 为一次 IPC
- 修 `reloadAfterFinalize()` 位置抖动: 替换消息前后记录 scroll 位置，替换后恢复
- 涉及: ChatPanel.vue, chat.ts `switchSession()`, streamController.ts `reloadAfterFinalize()`, MessageList.vue

### F5: LLM 消息耗时统计

**计时指标**:
- TTFB: 发送请求到收到首 token（delta start）
- 生成耗时: 首 token 到 done
- 端到端耗时: 发送到 done
- 单 tool call 耗时: tool:call 事件到 tool:result 事件

**展示位置**:
- 每条 assistant 消息右上角或底部: 该轮总耗时（如 "3.2s"，hover 展示 TTFB/生成/端到端明细）
- 每个 ToolCallCard: 单独耗时（如 "0.3s"，在 status text 旁）
- Session 底部: 累计总耗时

**持久化**:
- 前端计时用 `Date.now()` 记录时间戳
- 持久化到 SQLite: messages 表加 `ttfb_ms` / `gen_ms` / `total_ms` 可空字段（assistant 消息专用）
- tool call 耗时在 tool_results 表加 `duration_ms` 可空字段
- Rust migration: ALTER TABLE ADD COLUMN
- reloadAfterFinalize 后从 DB 恢复耗时数据
- 涉及: streamController.ts, chat.ts, MessageList.vue, ChatPanel.vue, ToolCallCard.vue, Rust DB migration

## Acceptance Criteria

### F1
- [ ] 切换到项目 A，选中 session X，切到项目 B，再切回 A → 自动选中 session X
- [ ] 项目 A 的 lastActiveSession 被删除后 → fallback 到 sessions[0]
- [ ] 首次使用（无 localStorage）→ 行为与当前一致（选 sessions[0]）

### F2
- [ ] 发送消息后，消息区域滚动到底部，assistant 占位消息可见
- [ ] 流式回复过程中持续滚底
- [ ] 用户上翻 >80px 后停止自动滚
- [ ] Stream done 后恢复"靠近底部才滚"的正常模式

### F3
- [ ] 删除有消息的 session → 弹出确认弹窗
- [ ] 删除空 session → 直接删除，不弹窗
- [ ] Escape 关闭弹窗，Enter 确认
- [ ] 弹窗风格与 app 一致（非系统原生对话框）

### F4
- [ ] 切换到冷 session（未缓存）→ 消息区短暂显示 spinner
- [ ] 切换到热 session（LRU 缓存命中）→ spinner 不可见或一闪而过
- [ ] 流式完成后 reloadAfterFinalize 不再导致滚动位置跳
- [ ] switchSession 不再有双 IPC 的时序问题

### F5
- [ ] 发送消息后，assistant 消息显示总耗时
- [ ] Hover 耗时可看到 TTFB / 生成 / 端到端明细
- [ ] 每个 ToolCallCard 显示单独耗时
- [ ] Session 底部显示累计耗时
- [ ] 切换 session 再切回 → 耗时数据仍在（DB 持久化）
- [ ] 重启 app → 历史 session 的耗时数据可查看

## Definition of Done

- [ ] `vue-tsc --noEmit` 通过
- [ ] `pnpm build` 通过
- [ ] `cargo check` 通过（如涉及 Rust 改动）
- [ ] 手动验证 5 个功能的 golden path
- [ ] ROADMAP.md 更新（第一档已全部完成，标注这 5 项为路线图外完成）

## Technical Approach

### 实施顺序

1. **F3** — 通用 ConfirmDialog + session 删除确认（最独立，热身）
2. **F1** — lastActiveSession（小，独立）
3. **F2 + F4** — 滚动修复 + loading + 时序修复（关联紧密，一起做）
4. **F5** — 耗时统计（最大，压轴）

### 关键技术决策

| 决策 | 选择 | 理由 |
|------|------|------|
| lastActiveSession 存储 | localStorage | 与 lastActiveProjectId 模式统一 |
| 滚动策略 | 发送后强制跟底 + 上翻停止 | 发送意图 = 看回复，但尊重用户主动浏览 |
| 确认弹窗 | 通用 ConfirmDialog 组件 | 原生 confirm() 在 Tauri 不可靠，已有两个可提取参考 |
| Loading 形式 | 小 spinner 在消息区中央 | 命中缓存时几乎不可见，冷加载有反馈 |
| 耗时数据存储 | SQLite ALTER TABLE | 持久化，重启可查，与现有 messages 表一体 |
| 前端计时方式 | Date.now() | 够用，不需要 performance.now() 微秒精度 |

### F3 ConfirmDialog 设计

从 `DeleteWorktreeConfirm.vue` 提取核心结构：
- Props: `open`, `title: string`, `variant: 'danger' | 'warning' | 'default'`, `confirmText?: string`
- Slot: `body`（自定义内容区）
- Emits: `confirm`, `cancel`
- 保留: 键盘处理、聚焦管理、过渡动画、CSS 变量风格

### F5 DB Schema

```sql
-- messages 表新增（仅 assistant 消息有值）
ALTER TABLE messages ADD COLUMN ttfb_ms INTEGER;
ALTER TABLE messages ADD COLUMN gen_ms INTEGER;
ALTER TABLE messages ADD COLUMN total_ms INTEGER;

-- tool_results 表新增
-- （如果没有独立表，考虑在 messages 的 tool_result content 中嵌入 duration_ms）
```

## Out of Scope

- F5（LLM 耗时统计）延后到下一个 session 单独实施——涉及 Rust DB migration + 前后端联动，工作量大
- 不改造 DeleteWorktreeConfirm / DeleteModelConfirm 使用通用组件（可后续做）
- 不做 token 速率（tokens/秒）统计
- 不做后端精确计时（如 Rust 侧 wall-clock）
- 不做耗时数据的导出/分析面板
- 不做 session 底部累计耗时的"按日/按周统计"

## Technical Notes

### 现有代码参考

- `config.ts` lines 65-84: lastActiveProjectId 的 readLastActive / writeLastActive 模式
- `DeleteWorktreeConfirm.vue`: 最完整的确认弹窗实现（header + body + footer + 键盘 + 动画）
- `DeleteModelConfirm.vue`: 较简化的确认弹窗（absolute overlay + card）
- `MessageList.vue` lines 57-77: 当前自动滚动逻辑（isNearBottom + watch）
- `MessageList.vue` lines 88-95: session 切换后 scrollToBottom（含 100ms retry）
- `streamController.ts` line 593: `reloadAfterFinalize()` — 流式完成后 DB 重载
- `chat.ts` line 459: `switchSession()` — 双 IPC（ensureLoaded + load_session）
- `ToolCallCard.vue`: 当前有 statusText（"running…"/"done"/"error"），耗时加在旁边
