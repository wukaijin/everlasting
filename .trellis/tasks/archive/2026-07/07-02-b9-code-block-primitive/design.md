# Design — B9-B code_block primitive

> 技术 design。需求见 `prd.md`，父决策见 parent `prd.md` D6（hljs）。

## 1. hljs 配置（`utils/highlight.ts`，共享）

```ts
import hljs from "highlight.js/lib/common";

/** Render `code` to highlighted HTML. Known language → `hljs.highlight`;
 *  unknown/missing → `hljs.highlightAuto` (best-effort, never throws).
 *  Shared by the markdown pipeline (marked-highlight) AND
 *  `<CodeBlockPrimitive>` so the two never diverge on language support. */
export function renderCodeHtml(code: string, language: string): string {
  if (language && hljs.getLanguage(language)) {
    try {
      return hljs.highlight(code, { language }).value;
    } catch {
      // fall through to auto
    }
  }
  return hljs.highlightAuto(code).value;
}
```

- **语言集 = `highlight.js/lib/common`**（~主流 30+ 语言：js/ts/py/rust/go/java/c/c++/json/bash/md/...）。否决 `highlight.js` full（~900KB，个人工具用不到冷门语言）；否决按需 import（语言一多管理烦，common 一行覆盖）。
- 两个入口共用 → 语言集只配一次，不会出现"markdown 高亮了但 primitive 没有"的分裂。

## 2. markdown 管线接 hljs（`utils/markdown.ts`）

```ts
import { marked } from "marked";
import markedHighlight from "marked-highlight";
import { renderCodeHtml } from "./highlight";

marked.use(markedHighlight({
  langPrefix: "hljs language-",
  emptyLangClass: "hljs",
  highlight(code, lang) {
    return renderCodeHtml(code, (lang ?? "").toLowerCase());
  },
}));
marked.setOptions({ gfm: true, breaks: true });  // 保持现有
```

- `renderMarkdown` 仍 `marked.parse → DOMPurify.sanitize`，hljs 高亮在 marked 内部完成。
- **DOMPurify 兼容**：现有 `PURIFY_CONFIG`（`USE_PROFILES:{html:true}` + `ADD_ATTR:["target","rel"]`）默认保留 `<span>` 和 `class` 属性，hljs 输出的 `<span class="hljs-keyword">` 不被 strip。`markdown.test.ts` 的 XSS fixtures 必须仍绿（hljs 输出是 escape 过的 span，不引入新 XSS 面）。

## 3. `<CodeBlockPrimitive>` 组件

```vue
<script setup lang="ts">
import { ref, computed } from "vue";
import { renderCodeHtml } from "../../utils/highlight";
import type { UiPrimitive } from "../uiCard.types";

const props = defineProps<{ primitive: UiPrimitive }>();
const code = computed(() => String(props.primitive.code ?? ""));
const language = computed(() => String(props.primitive.language ?? ""));
const highlighted = computed(() => renderCodeHtml(code.value, language.value));
const copied = ref(false);
async function copyCode() {
  try {
    await navigator.clipboard.writeText(code.value);
    copied.value = true;
    setTimeout(() => { copied.value = false; }, 2000);
  } catch { /* clipboard 不可用（非安全上下文等）→ 静默 */ }
}
</script>
<template>
  <div class="ui-prim ui-prim--code">
    <div class="ui-prim__head">
      <span class="ui-prim__type">{{ language || "code" }}</span>
      <span v-if="primitive.title" class="ui-prim__title">{{ primitive.title }}</span>
      <button class="ui-prim__copy" @click="copyCode">{{ copied ? "已复制" : "复制" }}</button>
    </div>
    <pre class="ui-prim__code"><code v-html="highlighted"></code></pre>
  </div>
</template>
```

- 不走 marked（primitive 是独立结构化卡片，不是 markdown 文本）；自己 `renderCodeHtml` → `v-html`。
- 复制按钮：`navigator.clipboard.writeText` + 2s 反馈。clipboard API 在非安全上下文（http）会抛 → try/catch 静默（Tauri 是 https/file，正常）。

## 4. registry 替换（`uiPrimitiveRegistry.ts`）

```ts
import CodeBlockPrimitive from "./primitives/CodeBlockPrimitive.vue";
import MockPrimitive from "./primitives/MockPrimitive.vue";

export const UI_PRIMITIVE_REGISTRY: Record<string, Component> = {
  diff: MockPrimitive,           // Child C 替换
  code_block: CodeBlockPrimitive, // 本 child
};
// fallback 仍 MockPrimitive
```

## 5. use_ui.rs description 补字段说明

`code_block` 的 `code` / `language` 字段在 description 文字里补一句（schema `additionalProperties: true` 不变，校验仍只查 `type`）：
> `code_block` 字段：`code`（string，必填）、`language`（string，可选，如 "rust"/"python"；省略则自动检测）。

## 关键设计决策

| 决策 | 选择 | 理由 | 否决的备选 |
|---|---|---|---|
| hljs 语言集 | `lib/common` | 主流覆盖，~30KB | full（~900KB 浪费）/ 按需（管理烦） |
| 两个入口共用 `renderCodeHtml` | 单 helper | 语言集分裂不可能 | 各自配 hljs |
| primitive 高亮方式 | 组件内 `hljs.highlight` + `v-html` | primitive 是结构化卡片，不走 markdown 文本管线 | 复用 marked（语义错位，primitive 不是 md） |
| DOMPurify 配置 | 不改（默认保留 span/class） | hljs span 不被 strip | 加 ADD_TAGS（无必要，先验证） |

## 兼容性 / 回归

- `markdown.ts` 改动是 `marked.use(markedHighlight)`，不改 `renderMarkdown` 签名 → MessageItem / MarkdownDetailModal 调用方零改动。
- `markdown.test.ts` 必须仍绿（XSS fixtures + 基本渲染）。若 DOMPurify strip 了 hljs class，加 `ADD_TAGS: ["span"]`（但默认应已允许）。
- registry 替换是单行 import 改动，UiCard / dispatch 逻辑零改动（Child A 的可扩展性兑现）。
- bundle +~30KB（hljs common），可接受（个人工具）。

## 风险点

- **marked-highlight 与 marked 18 兼容**：marked 18 的 `marked.use(extension)` API。marked-highlight 是官方配套扩展，兼容。→ 装后 `vue-tsc` + 运行时验证。
- **DOMPurify strip hljs class**：若高亮 class 被干掉，代码块无颜色但结构正常。→ markdown.test.ts 加一个"hljs class 保留"断言。
