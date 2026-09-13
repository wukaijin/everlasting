# Design:非图片文件路径可点击查看 + 工具输出路径 linkify

## 1. 总体架构:沿 09-13 四段链路各段加"文件"通道

```
识别            取数                        点击委托                  呈现
markdown.ts     imageUrl.ts +fileUrl        useCodeBlockCopy         ImageViewerModal(不变)
linkifyLocalPaths  →  daemon /files/raw     +data-file-path 分支  +  FileViewerModal(新)
(泛化扩展名)    (新路由,镜像 /files/image)  ToolOutputBody 根绑定     ├ md → renderMarkdown
                                                                    ├ code → CodeBlockPrimitive
                                                                    └ pdf → window.open(无弹层)
```

原则:**不新造机制,每一段都在 09-13 先例上加一条平行通道**。图片行为零改动。

## 2. 契约

### 2.1 扩展名白名单(前后端两份,评审时对齐;后端是安全闸门)

| 类别 | 扩展名 | 消费方式 |
|------|--------|----------|
| 文本 | md markdown txt log json jsonl csv tsv yaml yml toml ini conf cfg xml html htm css js mjs cjs jsx ts tsx vue svelte py rs go java kt kts c h cpp hpp cc cs rb php sh bash zsh fish sql proto graphql gql diff patch | FileViewerModal(text/plain; charset=utf-8) |
| pdf | pdf | window.open 新标签(application/pdf) |
| 图片(既有) | png jpg jpeg gif webp bmp avif ico | ImageViewerModal(不变) |

- 前端识别集 = 文本 ∪ pdf ∪ 图片(统一一个正则);后端 `/files/raw` 白名单 = 文本 ∪ pdf。
- `.html`/`.htm` **只作文本查看**:daemon 强制 `text/plain` 下发,MIME 即闸门,
  任何消费方(弹层 hljs / 新标签兜底)都只见到源码,不存在 text/html 下发路径。
  svg 不进 `/files/raw` 白名单(维持 09-13 决策,svg 连文本查看都不给——它不是
  LLM 常引用的文本产物,少一个特例)。

### 2.2 daemon:`GET /api/v1/files/raw?path=<abs|~前缀>`

- `commands/files.rs` 新增(镜像 `read_image_at_inner`,`app/src-tauri/src/commands/files.rs:198`):
  - `raw_content_type(ext) -> Option<(&'static str, RawClass)>`,文本类 →
    `("text/plain; charset=utf-8", Text)`,pdf → `("application/pdf", Pdf)`。
  - `read_raw_at_inner(path)`:`expand_home` → 绝对路径强制(相对 400)→ 白名单 →
    metadata is_file → 大小上限(文本 2 MiB / pdf 32 MiB,metadata + 读后复核双检)
    → 读取。文本类额外 `String::from_utf8` 校验,非法 UTF-8 → 400(弹层 v-html/
    hljs 消费的是字符串,不容 replacement char 糊弄;二进制误命名 .txt 直接拒)。
  - `ReadRawError` 枚举镜像 `ReadImageError`(InvalidRequest/NotFound/TooLarge/Io)。
- `daemon/routes/files.rs`:`.route("/raw", get(read_raw))`,错误映射 400/404/413/500,
  `Cache-Control: private, max-age=60` 同 image。GET binary 不进 CMD_TO_DOMAIN(先例)。
- 不加 IPC command 包装(前端只消费 URL,同 image 先例;GUI Thin/remote 都走 daemon HTTP)。
- pdf 不加 `Content-Disposition: filename`(省文件名转义面,浏览器默认内联 viewer 足够)。

### 2.3 前端 URL:扩展 `utils/imageUrl.ts`(不建新文件)

- `resolveImagePath` 名字保留但语义是通用本地路径解析(`/abs`、`~/` 原样,相对拼
  会话 cwd),模块注释更新;调用方:ImageViewerModal(既有)+ FileViewerModal/弹层取数(新)。
- 新增 `fileUrl(path): string`:`/api/v1/files/raw?path=`,三传输模式与 `imageUrl`
  同构(同源绝对 / DEV 跨源 / pwa-remote proxy + `access_token`)。

### 2.4 识别泛化:`utils/markdown.ts`

- `IMAGE_EXT` 保留(图片类判定);新增 `FILE_EXT = IMAGE_EXT|文本|pdf` 作为识别全集;
  `IMAGE_PATH_BODY` 参数化为 `(?:${extSet})` 构造,`FILE_PATH_RE` = 现有边界组逻辑 +
  新扩展集(段语法、纯文件名排除、`/api/` 排除全部不动)。
- 命中后按扩展名分流:图片 → `<a class="md-image-path" data-image-path>`(现状),
  其余 → `<a class="md-file-path" data-file-path>`。锚点构造、`tryDecodeUri`、
  pre/a 跳过逻辑复用。
- `linkifyImagePaths` 更名 `linkifyLocalPaths`(语义变宽;spec §5 同步改引用)。
- `downgradeExternalImages` 本地分支同步泛化:`![](本地)` 的 src 命中文件集 →
  按 ext 产出 `[图片]`(图片,data-image-path)或 `[文件]`(文件,data-file-path)
  预览链接——顺带修掉"![](out/x.md) 退化成相对 href 新标签链接打穿 SPA"的存量小 wart。
- 新导出 `linkifyPlainText(text): string`:给**非 markdown** 文本(工具输出)用——
  整体 `escapeHtml` → `FILE_PATH_RE` 全局替换插锚(锚文本用已转义切片,data 属性值
  同为转义形态,引号安全)→ `DOMPurify.sanitize`(双保险,保持单一约定)。

### 2.5 点击委托:`composables/useCodeBlockCopy.ts` 加分支

- `onMarkdownClick` 在 image 分支旁加 `a[data-file-path]` → preventDefault →
  `useFileViewer().open(raw)`。绑定面零改动(5 容器已绑根委托,新分支自动生效)。

### 2.6 弹层:`composables/useFileViewer.ts` + `components/common/FileViewerModal.vue`

- `useFileViewer` 模块级单例(镜像 `useImageViewer`):`open(rawPath)` 时先
  `resolveImagePath(rawPath, chatStore.currentCwd)`(点击时刻 cwd),再按扩展分派:
  - pdf → `window.open(fileUrl(resolved))`,**不开弹层**;
  - 文本 → 置 state(loading → fetch(fileUrl) → ok/error),挂全局 FileViewerModal。
- `FileViewerModal.vue`(reka-ui Dialog 六件套,镜像 ImageViewerModal 结构):
  - header:文件名 + 「新标签打开」(`window.open(fileUrl)`,文本类新标签见源码,text/plain
    语义一致)+ 关闭;
  - md 模式:`renderMarkdown(content)` + `@click="onMarkdownClick"`(嵌套路径递归可点
    ——spec §5 坑:新增 markdown 容器忘绑委托 = 交互静默失效)+ §2 排版镜像块
    (第六处,grep `.msg__markdown` 找全);
  - code 模式:构造 `{type:'code_block', code: content, language: ext, title: 文件名}`
    复用 `CodeBlockPrimitive`(hljs 别名覆盖 rs/ts/py 等常见 ext,未知名走
    `highlightAuto`,复制按钮免费);
  - 错误态:fetch 非 200(400/404/413/网络)统一错误文案 + 「新标签打开」兜底
    (ImageViewerModal 同款模式)。
- `App.vue` 全局挂载(ImageViewerModal 旁)。
- `style.css` 加 `.md-file-path`(cursor:pointer + word-break:break-all,镜像
  `.md-image-path` 两行;排版色仍归各容器 `:deep(a)`)。

### 2.7 工具输出:`ToolOutputBody.vue`

- `truncated` → `html = linkifyPlainText(truncated)`;`<pre>` 内改
  `v-html="html"`,pre 根绑 `@click="onMarkdownClick"`(真实 Vue 模板元素,直接绑,
  非 v-html 容器根也可——closest 委托两栖)。
- 组件引入 `useCodeBlockCopy()`(composable 非 store,不违反 FT-F-001 D3"无 store
  依赖";`useImageViewer`/`useFileViewer` 都是模块级单例)。
- 共用组件一处改,主面板 `ToolCallCard` 与 `DrawerToolCallCard` 同时生效。
- 截断边界:500 字切断的路径缺扩展名尾 → 正则不匹配 → 无半截链接(截断尾巴
  `… (N more chars)` 的空格边界天然安全);AC5 夹具锁死。

## 3. 关键取舍

| 决策 | 选择 | 理由 / 放弃项 |
|------|------|---------------|
| 单 `/raw` vs `/text`+`/pdf` 两路由 | 单 `/raw` | 白名单/上限/错误映射一套;pdf 与文本仅 MIME 和上限档不同,分路由是复制 |
| 文本上限 2 MiB | 独立小档 | 32 MiB 文本进 v-html/hljs 会卡死 UI;2 MiB ≈ 2M 字符,远超正常查看需求 |
| UTF-8 严格校验 | 严格 400 | 弹层按字符串消费,lossy 替换符体验差且掩盖误用;二进制误命名当拒 |
| 前后端白名单两份 | 接受漂移风险,评审对齐 + spec 记录 | 跨语言共享一份的机制(代码生成)成本高于收益;后端是唯一闸门,前端集偏大只会点开见 400 |
| ToolOutputBody 转 v-html | 转义→linkify→DOMPurify 三层 | 维持"所有 v-html 都过 DOMPurify"的仓库不变量;纯插值无法承载锚点 |
| FileViewerModal 不复用 MarkdownDetailModal 外壳 | 自有 Dialog 壳 | MarkdownDetailModal 带 trigger 交互语义;ImageViewerModal(单例 store 驱动、纯受控)才是对的模板 |

## 4. 兼容与回归面

- 图片链路(`data-image-path` / ImageViewerModal / `/files/image`)零改动,现有
  54 条夹具是回归网。
- `markdown.ts` 是 7 个消费面共享管线,改动必须跑全量 vitest(不只 markdown.test.ts)。
- `ToolOutputBody` 现有测试(若有断言 pre 纯文本)需同步更新为断言 html。
- pwa-remote proxy 是 catch-all,`/files/raw` 免配置透传。

## 5. 测试地图

- 后端:`daemon/routes/files.rs` tests 模块镜像三条 image 测试 + UTF-8 400 用例。
- `markdown.test.ts`:AC1/AC2 夹具(四形态×文件扩展、图片回归、排除项)。
- `imageUrl.test.ts`(更名语义不变)或新 `fileUrl` describe:AC7 三传输模式。
- `FileViewerModal.test.ts`:模式分派(md/code)、错误态、嵌套点击(AC3)、pdf 不开弹层(AC4)。
- `ToolOutputBody.test.ts`:链接化、XSS 转义、截断边界(AC5)。
- 门禁:`pnpm test` 全量 + `vue-tsc --noEmit` + `cargo test -p everlasting --lib`。
