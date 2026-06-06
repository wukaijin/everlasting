# 修复 6 个 UI/状态 bug（顶栏窗口控制 + Markdown 表格 + SSE 流同步架构）

## Goal

修复 `docs/prompt.md` 收录的 6 个 bug。前 5 个是 UI 层面的小修（窗口按钮、图标、边距、Markdown 样式），bug 6 是**前端 SSE 状态同步的架构性问题**，需要中等规模重构：抽出 `streamController` 单例、让 controller 成为 messages 的唯一真相源、per-session 独立流、LRU 缓存防内存无界。

## 现状（已读代码确认）

| 文件 | 关键点 |
|---|---|
| `app/src/components/layout/TitleBar.vue` | 自定义顶栏，`decorations: false`；`onToggleMaximize` 用 `setSize` + `setPosition` 走 `currentMonitor().size / scaleFactor` 试图填满整个显示器（4K 下行为异常 → 1920×1080）；最小化图标用了 `ellipsis`（错误） |
| `app/src/components/Icon.vue` | heroicons 24/outline 注册表，已有 `maximize`/`restore`/`x`，缺 `minus` |
| `app/src/stores/chat.ts` | 单 store 持有 `messages.value`（当前 session 单一）+ `streamingSessionId` + `sending` + `currentRequestId` + `streamingProjectIds` Set。`shouldApplyEvent` 过滤非当前 session 事件，导致切走期间 `done` 被丢、state 卡住。`switchSession` 调 `load_session` IPC，rehydrate 后**整体覆盖** `messages.value`，丢失正在流的 message |
| `app/src/components/chat/MessageItem.vue:354-364` | Markdown 表格 CSS：`border: 1px solid var(--color-bg-border)`，`#1E2530` 跟气泡底色 `#1A2030` 只差 4 个亮度单位，几乎看不见 |
| `app/src/style.css` | `--color-bg-border: #1E2530`、`--color-text-muted: #64748B`、`--color-text-secondary: #8B95A7` |
| `app/src-tauri/tauri.conf.json` | `decorations: false`、`titleBarStyle: "Overlay"`、默认 1440×900、`minWidth 1280`/`minHeight 800` |

## 关键设计决策（grill-me 收敛结果）

### Bug 6 架构

1. **范围**：中度重构。**不**做"流式框架"那种彻底重写（YAGNI：本机 IPC 不会断、单窗口无跨 tab、无多端需求）。
2. **真相源**：`streamController`（新文件 `app/src/stores/streamController.ts`，Pinia store 实现）持 `Map<sessionId, ChatMessage[]>`，是 messages 的唯一来源。`useChatStore` 改为**只代理** controller 暴露的 current-session messages。
3. **并发**：per-session 独立——一个 session 在流时不影响其他 session 的输入。
4. **缓存策略**：LRU 驱逐，**上限 20 个 session**（常量）。**Streaming 中的 session 永驻 LRU**（不能被驱逐，否则 bug 重现）。超过 20 时驱逐最久未访问的非流中 session；被驱逐的 session 再次访问时由 controller 透明从 DB 重读。
5. **Session card 订阅**：`streamController` 暴露 `streamingSessionIds: Ref<Set<string>>`，`SessionList.vue` 订阅此 Set，在匹配的 session card 上叠加流状态指示器。
6. **Listener 模型**：保持单一全局 SSE listener（`listen<ChatEventPayload>("chat-event", ...)`），由 controller 持有，event handler 在 controller 内部——不经过 `shouldApplyEvent` 过滤。`done`/`error` 事件始终会清理该 request 的状态（不依赖 current session）。
7. **取消**：per-session。`chatStore.cancel()` 取**当前 session 的 active requestId**，调 `controller.cancel(requestId)`，转 `invoke("cancel_chat", ...)`。

### Bug 1-5（简单 UI 修复）

- **Bug 1+2（maximize 按钮 + 4K 尺寸）**：用 `PhysicalSize` / `PhysicalPosition` 直接吃 `monitor.size` / `monitor.position`（物理像素），避免 `scaleFactor` 在 WSLg 上报错的踩坑。逻辑放在 controller 内一个独立函数 `applyMaximizeBounds(monitor, fill)`，让"fill work area"和"fill monitor"两种策略可切换（默认 fill monitor）。
- **Bug 3（图标）**：`Icon.vue` 注册表新增 `"minus": MinusIcon`（heroicons）；`TitleBar.vue` 最小化按钮从 `ellipsis` 改为 `minus`。
- **Bug 4（logo margin）**：`.titlebar__logo` 加 `padding-right: 12px`。
- **Bug 5（Markdown 表格边框）**：在 `style.css` 加新变量 `--color-bg-border-strong: #3B475A`（取自 text-muted/secondary 之间的中间值），表格 `td`/`th` 改用此变量，cell border 看得清。

## Requirements

### Bug 1: 最大化按钮响应
- 4K 显示器（2880×1920，scaleFactor=1.5）点最大化，窗口填满**整个显示器**（包括任务栏区域），尺寸 ≈ 2880×1920 物理像素 / 1920×1280 逻辑像素
- 再次点击还原回 1440×900 居中
- 手动拖动窗口到接近 monitor 尺寸时，最大化按钮的图标应自动从 `maximize` 切到 `restore`（`isMaximized` 状态同步）

### Bug 2: 最大化尺寸不贴左上角
- 上同——不要回退到 `toggleMaximize()` 的 work-area 上限
- 在 WSLg + Windows native 上行为一致

### Bug 3: 最小化图标
- 最小化按钮显示**水平短线**（heroicons `MinusIcon`），不是三点
- 视觉对比 Windows 11 标准：单条短横线在按钮中心

### Bug 4: 顶栏 logo 右边距
- logo 右侧有 12px 空白，再接项目 tabs
- 不影响现有 logo 左 8px padding / 32px cell 宽度

### Bug 5: Markdown 表格边框
- 表格 `td`/`th` border 颜色明显（dark mode 下至少 2:1 对比比），不"几乎看不见"
- 跟现有 `--color-bg-border` 不冲突（普通边框仍用原色）

### Bug 6: SSE 流与前端状态同步
- **症状 1**：stream 期间切换 session 再切回，正在流的消息**完整保留**（包括 streaming 光标、已积累的文本、tool call 卡片）
- **症状 2**：stream 期间切换 session，**顶栏项目红点**持续显示直到该 session 流真正结束（不因切走而消失，也不卡住）
- **症状 3**：stream 期间切换 session，**当前 session 卡片**显示流状态指示器（旋转点 / 脉冲）
- **症状 4**：stream 期间切换 session 再切回，输入框 / 打断按钮的状态随该 session 是否在流而正确（不是全局只有一个 `sending`）
- **症状 5**：切到不同 session 后 A 流**自然结束**（或被用户点 cancel），前端状态正确归零（sending/currentRequestId/red dot 全清），不被"事件被 shouldApplyEvent 丢"卡住
- **症状 6**：LRU 驱逐后，访问被驱逐的 session 透明从 DB 重读（用户无感）
- **症状 7**：streaming 中的 session 永驻 LRU（保证不因切换频次被驱逐）

## Acceptance Criteria

### Bug 1-5
- [ ] AC1.1：在 WSLg + 4K 显示器下点最大化，窗口填满整个显示器（实测尺寸 ≥ 2880×1920 物理像素）
- [ ] AC1.2：再点一次，窗口回到 1440×900 居中
- [ ] AC1.3：手动拖窗口到接近显示器尺寸，最大化按钮图标自动从 □ 切到 ↗
- [ ] AC3.1：最小化按钮显示水平短线图标（视觉确认）
- [ ] AC4.1：顶栏 logo 与项目 tabs 间有 12px 空白（视觉确认）
- [ ] AC5.1：dark mode 下渲染包含表格的 LLM 输出，单元格边框清晰可辨（视觉确认）
- [ ] AC5.2：light mode 下表格边框依然清晰（不应只针对 dark mode 调）

### Bug 6
- [ ] AC6.1：在 session A 发消息开始流 → 切到 session B → 再切回 A：A 的流消息完整，文本持续累加，streaming 光标在
- [ ] AC6.2：在 session A 发消息开始流 → 切到 session B：A 顶栏项目红点持续；切回 A：A session card 显示流状态指示器；B 的 session card 不显示
- [ ] AC6.3：在 session A 发消息开始流 → 切到 B → 等 A 自然结束：A 顶栏项目红点消失；切回 A：消息已结束，streaming=false，光标消失
- [ ] AC6.4：在 session A 发消息开始流 → 切到 B → 在 B 发消息也开始流（**两个 session 同时在流**）→ 两边输入框都显示打断按钮；点 A 的打断只停 A，B 继续
- [ ] AC6.5：访问 25 个 session（其中 3 个在流），验证最久未访问的 5 个被驱逐，访问它们时能正常加载；3 个流中 session 全部保留
- [ ] AC6.6：手动 reload（关窗口再开）后，所有 session 状态从 DB 还原（in-memory LRU 自然清空，无报错）

## Definition of Done

- [ ] `pnpm tauri build` 通过（type-check + build + 编译）
- [ ] `cargo test` 通过（不影响 Rust，但确认没破坏后端契约）
- [ ] 新增单元测试：`streamController` LRU 驱逐逻辑 + streaming 永驻逻辑
- [ ] 没有 console warning/error（新增代码）
- [ ] 至少在 WSLg 上手测过 AC1-5 全部 + AC6.1-6.4
- [ ] 文档：若 `app/src/stores/chat.ts` 的接口有破坏性变化，在 `.trellis/workspace/carlos/journal-1.md` 记一行

## Out of Scope（明确不做）

- 跨 tab / 多窗口同步（当前是单窗口 Tauri app）
- 自动重连 / SSE 断线重试（本地 IPC 不会断）
- 客户端持久化消息缓存（DB 已经持久化，in-memory 只为快速切回）
- "任务管理器" UI 列出所有正在流的 session（红点 + session card 指示器够用）
- 后端 agent loop 改动（前端 bug，后端契约不动）
- Tauri 配置改动（不动 `tauri.conf.json`，最大化逻辑在前端跑）
- 关闭主窗口时的 "正在流，提示用户" 弹窗（单独的 backlog 项）

## 风险与权衡

| 风险 | 缓解 |
|---|---|
| `streamController` 重构影响范围大（4-6 文件） | PR 拆分：先抽 controller + 改 store 接口（不接新功能）→ 再加 LRU + session card 订阅 |
| LRU 命中行为难测 | 单元测试：mock messages 数据，验证 21st 访问触发驱逐、streaming session 不被驱逐 |
| 物理 vs 逻辑像素在 Tauri 2 上文档不清 | 写注释引用 Tauri 2 docs；如行为不符合预期，备选方案是退回 `toggleMaximize()` 让用户接受 work-area 限制 |
| 现有 `useChatStore.sending` 的所有引用（ChatInput 等）要改成 `isCurrentSessionStreaming` | 全局 grep 替换，影响 2-3 文件；改名前后类型一致（都是 `Ref<boolean>`） |
| DB 加载延迟（evicted 后）让 session 切换出现瞬时空白 | 暂不处理（实测本地 SQLite < 50ms，dev 阶段可接受）。可作为未来增强 |

## Technical Approach 概览

### Bug 6 文件改动

**新增**：
- `app/src/stores/streamController.ts` — Pinia store 实现，单例。持 `Map<sessionId, ChatMessage[]>`（LRU 包装）、`activeRequests: Map<requestId, RequestState>`、`streamingSessionIds: Ref<Set<string>>`、全局 listener 注册。导出 `messagesFor(sessionId): ChatMessage[]`、`send({sessionId, text, history})`、`cancel(requestId)`、`ensureLoaded(sessionId)`（透明从 DB 读）、`evict(sessionId)`。
- `app/src/utils/lru.ts` — 通用 LRU Map 工具（get/touch/evict），独立可测。
- `app/src/utils/lru.test.ts` — vitest 单元测试（虽然项目目前没装 vitest，需确认是否要新加；备选：放 `app/src/utils/lru.test-d.ts` 作 type-level test）。

**修改**：
- `app/src/stores/chat.ts` — 删除 `messages`、`streamingSessionId`、`lastStreamedProjectId`、`streamingProjectIds`（迁移到 controller）、`shouldApplyEvent`、`handleChatEvent`、`handleToolCall`、`handleToolResult`、`clearStreamingSession`、`send`、`cancel`（这些全部委托给 controller）。保留 `sessions`/`currentSessionId`/`currentCwd`/`simplifiedCwd` 等 UI state。`switchSession` 改为调 `controller.ensureLoaded(id)`。
- `app/src/components/chat/ChatPanel.vue` — `chatStore.sending` 改名为 `isCurrentSessionStreaming`（computed from controller）。
- `app/src/components/chat/ChatInput.vue` — 同上。
- `app/src/components/layout/SessionList.vue`（或 `Sidebar.vue`）— 订阅 `streamController.streamingSessionIds`，在 card 上叠加指示器。
- `app/src/components/layout/ProjectTabs.vue`（或 `AppHeader.vue`）— 红点逻辑改读 `streamController.streamingProjectIds`（由 controller 维护，不再是 chat store）。
- `app/src/App.vue`（或合适位置）— `onMounted` 时 `streamController.start()`（注册全局 listener）。

### Bug 1-2 文件改动
- `app/src/components/layout/TitleBar.vue` — `onToggleMaximize` 重写：用 `PhysicalSize(monitor.size.width, monitor.size.height)` + `PhysicalPosition(monitor.position.x, monitor.position.y)`，移除 scaleFactor 换算。`syncMaximizedState` 同步改用物理像素比较。

### Bug 3 文件改动
- `app/src/components/Icon.vue` — 引入 `MinusIcon`（heroicons），注册 `"minus": MinusIcon`。
- `app/src/components/layout/TitleBar.vue` — 最小化按钮 `<Icon name="ellipsis">` → `<Icon name="minus">`。

### Bug 4 文件改动
- `app/src/components/layout/TitleBar.vue:317-325` — `.titlebar__logo` 加 `padding-right: 12px`。

### Bug 5 文件改动
- `app/src/style.css` — 在 `:root` 加 `--color-bg-border-strong: #3B475A`。
- `app/src/components/chat/MessageItem.vue:354-364` — 表格 `td`/`th` border 颜色改用 `var(--color-bg-border-strong)`。

## Implementation Plan（建议的 PR 拆分）

1. **PR1：UI 修（bug 1-5）**
   - 修改 4 文件，~80 行 diff
   - 独立可测，无架构依赖
   - 提交后用户先验收 UI

2. **PR2：抽出 streamController（脚手架）**
   - 新增 `streamController.ts` + `lru.ts` + 单元测试
   - 暂不接 chat store，先在 `App.vue` 跑通"创建 → 加载 → LRU 驱逐"逻辑
   - ~250 行

3. **PR3：chat store 切到 controller（行为迁移）**
   - 改 `chat.ts` 委托给 controller
   - 改 `ChatPanel.vue` / `ChatInput.vue` 用新 ref
   - 改 `ProjectTabs.vue` 红点逻辑
   - 跑通所有 AC6.1-6.6
   - ~200 行

4. **PR4：Session card 流状态指示器**
   - `SessionList.vue` 订阅 `streamingSessionIds`
   - 加新 CSS 类（旋转点 / 脉冲）
   - ~50 行

每个 PR 独立可测、独立可合（不卡后续）。

## Progress so far（2026-06-07 中段记录）

### ✅ 已修复（build + 测试通过）

- **Bug 3（最小化图标）**：`Icon.vue` 注册 `MinusIcon`，最小化按钮改用 `minus` 图标
- **Bug 4（logo margin）**：`.titlebar__logo` 加 `padding-right: 12px`
- **Bug 5（表格边框）**：`style.css` 新增 `--color-bg-border-strong: #3B475A`；`MessageItem.vue` 表格 `td`/`th` 改用此变量
- **Bug 1 部分（maximize 尺寸）**：在用户 host 主屏上尺寸现在正确（1920×1080）
- **PR2（streamController 脚手架）**：`streamController.ts` Pinia store + `lru.ts` + `lru.test.ts` 12 测试 + `App.vue` 启动钩子
- **Tauri capabilities 补全**：`core:window:allow-set-size` / `set-position` / `outer-size` / `outer-position` / `current-monitor` / `primary-monitor` / `available-monitors` / `scale-factor` / `cursor-position` + `core:event:allow-listen` / `allow-unlisten`

### ⚠️ 部分修复（size 修好，position 仍不对）

- **Bug 1+2（maximize 位置）**：用户环境是 RDP 双显示器（RDP 虚拟桌面 (0,0) 2560×1440 + host 主屏 (2560, 245) 1920×1080），窗口拖到 host 主屏后点最大化：
  - size 正确 → 1920×1080 ✓
  - position 错 → 视觉上向右扩大，**没有贴到 host 主屏左上角 (2560, 245)**
  - 试过方案：
    1. `setSize(PhysicalSize)` + `setPosition(PhysicalPosition(0,0))` → 选最大 monitor
    2. `setPosition` + `setSize` 顺序倒过来（先定位再 resize）
    3. `monitorAtCursor()` 读 cursorPosition 找 monitor → cursor 在窗口内 = 在 RDP 虚拟桌面，无效
    4. `monitorAtWindow()` = `currentMonitor()` → 拿到正确 monitor，但 setSize/setPosition 时序问题
  - 仍未解决。**下一步候选**：`setFullscreen(true)`（OS 强制全屏，绕过 setSize/setPosition 时序问题，但失去 maximize 语义，title bar 隐藏）
  - 已留诊断 `console.log` 等待进一步数据；用户测试后反馈再决定

### ❌ 未开始

- **PR3（chat store 迁移）**：依赖 streamController 已就绪 + bug 1 收尾后做
- **PR4（Session card 流状态指示器）**：依赖 PR3
- **Bug 6 实际体验验证**：controller 是 scaffold，chat.ts 还没切过去

### 用户原报告 "4K 2880×1920" 是误记

实际环境是 RDP session + 1920×1080 host 主屏。`docs/prompt.md` 和 prd.md 早期 "4K" 描述需要更正。

### 工作树状态

未 commit。`app/src/components/layout/TitleBar.vue` / `Icon.vue` / `MessageItem.vue` / `style.css` / `App.vue` / `app/src/stores/streamController.ts` / `app/src/utils/lru.ts` / `app/src/utils/lru.test.ts` / `app/src-tauri/capabilities/default.json` 都有改动。
