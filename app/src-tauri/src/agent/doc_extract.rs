//! F5 (2026-08-26, task 08-26-f5-doc-reading): native text extraction for
//! `@`-referenced PDF / docx files, consumed by `at_file::expand_single`
//! before the Degraded placeholder fallback.
//!
//! 纯函数模块:bytes 进,提取文本出;不碰 ToolContext / IO —— 注入形态的
//! 决策权留在 at_file(成功 → 文本 span 注入,失败 → 占位降级,turn 永不
//! 因提取失败而死,与 B1 `try_inject_image` 的 fail-open 姿态同构)。
//!
//! 依赖结论(prd D4 spike,2026-08-26):`pdf-extract`(纯 Rust)对
//! Chromium 生成的中文 PDF 零乱码零丢字,英文长文档与 pdftotext 语义
//! 等价,扫描件(位图页)返回 0 字符 —— 不需要 pdfium。页数经
//! `pdf_extract::Document`(lopdf re-export,零新增)读 `/Pages /Count`。
//!
//! 已知限制(评审记录,不做后处理):Chromium 类渲染器导出的 PDF 可能把
//! 英文单词按 kerning 拆开("Ag en t")。启发式合并(短 token 链拼接)对
//! 真实文本("to do"/"I am")的误伤风险大于收益,LLM 对此鲁棒 —— 保持
//! 原样,marker 不动文本。
//!
//! F5 验证后续(2026-08-26,task 08-26-f5-verify-followups 问题 1):
//! pdf-extract 0.12 对非嵌入 CJK 字体的 `/Encoding /UniGB-UCS2-H` 形态
//! panic(WPS/Word 中文导出主流形态),且无 ToUnicode 时全部字符被静默
//! 吞成 ""。现依赖 vendored 副本(`vendor/pdf-extract/`,path 依赖,
//! 版本号与上游 0.12.0 一致):① Uni{GB,CNS,JIS,KS}-UCS2-{H,V} 8 个
//! CMap 名按「码值即 UCS-2 码位」零资源解码;② 未知编码家族(GB-EUC/
//! B5pc 等)与畸形 ToUnicode 降级为跳过该字体,不再 panic。升级上游版本
//! 时需重放 patch,清单见 vendor/pdf-extract/Cargo.toml 头注释。
//!
//! F5 follow-up(2026-08-26,task 08-26-f5-xlsx-extraction):xlsx 原生
//! 提取(.xlsm 同管线;.xls OLE2/.ods/pptx 保持占位降级)。库选 calamine
//! (纯 Rust;其 zip 依赖同为 deflate-only,特征并集不变)。表格→文本
//! 形态(prd D3,用户拍板):每 sheet 一段 CSV 块,RFC4180 转义,sheet
//! 标题行带维度;空 sheet 输出 `(空)`。单元格渲染(D5):字符串原样,
//! 数字最短表示,布尔 true/false,错误值保留 `#REF!` 形态,公式取缓存
//! 值(calamine 默认),序列日期经 chrono 转 ISO(纯日期不带时间)。

use serde::{Deserialize, Serialize};
use std::io::Read;

/// 提取来源类型。wire snake_case(`"pdf"` / `"docx"` / `"xlsx"`),进
/// `InjectionAction::Extracted` 的前端判别。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtractKind {
    Pdf,
    Docx,
    Xlsx,
}

/// 提取结果。`units` 不进 wire(前端只显示字符数),只进 LLM marker:
/// pdf = 页数,docx = 段落数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractedText {
    pub text: String,
    pub units: usize,
    pub orig_chars: usize,
    pub truncated: bool,
}

/// 源文件字节上限(在解析前 fail-fast,防巨型 PDF 拖死提取器)。
pub const MAX_EXTRACT_SOURCE_BYTES: usize = 20 * 1024 * 1024;
/// 单文件提取文本字符上限(D6:≈ CJK 50k token,单文件占 200k 窗口的
/// 1/4 封顶)。超限保留头部截断,marker 标注原文规模。
pub const MAX_EXTRACT_CHARS: usize = 150_000;
/// 低于此字符数的 PDF 提取结果判为无文本层(扫描件)→ 降级占位。
/// spike 实证:位图页返回 0 字符;阈值取 32 留余量(纯页码/页眉型 PDF
/// 对 LLM 价值趋近于零,降级反而给了自助转换指引)。
pub const MIN_TEXT_LAYER_CHARS: usize = 32;

/// 统一入口。错误信息只进 tracing(降级原因),不面向用户。
pub fn try_extract(kind: ExtractKind, bytes: &[u8]) -> Result<ExtractedText, String> {
    if bytes.len() > MAX_EXTRACT_SOURCE_BYTES {
        return Err(format!(
            "source {} bytes exceeds {} cap",
            bytes.len(),
            MAX_EXTRACT_SOURCE_BYTES
        ));
    }
    match kind {
        ExtractKind::Pdf => extract_pdf(bytes),
        ExtractKind::Docx => extract_docx(bytes),
        ExtractKind::Xlsx => extract_xlsx(bytes),
    }
}

fn extract_pdf(bytes: &[u8]) -> Result<ExtractedText, String> {
    if !bytes.starts_with(b"%PDF") {
        return Err("missing %PDF magic".into());
    }
    // pdf-extract 内部对畸形输入有 unwrap 路径;turn 不死是硬约束,
    // catch_unwind 兜底(提取是纯计算,无 IO 状态可污染)。
    // 注意:catch_unwind 依赖 panic=unwind —— workspace 根 Cargo.toml 的
    // release profile 不得回设 panic="abort"(abort 不 unwind,任何第三
    // 方库 panic 都会直接杀掉 daemon 进程,此处兜底沦为死代码)。
    let raw = std::panic::catch_unwind(|| pdf_extract::extract_text_from_mem(bytes))
        .map_err(|_| "pdf extractor panicked".to_string())?
        .map_err(|e| format!("pdf extract: {e:?}"))?;
    let text = normalize_whitespace(&raw);
    let chars = text.chars().count();
    if chars < MIN_TEXT_LAYER_CHARS {
        return Err(format!(
            "no text layer (scanned?) — {chars} chars < {MIN_TEXT_LAYER_CHARS}"
        ));
    }
    let pages = std::panic::catch_unwind(|| {
        pdf_extract::Document::load_mem(bytes)
            .map(|doc| doc.get_pages().len())
            .unwrap_or(0)
    })
    .unwrap_or(0);
    Ok(cap(text, pages))
}

fn extract_docx(bytes: &[u8]) -> Result<ExtractedText, String> {
    if !bytes.starts_with(b"PK") {
        return Err("missing zip (PK) magic".into());
    }
    let mut zip =
        zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| format!("open zip: {e}"))?;
    let mut xml = String::new();
    zip.by_name("word/document.xml")
        .map_err(|e| format!("locate word/document.xml: {e}"))?
        .read_to_string(&mut xml)
        .map_err(|e| format!("read document.xml: {e}"))?;

    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_str(&xml);
    let mut out = String::with_capacity(xml.len() / 4);
    let mut paras = 0usize;
    let mut in_wt = false; // 收集 w:t 内的文本 run,屏蔽 instrText 等邻居
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => match e.name().as_ref() {
                "w:p" => paras += 1,
                "w:t" => in_wt = true,
                "w:br" | "w:cr" => out.push('\n'),
                _ => {}
            },
            Ok(Event::Empty(e)) => match e.name().as_ref() {
                "w:p" => {
                    paras += 1;
                    out.push('\n');
                }
                "w:tab" => out.push('\t'),
                "w:br" | "w:cr" => out.push('\n'),
                // 空 w:t:<w:t/> 是合法的空 run
                "w:t" => {}
                _ => {}
            },
            Ok(Event::Text(t)) => {
                if in_wt {
                    // docx body 是 XML 1.0(OOXML 规范);`html_content`
                    // 是 quick-xml 0.42 的 xml10 别名,处理预定义实体。
                    out.push_str(&t.html_content());
                }
            }
            // 0.42 把实体引用拆成独立 GeneralRef 事件,payload 是实体
            // 名("amp")或字符引用体("#20013"/"#x4E2D");OOXML 只允许
            // 预定义五实体 + 字符引用,其余忽略(fail-open,提取不死)。
            Ok(Event::GeneralRef(r)) => {
                if in_wt {
                    let name = r.into_inner();
                    match name.as_ref() {
                        "amp" => out.push('&'),
                        "lt" => out.push('<'),
                        "gt" => out.push('>'),
                        "apos" => out.push('\''),
                        "quot" => out.push('"'),
                        other => {
                            let hex = other
                                .strip_prefix("#x")
                                .or_else(|| other.strip_prefix("#X"));
                            let cp = if let Some(h) = hex {
                                u32::from_str_radix(h, 16).ok()
                            } else {
                                other.strip_prefix('#').and_then(|d| d.parse::<u32>().ok())
                            };
                            if let Some(c) = cp.and_then(char::from_u32) {
                                out.push(c);
                            }
                        }
                    }
                }
            }
            Ok(Event::End(e)) => match e.name().as_ref() {
                "w:t" => in_wt = false,
                "w:p" => out.push('\n'),
                _ => {}
            },
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(e) => return Err(format!("xml parse: {e}")),
        }
    }
    let text = normalize_whitespace(&out);
    if text.chars().count() == 0 {
        return Err("docx body has no text".into());
    }
    Ok(cap(text, paras))
}

fn extract_xlsx(bytes: &[u8]) -> Result<ExtractedText, String> {
    if !bytes.starts_with(b"PK") {
        return Err("missing zip (PK) magic".into());
    }
    // 与 pdf 路径同姿态:第三方解析器对畸形输入的鲁棒性未经审计,
    // catch_unwind 兜底(纯计算,无 IO 状态可污染;依赖 release profile
    // 保持 panic=unwind,见 extract_pdf 注释)。
    let (text, units) = std::panic::catch_unwind(|| render_xlsx_sheets(bytes))
        .map_err(|_| "xlsx parser panicked".to_string())??;
    Ok(cap(text, units))
}

/// calamine 解析 + CSV 渲染主体(供 catch_unwind 包裹)。返回
/// (文本, sheet 数)。
fn render_xlsx_sheets(bytes: &[u8]) -> Result<(String, usize), String> {
    use calamine::Reader as _;
    let mut book =
        calamine::Xlsx::new(std::io::Cursor::new(bytes)).map_err(|e| format!("open xlsx: {e}"))?;
    let sheets = book.worksheets();
    let units = sheets.len();
    if units == 0 {
        return Err("xlsx workbook has no sheets".into());
    }
    let mut out = String::new();
    let mut any_data = false;
    for (name, range) in sheets {
        let (h, w) = (range.height(), range.width());
        if h == 0 || w == 0 {
            out.push_str(&format!("## {name} (空)\n"));
            continue;
        }
        any_data = true;
        out.push_str(&format!("## {name} ({h}行×{w}列)\n"));
        for row in range.rows() {
            // 去尾随空单元格:稠密网格按 width 补齐,尾随 Empty 是
            // 维度噪音,不产生一串逗号。
            let mut end = row.len();
            while end > 0 && matches!(row[end - 1], calamine::Data::Empty) {
                end -= 1;
            }
            let line = row[..end]
                .iter()
                .map(render_cell)
                .collect::<Vec<_>>()
                .join(",");
            out.push_str(&line);
            out.push('\n');
        }
    }
    if !any_data {
        return Err("xlsx has no cell data".into());
    }
    Ok((out, units))
}

/// 单元格 → CSV 字段(prd D5)。注意 xlsx 路径不做 normalize_whitespace
/// —— 那会压并空行、trim 行尾,破坏 CSV 行语义。
fn render_cell(d: &calamine::Data) -> String {
    use calamine::DataType as _;
    let raw = match d {
        calamine::Data::Empty => String::new(),
        calamine::Data::Int(v) => v.to_string(),
        calamine::Data::Float(v) if v.is_finite() => v.to_string(),
        calamine::Data::Float(_) => String::new(),
        calamine::Data::String(s) => s.clone(),
        calamine::Data::Bool(b) => b.to_string(),
        calamine::Data::DateTimeIso(s) | calamine::Data::DurationIso(s) => s.clone(),
        calamine::Data::Error(e) => e.to_string(),
        // 序列日期时间:chrono feature 的 as_datetime 统一换算(1900/
        // 1904 日期系统 calamine 内部已归一);换算失败退回原始序列值。
        d => match d.as_datetime() {
            Some(ndt) => {
                if ndt.time() == chrono::NaiveTime::MIN {
                    ndt.format("%Y-%m-%d").to_string()
                } else {
                    ndt.format("%Y-%m-%d %H:%M:%S").to_string()
                }
            }
            None => d.as_f64().map(|v| v.to_string()).unwrap_or_default(),
        },
    };
    csv_escape(&raw)
}

/// RFC4180:含逗号/引号/换行的字段加引号,内部引号翻倍。
fn csv_escape(field: &str) -> String {
    if field.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", field.replace('"', "\"\""))
    } else {
        field.to_string()
    }
}

/// 头部截断到 [`MAX_EXTRACT_CHARS`],记录原文规模。
fn cap(text: String, units: usize) -> ExtractedText {
    let orig_chars = text.chars().count();
    let (text, truncated) = if orig_chars > MAX_EXTRACT_CHARS {
        (
            text.chars().take(MAX_EXTRACT_CHARS).collect::<String>(),
            true,
        )
    } else {
        (text, false)
    };
    ExtractedText {
        text,
        units,
        orig_chars,
        truncated,
    }
}

/// trim 头尾 + 3+ 连续空行压到 2(pdf-extract 按渲染坐标吐空行,
/// 分页/分栏会拉出长串空白;段落边界保留一个空行)。
fn normalize_whitespace(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut blank_run = 0usize;
    for line in raw.lines() {
        if line.trim().is_empty() {
            blank_run += 1;
            if blank_run <= 2 {
                out.push('\n');
            }
        } else {
            blank_run = 0;
            out.push_str(line.trim_end());
            out.push('\n');
        }
    }
    out.trim().to_string()
}

#[cfg(test)]
pub(crate) mod test_fixtures {
    pub const MINI_TEXT_PDF: &[u8] = b"%PDF-1.4\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>\nendobj\n4 0 obj\n<< /Length 73 >>\nstream\nBT /F1 12 Tf 72 720 Td (Hello PDF native text layer fixture for F5) Tj ET\nendstream\nendobj\n5 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>\nendobj\nxref\n0 6\n0000000000 65535 f \n0000000009 00000 n \n0000000058 00000 n \n0000000115 00000 n \n0000000241 00000 n \n0000000364 00000 n \ntrailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n434\n%%EOF\n";

    pub const MINI_SCAN_PDF: &[u8] = b"%PDF-1.4\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>\nendobj\n4 0 obj\n<< /Length 33 >>\nstream\n1 w 0 0 1 RG 72 700 m 540 700 l S\nendstream\nendobj\n5 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>\nendobj\nxref\n0 6\n0000000000 65535 f \n0000000009 00000 n \n0000000058 00000 n \n0000000115 00000 n \n0000000241 00000 n \n0000000324 00000 n \ntrailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n394\n%%EOF\n";

    /// F5 后续回归样本(2026-08-26):非嵌入 Type0 中文字体形态 ——
    /// /Encoding /UniGB-UCS2-H、DescendantFonts CIDFontType0
    /// (Adobe-GB1)、**无 ToUnicode**(真机 WPS/Word 导出同形态,
    /// pdffonts uni=no)。content stream 的 Tj 用 2 字节大端 UCS-2
    /// 码位 hex string 直出 34 个中文字符。上游 pdf-extract 0.12 对
    /// 该形态 panic(unsupported encoding)+ ToUnicode miss 静默吞字,
    /// vendored patch 后应完整恢复。xref/Length 由生成脚本精确计算。
    pub const UNIGB_UCS2_PDF: &[u8] = b"%PDF-1.4\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>\nendobj\n4 0 obj\n<< /Length 167 >>\nstream\nBT /F1 12 Tf 72 720 Td <98847B976CBB74064E0E4E2D658763D053D66D4B8BD5003A975E5D4C51655B574F5363097EDF4E0078014F4D76F451FA002C9A8C8BC15C424E8C89E378018DEF5F843002> Tj ET\nendstream\nendobj\n5 0 obj\n<< /Type /Font /Subtype /Type0 /BaseFont /STSong-Light /Encoding /UniGB-UCS2-H /DescendantFonts [6 0 R] >>\nendobj\n6 0 obj\n<< /Type /Font /Subtype /CIDFontType0 /BaseFont /STSong-Light /CIDSystemInfo << /Registry (Adobe) /Ordering (GB1) /Supplement 4 >> /FontDescriptor 7 0 R /DW 1000 >>\nendobj\n7 0 obj\n<< /Type /FontDescriptor /FontName /STSong-Light /Flags 4 /FontBBox [-25 -254 1000 880] /ItalicAngle 0 /Ascent 880 /Descent -120 /CapHeight 880 /StemV 93 >>\nendobj\nxref\n0 8\n0000000000 65535 f \n0000000009 00000 n \n0000000058 00000 n \n0000000115 00000 n \n0000000241 00000 n \n0000000459 00000 n \n0000000581 00000 n \n0000000761 00000 n \ntrailer\n<< /Size 8 /Root 1 0 R >>\nstartxref\n933\n%%EOF\n";

    /// F5 后续回归样本:未知编码家族降级 —— F1 Helvetica 的 ASCII 文本
    /// 必须存活,F2 Type0 /Encoding /GB-EUC-H(老家族,code ≠ Unicode,
    /// 无 CMap 资源不可解)的字形被跳过,提取整体不 panic。
    pub const MIXED_GB_EUC_PDF: &[u8] = b"%PDF-1.4\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R /F2 6 0 R >> >> >>\nendobj\n4 0 obj\n<< /Length 138 >>\nstream\nBT /F1 12 Tf 72 720 Td (Fallback ASCII text survives when the CJK font is skipped entirely.) Tj ET\nBT /F2 12 Tf 72 690 Td <B4F3C8FD> Tj ET\nendstream\nendobj\n5 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>\nendobj\n6 0 obj\n<< /Type /Font /Subtype /Type0 /BaseFont /SimSun /Encoding /GB-EUC-H /DescendantFonts [7 0 R] >>\nendobj\n7 0 obj\n<< /Type /Font /Subtype /CIDFontType0 /BaseFont /SimSun /CIDSystemInfo << /Registry (Adobe) /Ordering (GB1) /Supplement 2 >> /FontDescriptor 8 0 R /DW 1000 >>\nendobj\n8 0 obj\n<< /Type /FontDescriptor /FontName /SimSun /Flags 4 /FontBBox [-25 -254 1000 880] /ItalicAngle 0 /Ascent 880 /Descent -120 /CapHeight 880 /StemV 93 >>\nendobj\nxref\n0 9\n0000000000 65535 f \n0000000009 00000 n \n0000000058 00000 n \n0000000115 00000 n \n0000000251 00000 n \n0000000440 00000 n \n0000000510 00000 n \n0000000622 00000 n \n0000000796 00000 n \ntrailer\n<< /Size 9 /Root 1 0 R >>\nstartxref\n962\n%%EOF\n";

    /// F5 follow-up(08-26-f5-xlsx-extraction):测试内构造最小合法
    /// xlsx(zip writer 手写 OOXML 部件,需过 calamine 解析)。`sheets`
    /// = (sheet 名, `<sheetData>` 内部行 XML);`shared_strings` 供
    /// `t="s"` 单元格按下标引用;styles 固定带 numFmtId=164 yyyy-mm-dd
    /// 自定义格式(日期用例以 s="1" 引用)。at_file 集成测试同用。
    pub(crate) fn build_xlsx(sheets: &[(&str, &str)], shared_strings: &[&str]) -> Vec<u8> {
        const NS_MAIN: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
        const NS_REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
        const NS_PKG_REL: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut buf);
            fn put<W: std::io::Write + std::io::Seek>(
                w: &mut zip::ZipWriter<W>,
                name: &str,
                body: &str,
            ) {
                w.start_file(name, zip::write::SimpleFileOptions::default())
                    .unwrap();
                std::io::Write::write_all(w, body.as_bytes()).unwrap();
            }

            // [Content_Types].xml:calamine 宽容,但保持真实包形态。
            let mut ct = String::from(
                "<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
                 <Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
                 <Default Extension=\"xml\" ContentType=\"application/xml\"/>\
                 <Override PartName=\"/xl/workbook.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml\"/>",
            );
            for i in 1..=sheets.len() {
                ct.push_str(&format!(
                    "<Override PartName=\"/xl/worksheets/sheet{i}.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml\"/>"
                ));
            }
            ct.push_str("</Types>");
            put(&mut w, "[Content_Types].xml", &ct);

            put(
                &mut w,
                "_rels/.rels",
                &format!(
                    "<Relationships xmlns=\"{NS_PKG_REL}\">\
                     <Relationship Id=\"rId1\" Type=\"{NS_REL}/officeDocument\" Target=\"xl/workbook.xml\"/>\
                     </Relationships>"
                ),
            );

            // workbook.xml:sheet 声明按序 r:id=rId{i}(sheet 顺序即此序)。
            let mut decls = String::new();
            let mut rels = format!(
                "<Relationships xmlns=\"{NS_PKG_REL}\">\
                 <Relationship Id=\"rIdStyles\" Type=\"{NS_REL}/styles\" Target=\"styles.xml\"/>"
            );
            if !shared_strings.is_empty() {
                rels.push_str(&format!(
                    "<Relationship Id=\"rIdSst\" Type=\"{NS_REL}/sharedStrings\" Target=\"sharedStrings.xml\"/>"
                ));
            }
            for (i, (name, _)) in sheets.iter().enumerate() {
                decls.push_str(&format!(
                    "<sheet name=\"{name}\" sheetId=\"{}\" r:id=\"rId{}\"/>",
                    i + 1,
                    i + 1
                ));
                rels.push_str(&format!(
                    "<Relationship Id=\"rId{}\" Type=\"{NS_REL}/worksheet\" Target=\"worksheets/sheet{}.xml\"/>",
                    i + 1,
                    i + 1
                ));
            }
            rels.push_str("</Relationships>");
            put(
                &mut w,
                "xl/workbook.xml",
                &format!(
                    "<?xml version=\"1.0\"?><workbook xmlns=\"{NS_MAIN}\" xmlns:r=\"{NS_REL}\">\
                     <sheets>{decls}</sheets></workbook>"
                ),
            );
            put(&mut w, "xl/_rels/workbook.xml.rels", &rels);

            if !shared_strings.is_empty() {
                let items = shared_strings
                    .iter()
                    .map(|s| format!("<si><t>{s}</t></si>"))
                    .collect::<String>();
                put(
                    &mut w,
                    "xl/sharedStrings.xml",
                    &format!(
                        "<?xml version=\"1.0\"?><sst xmlns=\"{NS_MAIN}\" count=\"{}\" uniqueCount=\"{}\">{items}</sst>",
                        shared_strings.len(),
                        shared_strings.len()
                    ),
                );
            }

            // styles:cellXfs[1] 挂 numFmtId=164 yyyy-mm-dd(s="1" 引用)。
            put(
                &mut w,
                "xl/styles.xml",
                &format!(
                    "<?xml version=\"1.0\"?><styleSheet xmlns=\"{NS_MAIN}\">\
                     <numFmts count=\"1\"><numFmt numFmtId=\"164\" formatCode=\"yyyy\\-mm\\-dd\"/></numFmts>\
                     <fonts count=\"1\"><font><sz val=\"11\"/><name val=\"Calibri\"/></font></fonts>\
                     <fills count=\"2\"><fill><patternFill patternType=\"none\"/></fill><fill><patternFill patternType=\"gray125\"/></fill></fills>\
                     <borders count=\"1\"><border/></borders>\
                     <cellStyleXfs count=\"1\"><xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"0\"/></cellStyleXfs>\
                     <cellXfs count=\"2\"><xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"0\" xfId=\"0\"/>\
                     <xf numFmtId=\"164\" fontId=\"0\" fillId=\"0\" borderId=\"0\" xfId=\"0\" applyNumberFormat=\"1\"/></cellXfs>\
                     </styleSheet>"
                ),
            );

            for (i, (_, body)) in sheets.iter().enumerate() {
                put(
                    &mut w,
                    &format!("xl/worksheets/sheet{}.xml", i + 1),
                    &format!(
                        "<?xml version=\"1.0\"?><worksheet xmlns=\"{NS_MAIN}\">\
                         <sheetData>{body}</sheetData></worksheet>"
                    ),
                );
            }
            w.finish().unwrap();
        }
        buf.into_inner()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_fixtures::{MINI_SCAN_PDF, MINI_TEXT_PDF, MIXED_GB_EUC_PDF, UNIGB_UCS2_PDF};

    /// 测试内构造 docx:zip writer 写 word/document.xml(段落结构由
    /// w:p/w:t/w:tab 组成,覆盖 CJK / entity / tab / 空段)。
    fn build_docx(xml: &str) -> Vec<u8> {
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut buf);
            let opts = zip::write::SimpleFileOptions::default();
            w.start_file("word/document.xml", opts).unwrap();
            std::io::Write::write_all(&mut w, xml.as_bytes()).unwrap();
            w.finish().unwrap();
        }
        buf.into_inner()
    }

    #[test]
    fn pdf_text_extraction_works() {
        let ex = try_extract(ExtractKind::Pdf, MINI_TEXT_PDF).unwrap();
        assert!(
            ex.text.contains("Hello PDF native text layer"),
            "{:?}",
            ex.text
        );
        assert_eq!(ex.units, 1, "page count via lopdf");
        assert!(!ex.truncated);
        assert_eq!(ex.orig_chars, ex.text.chars().count());
    }

    #[test]
    fn pdf_scanned_no_text_layer_degrades() {
        let err = try_extract(ExtractKind::Pdf, MINI_SCAN_PDF).unwrap_err();
        assert!(err.contains("no text layer"), "{err}");
    }

    /// 层 1 + 层 2 联合回归(F5 后续):非嵌入 Type0 中文字体
    /// (/UniGB-UCS2-H,无 ToUnicode)—— 上游 0.12 panic 的形态,vendored
    /// patch 后应完整恢复中文(码值即 UCS-2 码位直出,非静默空串)。
    #[test]
    fn pdf_unigb_ucs2_cjk_extraction() {
        let ex = try_extract(ExtractKind::Pdf, UNIGB_UCS2_PDF).unwrap();
        assert!(
            ex.text.contains("预算治理与中文提取测试"),
            "UCS2 identity 解码失败: {:?}",
            ex.text
        );
        assert!(
            ex.text.contains("验证层二解码路径"),
            "整句应完整恢复: {:?}",
            ex.text
        );
        assert_eq!(ex.units, 1, "page count via lopdf");
    }

    /// 层 1 降级回归:未知编码家族(/GB-EUC-H)不再 panic —— 直接打
    /// pdf-extract(不经 catch_unwind,panic 会让本测试直接挂)证明层 1
    /// 修复真实生效;该字体字形被跳过,同页 Helvetica ASCII 照常提取。
    #[test]
    fn pdf_unknown_cid_encoding_skips_font_without_panicking() {
        let _ = pdf_extract::extract_text_from_mem(MIXED_GB_EUC_PDF)
            .expect("well-formed pdf parses; unknown encoding only skips the font");
        let ex = try_extract(ExtractKind::Pdf, MIXED_GB_EUC_PDF).unwrap();
        assert!(
            ex.text.contains("Fallback ASCII text survives"),
            "其余字体照常提取: {:?}",
            ex.text
        );
    }

    /// 真机文件手动验证(#[ignore],不进 CI):`out/xxx.pdf` 是
    /// /usr/local/code/typhoon/xxx.pdf 的本地副本(含用户数据,已
    /// gitignore,绝不提交)。跑法:
    /// `cargo test --lib -- --ignored real_world_unigb_pdf --nocapture`
    #[test]
    #[ignore]
    fn real_world_unigb_pdf_manual_check() {
        let path = std::path::Path::new("../../out/xxx.pdf");
        let bytes = std::fs::read(path).unwrap_or_else(|e| {
            panic!("fixture {path:?} missing ({e}); copy the real pdf to out/ first")
        });
        let ex = try_extract(ExtractKind::Pdf, &bytes).unwrap();
        let cjk = ex
            .text
            .chars()
            .filter(|c| ('\u{4e00}'..='\u{9fff}').contains(c))
            .count();
        assert!(
            cjk > 100,
            "expected substantial CJK text, got {cjk} in {} chars",
            ex.text.chars().count()
        );
        println!(
            "real pdf: {} chars ({} CJK), {} pages, truncated={}",
            ex.orig_chars, cjk, ex.units, ex.truncated
        );
    }
    #[test]
    fn pdf_corrupt_and_wrong_magic_fail_soft() {
        assert!(try_extract(ExtractKind::Pdf, b"not a pdf at all").is_err());
        // 截断/垃圾 PDF:唯一契约是不 panic(catch_unwind 兜底),
        // 返回 Err 或退化 Ok 都接受 —— turn 不死是硬约束。
        let _ = try_extract(ExtractKind::Pdf, b"%PDF-1.4 \x00\xff garbage");
    }

    #[test]
    fn docx_cjk_paragraphs_and_entities() {
        let xml = concat!(
            "<w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">",
            "<w:body>",
            "<w:p><w:r><w:t>第一段:预算治理</w:t></w:r></w:p>",
            "<w:p><w:r><w:t>A &amp; B &lt;tag&gt;</w:t></w:r>",
            "<w:r><w:tab/></w:r><w:r><w:t>after\ttab</w:t></w:r></w:p>",
            "<w:p/>",
            "<w:p><w:r><w:t>尾段</w:t></w:r></w:p>",
            "</w:body></w:document>"
        );
        let bytes = build_docx(xml);
        let ex = try_extract(ExtractKind::Docx, &bytes).unwrap();
        assert!(ex.text.contains("第一段:预算治理"), "{:?}", ex.text);
        assert!(
            ex.text.contains("A & B <tag>"),
            "xml entities decoded: {:?}",
            ex.text
        );
        assert!(ex.text.contains('\t'), "w:tab → tab char");
        // 4 个 w:p(含空段)→ 4 行
        assert_eq!(ex.text.lines().count(), 4, "{:?}", ex.text);
        assert_eq!(ex.units, 4, "paragraph count");
    }

    #[test]
    fn docx_corrupt_zip_and_missing_entry_fail_soft() {
        assert!(try_extract(ExtractKind::Docx, b"PK\x03\x04 truncated").is_err());
        let empty = build_docx("<w:document/>");
        assert!(
            try_extract(ExtractKind::Docx, &empty).is_err(),
            "no document.xml"
        );
    }

    // --- F5 follow-up (2026-08-26): xlsx 原生提取 ---

    #[test]
    fn xlsx_cjk_shared_strings_and_csv_escape() {
        let sst = ["月份", "备注,\"引\"注"];
        let rows = concat!(
            "<row r=\"1\"><c r=\"A1\" t=\"s\"><v>0</v></c><c r=\"B1\" t=\"s\"><v>1</v></c></row>",
            "<row r=\"2\"><c r=\"A2\"><v>12000</v></c><c r=\"B2\"><v>-0.05</v></c><c r=\"C2\" t=\"b\"><v>1</v></c></row>"
        );
        let bytes = test_fixtures::build_xlsx(&[("Sheet1", rows)], &sst);
        let ex = try_extract(ExtractKind::Xlsx, &bytes).unwrap();
        assert!(
            ex.text.starts_with("## Sheet1 (2行×3列)\n"),
            "sheet header with dims: {:?}",
            ex.text
        );
        assert!(
            ex.text.contains("月份,\"备注,\"\"引\"\"注\"\n"),
            "RFC4180 逗号+引号转义: {:?}",
            ex.text
        );
        assert!(
            ex.text.contains("12000,-0.05,true"),
            "数字最短表示 + bool: {:?}",
            ex.text
        );
        assert_eq!(ex.units, 1, "units = sheet 数");
    }

    #[test]
    fn xlsx_multi_sheet_order_and_empty_sheet() {
        let rows = "<row r=\"1\"><c r=\"A1\"><v>7</v></c></row>";
        let bytes = test_fixtures::build_xlsx(&[("数据", rows), ("空表", "")], &[]);
        let ex = try_extract(ExtractKind::Xlsx, &bytes).unwrap();
        let data_pos = ex.text.find("## 数据 (1行×1列)").expect("sheet 1 header");
        let empty_pos = ex.text.find("## 空表 (空)").expect("empty sheet marker");
        assert!(data_pos < empty_pos, "sheet 顺序保持: {:?}", ex.text);
        assert_eq!(ex.units, 2);
    }

    /// 序列日期 → ISO(prd D5):numFmt yyyy-mm-dd 样式(s="1")下的
    /// 整数序列 = 纯日期;带小数 = 补时间部分。44927 = 2023-01-01。
    #[test]
    fn xlsx_date_serial_to_iso() {
        let rows = concat!(
            "<row r=\"1\">",
            "<c r=\"A1\" s=\"1\"><v>44927</v></c>",
            "<c r=\"B1\" s=\"1\"><v>44927.5</v></c>",
            "</row>"
        );
        let bytes = test_fixtures::build_xlsx(&[("日期", rows)], &[]);
        let ex = try_extract(ExtractKind::Xlsx, &bytes).unwrap();
        assert!(
            ex.text.contains("2023-01-01,2023-01-01 12:00:00"),
            "date/datetime ISO rendering: {:?}",
            ex.text
        );
    }

    #[test]
    fn xlsx_inline_str_and_error_cells() {
        let rows = concat!(
            "<row r=\"1\">",
            "<c r=\"A1\" t=\"inlineStr\"><is><t>行内串</t></is></c>",
            "<c r=\"B1\" t=\"e\"><v>#REF!</v></c>",
            "</row>"
        );
        let bytes = test_fixtures::build_xlsx(&[("S", rows)], &[]);
        let ex = try_extract(ExtractKind::Xlsx, &bytes).unwrap();
        assert!(
            ex.text.contains("行内串,#REF!"),
            "inlineStr + error cell passthrough: {:?}",
            ex.text
        );
    }

    #[test]
    fn xlsx_corrupt_zip_wrong_magic_and_all_empty_fail_soft() {
        assert!(try_extract(ExtractKind::Xlsx, b"not a zip at all").is_err());
        assert!(try_extract(ExtractKind::Xlsx, b"PK\x03\x04 truncated").is_err());
        let bytes = test_fixtures::build_xlsx(&[("空表", "")], &[]);
        let err = try_extract(ExtractKind::Xlsx, &bytes).unwrap_err();
        assert!(err.contains("no cell data"), "{err}");
    }

    #[test]
    fn csv_escape_quotes_only_when_needed() {
        assert_eq!(super::csv_escape("plain"), "plain");
        assert_eq!(super::csv_escape("a,b"), "\"a,b\"");
        assert_eq!(super::csv_escape("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(super::csv_escape("多\n行"), "\"多\n行\"");
    }

    #[test]
    fn oversized_source_rejected_before_parse() {
        let big = vec![b'%'; MAX_EXTRACT_SOURCE_BYTES + 1];
        assert!(try_extract(ExtractKind::Pdf, &big).is_err());
        assert!(try_extract(ExtractKind::Docx, &big).is_err());
        assert!(try_extract(ExtractKind::Xlsx, &big).is_err());
    }

    #[test]
    fn cap_truncates_head_preserved() {
        let text = "汉".repeat(MAX_EXTRACT_CHARS + 500);
        let ex = cap(text, 7);
        assert!(ex.truncated);
        assert_eq!(ex.text.chars().count(), MAX_EXTRACT_CHARS);
        assert_eq!(ex.orig_chars, MAX_EXTRACT_CHARS + 500);
        assert_eq!(ex.units, 7);
    }
}
