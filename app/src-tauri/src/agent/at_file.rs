//! B2 PR2: `@relpath` file-content injection into user messages.
//!
//! Before the agent loop sends messages to the provider, every
//! `@<relative-path>` token in a user message's text is expanded:
//!
//! - **text file** → content reusing `read_file::truncate_full_output`
//!   so the injection is format-identical to the `read_file` tool output
//!   (`cat -n` line numbers + 50 KB head/tail truncation), wrapped in
//!   `<file path="...">…</file>`. The model sees no difference between
//!   "user-fed context" and "tool result" (opencode design cue).
//! - **image / PDF / Office / binary** → placeholder degradation. The
//!   LLM channel is text-only (`ContentBlock` has no Image/Document
//!   variant); multimodal injection is B1 (third-tier).
//! - **invalid path** (missing / unreadable / outside project root) →
//!   the original `@token` text is left untouched. This avoids mangling
//!   emails (`a@b.com`) and is friendlier to typos than a placeholder.
//!
//! Design references — 6-agent survey
//! `docs/research/at-file-injection-coding-agents-survey.md`:
//! placeholder degradation (Cline), binary detection via extension
//! blacklist + content sniff (opencode + Cline `isbinaryfile`), and the
//! consensus that *no* agent downgrades to "path text for the LLM to
//! read itself" — content injection is the only acceptable semantics.
//!
//! Injection entry point: [`inject_at_tokens`], called from
//! `chat_loop::run_chat_loop` after the user message is persisted (so
//! the DB keeps the original `@relpath` as source of truth) and before
//! the turn loop (so C3 compaction + `provider.send` see the expanded
//! content).

use crate::llm::types::{ChatMessage, MessageContent, Role};
use crate::projects::boundary::assert_within_root;
use crate::tools::read_file::truncate_full_output;
use crate::tools::ToolContext;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Bytes sampled for content-based binary sniffing (opencode uses 4096).
const SNIFF_BYTES: usize = 4096;

/// Control-character ratio threshold (strictly greater than). opencode
/// and `isbinaryfile` both use ~30%; common whitespace (`\t \n \r`) is
/// not counted. This is the third-tier check — NUL byte and invalid
/// UTF-8 (checked first) catch nearly all real binaries.
const NON_PRINTABLE_RATIO: f64 = 0.30;

/// Image extensions → placeholder (text-only channel; B1 will add
/// multimodal). Matched case-insensitively on the lowercased extension.
const IMAGE_EXTS: &[&str] = &[
    ".png", ".jpg", ".jpeg", ".gif", ".webp", ".bmp", ".tiff", ".tif", ".svg", ".ico", ".heic",
    ".avif",
];

/// PDF extension → placeholder (text-only channel).
const PDF_EXTS: &[&str] = &[".pdf"];

/// Office extensions → placeholder (would need mammoth/exceljs-equivalent
/// parsers; PR2 deliberately avoids the dependency — the placeholder
/// points the user at `pandoc` via the shell tool).
const OFFICE_EXTS: &[&str] = &[
    ".doc", ".docx", ".xls", ".xlsx", ".ppt", ".pptx", ".odt", ".ods", ".odp", ".rtf",
];

/// Binary extension blacklist → placeholder, never decoded. Union of
/// opencode's list and common archive/executable/object formats. A hit
/// short-circuits before any content sniff.
const BINARY_EXTS: &[&str] = &[
    ".zip", ".tar", ".gz", ".tgz", ".bz2", ".7z", ".rar",
    ".exe", ".dll", ".so", ".dylib", ".o", ".a", ".lib",
    ".class", ".jar", ".war",
    ".wasm", ".pyc", ".pyo",
    ".bin", ".dat", ".obj",
    ".mp3", ".mp4", ".mov", ".avi", ".mkv", ".flac", ".ogg", ".wav",
    ".ttf", ".otf", ".woff", ".woff2", ".eot",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileKind {
    Text,
    Image,
    Pdf,
    Office,
    Binary,
}

/// Compiled once. `@` followed by a run of non-space, non-`@` chars.
/// Excluding `@` from the run lets `a@b.com` match only `@b.com` (which
/// then resolves to a non-existent path and is left untouched), and
/// `@a@b` parse as two separate tokens.
fn at_token_regex() -> &'static regex::Regex {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"@([^\s@]+)").unwrap())
}

/// Lowercased extension of `path` including the leading dot (`.txt`),
/// or `""` when there is no extension. Drives the extension-blacklist
/// classification.
fn lower_ext(path: &Path) -> String {
    path.extension()
        .map(|e| format!(".{}", e.to_string_lossy().to_lowercase()))
        .unwrap_or_default()
}

/// Classify a file by extension first, then by content sniff. The
/// extension checks (image / pdf / office / binary blacklist) run before
/// reading bytes when the caller already has them; `bytes` lets the
/// NUL/UTF-8/ratio sniff catch extension-less binaries.
fn classify(path: &Path, bytes: &[u8]) -> FileKind {
    let ext = lower_ext(path);
    if IMAGE_EXTS.iter().any(|e| *e == ext) {
        return FileKind::Image;
    }
    if PDF_EXTS.iter().any(|e| *e == ext) {
        return FileKind::Pdf;
    }
    if OFFICE_EXTS.iter().any(|e| *e == ext) {
        return FileKind::Office;
    }
    if BINARY_EXTS.iter().any(|e| *e == ext) {
        return FileKind::Binary;
    }
    if is_binary_content(bytes) {
        FileKind::Binary
    } else {
        FileKind::Text
    }
}

/// Content-based binary sniff — three tiers, first hit wins:
///
/// 1. **NUL byte** in the sample → binary. Catches exe/zip/so/object
///    files and most real binaries (these formats almost always contain
///    a zero byte early).
/// 2. **Invalid UTF-8** → binary. Catches arbitrary-encoded / latin1
///    payloads. A valid-UTF-8 CJK file passes (multi-byte sequences are
///    legal UTF-8).
/// 3. **Control-character ratio > 30%** → binary. Catches the rare
///    valid-UTF-8-but-control-heavy payload. `\t \n \r` excluded.
///
/// Empty sample → text (vacuously).
fn is_binary_content(bytes: &[u8]) -> bool {
    let sample = &bytes[..bytes.len().min(SNIFF_BYTES)];
    if sample.is_empty() {
        return false;
    }
    if sample.iter().any(|&b| b == 0) {
        return true;
    }
    if std::str::from_utf8(sample).is_err() {
        return true;
    }
    let non_printable = sample.iter().filter(|&&b| is_control_char(b)).count();
    (non_printable as f64 / sample.len() as f64) > NON_PRINTABLE_RATIO
}

/// A control byte: C0 controls except the common whitespace runs
/// (`\t` `\n` `\r`), plus DEL (0x7F). High bytes (≥0x80) are NOT control
/// chars — by the time this runs, UTF-8 validity is already confirmed,
/// so they are legitimate multi-byte sequence bytes.
fn is_control_char(b: u8) -> bool {
    matches!(b, 0x01..=0x08 | 0x0E..=0x1F | 0x7F)
}

/// Expand every `@relpath` token in all **text** user messages. Blocks
/// messages (e.g. the B5 memory synthetic insert) are left alone — they
/// carry instruction-file bodies, never user `@` tokens. Assistant and
/// tool messages are skipped.
///
/// Only `MessageContent::Text` variants are rewritten in place; a
/// message with no `@` token at all is not reallocated (the expanded
/// string equals the original).
pub async fn inject_at_tokens(messages: &mut [ChatMessage], ctx: &ToolContext) {
    for msg in messages.iter_mut() {
        if msg.role != Role::User {
            continue;
        }
        if let MessageContent::Text(text) = &msg.content {
            let expanded = expand_at_tokens(text, ctx).await;
            if expanded != *text {
                msg.content = MessageContent::Text(expanded);
            }
        }
    }
}

/// Rewrite a single text string: each `@relpath` match is replaced by
/// its expansion (file content / placeholder), or left as the original
/// `@token` when the path is invalid. Non-matching text is preserved
/// verbatim.
async fn expand_at_tokens(text: &str, ctx: &ToolContext) -> String {
    let re = at_token_regex();
    let mut out = String::with_capacity(text.len());
    let mut last_end = 0;
    for m in re.find_iter(text) {
        out.push_str(&text[last_end..m.start()]);
        // m.as_str() includes the leading `@`; skip it for the path.
        let path_str = &m.as_str()[1..];
        match expand_single(path_str, ctx).await {
            Some(expanded) => out.push_str(&expanded),
            None => out.push_str(m.as_str()), // invalid path → keep original `@token`
        }
        last_end = m.end();
    }
    out.push_str(&text[last_end..]);
    out
}

/// Resolve and read one `@relpath`. Returns `Some(expansion)` for any
/// readable in-root file (text → content, non-text → placeholder), or
/// `None` when the path is out-of-root / missing / unreadable so the
/// caller leaves the original `@token` untouched.
async fn expand_single(rel_path: &str, ctx: &ToolContext) -> Option<String> {
    let raw = Path::new(rel_path);
    let resolved: PathBuf = if raw.is_absolute() {
        raw.to_path_buf()
    } else {
        ctx.cwd.join(raw)
    };
    let validated = assert_within_root(&ctx.worktree_path, &resolved).ok()?;
    let bytes = tokio::fs::read(&validated).await.ok()?;
    let kind = classify(&validated, &bytes);
    Some(expand_for_kind(rel_path, kind, &bytes))
}

/// Render the expansion for a classified, successfully-read file.
fn expand_for_kind(rel_path: &str, kind: FileKind, bytes: &[u8]) -> String {
    match kind {
        FileKind::Text => {
            let content = String::from_utf8_lossy(bytes);
            // Reuse the read_file tool's exact truncation + `cat -n`
            // numbering so injected content matches tool output.
            let body = truncate_full_output(&content);
            format!("<file path=\"{}\">\n{}\n</file>", rel_path, body)
        }
        FileKind::Image => format!(
            "[image: {} — 当前为纯文本通道，不支持图片注入（B1 计划）]",
            rel_path
        ),
        FileKind::Pdf => format!(
            "[binary: {} — 二进制文档未注入；可 shell 运行 pdftotext 转文本后引用]",
            rel_path
        ),
        FileKind::Office => format!(
            "[binary: {} — 二进制文档未注入；可 shell 运行 pandoc {} -t plain 转文本后引用]",
            rel_path, rel_path
        ),
        FileKind::Binary => format!(
            "[binary: {} — 二进制文件，无法注入文本内容]",
            rel_path
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn ctx_at(tmp: &tempfile::TempDir) -> ToolContext {
        ToolContext {
            worktree_path: tmp.path().canonicalize().unwrap(),
            cwd: tmp.path().canonicalize().unwrap(),
        }
    }

    // --- lower_ext ---

    #[test]
    fn lower_ext_handles_uppercase_and_dot() {
        assert_eq!(lower_ext(Path::new("foo.TXT")), ".txt");
        assert_eq!(lower_ext(Path::new("a/b/c.rs")), ".rs");
        assert_eq!(lower_ext(Path::new("noext")), "");
        assert_eq!(lower_ext(Path::new(".bashrc")), ""); // no basename ext
    }

    // --- is_binary_content ---

    #[test]
    fn binary_nul_byte_detected() {
        assert!(is_binary_content(&[0x41, 0x00, 0x42]));
    }

    #[test]
    fn plain_text_is_not_binary() {
        assert!(!is_binary_content(b"hello world\nfunction foo() {}\n"));
    }

    #[test]
    fn empty_is_not_binary() {
        assert!(!is_binary_content(&[]));
    }

    #[test]
    fn cjk_utf8_is_not_binary() {
        // 3-byte CJK glyphs — valid UTF-8, must NOT trip the ratio check.
        let cjk = "中文测试".repeat(100);
        assert!(!is_binary_content(cjk.as_bytes()));
    }

    #[test]
    fn invalid_utf8_is_binary() {
        // Lone 0xFF / continuation byte without a leading byte → invalid UTF-8.
        assert!(is_binary_content(&[0xFF, 0xFE, 0x41, 0x42]));
    }

    #[test]
    fn control_char_ratio_over_threshold_is_binary() {
        // > 30% control chars (0x01), valid ASCII (UTF-8 ok, no NUL).
        let mut v = vec![0x41u8; 100];
        for i in 0..40 {
            v[i] = 0x01; // 40% control chars
        }
        assert!(is_binary_content(&v));
    }

    #[test]
    fn control_char_ratio_under_threshold_is_text() {
        // 10% control chars → text.
        let mut v = vec![0x41u8; 100];
        for i in 0..10 {
            v[i] = 0x01;
        }
        assert!(!is_binary_content(&v));
    }

    // --- classify ---

    #[test]
    fn classify_by_extension() {
        assert_eq!(classify(Path::new("a.png"), &[]), FileKind::Image);
        assert_eq!(classify(Path::new("A.JPG"), &[]), FileKind::Image);
        assert_eq!(classify(Path::new("a.pdf"), &[]), FileKind::Pdf);
        assert_eq!(classify(Path::new("a.docx"), &[]), FileKind::Office);
        assert_eq!(classify(Path::new("a.xlsx"), &[]), FileKind::Office);
        assert_eq!(classify(Path::new("a.zip"), &[]), FileKind::Binary);
        assert_eq!(classify(Path::new("a.exe"), &[]), FileKind::Binary);
    }

    #[test]
    fn classify_unknown_ext_falls_back_to_content() {
        assert_eq!(classify(Path::new("noext"), &[0, 1, 2]), FileKind::Binary);
        assert_eq!(classify(Path::new("weird.dat2"), b"plain text"), FileKind::Text);
    }

    // --- expand_at_tokens: injection ---

    #[tokio::test]
    async fn text_file_content_is_injected_with_line_numbers() {
        let tmp = tempdir().unwrap();
        std::fs::write(tmp.path().join("foo.txt"), "hello\n").unwrap();
        let out = expand_at_tokens("see @foo.txt here", &ctx_at(&tmp)).await;
        assert!(out.starts_with("see "), "got: {:?}", out);
        assert!(out.ends_with(" here"), "got: {:?}", out);
        assert!(out.contains("<file path=\"foo.txt\">"), "got: {:?}", out);
        assert!(out.contains("</file>"), "got: {:?}", out);
        // read_file cat -n format: \t1\thello
        assert!(out.contains("\t1\thello"), "got: {:?}", out);
    }

    #[tokio::test]
    async fn multiple_tokens_all_expanded() {
        let tmp = tempdir().unwrap();
        std::fs::write(tmp.path().join("a.txt"), "AAA").unwrap();
        std::fs::write(tmp.path().join("b.txt"), "BBB").unwrap();
        let out = expand_at_tokens("@a.txt and @b.txt", &ctx_at(&tmp)).await;
        assert!(out.contains("\t1\tAAA"), "got: {:?}", out);
        assert!(out.contains("\t1\tBBB"), "got: {:?}", out);
        assert!(out.contains(" and "), "got: {:?}", out);
    }

    #[tokio::test]
    async fn large_text_file_is_truncated() {
        let tmp = tempdir().unwrap();
        // > 50 KB so truncate_full_output applies head+tail.
        let line = "x".repeat(80) + "\n";
        std::fs::write(tmp.path().join("big.txt"), line.repeat(700)).unwrap();
        let out = expand_at_tokens("@big.txt", &ctx_at(&tmp)).await;
        assert!(out.contains("<truncated:"), "expected truncation marker, got: {:?}", out);
        assert!(out.contains("<file path=\"big.txt\">"));
    }

    // --- expand_at_tokens: placeholder degradation ---

    #[tokio::test]
    async fn image_file_degrades_to_placeholder() {
        let tmp = tempdir().unwrap();
        std::fs::write(tmp.path().join("pic.png"), b"\x89PNG\r\n\x1a\n fake").unwrap();
        let out = expand_at_tokens("@pic.png", &ctx_at(&tmp)).await;
        assert!(out.contains("[image: pic.png"), "got: {:?}", out);
        assert!(!out.contains("<file"), "image must not inject as text, got: {:?}", out);
    }

    #[tokio::test]
    async fn pdf_file_degrades_to_placeholder() {
        let tmp = tempdir().unwrap();
        std::fs::write(tmp.path().join("doc.pdf"), b"%PDF-1.4 fake").unwrap();
        let out = expand_at_tokens("@doc.pdf", &ctx_at(&tmp)).await;
        assert!(out.contains("[binary: doc.pdf"), "got: {:?}", out);
        assert!(out.contains("pdftotext"), "got: {:?}", out);
    }

    #[tokio::test]
    async fn office_file_degrades_to_placeholder() {
        let tmp = tempdir().unwrap();
        std::fs::write(tmp.path().join("spec.docx"), b"PK\x03\x04 fake zip").unwrap();
        let out = expand_at_tokens("@spec.docx", &ctx_at(&tmp)).await;
        assert!(out.contains("[binary: spec.docx"), "got: {:?}", out);
        assert!(out.contains("pandoc"), "got: {:?}", out);
    }

    #[tokio::test]
    async fn binary_file_degrades_to_placeholder() {
        let tmp = tempdir().unwrap();
        std::fs::write(tmp.path().join("app.zip"), b"PK\x03\x04 fake").unwrap();
        let out = expand_at_tokens("@app.zip", &ctx_at(&tmp)).await;
        assert!(out.contains("[binary: app.zip"), "got: {:?}", out);
    }

    // --- expand_at_tokens: invalid path keeps original token ---

    #[tokio::test]
    async fn nonexistent_path_kept_verbatim() {
        let tmp = tempdir().unwrap();
        let out = expand_at_tokens("ref @nope.txt end", &ctx_at(&tmp)).await;
        assert_eq!(out, "ref @nope.txt end");
    }

    #[tokio::test]
    async fn traversal_outside_root_kept_verbatim() {
        let tmp = tempdir().unwrap();
        let out = expand_at_tokens("@../../etc/passwd", &ctx_at(&tmp)).await;
        assert_eq!(out, "@../../etc/passwd");
    }

    #[tokio::test]
    async fn email_is_not_mangled() {
        // `@b.com` matches the regex but `b.com` is not an in-root file →
        // the original `@b.com` is preserved, so `a@b.com` survives intact.
        let tmp = tempdir().unwrap();
        let out = expand_at_tokens("contact a@b.com please", &ctx_at(&tmp)).await;
        assert_eq!(out, "contact a@b.com please");
    }

    #[tokio::test]
    async fn no_token_leaves_text_unchanged() {
        let tmp = tempdir().unwrap();
        let out = expand_at_tokens("just plain text, no refs", &ctx_at(&tmp)).await;
        assert_eq!(out, "just plain text, no refs");
    }

    // --- inject_at_tokens: message-level wiring ---

    #[tokio::test]
    async fn inject_expands_user_text_messages_only() {
        let tmp = tempdir().unwrap();
        std::fs::write(tmp.path().join("foo.txt"), "content").unwrap();
        let ctx = ctx_at(&tmp);
        let mut messages = vec![
            ChatMessage {
                role: Role::User,
                content: MessageContent::Text("look @foo.txt".to_string()),
            },
            ChatMessage {
                role: Role::Assistant,
                content: MessageContent::Text("@foo.txt must NOT be touched".to_string()),
            },
        ];
        inject_at_tokens(&mut messages, &ctx).await;
        // user expanded
        assert!(matches!(&messages[0].content, MessageContent::Text(t) if t.contains("<file path=\"foo.txt\">")));
        // assistant left verbatim
        assert!(matches!(&messages[1].content, MessageContent::Text(t) if t == "@foo.txt must NOT be touched"));
    }

    #[tokio::test]
    async fn inject_skips_blocks_messages() {
        // B5 memory synthetic insert is Blocks — must not be scanned.
        let tmp = tempdir().unwrap();
        let ctx = ctx_at(&tmp);
        let blocks = vec![crate::llm::types::ContentBlock::Text {
            text: "instructions with @not-a-file inside".to_string(),
            cache_control: None,
        }];
        let mut messages = vec![ChatMessage {
            role: Role::User,
            content: MessageContent::Blocks(blocks),
        }];
        inject_at_tokens(&mut messages, &ctx).await;
        // unchanged: Blocks variant preserved exactly
        assert!(matches!(&messages[0].content, MessageContent::Blocks(_)));
    }
}
