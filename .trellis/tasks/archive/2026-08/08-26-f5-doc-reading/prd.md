# F5 PDF/Office 文档阅读(@文件原生注入)

## Goal

消除 B2 @文件对 PDF/Office 的占位降级:**文本型 PDF 与 docx 平台内置原生注入**(即时、确定、零环境依赖);其余格式(OLE2 老格式 / odf / rtf / 一切提取失败场景)降级为**引导 agent 自助**的指令式占位(LLM 经 shell 工具自行转换);扫描件 PDF(无文本层)MVP 降级占位,位图渲染走 follow-up(若依赖升级 pdfium 则连带解锁)。

## Background(代码勘察 2026-08-26)

- 占位降级现状 `agent/at_file.rs`(1367 行):`classify` 按扩展名分 `Text/Image/Pdf/Office/Binary`;`PDF_EXTS=[.pdf]`、`OFFICE_EXTS` 十种 → `InjectionAction::Degraded` 占位(PDF 提示 pdftotext、Office 提示 pandoc);测试 `pdf_file_degrades_to_placeholder`(at_file.rs:993)、`office_file_degrades_to_placeholder`(:1009)锁定占位行为,实现期改写。
- B1 图片通道模板 `try_inject_image`(at_file.rs:569):白名单 → 魔数校验 → 5 MiB cap → attachments 副本 → ImageRef + `(w×h)/750` 估算;失败 → Degraded,**turn 永不因注入失败而死**。
- @文件 token 治理:同请求临时 spans(D10)进关卡⑤硬卡(0.95×window)+ `turn_trace.at_files` 度量列;文本注入走 `Injected { lines }`。提取文本天然进同一体系,但**单请求内首个巨型 span** 需新增单文件 cap。
- 前端 `FileInjectionsHint.vue` 渲染 per-file 注入态,`file_kind === "pdf"` 现显示"未注入(可 pdftotext 转换)"。
- 依赖现状:`flate2`(rust_backend)已直接依赖;`quick-xml` 已在传递树(tauri 经 plist);无 zip/pdf 依赖。

## 决策记录(brainstorm 2026-08-26,用户拍板/推荐采纳)

| # | 决策 | 理由 |
|---|---|---|
| D1 | 范围 = **PDF(文本型)+ docx** | "读文档"场景价值主体;xlsx 表格→文本需单独形态设计,pptx 碎片化,管线通后增量 |
| D2 | **不引入 Node.js 运行时** | daemon 单二进制零依赖不变量(WSL/远程 PC/sidecar 三形态);提取在后端请求构造时;前端 node 仅为构建工具链。业界对照:Codex skills 式自助路线作为兜底层而非替代 |
| D3 | **分层:高频内置 + 长尾 agent 自助** | 内置(即时/确定/零环境依赖)覆盖 PDF+docx;Degraded 占位文案升级为指令式("agent 可 pandoc 转换后引用")点亮自助路线——LLM 有 shell 工具,读到指令自行转换。正式 document skill 走 B4 体系 follow-up |
| D4 | PDF 提取依赖走 **spike 闸门**:先实测 `pdf-extract`(纯 Rust)对真实中文 PDF 的质量;不过关升级 `pdfium-render`(工业级,连带解锁扫描件渲染,代价是 pdfium 动态库四平台打包) | 由实测数据决定,不猜 |
| D5 | 扫描件(无文本层)MVP **降级占位**(自助文案);pdfium 渲染成位图走 B1 通道为 follow-up 档(D4 升级则本档连带实现) | 纯 Rust 无可用 PDF 渲染实现 |
| D6 | 超长文档 **单文件截断 cap**(design 定数值)+ marker 标注原文规模;token 进既有 at_files span/关卡⑤体系 | 单请求内首个巨型 span 关卡⑤裁不动(裁的是旧 span) |

## Requirements

- **R1 @pdf 文本型注入**:`FileKind::Pdf` 先尝试文本提取;成功 → 提取文本按文本 span 注入,marker 标注页数/字符数(+截断信息);无文本层(扫描件)或提取失败 → Degraded 占位(自助文案),turn 不死。
- **R2 @docx 注入**:zip + XML 提取段落文本(段落感知,`w:p`→换行),CJK 正确;失败 → Degraded 占位。
- **R3 cap**:单文件提取文本超 `MAX_EXTRACT_CHARS` 截断(保留头部),marker 标注"已截断,原文 N 字符"。
- **R4 占位文案升级**:全部 Degraded 路径(OLE2/odf/rtf/扫描件/提取失败/corrupt zip)从"教用户跑命令"改为指令式自助文案(agent 为执行主体)。
- **R5 前端状态**:`FileInjectionsHint` 展示新注入态(pdf/docx 已注入 N 字符/页、截断徽标、扫描件降级)。
- **R6 回归**:图片通道/纯文本通道既有行为零变化;pdf/office 占位两条既有测试按新契约改写。

## Acceptance Criteria

> 回填说明(2026-08-26):全部验证通过 —— 后端 1982(含 doc_extract 7 + at_file F5 集成 3 新增)、前端 vitest 1201(含 extracted 3 新增)+ vue-tsc 零错;live 冒烟:turn-smoke at_files_token=432(553 字符中文 PDF 精确吻合)+ manifest {"kind":"extracted","format":"pdf","chars":553} + 模型正确答出文档标题「大语言模型 Agent 系统设计白皮书」;AC2/AC4/AC5 由单测层验证(与 AC1 同链路)。

- [x] AC1 `@`中文文本型 PDF → 注入提取文本,marker 含页数与字符数,`turn_trace.at_files` 落值。
- [x] AC2 `@`扫描件 PDF(无文本层)→ Degraded 占位 + 自助文案;turn 正常完成。
- [x] AC3 `@spec.docx` → 段落文本注入,CJK 无乱码。
- [x] AC4 超限文档 → 截断至 cap + marker 标注原文规模。
- [x] AC5 `@spec.doc`(OLE2)→ 占位 + 指令式自助文案。
- [x] AC6 图片通道回归:既有 image 注入测试全过,零改动。
- [x] AC7 前端 FileInjectionsHint 显示 pdf/docx 新注入态(vitest)。
- [x] AC8 全量回归:cargo(vitest 含新用例)全绿,live 冒烟经 turn-smoke 实跑一次 @PDF 注入。

## Out of Scope(follow-up 档)

- xlsx/pptx/odf/rtf 原生提取(docx 管线通后的增量,表格形态需单独设计)
- pdfium 渲染扫描件成位图走 B1 通道(D4 spike 失败升级时连带实现)
- 正式 document skill(B4 skill 体系,Codex openai/skills 式)
- OCR

## Open Questions

无(阻塞项已全部拍板;D4 判据在 implement PR0 落地)。
