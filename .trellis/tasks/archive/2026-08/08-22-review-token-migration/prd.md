# Review 四件套迁移正式设计 token（实证暗色应用内白底渲染）

## Goal

把 Review 可视化四件套（`ReviewMatrix` / `ReviewMatrixGrid` / `ReviewDimensionCompare` /
`ReviewFindingDetail`）从"未定义 CSS 变量 + 浅色 fallback"迁移到项目正式
`--color-*` 设计 token 体系，消除暗色应用内的白底面板割裂与近白文字不可读问题。

## Background（已实证，非推断）

2026-08-22 headless Chromium 实测取证（方法与原始数据见
`research/evidence-20260822.md`，截图在 `out/ui-review/20260822-verify-review/`）：

- 四件套全部通过 `var(--bg-default, #fff)` / `var(--text-tertiary, #9ca3af)` /
  `var(--border-subtle, rgba(0,0,0,…))` 等变量取样式，这些变量在 `app/src`
  内**零定义**（运行时 `getComputedStyle` 取值为空串已验证），实际渲染的全是
  Tailwind 浅色 fallback。
- 挂载点在 `ChatPanel.vue` 主对话流内（review 会话常驻渲染），应用底色
  `#0a0e14` 上面板背景实测纯白 `rgb(255,255,255)`；根节点文字继承
  `#cbd5e1`，白底上对比度 ≈1.1:1，继承该色的元素基本不可读。
- `ReviewMatrix.vue` 内有 `@media (prefers-color-scheme: dark)` 补丁
  （~L280），但用的是通用灰（#1f2937/#f3f4f6），仍偏离 Prussian blue 色板，
  且 OS 为 light 时面板就是纯白——补丁本身是症状掩盖，应随迁移删除。
- `ReviewFindingDetail` 严重度徽章硬编码 Tailwind 600 档 hex（实测：
  high=`#dc2626`、medium=`#d97706`、low=`#2563eb`），不经任何 token。

## Requirements

- **R1 token 迁移**：四个文件 style 块内所有"幽灵变量"（`--bg-default` /
  `--bg-elevated` / `--text-primary|-secondary|-tertiary` /
  `--border-subtle` 等带浅色 fallback 且无定义者）替换为正式 token：
  背景层级用 `--color-bg-elevated` / `--color-bg-surface`，文字用
  `--color-text-primary/-secondary/-muted`，边框用 `--color-bg-border` /
  `--color-bg-border-strong`（表格分隔线等在 elevated 上需可辨的场景），
  hover tint 用 `--color-bg-hover`。
- **R2 删除 `prefers-color-scheme` 补丁**：迁移后该 media query 及其通用
  灰色板整体移除；本应用 dark-only（见 `app/src/style.css` 主题扩展点
  注释），组件层不应有独立明暗分支。
- **R3 严重度徽章 token 化**：`ReviewFindingDetail` 的 `SEVERITY_META`
  色板改为语义映射。起点建议——critical/high → `--color-tool-error` /
  `--color-tool-error-text`、medium → `--color-status-warn`、low →
  `--color-accent` / `--color-accent-text`、info → `--color-text-muted`；
  徽章文字统一 `--color-text-on-accent`。若填充色确需 600 档对比度，
  允许在 `@theme` 增补语义 token 并注明理由，禁止组件内裸 hex。
- **R4 遵守 design-tokens spec**：不新增硬编码 hex/px；间距/圆角/字号按
  `.trellis/spec/frontend/design-tokens.md` 的 ladder 消费。

## Acceptance Criteria

- [ ] `grep -nE "var\(--(bg|text|border)-" app/src/components/chat/Review*.vue`
      零幽灵变量命中（注意区分正式 `--color-*` / `--color-bg-*` 前缀）。
- [ ] 四文件内零裸 hex（注释除外）；`ReviewMatrix.vue` 的
      `prefers-color-scheme` 块已删除。
- [ ] 实机验证（配方见下）：`.review-matrix` 背景 = 所选背景 token 的 RGB
      值；正文对 elevated 底对比度 ≥ AA（4.5:1）；严重度徽章颜色与 token
      一致。
- [ ] 截图对照：review 会话截图内面板与暗色主题一致、无白块（对照基线
      `out/ui-review/20260822-verify-review/15-review-matrix-element.png`）。
- [ ] 前端测试全绿（`cd app && pnpm test`）；`vue-tsc` 无新错误。

## 复现 / 验证方法（面板点亮配方）

Review 面板仅在 `plugin_name='review'` + `workflow_enabled=1` 会话且
`<cwd>/.everlasting/tasks/<slug>/review-state.json` 合法存在时渲染；
DB 无现成 review 会话。临时配方（全走正规 API，用完清理，勿直写 DB）：

1. 手写合法 `review-state.json` 放进已有 task 目录（schema 见
   `app/src-tauri/src/commands/review.rs` L62-165：顶层
   `schema_version`/`task_id`/`current_round`/`rounds[]` 必填；finding 的
   `finding_id`/`dimension`/`severity`/`issue`/`source_run_id` 必填；
   verdict 枚举 pass/pass_with_minor/revise/reject，severity 枚举
   critical/high/medium/low/info）。样例可从 research/evidence 恢复。
2. `POST /api/v1/projects/list_projects`（path 匹配，无则 create_project）
   → `POST /api/v1/sessions/create_session` `{project_id, initial_cwd}` →
   `POST .../set_session_plugin_name` `{session_id, name:"review"}` →
   `POST .../set_session_workflow_enabled` `{session_id, enabled:true}`。
   （create_session 不收 title 字段，标题会是默认"新对话"。）
3. 浏览器打开应用点该会话（最新更新排第一），等 `.review-matrix` 渲染；
   点 `.review-matrix-grid__cell--body` 展开 finding 明细行。别点
   `.review-matrix__collapse`——会把整个 body 收起。
4. 清理：`POST /api/v1/sessions/delete_session` + 删除 review-state.json。

可复用取证脚本：`~/.cache/everlasting-ui-review/verify-{review,severity}.mjs`
（playwright-core scratch 环境，与 ui-review.sh 同款）。

## Non-goals

- `MessageItem.vue` 的 `--ev-color-*` 死命名空间（speaker chip 仅群聊渲染，
  当前 DB 无群聊会话，休眠债）——另开任务。
- `NodeListView.vue:311` 状态点 `#22c55e` 硬编码（token
  `--color-status-success` 已存在）——一行改动，可捎带但不入验收。
- Button/focus-visible、Spinner/Skeleton、z-index 阶梯等系统性原语——后续任务。

## Notes

- dist 当前落后 HEAD 一个提交（SSE 输入动效），与本任务无关；实机验证前若
  需要最新 UI：`cd app && pnpm build` 后 `./scripts/daemon.sh bg --no-build`。
- VLM 视觉评审对"选中态缺失"类结论不可靠（本轮 sidebar 选中态被误报）；
  本任务验收以 computed style 数值 + 截图人工对照为准。
