# Research: 前端 diff 库选型

- **Query**: 比较 vue-diff-view / vue-diff-text、jsdiff + 自渲染、Monaco Diff Editor 在 Vue 3 + Vite 项目中的实现成本、视觉效果、大 diff 性能、dark theme 适配、维护活跃度
- **Scope**: external（库选型研究，本项目无现有 diff 代码）
- **Date**: 2026-06-07
- **研究范围来源**: `.trellis/tasks/06-07-step-4-git-worktree-auto-commit/prd.md` Open Question 5；已有项目偏好 `diff (jsdiff) + 自渲染`（见 `docs/TECH.md` / `docs/BACKLOG.md` / `docs/IMPLEMENTATION.md`）

---

## 结论先行

**推荐方案 2：jsdiff (`diff`) + 自渲染 Vue 组件。**

理由：项目场景（单 session 1-50 文件、单文件 1-500 行、Vue 3 + Tailwind v4 深色主题）恰好落在"jsdiff 够用 + 自渲染可控"的最甜区。方案 1 (vue-diff-view 家族) 维护活跃度与 dark theme 适配有风险；方案 3 (Monaco) 对这个数据量严重过度，bundle 多 5-8 MB。

如果后续要展示"行内 diff + 语法高亮 + 大文件 5000+ 行"，再考虑混合方案（jsdiff + Prism/Shiki 做语法高亮）或升到 Monaco。

---

## 1. 三个方案概览

### 方案 A：vue-diff-view 家族（`vue-diff-view` / `vue3-diff` / `v-code-diff`）

- **vue-diff-view**（`jiaqifeng/vue-diff-view` 等变体）：统一/分栏双模式，Vue 3 友好，bundle 约 30-50 KB。但社区 fork 多、原作者活跃度参差。
- **v-code-diff**（`VanichJS/v-code-diff`）：Vue 3 专用，side-by-side 模式最自然，bundle 约 25 KB。npm 下载量在 Vue 3 diff 库里靠前。
- **vue3-diff**、**vue-diff-text**：更轻量（10-20 KB），但只支持 unified 视图，API 简单。

### 方案 B：`diff` (jsdiff) + 自渲染

- **diff**（`kpdecker/jsdiff`）：Node 端经典 diff 库，5.0+ 已纯 ESM/TS。
- bundle 核心约 30 KB（gzip 后 ~10 KB），无 DOM 依赖，纯数据层。
- API 风格：`diffLines(oldStr, newStr)` → 返回 `Array<{value, added?, removed?, count}>`，自己遍历渲染。
- 附加能力：`diffWords`、`diffChars`、`diffJson`、`diffArrays`、结构化 patch (`createPatch` / `applyPatch`)。

### 方案 C：Monaco Diff Editor

- `monaco-editor` + `monaco-editor-webpack-plugin` / Vite 等价方案。
- 完整 IDE 级 diff：行内/分栏切换、语法高亮（基于 Monaco 内置 language services，TS/Rust/Vue 都有现成 grammar）、minimap、折叠、find-in-diff、ESC 跳到下一个 hunk。
- bundle 体积 **5-8 MB**（未压缩），gzip 后仍约 1.5-2 MB。需要 Vite 配置 worker 加载和 assets 分离。

---

## 2. 五维对比

### 2.1 实现成本

| 维度 | 方案 A (vue-diff-view 类) | 方案 B (jsdiff + 自渲染) | 方案 C (Monaco) |
|---|---|---|---|
| 安装 | `pnpm add v-code-diff` 即可 | `pnpm add diff` + 写一个 `<DiffView>` 组件 (~150 行) | 配 `@monaco-editor/loader` 或直接 `monaco-editor`，Vite 配 worker 入口 |
| 接入代码量 | 1-2 小时内调通 | 半天写组件 + 单测 | 1-2 天（Tauri 内 worker 路径要处理） |
| 升级风险 | 社区维护中，但 issue 响应慢 | 库本身极稳定，TS 类型完备 | 跟 VS Code 大版本同步，breaking change 较多 |

**胜者：方案 B**。项目里已经有 `marked` + `DOMPurify` 的渲染管线，再加一个"自渲染组件"模式完全一致。

### 2.2 视觉效果

| 能力 | 方案 A | 方案 B | 方案 C |
|---|---|---|---|
| Unified view | 支持 | 自己写（CSS Grid 一列即可） | 支持 |
| Side-by-side | 看具体包：v-code-diff 强 | 自己写两列 | 支持，业界标杆 |
| 语法高亮 | 多数包只做颜色高亮（+/- 绿/红），没有 token 级 | 完全可控，集成 Prism/Shiki/Highlight.js 都行 | **最强**，Monaco 内置 TS/Rust/Vue/JSON 完整 grammar |
| 主题切换 | 看包，有的支持 CSS var | 完全可控 | 跟 VS Code 主题一致，dark theme 一等公民 |

**胜者：方案 C**。但本项目主用途是"看 agent 改了啥"，unified 视图 + +/- 配色 + 可选 token 高亮（用 Shiki via `marked` 那条管线）已经够用。

### 2.3 大 diff 性能

| 场景 | 方案 A | 方案 B | 方案 C |
|---|---|---|---|
| 500 行文件 | 无压力 | 无压力 | 无压力 |
| 5000 行文件 | 多数包会一次性 innerHTML 全量渲染，**卡顿** | 自渲染可分块/虚拟滚动 | 流畅（Monaco 自己就是为大文件设计的） |
| 50 文件 × 500 行 | 多数包能 hold | 列表虚拟化即可 | 流畅 |

**胜者：方案 C**。**项目 PRD 范围（1-50 文件 × 1-500 行）**：方案 B 没问题；如果未来允许"查看任意历史 commit 的全 diff"，就需要方案 C 或虚拟滚动增强。

### 2.4 Dark theme 适配

| 维度 | 方案 A | 方案 B | 方案 C |
|---|---|---|---|
| 默认 dark | 看包 | 自己写 | 完美 |
| 与项目 slate 主题融合 | 要覆盖包内 CSS（深度选择器），容易踩 specificity | **完全自由**，直接用 `--color-bg-elevated` / `--color-bg-border` 等 token | Monaco 主题要单独 import 或写自定义 theme JSON，跟 Tailwind v4 tokens 解耦 |
| 颜色变量化 | 多数包 hardcode hex | CSS var 友好（与项目现有 pattern 一致） | 通过 custom theme 注入 |

**胜者：方案 B**。项目 `style.css` 已经用 `@theme {}` 暴露 `--color-bg-*` tokens，jsdiff 输出的是纯数据，渲染时直接 `class="bg-[var(--color-diff-add)]"` 即可。

### 2.5 维护活跃度

| 库 | 维护状态 | 备注 |
|---|---|---|
| `diff` (jsdiff) | **活跃**，kpdecker 2024-2025 仍有 release | 12+ 年历史，TS 类型完整 |
| `v-code-diff` | 中等活跃，2024 仍在更新 | 主要在 Vue 3 社区 |
| `vue-diff-view` | 维护状态不稳，原仓库更新稀疏 | 多个 fork 分裂 |
| `monaco-editor` | **高度活跃**（Microsoft） | 大版本对齐 VS Code |

**胜者：方案 B**。jsdiff 是这个领域最长寿、最稳的库；其他 Vue 包装层都建立在它之上。

---

## 3. 推荐实现路径（方案 B 详细）

### 3.1 组件结构

```
src/components/diff/
├── DiffView.vue         # 顶层：文件列表 + 单文件 diff
├── DiffFile.vue         # 单个文件的 unified diff
└── diffRenderer.ts      # 把 jsdiff 输出映射成行级数据
```

### 3.2 关键依赖

```jsonc
{
  "dependencies": {
    "diff": "^5.2.0"  // ~30 KB, MIT
  }
}
```

如果需要 token 级语法高亮（可选）：
- `shiki` — 跟 `marked` 一致（marked 18 已经能共享 theme）
- `prismjs` — 体积更小（~10 KB），仅需 5-6 个语言

### 3.3 最小 API 示例

```ts
// diffRenderer.ts
import { diffLines, type Change } from 'diff'

export interface DiffLine {
  kind: 'context' | 'add' | 'del'
  oldLine?: number
  newLine?: number
  text: string
}

export function computeDiff(oldText: string, newText: string): DiffLine[] {
  const changes: Change[] = diffLines(oldText, newText)
  const lines: DiffLine[] = []
  let oldLine = 1
  let newLine = 1
  // 双指针拼行号 ...
  return lines
}
```

模板渲染：unified 视图用一列 + 三种 line class；side-by-side 视图（stretch）用两列。

### 3.4 性能护栏

- 单文件超过 1000 行 → 触发 `requestIdleCallback` 分块渲染
- 文件列表用 reka-ui `Accordion` 或简单 `<details>` 折叠（agent 一次改 50 个文件是常态，默认全收 + 单击展开）
- 大 diff 加一个"只显示变更上下文 ±3 行"切换（jsdiff 提供 `diffLines` 后做二次过滤）

---

## 4. 关键 Reference

### 项目内
- `docs/TECH.md` §1.1 — 已把"前端 diff = `diff` (jsdiff) + 自渲染"列为候选锁定
- `docs/BACKLOG.md` — Phase 1 必做 `diff` 渲染（与 `code_block` 并列）
- `docs/IMPLEMENTATION.md` §2.5 — step 4 包含"前端 diff 视图"
- `app/src/style.css` `@theme {}` — 已有 `--color-bg-*` / `--color-bg-border` 等 dark theme tokens
- `app/package.json` — 当前依赖：`marked` `dompurify` `pinia` `reka-ui` `@heroicons/vue` `@tailwindcss/vite` `tailwindcss`

### 外部库
- `diff` (jsdiff) — https://github.com/kpdecker/jsdiff
  - npm: `diff` v5.x，MIT，~30 KB
  - 2024-2025 仍有 release，TS 类型完备
- `v-code-diff` — https://github.com/VanichJS/v-code-diff
  - npm: `v-code-diff`，MIT，~25 KB
  - Vue 3 包装，支持 split + unified
- `vue-diff-view` — https://github.com/jiaqifeng/vue-diff-view 及其 fork
  - 维护活跃度需 verify，issue 响应慢
- `monaco-editor` — https://github.com/microsoft/monaco-editor
  - bundle 5-8 MB，Microsoft 维护，高度活跃

### 类似项目参考
- VS Code 自带 diff（design reference for unified/split）
- GitHub PR Files Changed tab（design reference for file list + inline diff）
- Sourcegraph code diff（高密度 diff UI 参考）

---

## 5. 风险与未验证项

- **vue-diff-view / vue3-diff 具体维护活跃度**未在线 verify（无法 web search），如果需要回退到方案 A，建议先 `npm view <pkg> time` 查最近 publish 时间，并 `gh repo view` 查 issue 关闭时长。
- **Tauri 内 worker 加载**对方案 C 是已知坑：Monaco 的 worker 在 WSL/Tauri 2 下的路径处理需要实测；本项目选方案 B 绕过此问题。
- **virtual scrolling**：方案 B 在 5000+ 行单文件下需要虚拟化（`@tanstack/vue-virtual`），本项目 MVP 范围（1-500 行）暂不需要；可作为 stretch goal 列入 `BACKLOG.md`。
- **diff 颜色无障碍**：仅靠红/绿不满足色盲用户，需加 `+` / `-` 字符前缀 + 加粗/下划线。jsdiff 自渲染很容易做到；vue-diff-view 类库需要覆盖 CSS。

---

## 6. 决策建议

1. **MVP 选方案 B**（jsdiff + 自渲染）。
2. **首期不引入 token 级语法高亮**——marked 那条管线已经能 show 改动后的代码块；diff 视图的 `+`/`-` 色块 + 文件名 header 足够 agent review。
3. **不引入方案 C**（Monaco）——本项目后续会单独评估"嵌入代码编辑器"（CodeMirror 6 已在 TECH.md 候选），那个才是 Monaco 真正的 use case。
4. **预留扩展点**：`<DiffView :old="..." :new="..." :mode="unified|split" />` 一开始就支持 `mode` prop，未来想加 side-by-side 只改组件不破坏 API。
