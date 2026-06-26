# Memory UI — MemoryPreview Panel + Settings Tab + ProjectTabs Dropdown

> **基线**:2026-06-10 (B5 PR2, frontend)
> **同源文档**:
> - [llm-contract.md](./llm-contract.md) — backend `read_memory_layers` / `read_memory_content` / `open_memory_in_editor` IPC contract
> - [state-management.md](./state-management.md) — Pinia store patterns + listener registration
> - [reka-ui-usage.md](./reka-ui-usage.md) — reka-ui Tab / popover conventions
> - [design-tokens.md](./design-tokens.md) — color / spacing / typography tokens
> - [popover-pattern.md](./popover-pattern.md) — hand-rolled popover pattern
> - [cross-layer-thinking-guide.md](../guides/cross-layer-thinking-guide.md) — Tauri command contracts
>
> **何时读本文**:实现 B5 Memory PR2(前端 UI 预览),或修改 `useMemoryStore` / `<MemoryPreview>` / `<MemoryLayerItem>` / `<MemoryTab>` 时。

> **⚠️ Updated 2026-06-15 (RULE-C-001)**: the backend `notify`
> watcher was removed (freshness is now an mtime fence in
> `load_for_session`). The `memory:reloaded` event is therefore
> **never emitted**; the defensive `listen("memory:reloaded")` in
> `useMemoryStore` was deleted. Re-fetch now happens only via
> `loadForProject` (mount / project switch) and `refresh()` (刷新
> button) — both call `read_memory_layers`, which is always
> current thanks to the fence. The `memory:reloaded` / watcher
> mentions below are the **old** design. See
> `.trellis/tasks/06-15-p1-memory-watcher-appstate/`.

---

## Scenario: B5 Memory Preview UI (PR2)

### 1. Scope / Trigger

- Trigger: B5 PRD §R5 规定前端要做"只读预览 + 外部编辑器跳转"UI,
 入口是 Settings 页 + Project Tabs 双入口。本期不引入内嵌编辑器。
- Why code-spec depth: mandatory — `useMemoryStore` 是跨层契约的
  前端投影(3 个 Tauri command 的 Pinia 包装);`MemoryLayerInfo`
  类型是 Rust 序列化的镜像(serde 字段重命名, snake_case 边界);
 三状态渲染(`Loaded` / `Missing` / `Error`)的样式约定
 决定 ④ 关 UI 是否能传达 "memory 真的生效"。

### 2. Signatures

```typescript
// app/src/stores/memory.ts
export type MemoryKind = "user" | "project" | "session" | "runtime";
export type MemorySource = "claude" | "agents";
export type LayerStatus =
  | { kind: "loaded" }
  | { kind: "missing" }
  | { kind: "error"; reason: string };

export interface MemoryLayerInfo {
  kind: MemoryKind;
  source: MemorySource;
  path: string; // PathBuf → string on the wire
  tokens: number;
  status: LayerStatus;
  char_count: number;
}

export const useMemoryStore = defineStore("memory", () => {
  layers: Ref<MemoryLayerInfo[]>;
  contentCache: Ref<Map<string, string>>;
  loading: Ref<boolean>;
  error: Ref<string | null>;
  lastProjectId: Ref<string | null>;
  loadForProject(projectId: string): Promise<void>;
  refresh(): Promise<void>;
  fetchContent(path: string): Promise<string>;
  openInEditor(path: string): Promise<void>;
  layersOfKind(kind: MemoryKind): MemoryLayerInfo[];
});
```

```vue
<!-- app/src/components/memory/MemoryPreview.vue -->
<MemoryPreview
  :kind="'user' | 'project' | 'all'"
  :project-id="string | null"
/>

<!-- app/src/components/memory/MemoryLayerItem.vue -->
<MemoryLayerItem
  :layer="MemoryLayerInfo"
  @open-editor="(path) => ..."
/>
```

### 3. Contracts

#### Wire format (snake_case, matching Rust serde)

```jsonc
// invoke<MemoryLayerInfo[]>("read_memory_layers", { projectId })
[
  {
    "kind": "user",            // lowercase (#[serde(rename_all = "lowercase")])
    "source": "claude",        // snake_case (#[serde(rename_all = "snake_case")])
    "path": "/home/x/.claude/CLAUDE.md", // PathBuf → string; locked 2026-06-26 user-claude-md-home-dir (Claude Code interop)
    "tokens": 142,
    "status": { "kind": "loaded" },
    "char_count": 487
  },
  {
    "kind": "user",
    "source": "agents",
    "path": "/home/x/.config/everlasting/AGENTS.md", // PathBuf → string; AGENTS.md stays at the original location (only CLAUDE.md moved 2026-06-26)
    "tokens": 0,
    "status": { "kind": "missing" },
    "char_count": 0
  },
  {
    "kind": "project",
    "source": "claude",
    "path": "/home/x/code/foo/CLAUDE.md",
    "tokens": 89,
    "status": { "kind": "error", "reason": "Permission denied" },
    "char_count": 0
  }
]
```

**关键边界**:
- `PathBuf` 在 Tauri IPC 中序列化为 **string** (不是 object)
- `LayerStatus` 是 **tag/content 形式** 的判别联合
  (`#[serde(rename_all = "snake_case", tag = "kind", content = "reason")]`)
- 字段名一律 snake_case —— 不要在前端"贴心"地转 camelCase,
 那样会让 grep 困难、与 Anthropic / OpenAI 文档不匹配
  (沿用 A4 `TokenUsage` 的决策)

#### Component contract

`<MemoryPreview kind="user">` (Settings page):
- `kind="user"` → 过滤后只显示 2 个 User layer
- `projectId` 默认为 `null`,内部 fallback 到
  `useProjectsStore().currentProjectId`(Settings 是项目无关的,
  但渲染时拿当前 project 来 query,因为 backend 要求
  `project_id`)

`<MemoryPreview kind="project" :project-id="activeProjectId">`
 (ProjectTabs dropdown):
- `kind="project"` → 过滤后只显示 2 个 Project layer
- `:project-id` 显式传入 — dropdown 总是知道当前 active project

`<MemoryPreview kind="all">`:
- 测试 / debug 入口;不过滤

### 4. Validation & Error Matrix

| Condition | Result |
|-----------|--------|
| `read_memory_layers` 后端失败 (db down, project not found) | `store.error` 设为 `String(e)`;`layers` 保留上次值(不抛) |
| `read_memory_content` 后端拒绝 (path 不在 4 个固定路径中) | `fetchContent` reject 抛到 `<MemoryLayerItem>`;UI 显示"加载失败" |
| `open_memory_in_editor` 后端失败 (project_id 缺失 / $EDITOR spawn 失败) | `store.error` 设为 `String(e)`;panel 渲染 error banner |
| 4 个文件全部 Missing | Panel 渲染 2 个灰点 + "(文件不存在)" 提示;agent loop 仍正常 (PR1 已保证) |
| 4 个文件全部 Error | Panel 渲染 2 个黄点 + reason tooltip;**不弹**崩溃对话框 |
| Project 切换 (active project 改变) | `effectiveProjectId` watcher 触发 `loadForProject(newId)` |
| 同一个 project 多次打开 Memory dropdown | `loadForProject` 是 idempotent (path 一致 → 不重 fetch content) |
| `read_memory_layers` 期间用户切到无 project 状态 | `effectiveProjectId.value === null` → 渲染 "请先选择项目" |
| Markdown 渲染 XSS 攻击向量 | 走 `renderMarkdown` (marked + DOMPurify, 沿用 `app/src/utils/markdown.ts` 的 XSS 防护) |
| 大文件 (> 50K chars) | 截断 + 显示 "(内容已截断;在外部编辑器中查看完整文件)";原始文件不受影响 |
| 监听 `memory:reloaded` 事件 (PR1 当前不 emit,防御性注册) | 重新 `fetchLayers(lastProjectId)`;今天不会触发,明天 backend 加上 emit 后无感生效 |

### 5. Good / Base / Bad Cases

#### Good: Settings page → Memory tab → preview User CLAUDE.md

1. 用户点 Settings → Memory tab
2. `<MemoryTab>` 渲染 → `<MemoryPreview kind="user">` 渲染
3. `onMounted` → `store.loadForProject(currentProjectId)` → IPC
4. 后端返回 2 个 User layer (CLAUDE.md Loaded, AGENTS.md Missing)
5. Panel 渲染 1 个绿点 + 1 个灰点 + 顶部 chip "1 loaded · 1 missing"
6. 用户点绿点 → `MemoryLayerItem` 展开 → lazy fetch content →
   `renderMarkdown` 渲染 sanitized HTML
7. 用户点 "在外部编辑器打开" → `store.openInEditor(path)` →
   IPC → Rust spawn `$EDITOR` → 文件在 vim/code 中打开
8. 用户编辑保存 → Rust watcher 1s 内收到 Modify 事件 → cache
   invalidate(目前不 emit Tauri event,前端不感知;下个 turn
   注入新内容)

#### Base: Project CLAUDE.md 完全不存在

1. 用户切到 project A,Memory dropdown 打开
2. `<MemoryPreview kind="project">` 渲染 → 2 个 Project layer,
   status 都是 `Missing`
3. Panel 渲染 2 个灰点 + 顶部 "0 loaded · 2 missing"
4. 用户点灰点 → 不可展开(disabled)
5. 整个 chat 仍正常工作 (PR1 已保证 file-missing 不阻断)

#### Bad: `read_memory_layers` 后端错误

1. (假设) db migration 失败,projects 表 schema 不匹配
2. 后端 `read_memory_layers` 返回 `Err("...")`
3. `store.error` 设为错误字符串;`layers` 保留上次值(防御性)
4. Panel 顶部 error banner 显示 "Memory 暂不可用: ..."
5. Settings / Memory dropdown 仍能正常开关,UI 不崩
6. Agent loop 也不崩 — PR1 已经处理了 backend 加载失败,
   继续 chat

#### Bad: 嵌入了 `<script>` 的恶意 memory 文件

1. (假设) 用户的 `CLAUDE.md` 包含 `<script>alert(1)</script>`
2. 后端 `read_memory_content` 读取文件,内容传给前端
3. 前端 `renderMarkdown(text)` 走 `marked.parse` + `DOMPurify.sanitize`
4. DOMPurify 默认 strip `<script>` → 输出空字符串
5. Panel 渲染空白内容;**没有** XSS 漏洞
6. 这是 **必须** 走 `renderMarkdown` 而不是 `v-html="text"` 的原因

### 6. Tests Required

#### Frontend (vitest 已有 `streamController.test.ts` 模式)

- `useMemoryStore` 单测:`fetchLayers` 失败时 `error` 设置 +
  `layers` 保留;`loadForProject` 重复调用是 idempotent
- `MemoryPreview` 组件测试(vitest + @vue/test-utils):3 种
  status 渲染正确
- `MemoryLayerItem` 测试:展开 → 触发 `fetchContent`;不可点击
  的 Missing layer 不响应 click

#### Manual smoke test (PRD acceptance A2/A4)

1. `cd app && pnpm tauri dev`
2. 打开 Settings → Memory tab
   - 看到 User CLAUDE.md / User AGENTS.md 2 个卡片
   - 缺失的显示灰点 + "(文件不存在)"
   - 存在的显示绿点 + token 数
3. 点击存在的卡片 → 展开 → markdown 渲染
4. 点 "在外部编辑器打开" → 外部编辑器打开
5. 在 Settings Memory tab 之外,切到 ProjectTabs → 点 Memory 按钮
   - 看到 Project CLAUDE.md / Project AGENTS.md 2 个卡片
6. 切换 project → Memory dropdown 关闭(避免 stale state)
7. 修改 `~/.claude/CLAUDE.md`(Claude Code interop
   路径,2026-06-26 user-claude-md-home-dir)→ 1s 内 watcher
   触发 → 下一个 user message 重新加载
   (本期前端不感知此事件,backend 已处理)

### 7. Wrong vs Correct

#### Wrong: 直接用 `v-html="text"` 渲染 memory 内容

```vue
<!-- BAD — XSS 风险;不走 marked + DOMPurify -->
<div v-html="layer.content" />
```

攻击向量:`CLAUDE.md` 里写 `<img src=x onerror=alert(1)>` →
`v-html` 直接执行 → 任意 JS 执行。

#### Correct: 走 `renderMarkdown` 渲染 pipeline

```typescript
// GOOD — marked + DOMPurify,sanitize 后的 HTML 才能进 v-html
import { renderMarkdown } from "../../utils/markdown";

const bodyHtml = ref<string | null>(null);
// ... fetch content from store, then:
bodyHtml.value = renderMarkdown(text);
```

```vue
<div class="memory-layer__markdown" v-html="bodyHtml ?? ''" />
```

`renderMarkdown` 在 `app/src/utils/markdown.ts` 已经有 XSS
fixture 测试(`markdown.test.ts` 验证 `<script>` / onerror /
javascript: URL 都被 strip)。本期直接复用,不重新发明。

#### Wrong: 写新的 Pinia store 而不是用 `contentCache` 共享

```typescript
// BAD — 每个 MemoryLayerItem 独立 fetch,重复 IPC
function fetchContent(path: string): Promise<string> {
  return invoke<string>("read_memory_content", { projectId, path });
}
```

切到 layer A → fetch → 切到 layer B → fetch → 切回 A → 再次 fetch。
3 次 IPC,第二次命中不了任何缓存。

#### Correct: 在 store 集中缓存 content

```typescript
// GOOD — Map<path, string> 在 store 内部缓存
const contentCache = ref<Map<string, string>>(new Map());

async function fetchContent(path: string): Promise<string> {
  const cached = contentCache.value.get(path);
  if (cached !== undefined) return cached;
  const text = await invoke<string>("read_memory_content", { projectId, path });
  const next = new Map(contentCache.value);
  next.set(path, text);
  contentCache.value = next;
  return text;
}
```

切到 A → fetch → 切到 B → fetch → 切回 A → 命中缓存。2 次 IPC。

#### Wrong: 监听全局 `Window` click + 阻止其它点击

```typescript
// BAD — 阻止事件冒泡,影响其它组件
function onDocumentClick(e: MouseEvent) {
  if (memoryMenuOpen.value && !root.value?.contains(e.target)) {
    memoryMenuOpen.value = false;
    e.stopPropagation(); // ← 不要 stop,只是关掉自己
  }
}
```

`stopPropagation` 会让外层的 `WorktreeChip` / `ModelSelect` 之类的
其它 popover 收不到 click 信号,行为不可预测。

#### Correct: 关闭自己即可,不动事件

```typescript
// GOOD — 只关 dropdown,不阻拦 click
function onDocumentClick(e: MouseEvent) {
  if (memoryMenuOpen.value) {
    const target = e.target as Node | null;
    if (memoryMenuRoot.value && target && !memoryMenuRoot.value.contains(target)) {
      memoryMenuOpen.value = false;
    }
  }
}
```

沿用 `.trellis/spec/frontend/popover-pattern.md` 的约定:不
stopPropagation,不 preventDefault,只翻转自己内部的 `open` ref。

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
  备好混用通路。
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

CLAUDE.md / AGENTS.md 上限是 100KB(PR1 的 `MAX_FILE_SIZE`),
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

## Related

- `.trellis/spec/frontend/state-management.md` — Pinia store
  模式,`unlisten*` 模块级守卫
- `.trellis/spec/frontend/reka-ui-usage.md` — reka-ui Tab /
  Popover 模式;本期不用 Popover (手写 popover)
- `.trellis/spec/frontend/popover-pattern.md` — Memory dropdown
  ~~沿用 hand-rolled 模式~~(OBSOLETED 2026-06-11,见
  `06-11-memory-modal-appheader-entry`;Memory 改为 modal,
  popover-pattern 规则仍适用于其它真正的 popover 场景)
- `.trellis/tasks/06-11-memory-modal-appheader-entry/prd.md` —
  Memory 入口从 popover 迁移到 AppHeader corner action +
  reka-ui Dialog modal 的设计文档
- `app/src/components/memory/MemoryModal.vue` — 当前 Memory
  快查入口的 reka-ui Dialog 实现(本期 PR2 是 popover,2026-
  06-11 follow-up 替换)
- `app/src/components/chat/ChatPanel.vue` — Brain 图标 trigger
  挂载点(WorktreeChip 右侧);`useProjectsStore().currentProjectId`
  存在时才显示。MemoryModal 实例化在同一文件,`memoryModalOpen`
  ref 控制开关。
- `.trellis/spec/frontend/design-tokens.md` — Memory 状态点颜色
  走 token (`#4ade80` / `#fbbf24` 是 token-usage 同一调色板)
- `app/src/stores/streamController.test.ts` — vitest 单测模式
  参照;`memory.test.ts` 用同样的 setActivePinia / createPinia
  套路
- `app/src/utils/markdown.ts` — `renderMarkdown` 走 marked +
  DOMPurify,XSS 防护已锁定
- `app/src/components/chat/MessageItem.vue` — markdown 渲染
  参考(同一 `renderMarkdown` 路径)
- `app/src-tauri/src/memory/types.rs` — Rust 类型 → TS 镜像
  的源头
- `app/src-tauri/src/commands/memory.rs` — 3 个 Tauri command
  的 contract 本期的前端契约
