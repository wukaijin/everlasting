# HACKING-markdown: Markdown 渲染 + XSS 防护

> 写给未来的自己(或下个 session),改 markdown 渲染 / 加 sanitizer 配置 / 评估新库时别再踩 marked v18 删 `sanitize` 选项的坑。
>
> **触发场景**:改 `app/src/utils/markdown.ts`、改 `MessageItem.vue` 的 `v-html`、评估新的 markdown 库、加自定义 marked 扩展、调试 XSS fixture 失败。

---

## 现状一句话

- 库: `marked@18.0.5`(lockfile 锁精确版本,无 `^`/`~`)+ `dompurify@3.4.8`(lockfile 锁精确版本)
- 入口: `app/src/utils/markdown.ts` 的 `renderMarkdown()`(同步)+ `createDebouncedRenderer()`(流式 reactive)
- 集成点: `app/src/components/chat/MessageItem.vue` 的 `<span class="msg__markdown" v-html="rendered" />`
- 测试: `app/src/utils/markdown.test.ts`(14 个 fixture,vitest 跑)
- 流式: 50ms debounce 合并 SSE delta,流结束(`streaming=false`)时 `flush()` 立即渲染

---

## 关键陷阱:marked v8+ 已删 `sanitize` 选项

**这是 PR6 实施时最关键的安全决策,任何 markdown 相关改动都必须重新检查这一条。**

### 背景

- marked v1–v5: 有内建 `sanitize: true` 选项,自动剥 `<script>` / `onerror=` 等
- marked v5 之后: `sanitize` 选项**已被废弃**
- **marked v8+**: `sanitize` 选项**已被完全删除**。任何传 `sanitize: true` 的代码现在直接被忽略,marked 输出**完全未净化**,`<script>` 原样出现
- 官方推荐: 用外部 sanitizer (DOMPurify / sanitize-html)

### 项目对策(强制)

`renderMarkdown()` 函数**必须**把 `marked.parse()` 的输出**无例外**地过一遍 `DOMPurify.sanitize()`。代码里这条路径是:

```ts
const rawHtml = marked.parse(trimmed) as string;
return DOMPurify.sanitize(rawHtml, PURIFY_CONFIG);
```

**没有任何合理理由绕过 DOMPurify**:
- 觉得"内部项目不需要 XSS 防护"?错。LLM 输出不可信,恶意指令可能诱导 agent 输出含 `<script>` 的 markdown
- 觉得"v-html 跟 Vue scoped CSS 冲突想用别的方式"?Vue 提供 `:deep()` 处理,不要回到 `{{ }}` 纯文本(那样 markdown 就不渲染了)
- 觉得"DOMPurify 太大了"?gzipped 10 KB,Tauri 2 整体 100+ MB,微不足道

### 如何验证没绕过

`pnpm test` 的 XSS fixture 套件 6 个用例(`<script>` / `<img onerror>` / `<a href="javascript:">` / `[text](javascript:)` / 内联事件 / `<iframe>`)都断言输出不含危险字符。**任何改 markdown.ts 的提交都必须让这 6 个 fixture 全绿**。要加新 fixture?直接在 `markdown.test.ts` 加 `it(...)`,CI 会拦。

---

## DOMPurify 配置说明

```ts
const PURIFY_CONFIG: DOMPurify.Config = {
  USE_PROFILES: { html: true },
  ADD_ATTR: ["target", "rel"],
};
```

- `USE_PROFILES: { html: true }`: 允许标准 HTML 标签,不引入 SVG / MathML profile(我们不需要)
- `ADD_ATTR: ["target", "rel"]`: 默认 DOMPurify 会剥这两个属性(防止 `target="_blank"` 缺 `rel="noopener"` 的 tabnabbing 攻击),但我们将来想在外链上加 `target="_blank" rel="noopener"`,提前放行。要用时记得在 renderer 钩子里手动设 `rel="noopener"`

**默认就剥的**(无需配置,本项目依赖):
- `<script>` 整段
- 所有 `on*` 事件属性 (`onclick` / `onerror` / `onload` / ...)
- `javascript:` 协议的 `href` / `src`
- `data:` 协议(部分)
- `<iframe>` / `<object>` / `<embed>` 整段
- `<form>`(可能引入 form-jacking)

---

## 流式 debounce 设计

### 为什么是 50ms

- 实测单条 LLM 消息平均 1-3 KB markdown,`marked.parse()` + `DOMPurify.sanitize()` 一次约 0.5-1 ms
- Vue reactivity 重渲染 + DOM 更新一次约 5-10 ms
- 50 ms 远大于单帧处理时间,远小于人眼可感知的延迟(>100ms 才有感)
- SSE 高峰期 100 token/s,1 token ≈ 4 字符 ≈ 16 字符/50ms,debounce 把 5-10 次 delta 合并成 1 次重渲染

### 为什么用 factory + watch(不直接 computed)

- `computed` 同步重算,无法 debounce
- `watch` + `setTimeout` + `clearTimeout` 经典模式,50ms 窗口内 burst 合并成 1 次
- `flush()` 在 `streaming=false` 时同步触发,不让用户等 50ms 才看到终态
- `dispose()` 在 `onUnmounted` 触发,避免 message list 抖动时 timer 持有旧闭包

### 内存泄漏风险点

- `createDebouncedRenderer` 内部 `setTimeout` 闭包持有 `pendingText` / `lastScheduled`,如果 message 在 debounce 窗口内被 unmount,timer 仍会 fire,更新一个**已不存在的** `Ref`(`ref` 不再被任何组件 observe,但引用还在,直到下一次 GC)
- 解法: `onUnmounted(() => dispose())`,清 timer + 置 null pendingText
- 这是 PR6 review 时的重点检查项之一

---

## 已覆盖的 XSS fixture 列表(vitest 跑)

| 输入 | 预期净化结果 |
|------|------------|
| `<script>alert("XSS")</script>` | `<script` 子串不存在,`alert(` 不存在 |
| `<img src=x onerror=alert(1)>` | `onerror` 子串不存在(忽略大小写) |
| `<a href="javascript:alert(1)">x</a>` | `javascript:` 子串不存在(忽略大小写) |
| `[click me](javascript:alert(1))` | 同上 |
| `<div onclick="..."><span onmouseover="...">...` | `onclick` / `onmouseover` 不存在 |
| `<iframe src="https://evil.example">` | `<iframe` 子串不存在 |

**这些 fixture 在 CI 跑,任何 PR 改 markdown.ts / package.json(deps) 都必须保持全绿**。要加新 fixture(比如新的 XSS vector 公开了)直接在 `markdown.test.ts` 加 `it(...)`。

---

## 常见踩坑

### 1. `white-space: pre-wrap` 跟 `<pre>` 冲突

**现象**: markdown 渲染的代码块(`<pre><code>`)缩进/换行乱了

**根因**: `.msg__bubble` 之前的 `white-space: pre-wrap` 会把 `pre` 里的所有空白按字面渲染,跟 `<pre>` 自己的渲染叠加,出现怪异的双倍缩进

**修法**(PR6 已做): `.msg__bubble` 删 `white-space: pre-wrap`,换行交 `marked` 的 `breaks: true` 处理

### 2. 解析时机:trim 必须在 `marked.parse()` 之前

**现象**: LLM 第一条消息首字符是 `*`(列表项),渲染出来只剩 `<em></em>` 空标签,item 不见

**根因**: marked 把首字符 `*` 当 emphasis 开始,` * item one` 解析成 `<em>item one</em>`,item 没了

**修法**(PR6 已做): `renderMarkdown()` 第一行就 `text.replace(/^\s+/, "")`,trim 后再 parse。PR7 的 `displayContent` 保留作为"显示层"的薄包装,但 trim 实际只走 markdown 路径

### 3. marked 第一个字符是 `<` 时当 raw HTML

**现象**: `<script>...</script>` 输入,marked 输出 `<script>...` 原样(没把它当 markdown 文本包成 `<p>`)

**根因**: GFM/CommonMark 标准行为,首字符是 `<` 触发 raw HTML 模式

**修法**: 不修(标准行为),**完全靠 DOMPurify 兜底**。如果未来某天忘了调 sanitize,这一条会成为漏洞——CI 的 XSS fixture 正是为此设的

### 4. 升级 marked 大版本时

**步骤**:
1. 查 changelog(marked 8 → 9 / 9 → 10 / ... 都有 breaking change)
2. 跑 `pnpm test`,**任何 XSS fixture 红了就回滚**——大概率是新版本渲染逻辑改了,sanitize 兜不住某种边缘 case
3. 手动测一遍流式(`pnpm tauri dev` 实际发条消息),确认 `breaks: true` 行为没变、单换行变 `<br>` 还正常
4. lockfile 改精确版本(无 `^`/`~`),commit

### 5. 加新 marked 扩展(footnote / TOC / math)

**步骤**:
1. `pnpm add <extension-pkg>`,lockfile 锁精确版本
2. 在 `markdown.ts` 顶部 `import` + `marked.use(extension)`,**`marked.use` 必须在 `marked.setOptions` 之后调**
3. 加新 fixture 到 `markdown.test.ts`:**既要正向(扩展语法正确渲染),也要反向(扩展不会绕过 DOMPurify)**
4. 跑 `pnpm test` + 手动 Tauri dev 测一遍

---

## 引用文件

| 路径 | 角色 |
|------|------|
| `app/src/utils/markdown.ts` | 主入口:`renderMarkdown()` + `createDebouncedRenderer()` |
| `app/src/utils/markdown.test.ts` | XSS + 基础 markdown fixture |
| `app/vitest.config.ts` | vitest 配置(jsdom env + `@` alias) |
| `app/src/components/chat/MessageItem.vue` | `v-html` 集成点 + 流式 debounce watch |
| `app/package.json` | deps + scripts(`test` / `test:watch`) |
| `app/pnpm-lock.yaml` | 精确版本锁 |
| `.trellis/tasks/06-06-spike-005-follow-up/research/markdown-library.md` | 选型研究(为何选 marked + DOMPurify) |
| `.trellis/tasks/06-06-pr6-markdown-render/prd.md` | PR6 需求 + Acceptance Criteria |
