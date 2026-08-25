# Design — F5 PDF/Office 文档阅读(@文件原生注入)

> 对应 prd.md R1-R6/D1-D6。核心思路:**提取是注入的一种形态**——`FileKind::Pdf/Office` 在占位降级前先过提取尝试,成功即走文本 span 通道,失败落到指令式自助占位。turn 永不因提取失败而死(照抄 B1 `try_inject_image` 的 fail-open 姿态)。

## 1. 架构与边界

```
expand_at_file_token(rel_path)
  ├─ classify → FileKind::Pdf
  │    └─ try_extract_pdf(bytes)          [新模块 agent/doc_extract.rs]
  │         ├─ Ok(text 有内容)  → 文本 span 注入(marker: [pdf: a.pdf, N 页 M 字符(截断)])
  │         └─ 空/Err(含无文本层)→ Degraded 占位(自助文案)
  ├─ classify → FileKind::Office && ext == .docx
  │    └─ try_extract_docx(bytes)
  │         ├─ Ok(段落文本)  → 文本 span 注入(marker: [docx: spec.docx, N 段 M 字符])
  │         └─ Err(corrupt zip/无 document.xml)→ Degraded 占位(自助文案)
  ├─ FileKind::Image → B1 通道(零改动)
  └─ 其余(Text 直注 / OLE2 / odf / rtf / Binary)→ 现状
```

- **新模块 `agent/doc_extract.rs`**:提取纯函数(`bytes → Result<ExtractedText, ExtractError>`),不碰 ToolContext/IO——at_file 读盘后传入,提取结果由 at_file 决定注入形态。可单测、可被未来 xlsx/pptx 增量复用。
- **`ExtractedText { text, meta }`**:meta 携带页数(pdf)/段落数(docx)/原文字符数/是否截断,marker 与 manifest 共用。
- **`InjectionAction` 扩展(wire additive)**:新增 `Extracted { kind: "pdf"|"docx", chars, pages, truncated }` 变体——`Injected { lines }`/`Degraded { file_kind }` 形状不动,前端 `FileInjectionsHint` 按新变体渲染(旧会话回放不含新变体,前端需容错)。

## 2. PDF 文本提取(D4 spike 闸门)

- **PR0 spike**:headless Chromium 把中文 HTML 打印成 PDF(带真实文本层;ui-review.sh 同款 Chromium)+ `/usr/share/doc/shared-mime-info/shared-mime-info-spec.pdf`(英文长文档)+ pdftotext 输出作对照基准。用 `pdf-extract` 提取三个样本,判据:**CJK 无乱码/丢字、段落结构可读、与 pdftotext 对照语义等价**。
- **过关** → 直接用 `pdf-extract`(纯 Rust,零二进制负担);扫描件检测 = 提取结果为空/近空(文本层字符数 < 32)→ Degraded。
- **不过关** → 升级 `pdfium-render`(文本质量工业级 + 渲染解锁):此时扫描件路线升级为"渲染前 N 页位图走 B1 ImageRef 通道"(受 B1 两级张数闸约束),D5 的 follow-up 变本档实现。pdfium 动态库经 build.rs/分发脚本带四平台(daemon.sh 与 Tauri bundler 两处)。
- 提取入口统一 `MAX_EXTRACT_SOURCE_BYTES = 20 MiB`(读盘后先按字节 cap,防恶意巨大 PDF 把提取器拖死;比文本 cap 更早 fail-fast)。

## 3. docx 提取

- `zip` crate 定位 `word/document.xml` → `quick-xml`(转直接依赖,已在传递树)流式解析:`w:p`(段落)→ 输出行,`w:t`(文本 run)→ 拼接,`w:tab`/`w:br` → 制表/换行。忽略样式/关系/页眉脚(body 主体)。
- 魔数前置:非 `PK\x03\x04` 直接 Err(corrupt)。
- OLE2 老格式(`.doc/.xls/.ppt`)不进提取(扩展名分流在 at_file,`.docx` 专属)。

## 4. Token 治理(D6)

- `MAX_EXTRACT_CHARS = 150_000`(≈ CJK 50k token 量级,单文件占 200k 窗口的 1/4 封顶):超限**保留头部**截断,marker 标注"[已截断,原文 N 字符]"。
- 截断后的文本作为普通 @文件 span 进 D10 同请求临时 spans → 既有 `turn_trace.at_files` 度量 + 关卡⑤硬卡(多文件叠加超线时按既有裁剪规则处理,本任务不改关卡⑤)。

## 5. 占位文案升级(R4/D3)

`expand_for_kind` 全部 Degraded 文案从"教用户跑命令"改为**指令式(agent 为执行主体)**:

| kind | 现文案(用户主体) | 新文案(agent 主体) |
|---|---|---|
| Pdf(扫描件/失败) | 可 shell 运行 pdftotext 转文本后引用 | agent 可自行转换:`pdftotext <path> -` 后读取(或 OCR) |
| Office(docx 失败/其余) | 可 shell 运行 pandoc … | agent 可自行转换:`pandoc <path> -t plain`(或 libreoffice headless) |
| Binary | (现状) | 顺手统一为指令式 |

前端 `FileInjectionsHint` 的 hint 行(:103-110)同步。

## 6. 兼容与回滚

- 无 DB migration;wire 仅 additive(`InjectionAction::Extracted` 新变体);旧客户端/回放不含新变体,前端匹配按 kind 容错。
- 开关:不加新开关——提取失败自然落 Degraded,fail-open 语义与 B1 一致(B1 也没有 image 开关)。
- 回滚单元 = 整个任务单 revert:新模块独立、at_file 改动集中在两处分流点 + 文案。
- 测试改写:`pdf_file_degrades_to_placeholder` / `office_file_degrades_to_placeholder` 改为提取成功路径 + 降级路径双测试(fixture 详见 implement)。

## 7. 关键 trade-offs

- **提取在 at_file 内联(请求构造时)vs 独立工具让 agent 调**:选内联——@引用的语义就是"把内容给我看",即时注入免一轮工具往返;agent 自助(独立工具/转换)是兜底层不是主路径(D3)。
- **头部截断 vs 首尾截断**:选头部(简单可预期;PDF 首部通常是标题/摘要,信息密度最高)。尾部信息丢失由 marker 提示 agent 可自行转换全文。
- **pdf-extract 先行 vs 直接 pdfium**:选 spike 闸门——二进制负担是长期成本(四平台分发/daemon.sh/Tauri bundler),能免则免;实测不过再买断(D4)。
