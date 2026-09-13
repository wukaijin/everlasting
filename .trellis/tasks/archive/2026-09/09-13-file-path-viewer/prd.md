# 非图片文件路径可点击查看 + 工具输出路径 linkify

## Goal

把 09-13「图片路径可点击弹层预览」的链路延伸到**非图片文件**：聊天正文与工具输出里
LLM 写下的本地文件路径（`out/报告.md`、`src/main.rs`、`~/docs/spec.pdf` 等）可点击，
按文件类型分层查看（markdown 渲染 / 代码高亮 / pdf 新标签）。与 09-13 同痛点：
产出物落在聊天里是纯文本，用户要手切文件管理器才能看。

任务范围 = 方案讨论中的 **#1（非图片文件路径）+ #2（工具输出面板路径可点）**。
#3（附件图片查看统一弹层）、#4（会话内搜索）是独立任务，不在本任务内。

## Background（已确认事实，规划期证据）

- 09-13 落地的四段链路（spec `.trellis/spec/frontend/chat/message-list-and-markdown.md` §5）：
  识别（`utils/markdown.ts` `linkifyImagePaths` DOM 后处理）→ 取数（`utils/imageUrl.ts`
  + daemon `GET /api/v1/files/image`）→ 点击委托（`composables/useCodeBlockCopy.ts`
  `onMarkdownClick`，绑定面 5 处容器）→ 弹层（`useImageViewer` 单例 +
  `ImageViewerModal` 挂 App.vue）。
- 路径识别正则 `IMAGE_PATH_RE`（`utils/markdown.ts:176`）只认图片扩展名
  （png/jpg/jpeg/gif/webp/bmp/avif/ico）；段语法（前缀 `/` `~/` `./` `../`，裸相对须含
  `/`，纯文件名与 http URL 有意不识别，`/api/` 前缀排除）可直接复用到文件扩展名集合。
- daemon 侧唯一二进制 GET 路由 `GET /api/v1/files/image?path=`（`daemon/routes/files.rs:71`）：
  handler 薄壳，校验全在 `commands/files.rs` `read_image_at_inner`（`app/src-tauri/src/
  commands/files.rs:198`）：`~` 展开 → 绝对路径强制 → 扩展白名单 → metadata +
  读后复核双重大小上限（32 MiB，TOCTOU 兜底）→ 400/404/413/500 分类。GET binary 路由
  不进 CMD_TO_DOMAIN 守卫（先例）。
- `utils/imageUrl.ts`：`resolveImagePath(raw, cwd)` 相对路径按**点击时刻**会话 cwd 解析
  （cwd 会漂移，解析推迟到弹层）；`imageUrl(path)` 三传输模式 URL（同源绝对 / DEV 跨源 /
  pwa-remote proxy + `?access_token=`）。文件 URL 可完全同构。
- 工具输出：`ToolOutputBody.vue`（`components/chat/ToolOutputBody.vue`）是主面板
  `ToolCallCard` 与 `SubagentDrawer` `DrawerToolCallCard` 共用的纯 props 组件
  （`{content, isError}`，无 store 依赖），`<pre>{{ truncated }}</pre>` 文本插值，
  `truncateOutput(display, 500)` 截断（`utils/messageFormat.ts:19`，截断尾巴是
  `… (N more chars)`）。**不走 markdown 管线，路径点不了**——而 write_file/ls/grep
  的结果是路径最密集的面。
- 查看器可复用资产：`MarkdownDetailModal.vue` 接 markdown 字符串走 `renderMarkdown`
  （且已绑 `onMarkdownClick`）；`CodeBlockPrimitive.vue` 接
  `{type:'code_block', code, language?, title?}` 走共享 `renderCodeHtml`，自带复制按钮。
- `docs/DAEMON-API.md` 未收录 `/files/image`（09-13 只写了 spec §5），本任务补文档时
  一并决定 files 域 GET 路由的文档位置。

## Requirements

### R1 聊天正文：非图片文件路径识别与点击（markdown 容器全 5 面）

- `IMAGE_PATH_RE` 的段语法泛化为文件扩展名集合（文本类 + pdf + 现有图片类），非图片
  命中产出 `<a class="md-file-path" data-file-path="原始路径">`；图片命中行为不变
  （`data-image-path`）。
- 四形态覆盖与 09-13 一致：正文裸路径 / inline `<code>` 内 / `[x](本地路径)` 链接
  补 data 属性；围栏代码块（pre 祖先）与 `<a>` 内不动。`![](非图片)` 无意义，
  `downgradeExternalImages` 本地分支维持仅图片。
- 误伤防线沿用：纯文件名、http URL、`/api/` 前缀不识别。

### R2 查看弹层：按扩展名分流

- `.md` / `.markdown` → markdown 渲染查看（复用 `renderMarkdown` 管线 + `onMarkdownClick`
  委托，嵌套图片/文件路径递归可点）。
- 其余文本类 → 只读代码高亮卡（复用 `CodeBlockPrimitive`，复制按钮免费获得）。
- `.pdf` → 不开弹层，`window.open(fileUrl)` 新标签（浏览器原生 viewer）。
- 弹层形态镜像 `ImageViewerModal`（reka-ui Dialog + 模块级单例 composable + App.vue
  全局唯一挂载 +「新标签打开」兜底）；错误态覆盖 daemon 400/404/413。

### R3 工具输出：`ToolOutputBody` 内路径可点

- `<pre>` 从文本插值改为安全 linkify 渲染：先 HTML 转义、再对**截断后文本**做本地
  路径（图片+文件统一）替换、最后过 DOMPurify，`v-html` + 根绑 `onMarkdownClick`。
- 一处改动同时生效主面板工具卡与 SubagentDrawer 工具卡（共用组件）。
- 图片路径命中走既有 `data-image-path` → ImageViewerModal；文件路径走 `data-file-path`
  → R2 弹层。

### R4 daemon：`GET /api/v1/files/raw?path=` 本地文件直连路由

- 结构照 `/files/image` 先例：handler 薄壳 + commands 层 `read_raw_at_inner` 校验
  （`~` 展开 / 绝对路径强制 / 扩展白名单 / 大小双检）。
- 扩展白名单 = 文本类集合 ∪ {pdf}；文本类以 `text/plain; charset=utf-8` 下发
  （.html/.svg 也按 text/plain 下发——MIME 即闸门，任何消费方都不会执行它），
  pdf 以 `application/pdf` 下发。
- 上限分档：文本类 2 MiB（DOM 渲染安全），pdf 32 MiB（沿用 `MAX_IMAGE_BYTES` 量级）。
  文件内容非法 UTF-8 → 400（文本类）。
- svg 仍不可执行（在白名单里仅作文本查看，MIME 强制 text/plain；不提供任何
  text/html / image/svg+xml 的下发路径）。

### R5 文档

- spec `message-list-and-markdown.md` §5 扩写为「本地路径预览」通用约定
  （图片 + 文件 + 工具输出面）。
- `docs/DAEMON-API.md` 补 files 域 GET 路由条目（含 image 既有路由，顺手补齐）。

## Acceptance Criteria

- [ ] AC1 markdown 四形态：`out/report.md`、`` `src/main.rs` ``、`[日志](logs/x.log)`、
  `~/notes/TODO.md` 在 MessageItem 气泡渲染为可点 `<a data-file-path>`；围栏代码块内
  同文本不转；纯文件名 `index.ts`、`https://h/x.md`、`/api/v1/x` 不识别（vitest 夹具）。
- [ ] AC2 图片路径行为零回归：现有 `data-image-path` 夹具全绿。
- [ ] AC3 点击 `.md` 路径 → FileViewerModal 以 markdown 渲染内容；点击 `.rs` → 代码
  高亮卡（含复制按钮）；嵌套路径在弹层内可继续点（vitest 组件测试）。
- [ ] AC4 点击 `.pdf` 路径 → 不开弹层，`window.open` 新标签（vitest mock 断言 URL
  含 `/api/v1/files/raw?path=`）。
- [ ] AC5 ToolOutputBody：`wrote out/a.md` 文本渲染为可点链接；含 `<script>` 的输出
  被转义不执行；500 字截断边界处被切断的路径不产生半截链接（vitest）。
- [ ] AC6 daemon `/files/raw`：白名单文本 200 + `text/plain; charset=utf-8`；pdf 200 +
  `application/pdf`；相对路径/非白名单/非法 UTF-8 → 400；不存在 → 404；超上限 → 413
  （cargo route 测试，镜像 files.rs 既有三条 image 测试）。
- [ ] AC7 三传输模式：pwa-remote token 存在时 `fileUrl` 走 proxy + `access_token`
  （vitest，镜像 `imageUrl.test.ts`）。
- [ ] AC8 全量门禁：`cd app && pnpm test`、`vue-tsc` 零错、
  `cargo test -p everlasting --lib`（带 PKG_CONFIG_PATH）全绿。

## Out of Scope

- HTML/SVG **渲染态**预览（沙箱 iframe）、mermaid、音视频路径、office 二进制
  （docx/xlsx）。
- #3 附件图片查看统一到 ImageViewerModal、#4 会话内搜索（独立任务）。
- 工具输出截断上限调整（维持 500 字契约）。
- 交互式编辑查看的文件（只读）。

## Decisions

- **Q1 已决（2026-09-13 用户确认）**：pdf 进本期——白名单含 pdf 档，点击
  `window.open` 新标签走浏览器原生 viewer，不建弹层。
