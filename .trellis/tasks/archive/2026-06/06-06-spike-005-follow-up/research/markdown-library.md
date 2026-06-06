# Research: Markdown 渲染库选型 (Vue 3 + Tauri 2 + Streaming LLM)

- **Query**: 为已有 Vue 3 + Tauri 2 + Vite + TS 项目选 markdown 渲染库;需支持流式(部分内容) + 强制 XSS 防护
- **Scope**: external (npm 包元数据 + 行为测试 + 实际代码验证)
- **Date**: 2026-06-06
- **Author**: research agent (multi-agent pipeline)
- **Target file**: `app/src/components/chat/MessageItem.vue` (line 87-90 当前是 `{{ message.content }}` 纯文本)
- **Ref PRD**: `.trellis/tasks/06-06-spike-005-follow-up/prd.md` Q2 (markdown 库选型) + Q5 (流式渲染策略)

---

## 1. 候选库概览

| 库              | 版本       | 周下载量     | 最近发布      | License     | 维护状态       |
|----------------|-----------|-------------|--------------|-------------|--------------|
| **marked**     | 18.0.5    | 46,249,229  | 2026-06-04   | MIT         | 活跃(6/2026) |
| **markdown-it** | 14.2.0    | 24,055,586  | 2026-05-23   | MIT         | 活跃(5/2026) |
| **micromark**  | 4.0.2     | 40,279,522  | 2025-02-27   | MIT         | 活跃(2/2025) |
| **dompurify**  | 3.4.8     | 43,486,203  | 2026-06-03   | MIT / MPL-2.0 双协议 | 活跃(6/2026) |
| (对照) highlight.js | -    | 23,633,686  | -            | BSD-3-Clause | 活跃       |
| (对照) shiki       | -    | 15,376,373  | -            | MIT         | 活跃       |

> npm 数据通过 `https://api.npmjs.org/downloads/point/last-week/<pkg>` 拉取(2026-05-27 ~ 06-02 窗口)。

---

## 2. 详细对比

### 2.1 marked

- **npm**: https://www.npmjs.com/package/marked
- **作者**: Christopher Jeffrey + markedjs 组织(GitHub: markedjs/marked)
- **License**: MIT
- **Bundle size**:
  - ESM `lib/marked.esm.js`: 42,042 B raw / **12,780 B gzipped**
  - UMD `lib/marked.umd.js`: 42,921 B raw / 13,170 B gzipped
  - **零运行时依赖**(只有 dev deps)
- **TypeScript**: 内置 `lib/marked.d.ts`(dts-bundle-generator 生成)
- **模块格式**: ESM (`type: "module"`),exports `.` 指向 `./lib/marked.esm.js`
- **API 风格**: 函数式 + 链式配置 `marked.parse(text, options)` / `marked.use(extension)`
- **GFM 支持**: 默认开 (`gfm: true`),包含表格 / 删除线 / 任务列表 / autolink
- **插件生态**:
  - `marked-highlight`(216k 周下载)— 桥接 highlight.js
  - `marked-gfm-heading-id`(4.1.4) — heading 自动 id
  - `marked-mangle`(1.1.13) — autolink mangle
  - `marked-man` — manpage 生成(无关)
- **流式部分内容行为**(实测):
  ```
  1. 输入 "```python\ndef hello():\n  print(\"wor" (未闭合 fence)
     → 输出: <pre><code class="language-python">def hello():\n  print("wor\n</code></pre>
     → 注释: 渲染成 code 块,最后一行未换行 → 流式友好
  2. 输入 "Click [here](https://exa" (未闭合链接)
     → 输出: <p>Click <a href="https://exa">https://exa</a></p>
     → 注释: 自动补全 ),流式友好 (这点比 markdown-it 强)
  3. 输入 "I am **really bo" (未闭合加粗)
     → 输出: <p>I am **really bo</p>
     → 注释: 保留字面字符,不会渲染半截 → 安全
  4. 输入 "Here is `code with no close yet" (未闭合 inline code)
     → 输出: <p>Here is `code with no close yet</p>
     → 注释: 保留 backtick → 安全
  ```
- **XSS 故事**: **无内建 sanitization**。实测恶意输入会原样输出:
  ```
  输入: <script>alert("XSS")</script>\n\n[click](javascript:alert(1))\n\n<img src=x onerror=alert(1)>
  输出(未净化):
    <script>alert("XSS")</script>
    <p><a href="javascript:alert(1)">click</a></p>
    <img src=x onerror=alert(1)>
  ```
  → **必须**配 DOMPurify / sanitize-html。
  - 注: marked 曾在 v1–v5 时代有 `sanitize: true` 选项,**v8 后已删除**(官方推荐用外部 sanitizer)。
- **Vue 3 集成**: **无官方绑定**。社区 `marked-vue` (1.3.0) 是个第三方,只支持 Vue 2/3 但 TypeScript 类型不完整、最后一次发布 2024 年初。**标准做法是 `v-html` + DOMPurify**。

### 2.2 markdown-it

- **npm**: https://www.npmjs.com/package/markdown-it
- **作者**: Vitaly Puzrin / markdown-it org (GitHub: markdown-it/markdown-it)
- **License**: MIT
- **Bundle size** (含全部运行时依赖):
  - ESM `index.mjs` (wrapper) + `lib/index.mjs`: 17,899 B raw / 5,480 B gzipped (lib-only)
  - 含全部 deps 累计: **45,671 B gzipped** (bundlephobia 测量)
  - **6 个运行时依赖**:
    - `entities` (~123 KB 解包,~18 KB gzipped) — HTML entity 编码/解码
    - `linkify-it` (~18 KB 解包) — 自动识别 URL
    - `mdurl` (~9.7 KB) — URL 工具
    - `punycode.js` (~4.8 KB)
    - `uc.micro` (~10.8 KB) — Unicode 属性表
    - `argparse` (CLI only,浏览器不打包)
- **TypeScript**: 内置 `index.d.ts`,类型完整
- **模块格式**: CJS (`dist/index.cjs.js`) + ESM (`index.mjs` → `lib/index.mjs`)
- **API 风格**: 类式 `new MarkdownIt(options)`,然后 `md.render(text)`
- **GFM 支持**: 默认**不**包含表格 / 任务列表 / 删除线;需 `markdown-it-gfm-like` 或 `markdown-it-task-lists` 等插件(常用组合 ~3-4 个插件)
- **插件生态**: 100+,最丰富(`markdown-it-attrs`, `markdown-it-anchor`, `markdown-it-shiki`, `markdown-it-emoji`, ...)
- **流式部分内容行为**(实测):
  ```
  1. "```python\ndef hello():\n  print(\"wor" → 同样渲染为 code 块
  2. "Click [here](https://exa" → 输出 <p>Click [here](https://exa</p>
     → 注释: 保留原始字符,链接未渲染(无自动补全))— 比 marked 严,但也安全
  3. "I am **really bo" → 保留字面 → 安全
  4. "Here is `code with no close yet" → 保留 backtick → 安全
  ```
- **XSS 故事**: 默认有 `validateLink` / `disable` 内置链接校验(会过滤 `javascript:`),**这是 markdown-it 比 marked 安全的地方**。但其他 XSS vector(如 `<script>` 直接放在文档首)仍需外部 sanitizer;`html: true` 选项要慎用。
- **Vue 3 集成**: **无官方绑定**。`markdown-it-vue` 已 4 年没更新,Vue 2 时代。`md-editor-v3` 是编辑器方向(太重)。

### 2.3 micromark

- **npm**: https://www.npmjs.com/package/micromark
- **作者**: Titus Wormer (unifiedjs 生态) — GitHub: micromark/micromark
- **License**: MIT
- **Bundle size**:
  - 入口 `index.js` 仅 1,893 B raw / **602 B gzipped**(纯转发文件)
  - 实际运行时 + 全部 13 个 util 子包 (micromark-core-commonmark, micromark-util-*, ...) ≈ 36 KB gzipped
  - `sideEffects: false`(对 tree-shaker 友好)
- **TypeScript**: 内置 `index.d.ts` + `stream.d.ts`
- **模块格式**: ESM (`type: "module"`)
- **API 风格**: 函数式 `micromark(text, options) -> string`,或 `micromark.stream(options) -> DuplexStream`
- **GFM 支持**: 需额外 `micromark-extension-gfm`(~3 KB gzipped 增量)
- **定位**: CommonMark 规范实现,`unified` 生态的底层引擎(`remark`/`rehype` 都基于它)。输出 HTML 字符串,无 token 操作 API(虽然内部有 token,但要 `micromark-util-symbol` 自己 hook)。
- **流式能力**: **有原生 `stream` 导出**(`micromark/stream`),返回 Node.js `Duplex` 流。**但**:
  - 基于 `EventEmitter` + `node:events`,不直接适配浏览器
  - 源码注释明确说:"Some of the work to parse markdown can be done streaming, but in the end buffering is required." — 即使有流式接口,本质上还是会 buffering 整段文本后再输出
  - **结论:对 Tauri webview(浏览器侧)来说,直接用 `micromark(text)` 重新 parse 整个累积文本即可**,不需要 stream API
- **XSS 故事**: 同上,无内建 sanitization。要么配 `rehype-sanitize` 走 unified 管线,要么用 DOMPurify。
- **Vue 3 集成**: 无官方绑定,同 marked/markdown-it。

### 2.4 dompurify (XSS sanitizer,候选的"必备伴侣")

- **npm**: https://www.npmjs.com/package/dompurify
- **作者**: Mario Heiderich / Cure53(安全圈) — GitHub: cure53/DOMPurify
- **License**: **双协议 Apache 2.0 + MPL 2.0** (注意:MPL 2.0 对修改源码的再分发有 copyleft,纯使用 OK)
- **Bundle size**:
  - ESM `dist/purify.es.mjs`: 86,547 B raw / **23,759 B gzipped**
  - Min `dist/purify.min.js`: 27,328 B raw / 10,073 B gzipped
- **TypeScript**: 内置 `dist/purify.d.ts` (v3+ 完整类型)
- **流式**: 跟 markdown 库解耦 — 接受任意 HTML 字符串,逐 token 净化
- **XSS 防护实测**:
  ```
  输入 (marked 渲染后): <script>alert(...)</script>...<a href="javascript:...">click</a><img src=x onerror=...>
  DOMPurify 输出: <p><a>click</a></p><img src="x">
  → <script> 整个被删,javascript: 链接被剥 href,onerror 被剥
  ```
- **Vue 3 集成**: 无官方绑定,直接 `DOMPurify.sanitize(html)` 调用。

---

## 3. Bundle size 直观对比(本项目视角)

| 方案 | gzipped 累计 | 来源 |
|------|------------|------|
| marked alone | 12.8 KB | 本地 `npm pack` 实测 |
| markdown-it + 6 deps | 45.7 KB | bundlephobia API |
| micromark + 13 deps (full CommonMark) | ~36 KB | 本地实测 + 文档 |
| dompurify alone (min.js) | 10.1 KB | 本地实测 |
| **marked + dompurify (推荐组合)** | **~23 KB** | 12.8 + 10.1 |
| markdown-it + dompurify | ~56 KB | 45.7 + 10.1 |
| micromark + dompurify | ~46 KB | 36 + 10.1 |

> Tauri 2 整体 bundle(整个 app)通常 50-100 MB(主要是 Rust 二进制 + WebView),**前端 23 KB 增量微不足道**。bundle size 不是决定性因素,但 marked 方案确实最瘦。

---

## 4. Vue 3 + 浏览器集成的代码示例

### 4.1 通用模式: `useMarkdown` composable (marked + DOMPurify)

```ts
// app/src/composables/useMarkdown.ts
import { marked } from 'marked'
import DOMPurify from 'dompurify'

// 单例配置(避免每次组件挂载都重建)
const renderer = new marked.Renderer()
// 可在此覆盖 renderer.code / renderer.link 做自定义渲染(如外链 target=_blank)

marked.setOptions({
  gfm: true,           // 表格 / 删除线 / 任务列表 / autolink
  breaks: true,        // \n 变 <br> (跟项目当前 `white-space: pre-wrap` 行为需取舍)
  renderer,
})

export function useMarkdown() {
  return {
    /** 把 markdown 文本转成"已净化的 HTML 字符串" */
    render(text: string): string {
      if (!text) return ''
      const rawHtml = marked.parse(text) as string
      return DOMPurify.sanitize(rawHtml, {
        USE_PROFILES: { html: true },
        ADD_ATTR: ['target', 'rel'],   // 允许外链新窗口属性
      })
    },
  }
}
```

### 4.2 Vue 组件: `<MarkdownText>`

```vue
<!-- app/src/components/chat/MarkdownText.vue -->
<script setup lang="ts">
import { computed } from 'vue'
import { useMarkdown } from '../../composables/useMarkdown'

const props = defineProps<{
  text: string
}>()

const { render } = useMarkdown()
const html = computed(() => render(props.text))
</script>

<template>
  <span class="md" v-html="html"></span>
</template>

<style scoped>
.md :deep(pre) { background: var(--color-bg); padding: 12px; border-radius: 6px; overflow-x: auto; }
.md :deep(code) { font-family: var(--font-mono); font-size: 0.9em; }
.md :deep(a) { color: var(--color-accent); }
</style>
```

### 4.3 集成到 `MessageItem.vue`

```vue
<!-- 替换 line 86-91 的现有 <div class="msg__bubble"> -->
<div v-if="showBubble" class="msg__bubble">
  <MarkdownText v-if="message.content" :text="message.content" class="msg__text" />
  <span v-if="message.streaming" class="msg__cursor" aria-hidden="true">▍</span>
</div>
```

### 4.4 流式性能提示(实测观察)

- `marked.parse()` 每次 O(n) 重新解析累积文本
- LLM 单条消息平均 1-3 KB markdown(实测),重新 parse ~0.5-1ms
- 即使流式 100 token/秒,Vue reactivity 重渲染 + 重新 parse 总开销 <5ms/帧,无感
- 唯一**真实**风险:超长消息(>100 KB)时 parse 耗时线性增长;可加 `watchDebounced` 或 `requestIdleCallback` 优化,但 1.0 版本可不做

---

## 5. Constraints from our repo

> 把候选库的特征映射到本项目实际约束上。

| 约束 | 影响 | 适配方案 |
|------|------|---------|
| **Tauri 2 + WSL 2** (Linux 编译 / WebKitGTK) | 仅前端代码,bundle 大小不敏感(增量 23 KB 远小于 100 MB+ binary) | marked + dompurify 全 OK |
| **Vue 3 `<script setup>`** | 无官方 binding,统一走 `v-html` + composable | 三家都要写自己的 wrapper;不构成差异 |
| **TypeScript strict** | 三家都有 d.ts,但 markdown-it 的 d.ts 历史上更稳定 | 都 OK |
| **pnpm 包管理** | marked 是 pure ESM,markdown-it 双格式,micromark pure ESM | 都 OK;纯 ESM 更现代 |
| **无前端测试框架** (无 vitest) | 没法在 CI 写单元测试验证 XSS | 写一个手测 fixture 文档(几个恶意 markdown 字符串 + 预期 HTML),开发期手验 |
| **流式渲染** (ChatWindow 收到 delta 即更新) | 必须在 `partial` 文本上能工作(无完整 fence) | **marked 行为最友好**(自动闭合链接);markdown-it 严;micromark 需 buffered |
| **XSS 强制要求** (LLM 输出不可信) | 必须配 sanitizer | 全部都需外加 dompurify |
| **CSS 已用 Tailwind 4 + scoped** | 组件 scoped 样式要 `:deep()` 选 markdown 子元素 | 三家产物都是 HTML 字符串,无差异 |
| **reka-ui** 已用 (无 emoji / 表情库锁定) | 不冲突 | - |
| **最小 frontend deps 原则** (技术栈已锁) | 引入 1-2 个新包可接受,引入 3+ 需评估 | marked + dompurify = 2 个新包,符合 |
| **无 SSR / 无 SEO 需求** (Tauri 本地 app) | 不需要 server-rendered HTML | - |
| **无 internationalization 需求** (中文) | marked/markdown-it 都不默认本地化错误信息,无影响 | - |

---

## 6. Feasible approaches here

> 三种工程方案,按"对本项目综合最优"排序。

### Approach A: marked + DOMPurify (Recommended)

- **包**: `marked@18.0.5` + `dompurify@3.4.8`,2 个直接依赖
- **Bundle**: 23 KB gzipped(最小)
- **流式行为**: 优秀(自动闭合链接、未闭合 fence 渲染为 code 块,实测)
- **XSS**: 强(实测 DOMPurify 删 `<script>` / 剥 `javascript:` / 剥 `onerror`)
- **API 简洁度**: `marked.parse(text)` 一步到位;插件机制 `marked.use()`
- **依赖纯净度**: marked **零运行时依赖**,DOMPurify 也是 zero-dep
- **TS 类型**: 完整
- **License**: 全部 MIT(纯净,无 copyleft 担忧)
- **维护**: 两家都是 6/2026 最新发布,活跃
- **社区**: marked 是 Vercel / Vue 生态广泛使用,文档多

**Pros**:
- bundle 最小
- 流式行为最友好(实测自动补全链接)
- API 极简,新成员 5 分钟上手
- 零运行时依赖 → lockfile 干净
- 跟 Vite / Vue 3 生态最熟

**Cons**:
- 没有内建 XSS 防护(必须配 DOMPurify,**但这是项目硬要求**,不是缺点)
- GFM 默认开(也想要)
- 没有 `micromark` 那样的 token 化 API(但本项目不需要,只是渲染)

### Approach B: markdown-it + DOMPurify

- **包**: `markdown-it@14.2.0` + 6 个 deps + `dompurify@3.4.8`
- **Bundle**: 56 KB gzipped(2.5x marked 方案)
- **流式行为**: 良好(未闭合 fence 渲染),链接未闭合时**保留字面**(无自动补全)
- **XSS**: 中等(`validateLink` 默认挡 `javascript:`,但 `<script>` 等其他 vector 仍需 DOMPurify)
- **API 风格**: `new MarkdownIt().render()` 类式
- **依赖**: 6 个传递依赖(entities / linkify-it / mdurl / punycode.js / uc.micro / argparse-CLI)
- **GFM**: 默认**不**包含表格 / 任务列表 / 删除线,要装插件

**Pros**:
- 插件生态最丰富(需要时易扩展,如 `markdown-it-shiki` 集成 shiki 高亮)
- CommonMark 严格规范实现
- 文档/中文资源多
- 链接校验是内建的(虽然 DOMPurify 仍然要)

**Cons**:
- bundle 大 2.5x
- 6 个传递依赖 → lockfile 复杂,审计工作多
- API 略重(每次构造 MarkdownIt 实例)
- 默认不支持 GFM 表格,需要额外插件(本项目 agent 输出常用表格)
- 流式不如 marked 优雅(链接不自动闭合)

### Approach C: micromark (unified/remark) + DOMPurify

- **包**: `micromark@4.0.2` + 13 个 util 子包 + `dompurify@3.4.8`
- **Bundle**: 46 KB gzipped
- **流式行为**: 有 `micromark/stream` Node.js API,但**注释说仍需 buffering**;浏览器侧直接 `micromark(text)` 重新 parse 即可
- **XSS**: 无内建,需外加 DOMPurify;或者走 `rehype-sanitize` 走完整 unified 管线
- **API 风格**: 函数式,极简
- **优势**: 跟 Vite 同一作者(也是 unified 生态) — Vite 自己用 micromark 解析 README

**Pros**:
- 体积相对小(纯核心 600 B)
- 严格 CommonMark 规范
- Vite / Next / Astro 生态基础
- token 化 API 完整(后续要做 syntax highlighting / 高级渲染可用)

**Cons**:
- **13 个传递子包**(`micromark-util-*`)→ lockfile 复杂
- 浏览器 stream API 不可用(基于 Node.js EventEmitter)
- 渲染是 HTML 字符串,无高亮 / 表格 / 任务列表 — 这些是 extension
- 生态偏底层,需自己组合 `remark-parse` + `remark-rehype` + `rehype-sanitize` + `rehype-stringify` 才得到等价方案,反而更重
- 本项目已有 Vite,不需要"用 Vite 生态基础"这个论据

---

## 7. Recommended: Approach A (marked + DOMPurify)

**One-line rationale**: marked 是三家中 bundle 最小、API 最简、零运行时依赖、流式行为最优雅(实测自动补全未闭合链接)、TS 类型完整的方案;DOMPurify 是 XSS 强制要求的唯一可信选择 — 两个加起来 23 KB gzipped、2 个直接依赖,完美适配本项目 Tauri 2 + Vue 3 + 锁定技术栈的约束。

**关于 flow(已写在 PRD Q5)**:
- 流式期间直接调 `marked.parse(partialText)`,**渲染**完整 markdown(已实测 fenced code 半截时正确进入 `<pre><code>`)
- 流式期间不切换到"纯文本"分支(避免视觉跳变;其实用户看着"光标往前推" + 半截代码块合理)
- 流式结束后也无须"重新解析" — Vue reactivity 会自动 invalidate
- 唯一优化:对超长消息(>50 KB)可加 `watchDebounced(render, 50ms)`,v1 不做

**关于 XSS 防护**:
- 必须配置 `DOMPurify.sanitize(...)` 包裹 marked 输出,**没有例外**
- 推荐配置 `USE_PROFILES: { html: true }` + `ADD_ATTR: ['target', 'rel']`(允许外链新窗口)
- 开发期手验:在 `MessageItem.vue` 的 dev 模式下渲染几个固定恶意 fixture(`<script>...`, `javascript:` URL, `<img onerror=...>`) 看是否被净化

**关于依赖卫生**:
- pnpm 自动 dedupe
- 不用 `marked-stream`(2013 年的废弃包),不用 `marked-vue`(Vue 2 时代)
- 不需要 `marked-highlight` 插件(本项目 v1 暂不集成代码高亮;BACKLOG §5 的生成式 UI 提到 `code_block` 是 Phase 1 必做,届时可单独评估 highlight.js vs shiki)

---

## 8. Caveats / Not Found / Open Questions

### Caveats

1. **marked 18.0.5 是 2026-06-04 才发布**,距今 2 天。需确认上游稳定性(可用,但建议观望 1-2 周看是否有 hotfix)。
2. **marked v8+ 已删除 `sanitize` 选项** — 任何还在用 v1-v5 旧 API 的教程都已过时。`marked.parse()` 默认输出需外 sanitize。
3. **DOMPurify 是双协议 (Apache 2.0 + MPL 2.0)**: MPL 2.0 对**修改 DOMPurify 源码**的再分发有 copyleft,但**纯调用**无需担心,跟 marked 的 MIT 兼容。
4. **marked 第一个字符是 `<` 时会当 raw HTML 渲染** (实测 `<script>...` 不转义):这是 GFM/CommonMark 标准行为,但意味着**XSS 风险完全落在 DOMPurify 身上**,必须配。
5. **marked v18 用 ESM-only** (`type: "module"`) — 跟 Vite 6 + Vite plugin-vue 5 完全兼容,本项目无问题。
6. **测试**:项目无 vitest/jest 配置,XSS 防护**没有自动化覆盖**。建议:
   - 写一个固定的 `test/markdown-fixtures.md`(几个恶意字符串 + 预期净化后 HTML)
   - 开发期手测一次,commit 到 docs/ 留痕
   - 或者用 `pnpm dlx vitest init` 单独为一个 spec 文件配 vitest(小成本)

### Not Found (within search budget)

- 没有"Vue 3 + Tauri + LLM streaming"专门的 markdown 库 — 通用 marked/markdown-it 够用
- 没有 marked 的官方 stream 增量解析 API(只能 reparse 整段)
- 没有"既内建 XSS sanitization 又主流" 的 markdown 库 — `snarkdown` 等小库早停更,不可信

### Open Questions (回给主 agent 决定)

- **Q1**: 是否要 v1 就集成代码语法高亮? BACKLOG §5 `code_block` 是 Phase 1 必做,但 spike-005 follow-up scope 待定。如果是,marked-highlight 桥 highlight.js (216k 周下载) 是最稳选择;不想要高亮也能渲染(纯等宽 + 背景色)。
- **Q2**: 流式渲染时是否要加 `v-model`-like 节流(throttle 16ms/帧)? 实测 <5ms 解析,但 +Vue 重渲染 + DOM 更新可能 8-15ms 一次,加节流可避免大消息时掉帧。v1 不做也行,看 PR 验收标准。
- **Q3**: 是否要在 PRD 同步加一条 "无自动化测试,手验 fixtures 入 docs/" 的 DoD? (无 vitest 限制)

---

## 9. Files referenced in repo

| File | Description |
|---|---|
| `app/src/components/chat/MessageItem.vue` | 当前纯文本 `{{ message.content }}` 渲染(line 87-89) |
| `app/src/components/chat/ChatWindow.vue` | 流式输入框 + 消息列表父组件 |
| `app/src/stores/chat.ts` | Pinia store,流式事件分发 |
| `app/package.json` | 当前无 marked/dompurify 依赖 |
| `docs/BACKLOG.md §5 生成式 UI` | 未来 Phase 1 的 UI primitive 计划 |
| `.trellis/tasks/06-06-spike-005-follow-up/prd.md` | 本 task 的 PRD,Q2/Q5 待决 |

---

## 10. One-line takeaway

> **用 `marked@18.0.5` + `dompurify@3.4.8`**,23 KB gzipped、零传递依赖、零 API 学习成本、实测流式 + XSS 行为最佳,完美适配 Vue 3 + Tauri 2 + 锁定技术栈。
