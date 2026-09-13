# Implement:非图片文件路径可点击查看 + 工具输出路径 linkify

> 执行顺序 = A 后端 → B 识别/URL → C 弹层 → D 工具输出 → E 文档/spec。
> 每阶段可独立验证、独立提交;B 依赖 A 的路由契约(可先行写死 URL 形态并行开发)。

## A. daemon:`GET /api/v1/files/raw`

- [ ] A1 `app/src-tauri/src/commands/files.rs`:`raw_content_type`(文本集→
      `text/plain; charset=utf-8`,pdf→`application/pdf`)+ `MAX_TEXT_BYTES = 2 MiB`
      + `ReadRawError` + `read_raw_at_inner`(镜像 `read_image_at_inner`:`expand_home`
      →绝对强制→白名单→is_file→双重大小检→UTF-8 严格校验)
- [ ] A2 `app/src-tauri/src/daemon/routes/files.rs`:`.route("/raw", get(read_raw))`
      + 错误映射 400/404/413/500 + `Cache-Control: private, max-age=60`
- [ ] A3 route 测试(镜像 image 三条):200+两种 Content-Type / 400(相对路径、
      非白名单、非法 UTF-8)/ 404 / 413(构造 >2 MiB 文本)
- [ ] 验证:`cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib files::`

## B. 前端识别与 URL

- [ ] B1 `app/src/utils/markdown.ts`:`FILE_EXT` 全集常量 + `FILE_PATH_RE`(段语法
      不动)+ 命中分流(图片→data-image-path/其余→data-file-path)+
      `linkifyImagePaths`→`linkifyLocalPaths` 更名 + `downgradeExternalImages` 本地
      分支泛化(`[图片]`/`[文件]`)+ 导出 `linkifyPlainText`(escape→replace→sanitize)
- [ ] B2 `app/src/utils/imageUrl.ts`:`fileUrl(path)`(三传输模式,镜像 `imageUrl`);
      模块注释更新(`resolveImagePath` 语义通用化)
- [ ] B3 测试:`markdown.test.ts` AC1/AC2 夹具;`imageUrl.test.ts` 加 fileUrl describe(AC7)
- [ ] 验证:`cd app && pnpm test -- --run markdown imageUrl`

## C. 查看弹层

- [ ] C1 `app/src/composables/useFileViewer.ts`:单例 state(idle/loading/ok/error,
      content, ext)+ `open(rawPath)`(点击时刻 `resolveImagePath(raw, currentCwd)`;
      pdf→`window.open(fileUrl)` 不开弹层;文本→fetch(fileUrl)→state)
- [ ] C2 `app/src/components/common/FileViewerModal.vue`:Dialog 六件套镜像
      ImageViewerModal;md 模式(renderMarkdown + onMarkdownClick + §2 排版镜像块
      第六处)/ code 模式(构造 UiPrimitive 复用 CodeBlockPrimitive);header 文件名 +
      新标签打开;错误态文案 + 兜底
- [ ] C3 `app/src/App.vue` 挂载;`app/src/style.css` 加 `.md-file-path`(镜像
      `.md-image-path` 两行)
- [ ] C4 `composables/useCodeBlockCopy.ts`:`onMarkdownClick` 加 `a[data-file-path]`
      分支 → `useFileViewer().open()`
- [ ] C5 测试:`FileViewerModal.test.ts`(AC3 模式分派/错误态/嵌套点击;AC4 pdf
      window.open、URL 含 `/api/v1/files/raw?path=`)+ `useCodeBlockCopy.test.ts`
      补 data-file-path 分支用例
- [ ] 验证:`cd app && pnpm test -- --run FileViewer useCodeBlockCopy`

## D. 工具输出

- [ ] D1 `app/src/components/chat/ToolOutputBody.vue`:`html = linkifyPlainText(truncated)`
      + `<pre>` v-html + 根绑 `@click="onMarkdownClick"`(引入 useCodeBlockCopy)
- [ ] D2 测试:`ToolOutputBody.test.ts` 新增/更新——路径→锚点、`<script>` 转义、
      截断边界无半截链接(AC5);`ToolCallCard`/`DrawerToolCallCard` 既有输出断言同步
- [ ] 验证:`cd app && pnpm test -- --run ToolOutputBody ToolCallCard DrawerToolCallCard`

## E. 文档与 spec

- [ ] E1 `docs/DAEMON-API.md`:files 域 GET 路由小节(`/files/image` 既有 + `/files/raw`
      新,契约表:白名单/MIME/上限/错误码)
- [ ] E2 `.trellis/spec/frontend/chat/message-list-and-markdown.md` §5 扩写:
      「图片路径预览」→「本地路径预览(图片+文件)」,补 data-file-path 通道、
      ToolOutputBody 面、白名单对齐约定;§2 镜像块名单 5→6 处(FileViewerModal)

## 全量门禁(最终 check 前必跑)

```bash
cd app && pnpm test                                  # 全量 vitest
cd app && pnpm exec vue-tsc --noEmit                 # 类型零错
cargo test -p everlasting --lib                      # PKG_CONFIG_PATH 见 AGENTS.md
```

## 风险文件与回滚点

| 文件 | 风险 | 缓解 |
|------|------|------|
| `utils/markdown.ts` | 7 消费面共享管线,识别泛化误伤正文 | 现有 54 夹具 + AC1 排除项夹具;hint 预检不变 |
| `ToolOutputBody.vue` | 插值→v-html,XSS 面变化 | 三层防线(escape/linkify 构造/sanitize)+ AC5 夹具;DOMPurify 仓库不变量 |
| `commands/files.rs` | 二进制读路由,路径探测面 | 白名单统一 400 不给存在性旁信道(沿 image 先例);上限双检 |
| `FileViewerModal.vue`(新) | 低 | 纯增量,镜像成熟模式 |

回滚:A/B/C/D 四段互相独立,任一段 revert 不影响其余(仅 D 的锚点依赖 C 的
`data-file-path` 委托分支存在——revert C 需同时 revert D,或 D 独立留存但文件路径
点击无响应,见 spec §5 坑)。

## Start 前检查

- [ ] implement.jsonl / check.jsonl 已策展(非 _example 种子行)
- [ ] 用户已评审 prd/design/implement 并确认 Q1(pdf 进/出本期)
