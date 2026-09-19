<!-- Moved from memory-ui.md 2026-09-19 (doc-split) -->

---

## Design Decisions

### Decision: 复用 `renderMarkdown` 而不重新实现

**Context**: B5 memory 文件本质上是 markdown,前端需要安全地
渲染内容(包含代码块、列表、表格)。

**Decision**: 直接 `import { renderMarkdown } from "utils/markdown"`,
不重新发明 markdown pipeline。

**Consequences**:
- ✅ XSS 防护沿用 `markdown.test.ts` 锁定的 8 个 fixture
- ✅ 渲染选项(`gfm: true`, `breaks: true`)与 MessageItem
  一致,用户在 Settings 看到和在 chat 里看到的 markdown
  渲染一样
- ✅ 不增加新依赖
- ⚠️ Memory 文件不走 debounced renderer(只读、静态);`breaks:
  true` 行为仍然适用

### Decision: Memory content 走 `contentCache` 共享

**Context**: 多个 `<MemoryLayerItem>` 可能同时展开(理论上
 4 个都展开);用户在 Settings 和 ProjectTabs 之间切换
会重复打开同一组 layer(从不同 kind 视角)。

**Decision**: content cache 放在 `useMemoryStore().contentCache`
(Map<path, string>),所有 `MemoryLayerItem` 实例共享。

**Consequences**:
- ✅ 切到 layer A → fetch → 切到 B → fetch → 切回 A → 命中
- ✅ content cache 在 `fetchLayers` 时清空(防 stale),
  project 切换时也清空
- ⚠️ 如果 4 个文件都是 50KB,峰值内存是 200KB;可接受
- ⚠️ 用户清空 localStorage / 重启 app 后 cache 失效,下次
  打开重新 fetch — 这是预期行为

### Decision: 双入口(Settings + ProjectTabs)用同一个 `MemoryPreview` 组件

**Context**: PRD §8 锁定了双入口,但两个入口的
"侧重"不同 — Settings 关注 User,ProjectTabs 关注 Project。

**Decision**: 单一 `MemoryPreview` 组件,`kind` prop 控制
显示哪些 layer。`MemoryTab.vue` 是 thin wrapper(Settings
入口),ProjectTabs 直接 import `MemoryPreview`。

**Consequences**:
- ✅ 视觉、行为、状态管理 100% 一致
- ✅ 未来加 Session / Runtime layer(PR1 V2 2 期)只需
  在 `MemoryPreview` 加一个 `kind="session"` 分支
- ✅ MemoryTab 是纯 wrapper,容易 unit test
- ⚠️ 未来如果两个入口的 UX 大幅分歧(比如 Settings 要内嵌
  编辑器,ProjectTabs 不需要),需要拆 — 但本期 PRD 明确
  锁了"只读 + 跳外部",不分歧

### Decision: ~~Memory dropdown 走 hand-rolled popover,不用 reka-ui~~ (OBSOLETED 2026-06-11)

> **OBSOLETED 2026-06-11**:被 `06-11-memory-modal-appheader-entry`
> 替代,理由见下方新决策 "Memory entry 改为 AppHeader corner
> action + reka-ui Dialog modal"。本节保留作为决策日志。

**Context**: `.trellis/spec/frontend/popover-pattern.md` 锁定
项目用 hand-rolled popover(`onDocumentClick` + Esc close),
不用 reka-ui `DropdownMenu` / `Popover` 原因:(1) 视觉一致性
 (worktree dropdown 是参考);(2) 不引入新依赖路径;
(3) ~20 行 TS + CSS 就够。

**Decision**: ~~ProjectTabs 的 Memory dropdown 沿用
WorktreeChip / ModelSelect 的 hand-rolled 模式。~~

**为什么被推翻**:hand-rolled popover 的 `right: 0; min-width:
480px` 锚点策略只在 trigger 处于视窗最右端时安全。Memory
trigger 在 ProjectTabs 上的位置(项目 tab + add 按钮 之间)
意味着它经常在视窗中部 — popover 向左展开 480-600px 直接
溢出视窗左边界,文字被裁,与 sidebar 视觉重叠。viewport
collision detection 是 hand-rolled popover 没有的能力。

**Consequences (历史记录)**:
- ✅ 三个 popover 行为一致(都响应 onDocumentClick、Esc、
  不 stopPropagation)
- ✅ 视觉与 worktree dropdown 的 chip 风格对齐
- ❌ 横向溢出 bug(2026-06-11 用户截图证据)
- ❌ 语义混乱:Memory trigger 长得像 tab,误读为"Memory 项目"

### Decision: Memory entry 改为 ChatPanel header Brain 按钮 + reka-ui Dialog modal (2026-06-11)

**Context**: 上面的 hand-rolled popover 方案有横向溢出 bug +
语义混乱(详见上节"为什么被推翻")。需要一个不依赖 trigger
位置的承载方式,且让 Memory 与当前会话场景紧邻(memory 只
对"当前 chat 中的 LLM"有意义,放在 chat header 里语义最强)。

**Decision**: 把 Memory 入口从 ProjectTabs 上的 hand-rolled
popover 迁移到 ChatPanel header(WorktreeChip 右侧)的 Brain
图标按钮 + reka-ui Dialog modal。组件结构:

- `app/src/components/chat/ChatPanel.vue` — header row 内
  WorktreeChip 之后挂一个纯图标 button + MemoryModal,
  `memoryModalOpen` ref 控制开关
- `app/src/components/memory/MemoryModal.vue` — reka-ui
  `DialogRoot / DialogPortal / DialogOverlay / DialogContent /
  DialogClose` 五件套,内嵌 `<MemoryPreview kind="project">`
- Brain 图标来自新增依赖 `@lucide/vue@^1.17.0`(heroicons 无
  brain;CpuChip 不够精准)。Icon.vue 改造为支持 heroicons +
  lucide 混用,zero glue 必需。
  *(2026-09-02 superseded:全量迁移 lucide、`@heroicons/vue` 整链
  移除,Icon.vue 现 lucide-only,无混用态)*
- Modal 尺寸:`width: 80vw; min-width: 640px; max-width: 900px;
  max-height: 80vh`,内部 MemoryPreview 列表自滚

**为什么 ChatPanel header 而不是 AppHeader**:
- AppHeader 是项目无关的 chrome(window 控件 + 项目 tab 切换),
  Memory 是"当前会话 LLM 注入了哪些 memory"的查看面板,语义
  上挂在 chat 容器里比挂在窗口顶栏更对路
- ChatPanel header 已经承载 session 级的 chip(git branch、cwd、
  worktree),Memory 是同类"session 上下文摘要"信息,排在
  worktree chip 之后是自然延伸
- AppHeader 顶栏空间被 ProjectTabs 占据;在 macOS 上 80px 红
  绿灯 spacer + 项目 tab 后已经无 corner 空位

**Consequences**:
- ✅ 位置安全:reka-ui DialogPortal teleport 到 body,居中
  布局完全独立于 trigger 位置 — 不会有横向溢出 bug
- ✅ 视觉与 SettingsModal 统一(同样的 zoom + fade 动画曲线、
  同样的 z-index 层级 2000/2001、同样的 close button 风格)
- ✅ 语义清晰:Memory 与 session 上下文 chip 同行,不再混在
  项目 tab 列里
- ✅ 自带 a11y:focus trap / ESC / pointerdown-outside / aria-modal
  全由 reka-ui Dialog 提供,不需手写
- ⚠️ 新增 dependency `@lucide/vue` ~2KB tree-shake 后(只导
  Brain 一个图标)。如果未来需要更多 lucide 图标,Icon.vue 已
  备好混用通路。*(2026-09-02 superseded:lucide 已是唯一图标源,
  `@heroicons/vue` 依赖移除;tree-shake 收益随全量迁移放大)*
- ⚠️ `popover-pattern.md` "Don't: Use reka-ui Popover" 规则
  **仍然适用** — 它针对 popover/dropdown。Modal 走 reka-ui
  `Dialog*` 一直是项目惯例(SettingsModal 是参考)。两个规则
  互不冲突:**popover hand-rolled, modal reka-ui**。
- ⚠️ Settings → Memory tab 本期 **不动**(用户决策:留待下一
  轮"Memory 功能性重构");两个入口并存,语义已分流(modal
  = 快查,Settings tab = 管理台,后续重构会进一步区分)

---

## Common Mistakes

### Mistake: 监听 `memory:reloaded` 事件但没设防御性

后端 watcher **当前不 emit Tauri event**(PR1 没加 emit)。
如果在 store 里写 `await listen("memory:reloaded", ...)` 而
不检查 `unlistenReloaded !== null` 重入,会注册多个 listener
(每次 `loadForProject` 调一次),导致每次 emit 触发 N 次
`fetchLayers`。

**Fix**: 模块级 `let unlistenReloaded: UnlistenFn | null = null`,
`ensureReloadedListener` 守卫,跟 `projects.ts` 的
`unlistenRefresh` 一样的模式。

### Mistake: `PathBuf` 当成对象解析

Rust `PathBuf` 在 Tauri IPC 中序列化为 **string**,不是
`{ components: [...] }` 或 `OsString`。前端 interface 写
`path: { ... }` 会拿到 string,看起来"能跑"实际上读不到字段
(Vue template `{{ layer.path.display }}` 渲染 undefined,
但不报错 — silent data loss)。

**Fix**: `path: string`。

### Mistake: 切 project 时不关 Memory dropdown

用户在 project A 打开 Memory dropdown → 切到 project B
(`onTabClick` 调用 `store.switchProject(B)`) → Memory dropdown
仍打开,显示 project A 的 memory。30ms 后 `loadForProject(B)`
完成,UI 刷新成 project B 的 memory — 短暂闪烁 + 用户
困惑。

**Fix**: `onTabClick` 同时 `memoryMenuOpen.value = false`
(已经在 ProjectTabs.vue 里实现)。

### Mistake: 渲染 100KB markdown 不截断

EVERLASTING.md / AGENTS.md 上限是 100KB(PR1 的 `MAX_FILE_SIZE`),
marked + DOMPurify 解析 100KB markdown 可能要 1-2 秒 +
Panel 卡住等渲染。用户在 editor 保存 → 触发 reload → 下次
展开卡顿。

**Fix**: `MemoryLayerItem` 截断到 50KB(代码里 `MAX_BODY_CHARS
 = 50_000`),提示用户去外部编辑器看完整文件。

---

## Anti-Patterns

- **Don't** 用 `v-html="rawText"` 渲染 memory 内容 — XSS。
  一律走 `renderMarkdown`。
- **Don't** 在 `<MemoryLayerItem>` 里直接 `invoke` — 一律
  走 `useMemoryStore().fetchContent` 以利用 cache。
- **Don't** 给 memory 文件加内嵌编辑器 — PRD §8 锁定"只读
  + 跳外部编辑器"。任何"看起来更顺手"的内嵌 textarea 都
  是越界。
- **Don't** 用 reka-ui `DropdownMenu` / `Popover` 做新的 popover
  / dropdown — 沿用 hand-rolled pattern,见
  `popover-pattern.md`。**例外**:Modal 走 reka-ui `Dialog*`
  仍然是项目惯例(SettingsModal / MemoryModal 都是 reka-ui),
  这条规则不约束 modal 选型。
- **Don't** 把 `memory:reloaded` 当成必然事件 — 当前
  backend 不 emit,前端必须以"无 emit 也能正常工作"为
  baseline 设计。`refresh` 按钮是用户手动保险栓。
- **Don't** 把 Memory content 走 SSE 流式渲染 — 文件是
  静态的、已加载的,流式渲染没有任何价值;`renderMarkdown`
  一次解析就够。

---

## Future Work (Deferred from B5 V2 1 期)

| Item | Why deferred |
|------|-------------|
| `useMemory` tool (LLM 主动 read) | V2 2 期,本期不做(归 Runtime) |
| 后端 emit `memory:reloaded` 事件 | 当前 watcher 改 cache 但不 emit;前端防御性 listener 已经就位,backend 加 emit 是无感升级 |
| Session-level / Runtime-level memory | V2 2 期;`MemoryKind` 枚举已预留 4 个 variant |
| 内嵌 Markdown 编辑器 | PRD §8 锁定不做;`usePopover` composable 抽取是 OOS |
| `usePopover` 抽公共 composable | OOS,见 `popover-pattern.md` |
| Memory chip in sidebar (per-session usage) | 数据已经在 `SessionSummary` 里,但 UI 不在本期范围 |
| Token 估算迁移到 LLM 真实 token (claude-sonnet) | 当前 `cl100k_base` 估算足够;`/chat/completions` 真实值需要等 A5 `$ cost` 阶段 |
| Memory content 增量更新(diff) | OOS;用户点 "刷新" 走 `store.refresh()` 即可 |

---
