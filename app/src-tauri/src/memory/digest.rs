//! memory 指令块 digest(分级注入,08-15-memory-block-governance WP2)。
//!
//! CLAUDE.md(`<reference>` 层)首轮只注入**章节目录**(fence-aware 切节,
//! 每节 = 标题 + 首句摘要),另由 drive.rs 侧挂 `load_memory_sections`
//! 元工具,模型按需拉取全文节后粘性在场。AGENTS.md(`<primary>` 层)与
//! tokens ≤ [`DIGEST_THRESHOLD_TOKENS`] 的小层保持全量。
//!
//! 与 C7D(tools/stub.rs)同构:模型调用工具前拉 schema → 执行任务前拉
//! 规范。三条对齐 C7D 的结构事实:
//! 1. digest 纯机械生成(fence 状态机 + 首句截断),同输入同输出 —
//!    session 内注入内容固定(mtime fence),不破前缀缓存。
//! 2. 已加载节粘性:registry 记录 session → loaded 节集,下个 request
//!    组装时已加载节全文**追加在目录之后**(保住目录段前缀)。
//! 3. `load_memory_sections` 执行拦截在 chat_loop/tools.rs serial 顶部
//!    (同 `load_tool_schemas`),不走 `execute_tool_inner` 权限链 —
//!    read-only 自有数据,silent-allow。
//!
//! registry 是进程级 `OnceLock` 单例(对标 `memory/tokens.rs:35` ENCODER
//! 先例),**不走 AppState/run_chat_loop 穿参** — 那条路要动 72 个
//! `run_chat_loop` 调用点,收益为零(design §3.4 的 AppState 方案在实现
//! 期据此修正)。`delete_session_inner` 挂清理(同 stub_loaded 接线点)。

use std::collections::HashSet;
use std::sync::OnceLock;

use tokio::sync::RwLock;

use crate::llm::types::ToolDef;
use crate::memory::types::{LayerStatus, MemoryLayer, MemorySource};

/// digest 阈值:层 tokens ≤ 600 直接全量(小文件无收益,保 always-on
/// 语义;实机 user CLAUDE.md 36B 自然落入)。design §3.1。
pub const DIGEST_THRESHOLD_TOKENS: u32 = 600;

/// 摘要截断长度(字符,非字节 — CJK 安全)。design §3.2。
const SUMMARY_MAX_CHARS: usize = 120;

/// Preamble(首个 header 之前的内容)的节标题。
pub const PREAMBLE_TITLE: &str = "(preamble)";

// ---------------------------------------------------------------------------
// 切节(fence-aware 状态机)
// ---------------------------------------------------------------------------

/// 一节(或 Preamble)。`body` 含 header 行在内的完整节文本 — 拉取全文
/// 时原样返回,模型看到的与全量注入时逐字节一致。
#[derive(Debug, Clone, PartialEq)]
pub struct DocSection {
    /// 去掉 `#` 前缀的标题;Preamble 为 `(preamble)`。
    pub title: String,
    /// 0 = Preamble,1..=3 = ATX 级别。
    pub level: u8,
    /// 完整节文本(含 header 行)。
    pub body: String,
}

/// 解析一行 ATX header:`^#{1,3} <title>`(严格无缩进,需空格分隔)。
/// `#header`(无空格)与 `#### `+(更深级)不算节边界 — 前者是
/// CommonMark 非法形态,后者视为节内内容。
fn parse_header(line: &str) -> Option<(u8, String)> {
    let rest = line.strip_prefix('#')?;
    let hashes = rest.chars().take_while(|c| *c == '#').count();
    let level = 1 + hashes;
    if level > 3 {
        return None;
    }
    let title_part = &line[level..];
    let title = title_part.strip_prefix(' ')?;
    let title = title.trim_end();
    if title.is_empty() {
        return None;
    }
    Some((level as u8, title.to_string()))
}

/// fence 状态机:``` 行翻转 in_fence(fence 行本身属节 body)。
/// 未闭合 fence 到 EOF 保持 in_fence — 其后的 header 不切节(容错:
/// 宁可少切不可切碎 code block)。
fn is_fence_line(line: &str) -> bool {
    line.trim_start().starts_with("```")
}

/// 把 content 切成节。首个 header 之前的非空内容归 Preamble;无任何
/// header 的文件整体是一节 Preamble(目录仍可寻址它)。
pub fn split_sections(content: &str) -> Vec<DocSection> {
    let mut sections: Vec<DocSection> = Vec::new();
    let mut current: Option<DocSection> = None;
    let mut in_fence = false;

    for line in content.lines() {
        if is_fence_line(line) {
            in_fence = !in_fence;
            append_line(&mut current, line, PREAMBLE_TITLE, 0);
            continue;
        }
        if !in_fence {
            if let Some((level, title)) = parse_header(line) {
                if let Some(s) = current.take() {
                    sections.push(s);
                }
                current = Some(DocSection {
                    title,
                    level,
                    body: line.to_string(),
                });
                continue;
            }
        }
        append_line(&mut current, line, PREAMBLE_TITLE, 0);
    }
    if let Some(s) = current {
        sections.push(s);
    }
    // 全空文件 → 无节(调用方按空 content 处理;不会到达 — is_digest_layer
    // 只对 Loaded 层生效,空文件 tokens=0 低于阈值走全量)。
    sections
}

fn append_line(current: &mut Option<DocSection>, line: &str, preamble_title: &str, level: u8) {
    let section = current.get_or_insert_with(|| DocSection {
        title: preamble_title.to_string(),
        level,
        body: String::new(),
    });
    if !section.body.is_empty() {
        section.body.push('\n');
    }
    section.body.push_str(line);
}

/// 节摘要:节内首个非空、非 header、非 fence 行,截断 ≤120 chars;
/// 回退(整节就是一个 code block,如实测 CLAUDE.md 的 Common
/// Commands):节内首个非空行(允许 fence 内)。design §3.2。
fn section_summary(section: &DocSection) -> String {
    let mut in_fence = false;
    let mut fallback: Option<String> = None;
    for line in section.body.lines() {
        if is_fence_line(line) {
            in_fence = !in_fence;
            continue;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if !in_fence {
            // fence 外:header 行(嵌套子节标题)跳过,首个正文行即摘要。
            if parse_header(line).is_some() {
                continue;
            }
            return truncate_chars(trimmed);
        }
        // fence 内:shell 注释 `# 开发` 这类行**不是** markdown header —
        // 不做 parse_header 过滤,作回退候选。
        if fallback.is_none() {
            fallback = Some(truncate_chars(trimmed));
        }
    }
    fallback.unwrap_or_else(|| "(no text)".to_string())
}

fn truncate_chars(s: &str) -> String {
    if s.chars().count() <= SUMMARY_MAX_CHARS {
        return s.to_string();
    }
    let cut: String = s.chars().take(SUMMARY_MAX_CHARS).collect();
    format!("{cut}…")
}

// ---------------------------------------------------------------------------
// 层级判定 + 目录生成
// ---------------------------------------------------------------------------

/// 寻址用的层 key(无方括号):`Project CLAUDE.md` / `User CLAUDE.md`。
/// banner label(`[Project CLAUDE.md]`)的去括号形态 — 工具参数里干净。
pub fn layer_key(layer: &MemoryLayer) -> String {
    format!("{} {}", layer.kind.label_prefix(), layer.source.label())
}

/// 节 key:`Project CLAUDE.md#Architecture`。registry 与 load 请求共用。
pub fn section_key(layer_key: &str, title: &str) -> String {
    format!("{layer_key}#{title}")
}

/// 该层是否走 digest:仅 CLAUDE.md(`reference` 语义)、Loaded、且
/// tokens 超阈值。AGENTS.md(primary)永不 digest — 按大小一刀切会误伤
/// always-on 主指令(design §2 方案 E 的拒绝理由)。
pub fn is_digest_layer(layer: &MemoryLayer) -> bool {
    matches!(layer.source, MemorySource::Claude)
        && matches!(layer.status, LayerStatus::Loaded)
        && layer.tokens > DIGEST_THRESHOLD_TOKENS
}

/// digest 目录体(替换 `layer.content` 的位置;label 行由
/// `render_prompt_section` 保留)。结构:头部一行元信息 + 逐节目录 +
/// 尾部调用指引;`loaded` 内的节全文**追加在目录之后**(粘性在场,
/// 且保住目录段前缀 — 缓存命中最大化)。
pub fn digest_body(layer: &MemoryLayer, loaded: &HashSet<String>) -> String {
    let key = layer_key(layer);
    let sections = split_sections(&layer.content);
    let mut out = String::new();
    out.push_str(&format!(
        "[digest — {} sections, ~{} tokens in full. \
         Call load_memory_sections([\"{key}#<section title>\"]) to load a section's \
         full text, or [\"{key}\"] for the whole file.]\n",
        sections.len(),
        layer.tokens,
    ));
    for (i, s) in sections.iter().enumerate() {
        out.push_str(&format!(
            "{}. {} — {}\n",
            i + 1,
            s.title,
            section_summary(s)
        ));
    }
    // 粘性:已加载层(整体)→ 直接全文,等价 opt-back Full。
    if loaded.contains(&key) {
        out.push('\n');
        out.push_str(&layer.content);
        return out;
    }
    // 粘性:已加载节 → 全文追加在目录之后。
    let mut any = false;
    for s in &sections {
        if loaded.contains(&section_key(&key, &s.title)) {
            if !any {
                out.push_str("\n[loaded sections — full text]\n");
                any = true;
            }
            out.push_str(&s.body);
            out.push('\n');
        }
    }
    out
}

// ---------------------------------------------------------------------------
// 节寻址(load 请求 → 层/节)
// ---------------------------------------------------------------------------

/// 节标题匹配:精确(大小写不敏感)→ 唯一前缀 → 唯一子串;歧义/未命中
/// 返回 None(调用方报错附可用清单自愈)。标题是自然语言,模型精确
/// 复现易错 — 回退链把失败率压下来(design §3.3)。
pub fn match_section<'a>(sections: &'a [DocSection], title: &str) -> Option<&'a DocSection> {
    let want = title.trim();
    let lower = want.to_lowercase();
    let by_exact = |s: &DocSection| -> bool { s.title.trim().to_lowercase() == lower };
    if let Some(s) = sections.iter().find(|s| by_exact(s)) {
        return Some(s);
    }
    let prefix_hits: Vec<&DocSection> = sections
        .iter()
        .filter(|s| s.title.trim().to_lowercase().starts_with(&lower))
        .collect();
    if prefix_hits.len() == 1 {
        return Some(prefix_hits[0]);
    }
    let substr_hits: Vec<&DocSection> = sections
        .iter()
        .filter(|s| s.title.trim().to_lowercase().contains(&lower))
        .collect();
    if substr_hits.len() == 1 {
        return Some(substr_hits[0]);
    }
    None
}

// ---------------------------------------------------------------------------
// load_memory_sections 元工具
// ---------------------------------------------------------------------------

/// `load_memory_sections` 元工具 def(`load_tool_schemas` 的 memory 版)。
/// **不进** `builtin_tools()`;由 drive.rs 按 digest gate 侧挂 append,
/// 群聊/worker 路径不出现(design §3.5)。
pub fn load_memory_sections_def() -> ToolDef {
    ToolDef {
        name: "load_memory_sections".to_string(),
        description: Some(
            "Load the full text of memory instruction sections (CLAUDE.md layers are \
             injected as a section digest only). Pass \"<Layer>\" (e.g. \"Project \
             CLAUDE.md\") for the whole file, \"<Layer>#<section title>\" for one \
             section (exact title, or an unambiguous prefix/substring), or \
             [\"all\"] for every digest layer; the full section text is returned \
             verbatim."
                .to_string(),
        ),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "sections": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Layer/section specs to load, or [\"all\"] for every digest layer."
                }
            },
            "required": ["sections"]
        }),
    }
}

// ---------------------------------------------------------------------------
// Registry(session → loaded 节集,粘性)
// ---------------------------------------------------------------------------

/// Session → loaded 节/层 key 集合。进程级 `OnceLock` 单例(理由见模块
/// 注释);`delete_session_inner` 清理;daemon 重启自然清空(同 StubRegistry
/// 的接受语义:重载一轮往返成本)。
#[derive(Default)]
pub struct MemoryDigestRegistry {
    inner: RwLock<HashSet<String>>,
    // session 维度打包进 key("session\u{0}section")— 简化锁结构,量级
    // 单 session 数十个 key,无遍历需求。
    sessions: RwLock<HashSet<String>>,
}

static REGISTRY: OnceLock<MemoryDigestRegistry> = OnceLock::new();

/// 进程级单例访问点。
pub fn registry() -> &'static MemoryDigestRegistry {
    REGISTRY.get_or_init(MemoryDigestRegistry::default)
}

fn scoped(session_id: &str, key: &str) -> String {
    format!("{session_id}\u{0}{key}")
}

impl MemoryDigestRegistry {
    /// 返回 session 当前已 loaded 的 key 集合(无 `session\u{0}` 前缀)。
    pub async fn get(&self, session_id: &str) -> HashSet<String> {
        let prefix = format!("{session_id}\u{0}");
        self.inner
            .read()
            .await
            .iter()
            .filter_map(|k| k.strip_prefix(&prefix))
            .map(|s| s.to_string())
            .collect()
    }

    /// 把 key 并入 session 的 loaded 集合(幂等)。同时登记 session 本身
    /// (`clear` 的 O(1) 定位;失败无副作用 — best-effort 语义)。
    pub async fn extend(&self, session_id: &str, keys: impl IntoIterator<Item = String>) {
        let scoped_keys: Vec<String> = keys.into_iter().map(|k| scoped(session_id, &k)).collect();
        let mut guard = self.inner.write().await;
        for k in scoped_keys {
            guard.insert(k);
        }
        drop(guard);
        self.sessions.write().await.insert(session_id.to_string());
    }

    /// 清空 session 的 loaded 集合(`delete_session_inner` 接线点 —
    /// session_id 复用不得拿到残留 loaded 状态)。
    pub async fn clear(&self, session_id: &str) {
        let prefix = format!("{session_id}\u{0}");
        let mut guard = self.inner.write().await;
        guard.retain(|k| !k.starts_with(&prefix));
        drop(guard);
        self.sessions.write().await.remove(session_id);
    }
}

// ---------------------------------------------------------------------------
// 拦截执行核心(chat_loop/tools.rs 调用)
// ---------------------------------------------------------------------------

/// `load_memory_sections` 的执行体:解析 `sections` 数组 → 从 mtime-fence
/// cache 现取层 → 定位节 → 返回原文文本。成功 side-effect:registry 记
/// loaded key(粘性)。返回 `(content, is_error)` — 错误消息附可用层/节
/// 清单(模型自愈,同 C7D 直呼自愈先例)。
pub async fn execute_load_memory_sections(
    cache: &crate::memory::MemoryCache,
    project_id: &str,
    project_path: &str,
    session_id: &str,
    input: &serde_json::Value,
) -> (String, bool) {
    let specs: Vec<String> = match input.get("sections").and_then(|v| v.as_array()) {
        Some(arr) => arr
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect(),
        None => {
            return (
                "Error: `sections` must be an array of layer/section specs, e.g. \
                 [\"Project CLAUDE.md#Architecture\"] or [\"all\"]."
                    .to_string(),
                true,
            );
        }
    };
    if specs.is_empty() {
        return ("Error: `sections` is empty.".to_string(), true);
    }

    let layers = crate::memory::loader::load_for_session(cache, project_id, project_path).await;
    let digest_layers: Vec<&MemoryLayer> = layers.iter().filter(|l| is_digest_layer(l)).collect();
    if digest_layers.is_empty() {
        return (
            "Error: no digest layers loaded in this session (memory digest not active \
             or files below threshold)."
                .to_string(),
            true,
        );
    }

    let want_all = specs.iter().any(|s| s.trim().eq_ignore_ascii_case("all"));
    let mut out = String::new();
    let mut loaded_keys: Vec<String> = Vec::new();

    for layer in &digest_layers {
        let key = layer_key(layer);
        let sections = split_sections(&layer.content);
        let requested = want_all
            || specs.iter().any(|spec| {
                let s = spec.trim().to_lowercase();
                s == key.to_lowercase() || s.starts_with(&format!("{}#", key.to_lowercase()))
            });
        if !requested {
            continue;
        }
        // 整层请求(spec == key 或 all):全文 + 登记层 key。
        let whole_layer = want_all
            || specs
                .iter()
                .any(|spec| spec.trim().to_lowercase() == key.to_lowercase());
        if whole_layer {
            out.push_str(&format!("# {key} (full)\n\n{}\n", layer.content));
            loaded_keys.push(key.clone());
            continue;
        }
        // 节请求:逐 spec 定位。
        out.push_str(&format!("# {key}\n\n"));
        for spec in &specs {
            let Some(title) = spec.trim().split_once('#').map(|(_, t)| t.trim()) else {
                continue;
            };
            if title.is_empty() {
                continue;
            }
            match match_section(&sections, title) {
                Some(s) => {
                    out.push_str(&s.body);
                    out.push_str("\n\n");
                    loaded_keys.push(section_key(&key, &s.title));
                }
                None => {
                    let available: Vec<String> = sections.iter().map(|s| s.title.clone()).collect();
                    out.push_str(&format!(
                        "Error: no section matches \"{title}\" in {key}. \
                         Available: {}.\n\n",
                        available.join(", ")
                    ));
                }
            }
        }
    }

    if loaded_keys.is_empty() {
        let available: Vec<String> = digest_layers
            .iter()
            .map(|l| {
                let key = layer_key(l);
                let titles: Vec<String> = split_sections(&l.content)
                    .iter()
                    .map(|s| s.title.clone())
                    .collect();
                format!("{key}: [{}]", titles.join(", "))
            })
            .collect();
        return (
            format!(
                "Error: no loaded layer/section matched. Available — {}",
                available.join(" | ")
            ),
            true,
        );
    }

    registry().extend(session_id, loaded_keys).await;
    (out.trim_end().to_string(), false)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::types::MemoryKind;

    fn claude_layer(content: &str, tokens: u32) -> MemoryLayer {
        MemoryLayer {
            kind: MemoryKind::Project,
            source: MemorySource::Claude,
            path: "/proj/CLAUDE.md".into(),
            content: content.to_string(),
            tokens,
            status: LayerStatus::Loaded,
        }
    }

    fn agents_layer(content: &str, tokens: u32) -> MemoryLayer {
        MemoryLayer {
            kind: MemoryKind::Project,
            source: MemorySource::Agents,
            path: "/proj/AGENTS.md".into(),
            content: content.to_string(),
            tokens,
            status: LayerStatus::Loaded,
        }
    }

    // ---------------- split_sections ----------------

    #[test]
    fn fenced_hash_comments_do_not_split() {
        // repo CLAUDE.md 的 Common Commands 形态:code block 内 `# 开发`
        // 是 shell 注释,不得成为节界。
        let content = "# T\n\n## A\n\ntext\n\n## B\n\n```bash\n# 开发\npnpm dev\n```\nafter";
        let sections = split_sections(content);
        let titles: Vec<&str> = sections.iter().map(|s| s.title.as_str()).collect();
        assert_eq!(titles, vec!["T", "A", "B"]);
        // B 的 body 含整个 fence(含 `# 开发` 注释行)。
        let b = &sections[2];
        assert!(b.body.contains("# 开发"));
        assert!(b.body.contains("after"));
    }

    #[test]
    fn preamble_before_first_header_is_its_own_section() {
        let content = "intro line\nanother\n\n## First\nbody";
        let sections = split_sections(content);
        assert_eq!(sections[0].title, PREAMBLE_TITLE);
        assert_eq!(sections[0].level, 0);
        assert_eq!(sections[1].title, "First");
        assert_eq!(sections.len(), 2);
    }

    #[test]
    fn no_header_file_is_single_preamble() {
        let sections = split_sections("just text\nmore text");
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].title, PREAMBLE_TITLE);
    }

    #[test]
    fn header_requires_space_and_max_level_three() {
        let content = "#ok not header\n#### deep\n## Real\nx";
        let sections = split_sections(content);
        // `#ok`(无空格)与 `#### deep`(4 级)都不切 — 全归 Preamble。
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].title, PREAMBLE_TITLE);
        assert_eq!(sections[1].title, "Real");
    }

    #[test]
    fn unclosed_fence_swallows_rest() {
        let content = "## A\n```\ncode\n## not a section";
        let sections = split_sections(content);
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].title, "A");
        assert!(sections[0].body.contains("## not a section"));
    }

    // ---------------- section_summary ----------------

    #[test]
    fn summary_takes_first_prose_line() {
        let s = DocSection {
            title: "A".into(),
            level: 2,
            body: "## A\n\nfirst prose\nsecond".into(),
        };
        assert_eq!(section_summary(&s), "first prose");
    }

    #[test]
    fn summary_falls_back_inside_pure_fence_section() {
        // Common Commands 实测形态:整节一个大 code block → 回退取节内
        // 首个非空行(设计 §3.2 回退规则)。
        let s = DocSection {
            title: "Common Commands".into(),
            level: 2,
            body: "## Common Commands\n\n```bash\n# 开发\npnpm dev\n```".into(),
        };
        assert_eq!(section_summary(&s), "# 开发");
    }

    #[test]
    fn summary_truncates_by_chars_cjk_safe() {
        let long: String = "字".repeat(200);
        let s = DocSection {
            title: "A".into(),
            level: 2,
            body: format!("## A\n{long}"),
        };
        let summary = section_summary(&s);
        assert_eq!(summary.chars().count(), SUMMARY_MAX_CHARS + 1); // 120 字 + …
        assert!(summary.ends_with('…'));
    }

    // ---------------- tier + digest_body ----------------

    #[test]
    fn agents_layers_never_digest_small_claude_exempt() {
        let big_agents = agents_layer(&"x".repeat(3000), 2000);
        assert!(!is_digest_layer(&big_agents));
        let small_claude = claude_layer("tiny", 10);
        assert!(!is_digest_layer(&small_claude));
        let big_claude = claude_layer(&"x".repeat(3000), 2000);
        assert!(is_digest_layer(&big_claude));
    }

    #[test]
    fn digest_body_is_index_plus_guidance_not_full_text() {
        // 摘要 = 首句;首句之外的正文(SECRET-*-DEEP)不得出现在目录里。
        let layer = claude_layer(
            "## Alpha\nFIRST-LINE-ALPHA\nSECRET-ALPHA-DEEP-CONTENT\n\n## Beta\nbeta first\nSECRET-BETA-DEEP",
            900,
        );
        let body = digest_body(&layer, &HashSet::new());
        assert!(body.contains("[digest — 2 sections"));
        assert!(body.contains("FIRST-LINE-ALPHA"));
        assert!(body.contains("load_memory_sections([\"Project CLAUDE.md#<section title>\"])"));
        assert!(!body.contains("SECRET-ALPHA-DEEP-CONTENT"));
        assert!(!body.contains("SECRET-BETA-DEEP"));
    }

    #[test]
    fn digest_body_appends_loaded_sections_after_index() {
        // 节体首行(=摘要)与深层正文用不同标记,避免 find 撞到目录行。
        let layer = claude_layer(
            "## Alpha\nalpha summary line\nALPHA-DEEP-BODY\n\n## Beta\nbeta summary line\nBETA-DEEP-BODY",
            900,
        );
        let loaded: HashSet<String> = [section_key("Project CLAUDE.md", "Alpha")]
            .into_iter()
            .collect();
        let body = digest_body(&layer, &loaded);
        let idx = body.find("2. Beta").expect("index present");
        let full = body
            .find("ALPHA-DEEP-BODY")
            .expect("loaded section appended");
        assert!(full > idx, "loaded full text must come AFTER the index");
        assert!(
            !body.contains("BETA-DEEP-BODY"),
            "unloaded section stays digest-only"
        );
    }

    #[test]
    fn digest_body_whole_layer_loaded_returns_full_content() {
        let layer = claude_layer("## Alpha\nalpha body", 900);
        let loaded: HashSet<String> = ["Project CLAUDE.md".to_string()].into_iter().collect();
        let body = digest_body(&layer, &loaded);
        assert!(body.contains("alpha body"));
        assert!(body.contains("[digest — 1 sections")); // 头部元信息仍在
    }

    // ---------------- match_section ----------------

    #[test]
    fn match_section_exact_prefix_substring_and_ambiguity() {
        let mk = |t: &str| DocSection {
            title: t.to_string(),
            level: 2,
            body: String::new(),
        };
        let sections = vec![mk("Architecture"), mk("核心数据流"), mk("核心架构决策")];
        assert_eq!(
            match_section(&sections, "Architecture").unwrap().title,
            "Architecture"
        );
        assert_eq!(
            match_section(&sections, "architecture").unwrap().title,
            "Architecture"
        );
        // 唯一前缀:核心数据 ← 核心数据流。
        assert_eq!(
            match_section(&sections, "核心数据").unwrap().title,
            "核心数据流"
        );
        // 歧义(两个"核心"前缀)→ 前缀失败,子串也歧义 → None。
        assert!(match_section(&sections, "核心").is_none());
        // 唯一子串:架构决策。
        assert_eq!(
            match_section(&sections, "架构决策").unwrap().title,
            "核心架构决策"
        );
        assert!(match_section(&sections, "nope").is_none());
    }

    // ---------------- registry ----------------

    #[tokio::test]
    async fn registry_extend_get_clear_is_scoped_per_session() {
        let r = MemoryDigestRegistry::default();
        r.extend("s1", ["Project CLAUDE.md#A".to_string()]).await;
        r.extend("s2", ["Project CLAUDE.md#B".to_string()]).await;
        let s1 = r.get("s1").await;
        assert!(s1.contains("Project CLAUDE.md#A"));
        assert!(!s1.contains("Project CLAUDE.md#B"));
        r.clear("s1").await;
        assert!(r.get("s1").await.is_empty());
        assert!(!r.get("s2").await.is_empty(), "s2 untouched");
    }

    // ---------------- 静态度量(AC2 形态锁) ----------------

    #[tokio::test]
    async fn digest_of_large_layer_stays_small() {
        // 用放大版 repo 形态(7 节 + 每节约 4KB 正文,模拟 28KB CLAUDE.md)
        // 锁定目录体量级 — 全量 ~8k tok 时目录应 ≤ 1/8。
        let mut content = String::from("# T\n\nintro\n");
        for i in 0..7 {
            content.push_str(&format!("\n## Section {i}\n"));
            content.push_str(&format!("{}\n", "正文内容。".repeat(600))); // ~4.2KB/节
        }
        let layer = claude_layer(&content, 8_000);
        let body = digest_body(&layer, &HashSet::new());
        let body_tokens = crate::memory::tokens::count_tokens(&body).await;
        let full_tokens = crate::memory::tokens::count_tokens(&content).await;
        assert!(
            body_tokens * 8 <= full_tokens,
            "digest {body_tokens} should be ≤ 1/8 of full {full_tokens}"
        );
    }
}
