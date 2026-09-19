<!-- Moved from memory-ui.md 2026-09-19 (doc-split) -->

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
// 2026-09-10 hard switch: "everlasting" | "agents". The DTO field
// is MemorySourceWire (union + string passthrough) — unknown wire
// values render raw; the store maps the legacy "claude" → "everlasting".
export type MemorySource = "everlasting" | "agents";
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
    "source": "everlasting",   // snake_case (#[serde(rename_all = "snake_case")]);
                               // 2026-09-10 hard switch — pre-rename payloads said
                               // "claude"; the store normalizes at the read boundary
    "path": "/home/x/.config/everlasting/EVERLASTING.md", // PathBuf → string
    "tokens": 142,
    "status": { "kind": "loaded" },
    "char_count": 487
  },
  {
    "kind": "user",
    "source": "agents",
    "path": "/home/x/.config/everlasting/AGENTS.md", // PathBuf → string; AGENTS.md stays at the original location (only EVERLASTING.md moved 2026-06-26)
    "tokens": 0,
    "status": { "kind": "missing" },
    "char_count": 0
  },
  {
    "kind": "project",
    "source": "everlasting",
    "path": "/home/x/code/foo/EVERLASTING.md",
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

#### Good: Settings page → Memory tab → preview User EVERLASTING.md

1. 用户点 Settings → Memory tab
2. `<MemoryTab>` 渲染 → `<MemoryPreview kind="user">` 渲染
3. `onMounted` → `store.loadForProject(currentProjectId)` → IPC
4. 后端返回 2 个 User layer (EVERLASTING.md Loaded, AGENTS.md Missing)
5. Panel 渲染 1 个绿点 + 1 个灰点 + 顶部 chip "1 loaded · 1 missing"
6. 用户点绿点 → `MemoryLayerItem` 展开 → lazy fetch content →
   `renderMarkdown` 渲染 sanitized HTML
7. 用户点 "在外部编辑器打开" → `store.openInEditor(path)` →
   IPC → Rust spawn `$EDITOR` → 文件在 vim/code 中打开
8. 用户编辑保存 → Rust watcher 1s 内收到 Modify 事件 → cache
   invalidate(目前不 emit Tauri event,前端不感知;下个 turn
   注入新内容)

#### Base: Project EVERLASTING.md 完全不存在

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

1. (假设) 用户的 `EVERLASTING.md` 包含 `<script>alert(1)</script>`
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
   - 看到 User EVERLASTING.md / User AGENTS.md 2 个卡片
   - 缺失的显示灰点 + "(文件不存在)"
   - 存在的显示绿点 + token 数
3. 点击存在的卡片 → 展开 → markdown 渲染
4. 点 "在外部编辑器打开" → 外部编辑器打开
5. 在 Settings Memory tab 之外,切到 ProjectTabs → 点 Memory 按钮
   - 看到 Project EVERLASTING.md / Project AGENTS.md 2 个卡片
6. 切换 project → Memory dropdown 关闭(避免 stale state)
7. 修改 `~/.config/everlasting/EVERLASTING.md`(2026-09-10
   硬切换后的统一用户层路径)→ 下一次 read_memory_layers 的
   mtime fence 重新加载(本期前端不感知此事件,backend 已处理)

### 7. Wrong vs Correct

#### Wrong: 直接用 `v-html="text"` 渲染 memory 内容

```vue
<!-- BAD — XSS 风险;不走 marked + DOMPurify -->
<div v-html="layer.content" />
```

攻击向量:`EVERLASTING.md` 里写 `<img src=x onerror=alert(1)>` →
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

