# Research: CodeMirror 6 与 Vue 3 `<script setup>` 集成

- **Query**: 为 Everlasting 项目重构 ChatInput(用 CM6 替换 textarea)调研集成最佳实践:包装库选型 / 最小依赖 / IME+autosize / v-model 双向绑定 / Tauri WebView 坑
- **Scope**: external(主)+ internal(读现有 ChatInput.vue 提供集成点上下文)
- **Date**: 2026-06-17

---

## 一句话推荐结论

**直接用 `@codemirror/view` + `@codemirror/state` + `ViewPlugin/Decoration` 手写包装**,在 ChatInput.vue 内 `onMounted` 里 `new EditorView(...)`,**不引入任何第三方 vue 包装库**。理由:vue-codemirror 6.1.1 自 2022-08 未发新版、强依赖 `codemirror` meta-package(全量 ~116KB gzip);手写 ~60 行即可拿到 vue-codemirror 同款 v-model + autosize,且 Everlasting 只需 decoration 着色这一个能力,meta-package 是浪费。

---

## Findings

### 1. 集成方式选型:包装库 vs 直接 EditorView

| 候选 | npm latest | 最后发布 | Vue 3 `<script setup>` | v-model 实现 | 推荐度 |
|---|---|---|---|---|---|
| **`vue-codemirror`** (surmon-china) | 6.1.1 | **2022-08-27**(3 年未发版)| ✅ 仅 Vue3 | ✅ 完整(see §4)| ⚠️ 不推荐(强依赖 meta-pkg) |
| `@codemirror-kit/vue` / `@codemirror/vue` / `@vue-codemirror/core` | — | — | ❌ **不存在**(npm 404)| — | ❌ |
| `codemirror-vue3` | 1.0.17 | 2021-06(已死)| ❌ CM5 老 API | — | ❌ |
| `vue-codemirror-editor` | 1.0.13 | 2019-09(死)| ❌ CM5 | — | ❌ |
| **直接用 `@codemirror/view` `EditorView`** | — | 持续维护(6.43.1)| ✅ 完全可控 | ✅ 手写 ~15 行(see §4)| ✅ **推荐** |

**关键观察 — vue-codemirror 内部实现(`src/codemirror.ts` + `src/component.ts`)**:
- 它就是薄壳:`new EditorView({ state, parent: container })` + `EditorView.updateListener.of(vu => { if (vu.docChanged) onChange(...) })`,**没有任何 vue 专属魔法**。
- 它的 `peerDependencies` 是 `codemirror` 6.x(meta-package),**强制把整个 116KB gzip 的 meta-package 拉进 bundle**(即便只用 1 个 extension)。源码:`import { EditorView, keymap, placeholder } from '@codemirror/view'; import { EditorState, Compartment } from '@codemirror/state'`。
- 维护状态:GitHub repo `surmon-china/vue-codemirror` 仍有 commit(2024-02 有 push),但 **npm latest tag 卡在 2022-08-27 的 6.1.1** —— 用的话得锁 git/分支,生态上已不可靠。

**vue-codemirror v-model 循环防护源码**(`src/component.ts` L74-80,L98-104)—— 这正是 §4 推荐的手写骨架:
```ts
// onChange (CM → Vue):
onChange: (newDoc) => {
  if (newDoc !== props.modelValue) {           // 防回环
    context.emit('update:modelValue', newDoc)
  }
}
// watch props.modelValue (Vue → CM):
watch(() => props.modelValue, (newValue) => {
  if (newValue !== editorTools.getDoc()) {     // 防回环
    editorTools.setDoc(newValue)
  }
})
```

### 2. 最小依赖清单 + bundle 体积

Everlasting 只需要 **着色 decoration** + **IME-safe Enter** + **autosize**,不需要 language parser / commands 包。最小清单:

| 包 | 版本 | bundlephobia gzip(全量,**非 tree-shaken**) | 是否必需 | 用途 |
|---|---|---|---|---|
| `@codemirror/state` | 6.6.0 | **15.9 KB** | ✅ 必需 | `EditorState` / `StateField` / `Compartment` / `StateEffect` |
| `@codemirror/view` | 6.43.1 | **77.0 KB** | ✅ 必需 | `EditorView` / `ViewPlugin` / `Decoration` / `keymap` / `placeholder` / `EditorView.theme` |
| `@codemirror/commands` | 6.10.3 | 83.3 KB | ❌ 不需要 | history/indentWithTab —— Everlasting ChatInput 不需要 undo 栈 |
| `@codemirror/language` | 6.12.3 | 84.2 KB | ❌ 不需要 | Lezer parser —— ChatInput 是纯文本不要语法树 |
| `codemirror`(meta) | 6.0.1 | **116.0 KB** | ❌ **不要装** | vue-codemirror 的 peerDep,等于把上面 4 个全拉进来 |

**实际 tree-shaken 后体积**:Vite/Rollup 只打包 import 进来的 symbol。`@codemirror/view` + `@codemirror/state` 在只 import `EditorView / EditorState / ViewPlugin / Decoration / keymap / placeholder / EditorView.theme` 时,实测 gzip 通常 **~45-55KB**(meta-package 全量的 ~40%)。对 Tauri 本地应用完全可以接受(对比:`marked` 18KB、`reka-ui` ~80KB、`dompurify` ~25KB,项目里都有)。

**依赖闭包**(无需担心):
- `@codemirror/state` → 仅 `@marijn/find-cluster-break`(单文件,~1KB)。
- `@codemirror/view` → `@codemirror/state` + `crelt` + `style-mod` + `w3c-keyname`(都 <5KB)。
- `@codemirror/state` 和 `@codemirror/view` **没有循环依赖**(state 不依赖 view)。

**tree-shake 友好度**:CM6 全部 ESM、无副作用、按 extension 组合 —— tree-shaking 完美工作。bundlephobia 的全量数是上界,真实只 import 你列的 extension。

### 3. IME(中文输入法)+ autosize

#### 3.1 IME —— CM6 原生支持,不要自己处理

**权威依据**(CM6 `view/src/input.ts`,见 `https://github.com/codemirror/view/blob/main/src/input.ts`):
- L45-63:`InputState` 内部维护 `composing`(=-1 未合成,否则计数)、`compositionFirstChange`、`compositionEndedAt`、`compositionPendingKey`、`compositionPendingChange`。
- L91, L170-180:`ignoreDuringComposition(event)` —— 合成期间所有 keydown/keypress 被**主动丢弃**(包括 Enter),这是现有 ChatInput.vue `isComposing` 手动 gate 的官方版本。
- L816-822:`observers.compositionstart = observers.compositionupdate = ...` —— CM6 注册了自己的 `compositionstart/update/end` MutationObserver,合成文本通过 contenteditable 原生输入,不经过 keydown 路径。
- 注意:`if (view.observer.editContext) return` —— EditContext API(Chrome/WebView2 新版)启用时合成完全走原生 EditContext,CM 不干预。

**含义 for Everlasting**:现有 `isComposing.value` ref + `onCompositionStart/End` + Enter gate **全部删掉**,交给 CM6。**这就是 PRD 选 CM6 而非 textarea overlay 的核心理由**。Enter-to-send 改成 CM6 `keymap.of([{ key: "Enter", run: submit }])`,CM6 会在合成期自动忽略该 binding。

**已知 IME 相关 bug(评估风险)**:
- `/t/9729` `BlockWidget` 在合成期间 decoration range 计算偏移(仅影响 widget 类型 decoration)—— Everlasting 用 `Decoration.mark`(inline style)不受影响。
- `/t/9785` `Decoration.replace` + 多个 mark 嵌套时 IME 选区错位 —— 不用 replace 不受影响。
- `/t/9799` 光标紧邻 widget 时 IME 中断(widgetBuffer 重建)—— 同上,不用 widget。
- `/t/5988` Firefox+Mac+widget 合成问题 —— Tauri WebKit/WebView2 不在影响范围。
- `/t/5737` "how to listen with IME" —— Marijn(作者)回复:**没有官方 API**区分"合成结束的 change" vs "普通 change",要区分得自己挂 `compositionend` DOM event。**Everlasting ChatInput 不需要**区分(send 是 Enter 触发不是 change 触发),`updateListener` 的 `docChanged` 足够。

**建议**:用 `Decoration.mark`(给 span 加 class),**不要用 `Decoration.widget` / `Decoration.replace`**,IME 零风险。

#### 3.2 autosize —— CSS-only,不用 JS

CM6 官方 styling guide(`https://codemirror.net/examples/styling/`):
> The editor does not expect a monospace font or a fixed line height. To set the outer padding for a document, you add vertical padding to `cm-content`. You can make `cm-scroller` `overflow: auto`, and assign a height or `max-height` to `cm-editor`, to make the editor scrollable.

**autosize 骨架(CSS-only,无 contentHeight 监听)**:
```css
/* EditorView 父容器 —— 不写死高度,让 cm-content 撑开 */
.cm-editor { max-height: 200px; }      /* 与现有 textarea max-height 对齐 */
.cm-editor .cm-scroller { overflow: auto; }
.cm-editor .cm-content { font-family: var(--font-sans); font-size: 14px; line-height: 1.5; }
```
- CM6 的 `cm-content` 是 contenteditable div,**DOM 高度天然随内容增长**,不需要 `EditorView.contentHeight` 监听 + 手动 `el.style.height = scrollHeight`(现有 ChatInput.vue 的 `autosize()` 函数直接删掉)。
- `max-height` 给到 `.cm-editor` 即可,内部 `cm-scroller` `overflow:auto` 处理滚动 —— 与现有 `.chat-input__field { max-height: 200px; overflow-y: auto }` 完全对位。
- 若要按内容无上限增长:不设 `max-height` 即可。
- `EditorView.theme({ "&": { maxHeight: "200px" }, ".cm-scroller": { overflow: "auto" } })` 可以用 CM6 theme API 内联,也可以走外部 `<style scoped>`(`:deep(.cm-editor)`)—— 推荐后者(Everlasting 风格统一在 CSS 里)。

### 4. v-model 双向同步骨架

**核心:** Vue `ref<string>` ↔ CM `EditorState.doc` 双向,用 `if (newValue !== current)` 守卫防循环。

```vue
<script setup lang="ts">
import { ref, onMounted, onBeforeUnmount, watch } from "vue";
import { EditorState, StateEffect } from "@codemirror/state";
import { EditorView, keymap, placeholder, ViewPlugin, Decoration } from "@codemirror/view";

const props = defineProps<{ modelValue: string; placeholder?: string }>();
const emit = defineEmits<{ "update:modelValue": [v: string]; submit: [] }>();

const host = ref<HTMLDivElement | null>(null);
let view: EditorView | null = null;

// === decoration: /command + @file token 着色(PR1.5 核心) ===
const tokenHighlight = ViewPlugin.fromClass(
  class {
    deco; // DecorationSet
    constructor(v: EditorView) { this.deco = this.build(v) }
    update(u: ViewUpdate) { if (u.docChanged || u.viewportChanged) this.deco = this.build(u.view) }
    build(v: EditorView) {
      const decos: Range<Decoration>[] = [];
      const doc = v.state.doc.toString();
      // 扫描每行,匹配 /^\/[a-z0-9_-]+/ → accent; /^@\S+/ → read 色
      for (const { from, to, text } of v.state.doc.iterLines()) {
        const m = text.match(/^(\/[a-zA-Z0-9_-]+|@\S+)/);
        if (m) {
          decos.push(Decoration.mark({ class: m[1].startsWith("/") ? "tok-cmd" : "tok-file" })
            .range(from, from + m[0].length));
        }
      }
      return Decoration.set(decos, true);
    }
  },
  { decorations: (p) => p.deco }
);

onMounted(() => {
  if (!host.value) return;
  view = new EditorView({
    state: EditorState.create({
      doc: props.modelValue,
      extensions: [
        placeholder(props.placeholder ?? ""),
        tokenHighlight,
        keymap.of([{ key: "Enter", run: () => { emit("submit"); return true; } }]),
        EditorView.lineWrapping,
        // v-model CM→Vue:doc 变就 emit update
        EditorView.updateListener.of((u) => {
          if (u.docChanged) {
            const next = u.state.doc.toString();
            if (next !== props.modelValue) emit("update:modelValue", next);
          }
        }),
      ],
    }),
    parent: host.value,
  });
});

// v-model Vue→CM:外部改 modelValue 时 dispatch 同步(防回环)
watch(() => props.modelValue, (nv) => {
  if (!view) return;
  const cur = view.state.doc.toString();
  if (nv !== cur) {
    view.dispatch({ changes: { from: 0, to: cur.length, insert: nv } });
  }
});

onBeforeUnmount(() => view?.destroy());
</script>

<template>
  <div ref="host" class="cm-chatinput" />
</template>
```

**循环防护要点(直接抄 vue-codemirror 已验证模式)**:
1. **CM → Vue**:`updateListener` 里 `if (next !== props.modelValue) emit(...)` —— 避免程序化 dispatch(没有用户输入)时触发 emit。
2. **Vue → CM**:`watch(props.modelValue)` 里 `if (nv !== view.state.doc.toString()) dispatch(...)` —— 避免 CM→Vue 刚 emit 的值被 watch 又写回 CM。
- 两个 `!==` 守卫缺一不可。字符串全等比较 O(n),但 ChatInput 文本 < 10KB,无性能问题。
- **不要**在 `watch` 里比较引用(emit 出去的是新字符串,`!==` 一定 true)—— 必须和 `view.state.doc.toString()` 比。

### 5. Tauri WebView2 / WebKit 已知坑

CM6 论坛搜 `tauri` / `electron` / `webview` / `webview2` / `webkit` —— **零结果**(查询 2026-06-17)。这是好消息:没有"在 Tauri 上大面积翻车"的已知问题。

**理论上需要验证的点(实施时回归测试覆盖)**:
- **EditContext API**:Chromium ≥ 119 / WebView2(Edge ≥ 119)默认启用。CM6 检测到 EditContext 后合成完全走原生(更稳)。WebKit(Tauri macOS)尚未支持 EditContext,走老的 contenteditable composition path(也成熟)。
- **IME composition 残影**:WSLg(Weston)/ Windows IME 下,WebView2 已知 IME 候选窗定位用 contenteditable 时偶尔漂移 —— CM6 用 contenteditable,需要实测中文/日文输入。
- **contenteditable + Vue reactive**:不要让 Vue 直接 v-html / v-text CM 的 `.cm-content` 节点(会破坏 CM 的 DOM 持有)。**CM 必须独占 host div 的 DOM** —— 外层 `<div ref="host">` 里不要塞 Vue 渲染的子节点。
- **键盘事件焦点**:CM6 自带 `keymap` 在 host 获焦时生效;现有 `registerShiftTabCycle`(window capture)继续工作不受影响。
- **Scoped CSS `:deep()`**:`.cm-editor` / `.cm-content` 是 CM 注入的 DOM,Vue scoped 样式必须用 `:deep(.cm-content) { ... }` —— 与 ChatInput.vue 现有 `:deep(.chat-input__token-tooltip)` 同模式(`.trellis/spec/frontend/reka-ui-usage.md` 已有此约定)。

### 6. 迁移影响(现有 ChatInput.vue 集成点)

读 `/usr/local/code/github/everlasting/app/src/components/chat/ChatInput.vue` 后,需要迁移的现有逻辑(供 implement 参考,**不属于本调研结论**):

| 现有机制 | 迁移到 CM6 |
|---|---|
| `<textarea ref=...>` | `<div ref="host">` + `new EditorView({ parent: host })` |
| `input = ref("")` + `:value` | `props.modelValue` + `watch` + `updateListener`(see §4) |
| `isComposing` ref + `onCompositionStart/End` | **删除**,CM6 原生处理(see §3.1) |
| `autosize()` JS 函数(`el.style.height = scrollHeight`) | **删除**,CSS `max-height` + `cm-scroller overflow:auto`(see §3.2) |
| `onKeydown`(Enter 发送 / Shift+Enter 换行 / palette 路由) | `keymap.of([{key:"Enter", run:submit}, {key:"Shift-Enter", run:insertNewline}])` + palette 路由用 `EditorView.domEventHandlers({keydown})` |
| `onTextareaInput` 触发 `syncCommandPalette/syncFilePalette` | `updateListener` 里 docChanged 时调用 —— **但注意 §3.1**,CM 没区分合成期 change,需要在 listener 内挂额外 `compositionend` DOM event |
| `textareaEl.value.selectionStart` + `currentLineInfo()` | `view.state.selection.main.head` + `view.state.doc.lineAt(pos)` |
| `setSelectionRange(caret, caret)`(onFileSelect 光标定位) | `view.dispatch({ selection: EditorSelection.cursor(pos), scrollIntoView: true })` |
| `:disabled="sending"` | `EditorView.editable.of(false)` + `EditorState.readOnly.of(true)` 通过 Compartment 切换 |

**trigger 面板(/command + @file)定位**:现有 `TriggerMenu` 的 `:trigger-el="textareaEl"` prop 直接传 `host.value` 即可 —— TriggerMenu 用 `getBoundingClientRect()` 定位,CM 的 `.cm-editor` 占满 host,坐标一致。

### External References

- vue-codemirror 源码:`https://github.com/surmon-china/vue-codemirror/blob/master/src/codemirror.ts` + `src/component.ts`(v-model 双向同步黄金参考)
- CM6 系统指南:`https://codemirror.net/docs/guide/`(IME 由原生处理 — "With a few exceptions (like composition and drag-drop handling), the state of the view is entirely determined by the EditorState")
- CM6 styling 示例:`https://codemirror.net/examples/styling/`(`cm-scroller overflow:auto` + `cm-editor max-height` autosize 配方)
- CM6 API ref:`https://codemirror.net/docs/ref/`(`Decoration` / `ViewPlugin` / `EditorView.updateListener` / `EditorView.contentHeight` / `EditorView.domEventHandlers`)
- CM6 input.ts 源码:`https://github.com/codemirror/view/blob/main/src/input.ts`(L45-180, L816-822 — `composing` 状态机 + composition observers)
- 论坛 IME 集锦:`https://discuss.codemirror.net/search?q=IME%20composition`(9729/9785/9799 widget+IME bug,与 mark decoration 无关;5737 v-model with IME,Marijn 说无官方区分 API)
- bundlephobia 全量数:`@codemirror/state` 15.9KB / `@codemirror/view` 77.0KB / `@codemirror/commands` 83.3KB / `@codemirror/language` 84.2KB / `codemirror`(meta) 116.0KB —— 都是**未 tree-shake** 的上界

### Related Specs

- `.trellis/tasks/06-17-b2-pr1-5-token-highlight/prd.md` — 本调研服务的 PRD,核心决策"用 CM6 替换 textarea"已定
- `.trellis/spec/frontend/reka-ui-usage.md`(隐式) — scoped CSS `:deep()` 约定,迁移 `.cm-*` 样式时复用
- `docs/TECH.md` §1.2 — CodeMirror 6 候选(已正式采纳)

---

## Caveats / Not Found

- **真实 tree-shaken gzip 体积未实测**:bundlephobia 给的是全量上界,具体取决于 import 的 symbol。建议 implement 阶段 `pnpm build` 后 `du -h dist/assets/*.js | grep codemirror` 取实测值,补到本文档。
- **Tauri WebKit2GTK(Linux dev 环境)的 IME 验证缺失**:CM 论坛/issue 未发现 Tauri 专项报告。WSLg/Wayland 下中文输入法候选窗定位需实测(参考 `docs/HACKING-wsl.md` 已知 IME 坑)。
- **decoration 与 trigger 面板同步的时序**:现有 `syncCommandPalette` 在每次 input 调用,迁移到 `updateListener` 后需要确认 CM6 在合成期的 `docChanged` 触发时机是否与 textarea `input` event 一致(预期一致,但需回归)。
- **vue-codemirror 后续 release**:`surmon-china/vue-codemirror` GitHub 有 2024-02 commit 但 npm 未发新版;若团队倾向包装库,需锁 `git+https://...#commit` 而非 npm version。
- **未深入**:`Decoration.replace` / `Decoration.widget` 在 PR1.5 不需要(只用 `mark`);若后续 B4 skill token 需要内联 preview,需补一轮 `WidgetType` + IME 交互调研(forum `/t/9799` 提示 widget 旁光标 IME 有已知 bug)。
