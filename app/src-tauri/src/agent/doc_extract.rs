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

use serde::{Deserialize, Serialize};
use std::io::Read;

/// 提取来源类型。wire snake_case(`"pdf"` / `"docx"`),进
/// `InjectionAction::Extracted` 的前端判别。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtractKind {
    Pdf,
    Docx,
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
    }
}

fn extract_pdf(bytes: &[u8]) -> Result<ExtractedText, String> {
    if !bytes.starts_with(b"%PDF") {
        return Err("missing %PDF magic".into());
    }
    // pdf-extract 内部对畸形输入有 unwrap 路径;turn 不死是硬约束,
    // catch_unwind 兜底(提取是纯计算,无 IO 状态可污染)。
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_fixtures::{MINI_SCAN_PDF, MINI_TEXT_PDF};

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

    #[test]
    fn oversized_source_rejected_before_parse() {
        let big = vec![b'%'; MAX_EXTRACT_SOURCE_BYTES + 1];
        assert!(try_extract(ExtractKind::Pdf, &big).is_err());
        assert!(try_extract(ExtractKind::Docx, &big).is_err());
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
