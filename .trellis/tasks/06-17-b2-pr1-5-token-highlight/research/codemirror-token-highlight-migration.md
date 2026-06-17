# Research: CodeMirror 6 Token Coloring + ChatInput 迁移

- **Query**: 用 CM 6 实现 `/command` + `@file` token 着色（skill 预留），并把 ChatInput.vue 的 textarea + onKeydown trigger 面板路由迁移到 CM 6
- **Scope**: mixed（内部代码现状 + 外部 CM 6 API 参考）
- **Date**: 2026-06-17

## TL;DR

- **决策已锁定**（PRD §核心决策 2026-06-17）：用 CodeMirror 6 替换 `ChatInput.vue` 的 textarea。
- **当前依赖**：`app/package.json` **不含** `@codemirror/*`。需要新增 `@codemirror/state` + `@codemirror/view`（`TECH.md §1.2` 已列 CodeMirror 6 为候选 Editor）。
- **推荐 PR 切分**：
  - **PR-A**：CM 6 骨架 + v-model + autosize + IME-safe Enter 发送 + 基础样式，**功能等价、无 token 着色、无 trigger 面板**。这一步是最大风险点，要在没有 trigger 面板的情况下先把"光秃秃的 CM 编辑器"跑得跟现在 textarea 一模一样。
  - **PR-B**：token 着色（ViewPlugin + `Decoration.mark`，正则标 `/\w+` + `@[\w/.-]+`），skill 预留 CSS class 位。**纯加法**，不动 PR-A 的逻辑。
  - **PR-C**：trigger 面板接入 CM（`updateListener` + 光标/行读取 + keymap 路由）。这一步把现有 `currentLineInfo` / `onKeydown` / `syncCommandPalette` / `syncFilePalette` 全部改写到 CM API。
- **最大风险点**：**PR-A 的 IME-safe Enter**。textarea 用 `compositionstart`/`compositionend` + `isComposing` ref 实现；CM 6 用 `view.composing` flag。如果 PR-A 漏判 composition 状态，中文输入法选词 Enter 会直接提交半截拼音。**规避**：PR-A 必须复测中文 IME（小鹤双拼 / 拼音 / 五笔），确认 Enter 提交 + Enter 选词两场景都正确。

---

## Findings

### 现状（要替换的东西）

#### 文件

| File Path | Description |
|---|---|
| `/usr/local/code/github/everlasting/app/src/components/chat/ChatInput.vue` | 1411 行，textarea + 触发面板 + popover 集成主体 |
| `/usr/local/code/github/everlasting/app/src/components/chat/TriggerMenu.vue` | 566 行，可复用 trigger 面板骨架（B3 引入，B2/B4 复用） |
| `/usr/local/code/github/everlasting/app/src/utils/useKeyboard.ts` | 全局 keybinding 注册（Shift+Tab Mode 循环的唯一消费者） |
| `/usr/local/code/github/everlasting/.trellis/spec/frontend/design-tokens.md` | CSS 变量系统（着色要用的 `--color-accent` / `--color-tool-read` 在此） |
| `/usr/local/code/github/everlasting/.trellis/spec/frontend/popover-pattern.md` | 手写 popover 模式（TriggerMenu 是其第四个生产实例） |

#### 现有 textarea + onKeydown 关键事实

| 功能 | 实现位置 | 关键代码 |
|---|---|---|
| textarea + v-model | `ChatInput.vue:790-801` | `<textarea :value="input" @input="onTextareaInput" ...>`（注意是 `:value` 而非 `v-model`，靠 `onTextareaInput` 同步 `input.value`） |
| autosize | `ChatInput.vue:233-238` | `el.style.height = "auto"; el.style.height = "${el.scrollHeight}px"`，CSS cap `max-height: 200px; overflow-y: auto`（行 1034-1035） |
| IME-safe Enter | `ChatInput.vue:99, 250-262, 306-309` | `isComposing` ref + `compositionstart`/`compositionend` 监听 + `onKeydown` 内 `if (e.key === "Enter" && !e.shiftKey && !isComposing.value) submit()` |
| 当前行检测 | `ChatInput.vue:415-427` | `currentLineInfo()`：`el.selectionStart` + `input.value.slice(0,pos).lastIndexOf("\n")+1` + `indexOf("\n", pos)` |
| 触发面板同步 | `ChatInput.vue:240-248, 499-511, 675-687` | `onTextareaInput` 调 `syncCommandPalette()` + `syncFilePalette()`；两者各自调 `detectCommandTrigger()` / `detectFileTrigger()` → 用 `currentLineInfo()` 读行首 |
| 键盘路由 | `ChatInput.vue:264-310` | `onKeydown` 大 if：trigger 面板开时 ArrowUp/Down/Tab/Enter/Esc 路由到 `triggerMenu.value.moveActive/confirmActive`，Enter 不提交 |
| Shift+Tab Mode 循环 | `ChatInput.vue:355-360` | `registerShiftTabCycle` 注册到 `window` capture 阶段，独立于 textarea 的 `onKeydown`（**不放在 textarea 上**，因为 preventDefault 在 per-element 上不可靠） |
| ModeSelect / ModelSelect popover 定位 | `ChatInput.vue:736-826` | `.chat-input__row { position: relative }`，ModeSelect 在 textarea 左、ModelSelect 在 hint row，都用 `position: absolute; bottom: calc(100% + 4px); top: auto` 向上开 |
| TriggerMenu popover 定位 | 同上 `.chat-input__row`，`:trigger-el="textareaEl"` 把 textarea 当作 popover 的"内部"以防 click-to-reposition 误关 |
| Latency popover | `ChatInput.vue:162-206, 833-926` | 手写 popover，独立于 trigger 面板 |
| Token usage chip | `ChatInput.vue:934-989` | reka-ui Tooltip，独立于 textarea |
| send/stop 按钮 | `ChatInput.vue:809-825` | `<button v-if="sending" @click="onStop">` + `<button v-else @click="onSubmit">` |

#### TriggerMenu 组件骨架要点

- **数据源无关**：`items` + `filter` + `#row` slot 让 ChatInput 注入数据（B3 来自 `list_commands`，B2 来自 `list_files`）。
- **键盘导航 API**：`defineExpose({ moveActive, confirmActive })`。父组件 `onKeydown` 路由时直接调 `triggerMenu.value.moveActive(1)` / `confirmActive()`。
- **triggerEl prop**（`TriggerMenu.vue:114`）：把外部 textarea 视为 popover 内部，click-to-reposition-caret 不会误关。**CM 迁移后要把 `triggerEl` 改为 CM 的 `.cm-editor` DOM 节点**（`view.dom`）。
- **fuzzy prop**：B2 (@file) 启用 fuzzysort；B3 (/command) 不启用。

---

### 1. Token 着色 Decoration

#### 1.1 所需 CM 6 包

```jsonc
// app/package.json (additions)
{
  "dependencies": {
    "@codemirror/state": "^6.x",   // EditorState, Prec, Annotation
    "@codemirror/view": "^6.x"     // EditorView, ViewPlugin, Decoration, keymap, RangeSetBuilder, ViewUpdate
    // @codemirror/commands 是可选的（如果需要 history/undo，但 ChatInput 不需要）
    // @codemirror/language 不需要（我们用正则标 span，不上 Lezer grammar）
  }
}
```

TECH.md §1.2 已把 CodeMirror 6 列为候选 Editor，这次落地即"从候选到锁定"。

#### 1.2 ViewPlugin + Decoration.mark + RangeSetBuilder 骨架

```ts
// chat-input-tokens.ts (建议新文件，独立扩展便于 PR-B 单独 review)
import { ViewPlugin, ViewUpdate, Decoration, DecorationSet, EditorView } from "@codemirror/view";
import { RangeSetBuilder } from "@codemirror/state";

/** 命令 token: 行首 `/` + 命令名字符 [a-zA-Z0-9_-]，到第一个非命令字符为止 */
const COMMAND_RE = /(^|\n)(\/[a-zA-Z0-9_-]*)/g;
/** 文件 token: 行首 `@` + 路径字符 [\w/.-]，到第一个空白为止 */
const FILE_RE = /(^|\n)(@[\w/.-]+)/g;

const commandMark = Decoration.mark({ class: "cm-token-command" });
const fileMark = Decoration.mark({ class: "cm-token-file" });

function buildTokenDecorations(view: EditorView): DecorationSet {
  const builder = new RangeSetBuilder<Decoration>();
  const doc = view.state.doc;
  const text = doc.toString();

  // RangeSetBuilder 要求严格升序插入，先收集再排序
  type R = { from: number; to: number; deco: Decoration };
  const ranges: R[] = [];

  for (const m of text.matchAll(COMMAND_RE)) {
    const prefix = m[1] ?? "";
    const from = (m.index ?? 0) + prefix.length;
    const to = from + m[2].length;
    if (to > from) ranges.push({ from, to, deco: commandMark });
  }
  for (const m of text.matchAll(FILE_RE)) {
    const prefix = m[1] ?? "";
    const from = (m.index ?? 0) + prefix.length;
    const to = from + m[2].length;
    if (to > from) ranges.push({ from, to, deco: fileMark });
  }

  ranges.sort((a, b) => a.from - b.from || a.to - b.to);
  for (const r of ranges) {
    builder.add(r.from, r.to, r.deco.range(r.from, r.to));
  }
  return builder.finish();
}

export const tokenHighlightPlugin = ViewPlugin.fromClass(
  class {
    decorations: DecorationSet;
    constructor(view: EditorView) {
      this.decorations = buildTokenDecorations(view);
    }
    update(u: ViewUpdate) {
      // 只在文档变化时重建；光标移动不需要重新着色
      if (u.docChanged || u.viewportChanged) {
        this.decorations = buildTokenDecorations(u.view);
      } else {
        this.decorations = u.startState.field(tokenField, this.decorations);
      }
    }
  },
  { decorations: (v) => v.decorations },
);
```

> **RangeSetBuilder 排序约束**：必须按 `from` 升序 add，否则 `.finish()` 抛错。如果两个 range 重叠会抛 "overlapping ranges"。我们的 `/` 和 `@` 正则互斥（一行不会同时是 `/cmd` 和 `@file`），不会重叠。

#### 1.3 design-tokens 映射到 Decoration style

CM 的 mark decoration 只添加 CSS class，颜色靠项目原有 `var(--color-*)` token。把规则放在 `ChatInput.vue` 的 `<style scoped>`，并用 `:deep()` 穿透到 CM 内部 DOM：

```vue
<style scoped>
/* CM 6 编辑器容器基础：让外观跟原 textarea 一致 */
:deep(.cm-editor) {
  flex: 1;
  background: transparent;
  color: var(--color-text-primary);
  font-family: var(--font-sans);
  font-size: 14px;
  line-height: 1.5;
  max-height: 200px;
  overflow-y: auto;
}
:deep(.cm-editor .cm-scroller) {
  font-family: inherit;
}
:deep(.cm-editor.cm-focused) {
  outline: none; /* 焦点环画在外层 .chat-input__row 上，避免双层 */
}

/* /command token → accent 色（蓝色，PRD §Requirements 指定） */
:deep(.cm-token-command) {
  color: var(--color-accent);
  font-weight: 600;
}

/* @file token → read 色（青色，对齐 read_file tool family） */
:deep(.cm-token-file) {
  color: var(--color-tool-read);
  font-weight: 600;
}

/* B4 skill token 预留位（PR1.5 不实现，PR2 / B4 接入时只需在此加 rule） */
:deep(.cm-token-skill) {
  color: var(--color-tool-thinking); /* violet，对齐 extended-thinking 家族 */
  font-weight: 600;
}
</style>
```

**为什么 `:deep()`**：`<style scoped>` 给当前组件 DOM 加 `data-v-xxx` 属性，但 CM 创建的内部 DOM（`.cm-content` / `.cm-line` / mark span）是命令式生成的，**不会**带 `data-v-xxx`。Vue 文档明确 `:deep()` 用于"穿透到子组件 / 动态生成的 DOM"（项目 `.trellis/spec/frontend/reka-ui-usage.md` "Gotcha: `<style scoped>` does NOT apply to portal children"是同一类坑）。

**为什么用 design-tokens 而非硬编码**：`design-tokens.md §Don't: Hardcode color values in component CSS` 明确要求。

---

### 2. Trigger 面板接入 CM 6

#### 2.1 读光标 + 当前行（替代 `currentLineInfo`）

```ts
// 旧实现 (ChatInput.vue:415-427)：手动 lastIndexOf("\n")
function currentLineInfo(): { line: string; lineStart: number } { /* ... */ }

// 新实现：CM 6 原生 API
function currentLineInfo(view: EditorView): { line: string; lineStart: number } {
  const head = view.state.selection.main.head;
  const lineObj = view.state.doc.lineAt(head);
  return {
    line: lineObj.text,
    lineStart: lineObj.from,
  };
}
```

`doc.lineAt(pos)` 返回 `{ number, from, to, text, length }`，`text` 已经是该行不含 `\n` 的字符串，`from` 是该行起始的 doc 偏移。**远比手写 split 简单且无 off-by-one**。

#### 2.2 监听 doc/selection 变化触发 `/` `@` 检测（替代 `onTextareaInput`）

```ts
// ChatInput.vue setup
const view = shallowRef<EditorView | null>(null);

const updateListenerExt = EditorView.updateListener.of((u: ViewUpdate) => {
  // IME 期间不触发（CM 在 composition 期间会发 update 但 docChanged 为 false；
  // compositionEnded 为 true 的那一帧才会被 docChanged 捕获）
  if (!u.docChanged && !u.selectionSet) return;

  // v-model bridge：同步到 input ref（用于 send / 外部 reset）
  if (u.docChanged) {
    input.value = u.state.doc.toString();
    autosize(); // 见 §4
  }

  // 触发面板同步（替代旧 onTextareaInput 的 syncCommandPalette/syncFilePalette）
  syncCommandPalette();
  syncFilePalette();
});
```

`detectCommandTrigger()` / `detectFileTrigger()` / `syncCommandPalette()` / `syncFilePalette()` 的**逻辑完全不变**，只把内部对 `textareaEl.value` 的引用换成 `view.value`，对 `el.selectionStart` 的引用换成 `view.state.selection.main.head`。

#### 2.3 IME 兼容

CM 6 在 composition 期间**不会**触发 `docChanged` 的 user-typed transaction；只有 composition 结束时才会触发一次合并后的 transaction。`updateListener` 因此天然"composition 安全"，**比 textarea 模式下手动维护 `isComposing` ref 更可靠**。

但 `onKeydown` 路由里的 `!isComposing.value` 检查要替换成 `!view.composing`（CM 6 EditorView 有 `composing: boolean` 只读属性，composition 进行中为 true）。

---

### 3. 键盘路由（CM keymap + Prec）

#### 3.1 keymap 骨架

CM 6 的 keymap 与 Vue 的 `@keydown` **不共存**——一旦 CM 接管 textarea，所有按键必须通过 keymap。原 `onKeydown` 的逻辑迁移到 `keymap.of([...])`：

```ts
import { EditorView, keymap } from "@codemirror/view";
import { Prec } from "@codemirror/state";

const chatInputKeymap = Prec.highest(
  keymap.of([
    {
      key: "ArrowDown",
      run: (view) => {
        if (!commandPaletteOpen.value && !filePaletteOpen.value) return false;
        const menu = filePaletteOpen.value ? fileTriggerMenu.value : triggerMenu.value;
        menu?.moveActive(1);
        return true; // 拦截，阻止 CM 默认光标下移
      },
    },
    {
      key: "ArrowUp",
      run: (view) => {
        if (!commandPaletteOpen.value && !filePaletteOpen.value) return false;
        const menu = filePaletteOpen.value ? fileTriggerMenu.value : triggerMenu.value;
        menu?.moveActive(-1);
        return true;
      },
    },
    {
      key: "Enter",
      run: (view) => {
        // IME-safe：CM composition 期间 keymap 不触发（composition 自己吞键）
        // 但双重保险：if (view.composing) return false;
        if (view.composing) return false;
        if (commandPaletteOpen.value || filePaletteOpen.value) {
          const menu = filePaletteOpen.value ? fileTriggerMenu.value : triggerMenu.value;
          menu?.confirmActive();
          return true;
        }
        // Shift+Enter = 换行（不拦截，CM 默认插入 \n）
        // 无 Shift 的 Enter = 提交
        submit();
        return true;
      },
      shift: undefined, // keymap 不显式支持 shift 反向；CM 用 "Shift-Enter" 单独 binding
    },
    {
      key: "Shift-Enter",
      run: () => false, // 不拦截，让 CM 插入换行（覆盖任何可能的高优先级 binding）
    },
    {
      key: "Tab",
      run: (view) => {
        // Tab = 接受 trigger 面板选择（与 Enter 等价）
        if (commandPaletteOpen.value || filePaletteOpen.value) {
          const menu = filePaletteOpen.value ? fileTriggerMenu.value : triggerMenu.value;
          menu?.confirmActive();
          return true;
        }
        return false; // 无面板时让 CM 默认（indent 或不做）
      },
    },
    {
      key: "Escape",
      run: (view) => {
        if (filePaletteOpen.value) {
          closeFilePalette();
          return true;
        }
        if (commandPaletteOpen.value) {
          closeCommandPalette();
          return true;
        }
        return false; // 让外部 Esc 链（onEscKeydown → stop）继续
      },
    },
  ]),
);
```

#### 3.2 `Prec.highest` 为什么必须

CM 6 默认带 `defaultKeymap`（含 Enter 插入换行、ArrowUp/Down 移动光标、Tab 缩进）。如果我们的 keymap 不用 `Prec.highest`，按下 ArrowDown 会**同时**移动光标和路由到面板。`Prec.highest` 确保我们的 binding 优先级高于 defaultKeymap。

#### 3.3 Shift+Tab Mode 循环：**保留 `useKeyboard`，不动**

`registerShiftTabCycle` 在 `window` capture 阶段监听，**完全独立于 CM 的 keymap**。CM 收到 Shift+Tab 时（如果 CM 没 binding 拦截），事件冒泡到 window，被 `useKeyboard` 捕获并 preventDefault。

**关键陷阱**：CM 默认 `defaultKeymap` 的 Shift+Tab 是 "inverse-indent" / "unindent"。如果我们没在 CM keymap 里拦截 Shift+Tab，CM 会先吃掉（执行 unindent），然后才轮到 window listener——那时 `e.defaultPrevented` 已经被 CM 调过，但 listener 仍能跑（capture phase 早于 target）。

**规避**：在 `chatInputKeymap` 里**显式加一条 Shift+Tab 拦截 + return false**？不行，return false 会 fall-through，CM 又会执行 unindent。正解：让 `chatInputKeymap` 不绑 Shift+Tab，依赖 `useKeyboard` 的 capture 阶段 `e.preventDefault()` + `e.stopPropagation()`——后者必须加，否则 CM 仍会 unindent。

**需要给 `useKeyboard.registerShiftTabCycle` 加 `e.stopPropagation()`**（见迁移风险 §5 Shift+Tab 条目）。

---

### 4. Popover 定位 + autosize

#### 4.1 autosize

CM 6 没有 `scrollHeight` 这种 textarea API。两种方案：

**方案 A（推荐）**：用 `view.contentHeight`（CM 提供的只读属性，跟随 content 自适应）：

```ts
function autosize() {
  const v = view.value;
  if (!v) return;
  const h = Math.min(200, v.contentHeight);
  v.scrollDOM.style.maxHeight = "200px";
  v.scrollDOM.style.overflowY = "auto";
  // CM 的 .cm-editor 自动随 content 高度变化，无需手动 setSize
}
```

并在 `updateListener` 里 docChanged 时调一次。

**方案 B（兜底）**：读 `v.scrollDOM.scrollHeight`：

```ts
v.scrollDOM.style.height = "auto";
v.scrollDOM.style.height = `${Math.min(200, v.scrollDOM.scrollHeight)}px`;
```

A 更地道；B 跟现有 textarea 写法最像，迁移成本最低。建议 PR-A 用 B，验证后再考虑切到 A。

#### 4.2 popover 定位锚点

**关键洞察**：现有所有向上开 popover（ModeSelect / ModelSelect / TriggerMenu / Latency popover）都 anchor 在 `.chat-input__row`（`position: relative`），**不 anchor 在 textarea 上**。所以 CM 替换 textarea 后，**所有 popover 的 CSS 定位都不用改**——只要 `.chat-input__row` 还在、还是 `position: relative`，ModeSelect/ModelSelect/TriggerMenu 就仍然 anchor 在它上面。

唯一要改的是 `TriggerMenu` 的 `:trigger-el` prop：原来是 textarea 的 DOM 节点，迁移后改成 CM 的 `view.dom`（即 `.cm-editor` 节点）：

```ts
// 把 view.dom 暴露给 TriggerMenu 的 triggerEl prop
<TriggerMenu
  ref="triggerMenu"
  :trigger-el="viewDom"  // computed(() => view.value?.dom ?? null)
  ...
/>
```

CM 的 `.cm-editor` 元素取代了原 `<textarea>` 的 DOM 位置，click-to-reposition-caret 事件仍然发生在它内部，所以 popover 不会误关。

#### 4.3 autosize 时 popover 的锚定稳定性

`.chat-input__row` 的高度随 CM contentHeight 变化（CM 在 row 内部 flex: 1）。ModeSelect / ModelSelect 是 row 的子元素（绝对定位），它们的 `bottom: calc(100% + 4px)` 是相对 row 算的——**只要 row 的高度变化是发生在 CM 那一侧（flex: 1），ModeSelect 自己的位置不变**，所以 popover 跟随 row 顶部移动，跟 CM 高度变化无直接耦合。

**唯一需要注意**：当 row 高度从 1 行（28px）长到 8 行（200px）时，整个 row 上移，popover（向上开）跟着上移。这跟现有 textarea 行为一致——**不是新引入的回归点**。

---

### 5. 迁移风险清单（易回归点 + 规避）

| # | 功能 | 回归风险 | 规避 |
|---|---|---|---|
| 1 | **IME-safe Enter** | CM `view.composing` 与 textarea `compositionstart/end` 行为不完全等价；某些 IME（macOS 拼音 / Win 微软拼音）在 candidate 选择时不发标准 composition 事件，CM 可能短暂 `composing=false` | PR-A 验收必须实测 3 类 IME（小鹤双拼 / 微软拼音 / macOS 拼音）；keymap Enter handler 双重保险：`if (view.composing) return false;` |
| 2 | **autosize** | CM 没原生 scrollHeight，contentHeight 跟 textarea 计算可能有 1-2px 偏差 | PR-A 先用方案 B（`v.scrollDOM.scrollHeight`），跟现状像素级一致；后续切方案 A |
| 3 | **trigger 面板互斥** | `/` 和 `@` 同时检测时要保证只有一个开（行首规则保证）；CM 多行编辑时光标跳到另一行，syncXxxPalette 可能在两面板间抖动 | `syncCommandPalette` 和 `syncFilePalette` 互斥逻辑保持；只在 `updateListener.docChanged` 时跑（不在 `selectionSet` 时跑，避免点选切换面板） |
| 4 | **Tab = Enter 确认** | CM 默认 Tab = indent，没 `Prec.highest` 会先吃 Tab | 用 `Prec.highest`，且 `run` 返回 `true` |
| 5 | **Shift+Tab Mode 循环** | CM 默认 Shift+Tab = unindent，先于 window listener 执行；window listener 跑到了但 CM 已经 unindent | **必须**给 `useKeyboard.registerShiftTabCycle` 内部加 `e.stopPropagation()`（capture phase），或在 CM keymap 里显式拦截 Shift+Tab 返回 `true`（牺牲 indent 换 Mode 循环不被打断）。建议前者，影响面更小（只改 `useKeyboard.ts` 一处） |
| 6 | **Mode/Model popover** | 不在 textarea 上，理论上不受影响；但 `.chat-input__row:focus-within` 的焦点环要确认 CM 聚焦时 `.chat-input__row` 仍能 `:focus-within` | CM 的 `.cm-editor.cm-focused` 内部 contenteditable 会 bubble focus 事件，`:focus-within` 应当仍工作；PR-A 验收必须看焦点环 |
| 7 | **latency popover + token chip** | 完全独立于 textarea，理论上零回归；但 latency popover 的 onDocumentClick 用 `latencyPopoverRoot.contains(target)` 判定，CM 的 contenteditable 可能在点击时插入额外的 focus shift | PR-A 验收：开 latency popover，点 CM 内部，确认 popover 不被误关（如果被关，参考 TriggerMenu 的 triggerEl 模式扩展 latency popover） |
| 8 | **send/stop 按钮 disabled 联动** | `sendDisabled()` 依赖 `input.value.trim()`；v-model bridge 必须 emit input 变化 | `updateListener` 里 `if (u.docChanged) input.value = u.state.doc.toString();`，保证 `input` ref 实时同步 |
| 9 | **`@file` select 后光标定位** | `onFileSelect` 用 `el.setSelectionRange(caret, caret)`；CM 用 `view.dispatch({ selection: { anchor: caret } })` | 直接对应改写，CM dispatch 一行搞定 |
| 10 | **submit 后清空** | `submit()` 把 `input.value = ""` + `el.style.height = "auto"`；CM 模式下要 `view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: "" } })` | dispatch 后 v-model bridge 自然更新 `input.value`；autosize 在 updateListener 里自动触发 |
| 11 | **`registerShiftTabCycle` enabled 闭包** | 闭包读 `chatStore.isCurrentSessionStreaming`，跟 CM 无关；只要 `ChatInput.vue` 的 setup 还在，闭包就能读 | 不需要改 |
| 12 | **placeholder** | textarea 用 `:placeholder="..."`；CM 用 `EditorView.placeholder` extension | `EditorView.placeholder(placeholderText)`，PR-A 切换 |
| 13 | **disabled 状态** | textarea 用 `:disabled="sending"`；CM 通过 `EditorState.readOnly.of(true)` 或 `view.dispatch` 拒绝 | 通过给 `EditorView` 的 extension 列表动态加 `EditorState.readOnly` 切换；或 `view.dom.setAttribute("aria-disabled", "true")` + 在 `updateListener` 里 filter 用户输入。**推荐**：reactive extension via `StateField` 或 unmount/remount view。最简单：sending 时 `view.dispatch` 回滚任何用户输入 |

---

## 迁移 PR 切分建议

> **整体策略**：先做"无着色、无 trigger 面板"的纯骨架 PR-A，让 CM 跟现有 textarea 像素级行为一致；再着色（PR-B 纯加法）；最后接入 trigger 面板（PR-C，最复杂）。每一步都可独立 review + 可独立部署到 dev 验收。

### PR-A：CM 6 骨架（功能等价，无着色无面板）

**范围**：
- 新增 `@codemirror/state` + `@codemirror/view` 到 `app/package.json`
- 在 `ChatInput.vue` 用 `EditorView` 替换 `<textarea>`
- v-model bridge（prop `value` ↔ `updateListener` emit）
- `EditorView.placeholder` 替代 `:placeholder`
- autosize（方案 B：`v.scrollDOM.scrollHeight`）
- IME-safe Enter 发送（`view.composing` 双重保险）
- Shift+Enter 换行（CM 默认）
- `<style scoped>` + `:deep()` 让 CM 外观跟原 textarea 一致（含 max-height 200px / 焦点环由 `.chat-input__row:focus-within` 提供）
- `submit()` 改写为 `view.dispatch({ changes: ... })`
- `:disabled="sending"` 改写为 CM 的 readOnly 切换
- **暂不引入** tokenHighlightPlugin
- **暂不引入** trigger 面板接入（**临时移除** `<TriggerMenu>` 调用，或保留组件但通过其他方式触发——建议移除调用，PR-C 再接回）

**验收**：
- [ ] 输入、删除、中文 IME、Shift+Enter 换行、Enter 发送行为与原 textarea 完全一致
- [ ] autosize 1 行→8 行（max 200px）行为一致
- [ ] Mode/Model popover、latency popover、token chip 行为不变
- [ ] send/stop 按钮联动正确
- [ ] Shift+Tab Mode 循环工作（**需同步修改 `useKeyboard.ts` 加 stopPropagation**）
- [ ] /command 和 @file 暂时不工作（PR-C 接回）—— 在 changelog 显式说明

**风险**：高（最大风险点 = IME-safe Enter）

### PR-B：Token 着色（纯加法）

**范围**：
- 新建 `app/src/components/chat/chatInputTokens.ts`，导出 `tokenHighlightPlugin`
- 在 `ChatInput.vue` 的 `EditorView` extension 列表里加入该 plugin
- `<style scoped>` 加 3 条 `:deep(.cm-token-*)` 规则（command / file / skill 预留）

**验收**：
- [ ] 输入 `/help` `/clear` 显示蓝色（`--color-accent`）
- [ ] 输入 `@app/src/...` 显示青色（`--color-tool-read`）
- [ ] 普通文本颜色不变
- [ ] 行首 `/` 后输入空格 `/ hello` 着色消失（COMMAND_RE 边界正确）
- [ ] 删除 token 后着色立即消失

**风险**：低（纯加法，正则错了就修正则）

### PR-C：Trigger 面板接入 CM

**范围**：
- `updateListener` 里加 `syncCommandPalette()` / `syncFilePalette()` 调用
- `currentLineInfo()` 重写为 `view.state.doc.lineAt(head)` 版本
- `chatInputKeymap`（Prec.highest）路由 ArrowUp/Down/Enter/Tab/Esc 到 `triggerMenu` / `fileTriggerMenu`
- `<TriggerMenu>` 组件重新启用，`:trigger-el="viewDom"`（`view.value.dom`）
- `onCommandSelect` / `onFileSelect` 的光标重定位改用 `view.dispatch({ selection: { anchor } })`
- autosize 在面板关闭后重新触发（select 后内容变化）

**验收**：
- [ ] `/` 触发命令面板、`@` 触发文件面板
- [ ] ArrowUp/Down 在面板内移动高亮、Enter/Tab 确认
- [ ] Esc 关闭面板
- [ ] IME composition 期间面板不被触发（CM updateListener 天然安全）
- [ ] `/help` 选 help 后 textarea 清空、焦点回到 CM
- [ ] `@path` 选 path 后 caret 落在 `@path` 末尾
- [ ] Tab=Enter 行为正确（CM 默认 Tab 被 Prec.highest 拦截）
- [ ] Shift+Tab Mode 循环不被打断（useKeyboard stopPropagation 生效）
- [ ] 面板开时 Enter 不提交、面板关时 Enter 提交

**风险**：中（keymap 优先级 + 光标重定位是新代码，但逻辑清晰）

---

## External References

- **CodeMirror 6 Decoration** — https://codemirror.net/docs/ref/#view.Decoration （`Decoration.mark` / `DecorationSet` / `RangeSetBuilder`）
- **CodeMirror 6 ViewPlugin** — https://codemirror.net/docs/ref/#view.ViewPlugin （`ViewPlugin.fromClass` + `{ decorations }` option）
- **CodeMirror 6 Keymap** — https://codemirror.net/docs/ref/#view.keymap （`keymap.of` + binding 形状 + `run: (view) => boolean`）
- **CodeMirror 6 Precedence** — https://codemirror.net/docs/ref/#state.Prec （`Prec.highest` / `Prec.default`）
- **CodeMirror 6 Composition** — https://codemirror.net/docs/ref/#view.EditorView.composing （`view.composing` 只读属性；CM 内部管理 composition state）
- **CodeMirror 6 EditorState.doc** — https://codemirror.net/docs/ref/#state.EditorState.doc （`doc.lineAt(pos)` / `doc.sliceString`）
- **Vue 3 `<style scoped>` + `:deep()`** — https://vuejs.org/api/sfc-css-features.html （穿透到命令式生成的子 DOM）

## Related Specs

- `.trellis/spec/frontend/design-tokens.md` — 着色要用的 `--color-accent` (`#3b5bdb`) / `--color-tool-read` (`#06b6d4`) / `--color-tool-thinking` (`#a78bfa`) token 定义
- `.trellis/spec/frontend/popover-pattern.md` — 手写 popover 模式（TriggerMenu 是第 4 个生产实例 + 外部 triggerEl 变体）
- `.trellis/tasks/06-17-b2-pr1-5-token-highlight/prd.md` — 本任务 PRD（CodeMirror 6 决策已锁定）
- `docs/TECH.md §1.2` — CodeMirror 6 列为候选 Editor，本次落地即"从候选到锁定"

---

## Caveats / Not Found

- **未实测** CM 6 在 Tauri WebView2（Windows）+ WSLg（Linux Wayland）下的中文 IME 行为；PR-A 必须在两个平台分别验收。CLAUDE.md "WSL 环境注意" 与 HACKING-wsl.md 提到中文输入法是该环境的反复踩坑点。
- **未实测** CM 6 的 `view.composing` 在所有 IME 候选确认场景下都及时翻转；macOS 某些老 IME 有过 composition 事件丢失的历史 bug（CM issue 跟踪中）。**PR-A 实测是唯一保险**。
- **未确认** CM 6 bundle size 增量（`@codemirror/state` + `@codemirror/view` 约 100-150KB minified）。Tauri 是本地应用，不像 web 要担心首屏，但 PR-A changelog 应记录。
- **未调研** 是否要用 `@codemirror/commands` 的 `history()` 给 ChatInput 加 undo/redo（textarea 现在没有 undo；CM 默认也无 undo 除非加 `history()`）。**建议 PR-A 不加**，保持与现状一致；未来如果用户反馈需要再加。
- **未调研** `nucleo`（TECH.md §1.4 提到的 Rust fzf 端口）是否在 PR1.5 范围内替换前端 fuzzysort——PRD 明确 OOS，本次只动前端 CM 迁移。
