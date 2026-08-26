# Pattern: @文件 PDF/docx/xlsx 原生文本提取注入(F5 + follow-up,2026-08-26)

## Problem

`@relpath` 引用 PDF/Office 时占位降级,LLM 只能看到"可 shell 运行 pdftotext"的提示——用户要的"读这份文档"变成了一轮自助转换折腾。而引入完整解析栈(pdfium 动态库四平台分发)成本过重。

## Solution: 提取是注入的一种形态(纯 Rust 轻依赖)

`agent/doc_extract.rs` 纯函数(bytes 进 → `ExtractedText` 出,不碰 ToolContext/IO);`at_file::expand_single` 在 Degraded 兜底**之前**先尝试提取——成功即走文本 span 通道(与 Text 注入同构:marker + span + `at_files_token` 度量),失败(扫描件/corrupt/超限)落指令式自助占位。turn 永不因提取失败而死(照抄 B1 `try_inject_image` 的 fail-open)。

```
FileKind::Pdf          → try_extract(Pdf)   → pdf-extract::extract_text_from_mem
FileKind::Office && .docx → try_extract(Docx) → zip(deflate-only) + quick-xml 提 w:t
FileKind::Office && .xlsx/.xlsm → try_extract(Xlsx) → calamine(Xlsx reader)
其余 Office(OLE2/odf/rtf/pptx)→ 不进提取,直接 Degraded(D1:老格式纯 Rust 不可行;pptx 用户裁定不做)
```

## 硬约束(违反即回归)

1. **三级 cap 顺序**:源字节 `MAX_EXTRACT_SOURCE_BYTES`(20 MiB,解析前 fail-fast)→ 扫描件判定 `< MIN_TEXT_LAYER_CHARS`(32 字符,spike 实证位图页返回 0;**仅 pdf 路径**)→ 文本截断 `MAX_EXTRACT_CHARS`(150_000,≈CJK 50k tok,保头截断 + marker 标注 `orig_chars`)。截断后的文本走既有 D10 同请求 span → 关卡⑤硬卡,不改闸门本身。
2. **pdf-extract / calamine 必须 catch_unwind 包裹**:第三方解析器对畸形输入的鲁棒性未经审计;turn 不死是硬约束(纯计算无 IO 状态,兜底安全)。依赖 release profile 保持 panic=unwind(不得回设 abort)。
3. **wire 变体字段名避开 serde tag**:`InjectionAction` 用 `#[serde(tag = "kind")]`,新变体 `Extracted` 的来源字段必须叫 `format`(叫 `kind` 会冲突编译失败)。TS 镜像同步 `format: "pdf" | "docx" | "xlsx"`。
4. **页数/段落/sheet 数与原文规模只进 LLM marker**(`<doc path kind pages|paras|sheets chars truncated orig_chars>`),wire 只带 `format/chars/truncated`——前端 hint 行够用,避免 manifest 膨胀。`ExtractKind` 加变体会被 `render_extracted_marker` 的穷尽 match 编译器强制补臂(kind_str + unit_label 成对出现)。
5. **docx 实体在 quick-xml 0.42 是独立 `GeneralRef` 事件**(payload 为实体名 "amp" 或字符引用体 "#x4E2D"),不是 Text 事件内容——必须显式映射预定义五实体 + 字符引用,否则静默丢字。空段 `<w:p/>` 走 Empty 事件,同样要计数 + 换行。
6. **占位文案是 prompt(D3 分层兜底)**:Degraded 文案从"教用户跑命令"改为指令式(agent 主体:"agent 可自行转换:pdftotext <path> - 后读取")——LLM 有 shell 工具,读到指令即自助。这是 Codex skills 式路线的零成本形态。
7. **xlsx 表格→文本 = 每 sheet 一段 CSV 块**(follow-up prd D3,用户拍板):sheet 标题行 `## <名> (<R>行×<C>列)`、RFC4180 转义(逗号/引号/换行加引号翻倍)、每行去尾随 Empty、空 sheet `(空)`。**xlsx 路径不做 normalize_whitespace**(压并空行/trim 会破坏 CSV 行语义);全 sheet 无数据 → Err 走 Degraded 兜底。单元格渲染:字符串原样 / 数字最短表示 / bool true·false / 错误值保留 `#REF!` 形态 / 公式取缓存值(calamine 默认)/ 序列日期经 chrono 转 ISO(`%Y-%m-%d`,非零点补 ` %H:%M:%S`)。

## xlsx 依赖结论(F5 follow-up,2026-08-26)

calamine 0.36(纯 Rust):其 zip 依赖同为 **default-features=false + deflate-only**,与既有 zip 特征并集不变(zstd-sys 不回归);chrono feature 启用序列日期 → NaiveDateTime 转换,而 chrono 已是直接依赖 → 零新增 crate。`.xlsm` 同为 OOXML zip 包,Xlsx reader 直接吃;扩展名分流在 at_file(`lower_ext` match `.docx` / `.xlsx | .xlsm`),`.xls`(OLE2)不进提取。

## 依赖结论(prd D4 spike 判定)

`pdf-extract` 0.12(纯 Rust,传递 lopdf 被 `pub use lopdf::*` 全量 re-export,页数零新增依赖)+ `zip`(**default-features = false, features = ["deflate"]**——默认 features 拉 zstd-sys C 编译依赖,docx 只需 deflate)+ `quick-xml`(传递树转直接)。spike 实证:中文(Chromium 生成)零乱码零丢字;英文长文档与 pdftotext 语义等价(33,957 vs 34,104 chars)。

已知限制(不做后处理):Chromium 类渲染器 kerning 拆词("Ag en t")——启发式合并对真实文本("to do"/"I am")误伤风险大于收益,LLM 鲁棒,保持原样。

## Wrong vs Correct

```rust
// Wrong — cargo init 在 workspace 目录内建 spike 项目会把自己挂进
// 根 workspace members(污染 Cargo.lock + 解析失败);zip 默认 features
// 拉进 zstd-sys(C sys 依赖)
cargo add zip

// Correct — spike 项目放仓库外(或用完即删 + 手动修 members);
// zip 只要 deflate
cargo add zip --no-default-features --features deflate
```

## Tests

- `agent/doc_extract.rs` 内联:pdf 提取/扫描件判定/corrupt 不 panic/cap 截断/docx CJK+实体+tab+空段/corrupt zip/超源拒绝(fixture = 手写最小 PDF 字节常量 602B + 运行时 zip writer 构造 docx);xlsx:CJK sharedStrings + RFC4180 转义/多 sheet 顺序 + 空 sheet/序列日期 ISO/inlineStr + 错误单元格/corrupt fail-soft + 全空降级/csv_escape 单测(fixture = `test_fixtures::build_xlsx` 运行时手写 OOXML 部件)
- `agent/at_file.rs` 集成:@doc.pdf → `<doc kind="pdf" pages="1">` marker + `Extracted` action + span;@spec.docx 同构;@old.doc(OLE2)→ Degraded + 指令式文案;@data.xlsx → `<doc kind="xlsx" sheets="1">`;@m.xlsm 同 xlsx 通道 + @old.xls → Degraded
- vitest `FileInjectionsHint.test.ts`:extracted 三态(pdf/docx/xlsx 标签、截断徽标)
- live:`turn-smoke.sh --message "@sample-zh.pdf …"` → at_files_token 落值(432 tok / 553 chars 精确吻合)+ manifest `{"kind":"extracted","format":"pdf","chars":553}` + 模型正确答出文档标题;xlsx follow-up live 冒烟见 task 08-26-f5-xlsx-extraction
