# Research: @文件内容注入 — 主流 Coding Agent 调研

- **Query**: 主流 coding agent 对 `@`/`/add` 文件引用的处理方式，重点是文本 / 图片 / PDF / Office / 二进制 的注入语义与降级策略 — 为 Everlasting B2 PR2（@文件内容注入）的后端降级策略提供决策依据
- **Scope**: 外部调研（GitHub 源码逐行核对 + 官方文档交叉确认）
- **Date**: 2026-06-17
- **对比对象**: opencode、Aider、Cline、pi（earendil-works）、PearAI（Continue fork）、Cursor
- **调研方法**: 派 general-purpose agent 实抓 GitHub raw 源码逐行核对（精确到 file:line）+ 官方文档交叉确认；Cursor 闭源，仅文档/社区印证
- **关联**: Everlasting `.trellis/tasks/06-17-b2-pr2-at-file-injection/`；父调研 `06-17-b2-at-file-completion/research/at-file-ux-conventions.md`（CC @-mention UX 约定）

---

## TL;DR 核心结论

1. **注入语义 5 家一致 = 注入文件内容到 context，不是发路径让 LLM 自己读**。当前 Everlasting PR1 的「纯文本 `@relpath` 路径提示」是**业界唯一被判最差的形态**（pi 的 TUI 粘贴转路径也是 bug 非设计）。PR2 必须升级为内容注入。

2. **二进制必须检测，绝不能把乱码喂给 LLM** — 4 家检测，**pi 不检测（utf-8 乱码直接喂）是公认短板**，是 Everlasting 的反面教材。检测算法业界收敛到：**扩展名黑名单 + 内容嗅探（null byte / 30% 非打印比）**。

3. **非文本降级有三种哲学**，没有「降级为路径文本」这个中间态：
   - **A. fail 报错**（opencode）— 二进制直接 `Effect.fail`，合成错误 text part
   - **B. 占位串降级**（Cline）— 注入 `"(Binary file, unable to display content)"`
   - **C. 静默丢弃**（Aider）— `read_text` 失败 → "Dropping X from the chat"

4. **截断默认值收敛到 2000 行 / 50KB**（opencode、pi read 工具、Claude Code 一致）。

5. **图片处理需 multimodal 基础设施**（ContentBlock 加 Image variant + provider wire base64 + 缩放）— 5 家都支持，但对 Everlasting 当前**纯文本通道**是 B1（第三档）量级的活，**PR2 不碰**。

---

## 一、三种降级哲学对比

| 哲学 | 代表 | 行为 | 取舍 |
|---|---|---|---|
| **A. fail 报错** | opencode | `Effect.fail("Cannot read binary file")` + 合成错误 text part | 诚实严格，但 @ 是用户主动引用，fail 打断 agent loop |
| **B. 占位串降级** | Cline | 注入 `"(Binary file, unable to display content)"` | 友好、不阻塞、用户可见处理结果 |
| **C. 静默丢弃** | Aider | `read_text` 返回 None → "Dropping X from the chat" | 最朴素，但用户不知道引用被吃 |

> **没有一家**采用「降级为路径文本让 LLM 自己读」—— 这正是 Everlasting PR1 当前纯文本 `@token` 的形态。

---

## 二、完整对比矩阵

| 维度 | opencode | Aider | Cline | pi | PearAI | Cursor |
|---|---|---|---|---|---|---|
| **注入语义** | 注入内容 | 注入内容 | 注入完整内容 | 注入内容(CLI)/路径(TUI粘贴⚠️) | 注入全文 | 注入内容(@Docs 走索引) |
| **图片** | ✅ base64+缩放 | ✅ vision 守卫+base64 | ✅ vision 模型 | ✅ **WASM 缩放最强** | ✅ 独立通道 | ✅ vision 模型 |
| **PDF** | ✅ 透传 base64 | ✅ 原生 document block | ⚠️ 文本提取(pdf-parse) | ❌ **乱码喂 LLM** | ❌ | ❌ 原生不支持 |
| **Office** | ❌ fail | ❌ 静默丢 | ⚠️ 文本提取(mammoth/exceljs) | ❌ **乱码喂 LLM** | ❌ | 未确认 |
| **纯二进制** | ❌ fail | 静默丢 | ✅ 占位串 | ❌ **无检测(乱码)** | ❌ 无检测 | 未确认 |
| **二进制检测** | 扩展名+null byte+30% | 靠解码失败 | isbinaryfile 库 | **无** | 无 | — |
| **大文件截断** | 2000行/50KB/单行2000 | 无 per-file(全局预算) | 400KB+20MB | 图片4.5MB/**文本无截断** | 无 per-file | 未确认 |
| **越界防护** | 拒绝`..`/绝对/symlink | cwd 内 | workspace 内 | **无沙箱** | — | — |
| **注入形态** | synthetic "Called Read tool" | \`\`\`path fence + 图片独立 block | `<file_content path>` 标签 | `<file name>` XML | \`\`\`path fence | inline / 索引 |
| **降级哲学** | A. fail | C. 静默丢 | B. 占位串 | (无,乱码) | (无) | — |

---

## 三、二进制检测算法（业界共识，可直接移植 Rust）

opencode（`tool/read.ts:182-227`）和 Cline（`isbinaryfile` npm）的检测逻辑高度一致：

1. **扩展名黑名单**（opencode 26 项，命中即判二进制，不读内容）：
   `.zip .tar .gz .exe .dll .so .class .jar .war .7z .doc .docx .xls .xlsx .ppt .pptx .odt .ods .odp .bin .dat .obj .o .a .lib .wasm .pyc .pyo`
2. **内容嗅探**：读前 `SAMPLE_BYTES=4096` 字节 → 含 NULL 字节（`\x00`）→ 二进制；否则非打印字符占比 **> 30%**（严格大于，`\t \n \r` 不计）→ 二进制；空文件 → 非二进制
3. **双保险**：黑名单 + 嗅探（防无扩展名文件漏判）

**坑（opencode 实例）**：扩展名表与 magic-byte 嗅探表必须对齐 — opencode 的 `SUPPORTED_IMAGE_MIMES` 不含 `bmp`，但 `sniffAttachmentMime` 能识别 `image/bmp`，导致 `.bmp` 掉进二进制分支被误杀。Everlasting 实现时两张表要同步。

> Rust 实现起步极轻：`content_bytes.iter().take(8192).any(|&b| b == 0)` 即 null byte 判定；非打印比例遍历前 4KB 统计即可。

---

## 四、各家详细（源码级）

### opencode（sst/opencode，TypeScript / Bun / Effect-TS）

> 核心服务端是 TS（非 Go）。`@` 逻辑在 `packages/opencode/src/`。

- **触发/插入**：TUI 内 `@` 触发补全，插入 `@<相对路径>` 或 `@<别名>/<子路径>`（references 别名系统）。
- **注入语义**：注入内容。进阶做法是**注入内容格式 = read 工具输出格式**（合成 `"Called the Read tool with the following input: {filePath,...}"` + read 真实输出），模型不会因「人喂 context」vs「工具返回」格式差异困惑。
- **注入形态**：全部合成进 **user message content block**（不拼 system prompt）：
  - 文本/目录 → 两条 synthetic text part（"Called Read tool..." + read 输出）
  - 图片/PDF → 一条 text part + 一条 `{type:"file",mime,url:"data:<mime>;base64,..."}` 附件 part
- **类型路由**：`session/prompt.ts:791-953` switch，`isMedia(mime)` 优先于 `isBinaryFile` 判定（media 不被二进制误杀）。
- **二进制降级**：`Effect.fail("Cannot read binary file: <path>")`（`tool/read.ts:327-329`），注入侧推 synthetic 错误 text part，**不静默跳过**。
- **PDF 特殊**：不在黑名单，`%PDF-` magic byte 嗅探后走 attachment 分支透传 base64（不转文本、不降级）。
- **图片缩放**：`image.normalize()` photon WASM，`MAX_BASE64_BYTES=5MB`、`MAX_WIDTH=MAX_HEIGHT=2000`，Lanczos3 + scale×0.75 阶梯 + JPEG 质量 `[80,85,70,55,40]` 阶梯。
- **截断**：`DEFAULT_READ_LIMIT=2000` 行、`MAX_BYTES=50KB`、`MAX_LINE_LENGTH=2000`（单行截断）。
- **安全**：拒绝绝对路径 / `..` / symlink 越界，按 canonical resource identity 断言；`.env` 默认拒读。
- 来源：[`session/prompt.ts`](https://github.com/sst/opencode/blob/dev/packages/opencode/src/session/prompt.ts)、[`tool/read.ts`](https://github.com/sst/opencode/blob/dev/packages/opencode/src/tool/read.ts)、[`image/image.ts`](https://github.com/sst/opencode/blob/dev/packages/opencode/src/image/image.ts)

### Aider（aider.chat，Python，开源 CLI）

- **触发/插入**：`/add`、`/drop` 命令 + 聊天内 `@`。
- **注入语义**：注入内容。文本走 \`\`\`fence 包裹；图片/PDF 走独立 `get_images_message()` 多模态 block。
- **类型支持**（`utils.py:13`）：`IMAGE_EXTENSIONS = {.png,.jpg,.jpeg,.gif,.bmp,.tiff,.webp,.pdf}`
  - 图片：检查 `main_model.info["supports_vision"]`，base64 编码成 `data:image/...;base64,...` → OpenAI `image_url` block（detail:high），前置 `"Image file: <rel_fname>"` 文本。
  - PDF：`application/pdf` 走 image_url block，条件 `supports_pdf_input`。
  - **值得借鉴**：图片消息后紧跟一条 `{"role":"assistant","content":"Ok, I will use these images as references."}`（显式确认收到模式，`base_coder.py:780-815`）。
- **Office/二进制降级**：**无 is_binary 函数**，纯靠 `io.read_text()` 的 `UnicodeDecodeError` 副作用 → 返回 None → "Dropping <fname> from the chat" 静默移除。
- **图片+非 vision 模型**：显式报错 `"Cannot add image file X as the <model> does not support images."`（`commands.py:888`）。
- **截断**：无 per-file 字节硬截断（依赖 `--max-chat-history-tokens` / repo map 预算）。
- 来源：[base_coder.py](https://github.com/Aider-AI/aider/blob/main/aider/coders/base_coder.py)、[commands.py](https://github.com/Aider-AI/aider/blob/main/aider/commands.py)、[utils.py](https://github.com/Aider-AI/aider/blob/main/aider/utils.py)、[Images & web pages 文档](https://aider.chat/docs/usage/images-urls.html)

### Cline / Roo Code（VSCode 扩展，TypeScript）

> **二进制/Office 降级最完整的一家**。

- **触发/插入**：`@/path/to/file` mention，文档原文 *"Cline sees the complete file content"*。
- **类型支持**（官方文档）：text + images + **PDFs, CSVs, Excel files**；images 需 multimodal 模型。
  - PDF：`pdf-parse` 抽纯文本（`extract-text.ts:51,79-83`）— **降级为文本，非原生 block**。
  - docx：`mammoth.extractRawText()`；xlsx：`exceljs` 按行列格式化；ipynb：`sanitizeNotebookForLLM`；**pptx 不支持**。
- **二进制降级**（`mentions/index.ts:337-341`）：`isbinaryfile` npm 库检测 → 占位串 `"(Binary file, unable to display content)"`，不报错不阻塞。
- **分层降级**：扩展名命中 pdf/docx/ipynb/xlsx → 专用解析器；否则 default 分支检查 size > **20MB** 抛 "File is too large"，否则 `detectEncoding`（jschardet + iconv-lite）解码（处理非 UTF-8 文本）。
- **截断**（`content-limits.ts`）：`MAX_CONTENT_SIZE_BYTES=400KB`，超限切断 + 追加注解 `"[FILE TRUNCATED: ... only the first 400.0 KB is shown ... Use search_files ...]"`；default 文本分支另有 20MB 硬上限。
- **注入形态**：inline `<file_content path="...">\n<content>\n</file_content>`；folder mention 用树状前缀。
- 来源：[Working with files 文档](https://docs.cline.bot/core-workflows/working-with-files)、[mentions/index.ts](https://github.com/cline/cline/blob/main/apps/vscode/src/core/mentions/index.ts)、[extract-text.ts](https://github.com/cline/cline/blob/main/apps/vscode/src/integrations/misc/extract-text.ts)、[content-limits.ts](https://github.com/cline/cline/blob/main/apps/vscode/src/shared/content-limits.ts)

### pi（earendil-works/pi，TypeScript，CLI+TUI）

> **图片处理顶级，二进制处理为零 — Everlasting 的反面教材。**

- **基础定位**：CLI+TUI（Ink/React），工具驱动（类 Claude Code 本体）。文件引用走 **CLI 启动参数 `@file`**（`cli/args.ts:186-187`），TUI 内无 @ 补全；TUI 粘贴图片（Ctrl+V）写成临时文件再插路径字符串（**两条通道语义不一致**）。
- **注入语义**：CLI `@file` 通道注入内容；TUI 粘贴只插路径文本（靠 LLM 调 read 工具）。
- **注入形态**：CLI 通道文本用 `<file name="绝对路径">\n<内容>\n</file>` XML 包裹；拼进首条 user message。
- **类型支持**：
  - 图片：magic bytes 嗅探（`utils/mime.ts`，非扩展名）识别 png/jpg/gif/webp → Photon WASM 缩放 → base64 `ImageContent`。**缩放管线全仓库最用心**：max 2000×2000、base64 ≤ 4.5MB、PNG/JPEG 双编码取小、JPEG 质量阶梯 [80,85,70,55,40]、尺寸×0.75 递减、缩放后注入坐标比例提示（"Multiply coordinates by X to map to original image"）、四套剪贴板后端（macOS/WSL/Wayland/X11）、BMP→PNG 转换。
  - PDF/Office/二进制：**完全无检测**，落入 `buffer.toString("utf-8")` 文本分支 → 乱码（U+FFFD replacement chars）**直接喂 LLM**。全仓库 `isBinary`/`null byte`/`pdf`/`docx` 0 命中。
- **降级**：不支持图片格式/缩放失败 → 占位 `<file name="...">[Image omitted: ...]</file>`；非 vision 模型 → 追加文本提示但 **image block 仍加入 content 数组**（真正丢弃交上层）。
- **截断**：文本 @file 通道**无截断**（全量注入）；截断只在 read 工具（2000 行 / 50KB，谁先到谁触发，附续读提示）；图片 4.5MB。
- **安全**：**无路径越界校验**（`resolvePath` 只归一化不拦 `../`）。
- 来源：[`cli/file-processor.ts`](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/src/cli/file-processor.ts)、[`utils/mime.ts`](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/src/utils/mime.ts)、[`utils/image-resize-core.ts`](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/src/utils/image-resize-core.ts)、[`core/tools/read.ts`](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/src/core/tools/read.ts)

### PearAI（Continue fork，TypeScript）

> 最简单朴素。

- **触发/插入**：`@filename/foldername`、`@docs`、`@codebase`、`@code` 等（继承 Continue context-provider 架构）。
- **注入语义**：注入全文。`FileContextProvider.ts` 读文件 → 包成 \`\`\`<relativePath>\n<content>\n\`\`\` markdown 代码块。
- **类型支持**：图片走**独立通道**（非 @file）；PDF/Office/二进制 **无专门处理、无 isBinary 检测、无降级**（`ide.readFile` 纯文本读取，二进制得乱码或抛错）。
- **截断**：provider 层无 per-file 截断（Continue 上层有全局 `MAX_CONTEXT_ITEMS_TOKENS` 预算）。
- 来源：[PearAI docs](https://github.com/trypear/pearai-documentation/blob/main/docs/index.md)、[Continue FileContextProvider.ts](https://github.com/continuedev/continue/blob/main/core/context/providers/FileContextProvider.ts)

### Cursor（闭源 IDE）

- **注入语义**：@file/@docs 注入内容；@Docs 走索引 + embedding 检索（非全文注入）。
- **图片**：vision 模型（拖拽/粘贴）。
- **PDF**：原生不支持（社区反馈 "Cursor can't read PDF natively via @Docs"，需 MCP 转换或放项目文件夹 @ 引用）。
- **其余**：闭源，截断/二进制/Office 未公开确认。
- 来源：[docs.cursor.com/learn/context](https://docs.cursor.com/learn/context)、[Forum: Add local PDF via @Docs](https://forum.cursor.com/t/add-local-pdf-via-docs/1945)

---

## 五、跨 5 家共识（业界标准）

1. **注入内容，不是路径** — 5 家全部。Everlasting PR1「纯文本路径提示」是唯一被判最差形态。
2. **二进制必须检测，不能乱码喂 LLM** — 4 家检测，pi 不检测 = 公认短板。
3. **降级而非崩溃** — 占位串 / fail / 静默丢三选一，无一家硬塞乱码、无一家降级为路径文本。
4. **截断默认 2000 行 / 50KB** — opencode、pi read 工具、Claude Code 一致。
5. **类型判定做成独立 dispatch 层**（media 优先于 binary 判定）— 即使暂不支持图片注入，也建议分离，未来加多模态时图片/PDF 不会被二进制检测误杀。

---

## 六、对 Everlasting PR2 的定稿建议

**场景约束**：纯文本通道（`llm/types.rs` `ContentBlock` 只有 text/tool_use/tool_result，wire 零 multimodal）+ 自研学习项目（不引 docx/xlsx 解析 crate）。

| 决策 | 定稿 | 依据 |
|---|---|---|
| **降级哲学** | **Cline 占位串** | 4 家共识"降级不崩溃"；占位串比 fail 友好、比静默丢可见；纯文本通道全 fail 太激进 |
| **二进制检测** | **扩展名黑名单 + null byte + 30% 非打印比** | opencode/Cline 算法一致；pi 无检测是反面教材；Rust 实现轻量 |
| **图片/PDF/Office** | **一律占位降级**（纯文本通道做不了多模态） | 5 家图片都需 multimodal 基础设施；属 B1（第三档）范围 |
| **截断** | **复用 `read_file` 的 50KB head+tail** | opencode/pi 共识值；复用避免两套截断逻辑 |
| **注入形态** | **注入内容 = `read_file` 输出格式**（cat -n 行号） | opencode 核心设计直觉（内容 vs 工具输出格式统一） |
| **越界** | **复用 `projects::boundary::assert_within_root`** | opencode 拒绝 `..`/绝对/symlink 三连；Everlasting 已有 5-tier 权限层 |
| **占位引导** | PDF→`pdftotext`、Office→`pandoc`（shell 工具转换） | 零新依赖，复用已有 shell 工具 |

### 分层降级占位文案（PR2 范围，零新依赖）

| 类型 | PR2 处理 | 占位提示文案 |
|---|---|---|
| 文本 | ✅ 注入内容（复用 read_file 截断） | — |
| 图片 | 占位 | `[image: x.png — 当前模型为纯文本通道，不支持图片注入（B1 计划）]` |
| PDF | 占位 | `[binary: x.pdf — 二进制文档未注入；可 shell 工具运行 pdftotext 转文本后引用]` |
| Office | 占位 | `[binary: x.docx — 二进制文档未注入；可 shell 工具运行 pandoc x.docx -t plain 转文本后引用]` |
| 纯二进制 | 占位 | `[binary: x.zip — 二进制文件，无法注入文本内容]` |

### 未来扩展（B1 多模态，留档参考）

- 图片缩放管线直接抄 **pi 的 `image-resize-core.ts`**（PNG/JPEG 双编码取优 + 质量阶梯 + 尺寸递减 + 坐标比例提示）或 opencode 的 photon WASM 方案（2000²/5MB/Lanczos3）。
- 图片识别用 **magic bytes 嗅探**（pi/opencode 共识）而非扩展名，更鲁棒。
- 注入图片走**独立 content block**（Aider `get_images_message` 模式），前置 `"Image file: <name>"` 文本 + 视模型能力守卫（Aider `supports_vision` / Cline multimodal 模型判定）。
- 文本类型判定 dispatch 层（media 优先于 binary）— PR2 就分离好，B1 加图片时不被二进制检测误杀。

---

## 来源汇总

- opencode: [session/prompt.ts](https://github.com/sst/opencode/blob/dev/packages/opencode/src/session/prompt.ts) · [tool/read.ts](https://github.com/sst/opencode/blob/dev/packages/opencode/src/tool/read.ts) · [image/image.ts](https://github.com/sst/opencode/blob/dev/packages/opencode/src/image/image.ts) · [tui.mdx](https://github.com/sst/opencode/blob/dev/packages/web/src/content/docs/tui.mdx)
- Aider: [base_coder.py](https://github.com/Aider-AI/aider/blob/main/aider/coders/base_coder.py) · [commands.py](https://github.com/Aider-AI/aider/blob/main/aider/commands.py) · [utils.py](https://github.com/Aider-AI/aider/blob/main/aider/utils.py) · [images-urls 文档](https://aider.chat/docs/usage/images-urls.html)
- Cline: [mentions/index.ts](https://github.com/cline/cline/blob/main/apps/vscode/src/core/mentions/index.ts) · [extract-text.ts](https://github.com/cline/cline/blob/main/apps/vscode/src/integrations/misc/extract-text.ts) · [content-limits.ts](https://github.com/cline/cline/blob/main/apps/vscode/src/shared/content-limits.ts) · [Working with files 文档](https://docs.cline.bot/core-workflows/working-with-files)
- pi: [cli/file-processor.ts](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/src/cli/file-processor.ts) · [utils/mime.ts](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/src/utils/mime.ts) · [utils/image-resize-core.ts](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/src/utils/image-resize-core.ts) · [core/tools/read.ts](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/src/core/tools/read.ts)
- PearAI: [docs/index.md](https://github.com/trypear/pearai-documentation/blob/main/docs/index.md) · [Continue FileContextProvider.ts](https://github.com/continuedev/continue/blob/main/core/context/providers/FileContextProvider.ts)
- Cursor: [docs.cursor.com/learn/context](https://docs.cursor.com/learn/context) · [Forum: Add local PDF via @Docs](https://forum.cursor.com/t/add-local-pdf-via-docs/1945)
