# Implement — B9-B code_block primitive

## 执行清单（有序）

1. **装依赖**
   - `cd app && pnpm add highlight.js marked-highlight`
   - 确认 `marked-highlight` 兼容 `marked@18`（peerDependencies）
2. **`utils/highlight.ts`**（新建）
   - `import hljs from "highlight.js/lib/common"`
   - `export renderCodeHtml(code, language)`（known lang → highlight；else highlightAuto）
3. **`utils/markdown.ts`**（接 marked-highlight）
   - `import markedHighlight from "marked-highlight"` + `import { renderCodeHtml } from "./highlight"`
   - `marked.use(markedHighlight({ langPrefix:"hljs language-", emptyLangClass:"hljs", highlight(code,lang){ return renderCodeHtml(code,(lang??"").toLowerCase()) } }))`
   - 保留现有 `setOptions({gfm,breaks})` + `renderMarkdown` 签名不变
4. **`<CodeBlockPrimitive>`**（`components/chat/primitives/CodeBlockPrimitive.vue`）
   - props `primitive: UiPrimitive`；读 code/language/title
   - `renderCodeHtml` 高亮 + 复制按钮（clipboard + 2s 反馈）
   - 样式复用 MockPrimitive 的 token（--color-bg-surface 等）+ hljs 主题 CSS（import "highlight.js/styles/<theme>.css" 在 main.ts 或组件）
5. **hljs 主题 CSS**：在 `main.ts`（或组件内）`import "highlight.js/styles/github-dark.css"`（或项目暗色主题匹配的）。确认项目主题色后选。
6. **registry 替换**（`uiPrimitiveRegistry.ts`）
   - `code_block: CodeBlockPrimitive`（diff 仍 MockPrimitive）
7. **`use_ui.rs` description**：补 code_block 的 code/language 字段说明（schema 不改）
8. **测试**
   - `CodeBlockPrimitive.test.ts`：高亮渲染（含 hljs class）+ 复制按钮点击 → clipboard.writeText（mock navigator.clipboard）+ "已复制" 反馈 + 未知 language 不崩
   - `markdown.test.ts`：补一个"代码块含 hljs class"断言；确认 XSS fixtures 仍绿（无回归）

## 验证命令

```bash
cd app && pnpm exec vue-tsc --noEmit
cd app && pnpm vitest run            # 含 markdown.test 无回归 + 新 CodeBlockPrimitive.test
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib use_ui   # description 改动不破坏 use_ui 测试
# 端到端(可选): pnpm tauri dev,让 LLM 调 use_ui({type:"code_block",code:"fn main(){}",language:"rust"})
```

## 风险点 / 回滚

- **风险**：marked-highlight 与 marked 18 API 不兼容（`highlight` 返回类型）。→ 装后立即 `vue-tsc` + vitest markdown.test 验证；不兼容则查 marked-highlight 版本 release notes。
- **风险**：DOMPurify strip hljs class（高亮失效但功能正常）。→ markdown.test 加 class 保留断言；失效则 `ADD_TAGS:["span"]`。
- **风险**：hljs 主题 CSS 与项目暗色主题冲突。→ 选匹配的 hljs 主题；组件 scoped 不污染全局。
- **回滚**：markdown.ts 改动是 `marked.use(...)` 一段，删掉即恢复；CodeBlockPrimitive + highlight.ts 是新增；registry 单行回滚到 MockPrimitive。

## Review gates

1. 依赖装好 + `utils/highlight.ts` + `markdown.ts` 接好 → `vue-tsc` + `markdown.test` 绿（验证 hljs 接入无回归）
2. `CodeBlockPrimitive` + registry 替换 → `CodeBlockPrimitive.test` 绿
3. use_ui.rs description 改动 → `cargo test --lib use_ui` 绿
4. 端到端（LLM 调 code_block → 高亮 + 复制）→ **Child B done**
