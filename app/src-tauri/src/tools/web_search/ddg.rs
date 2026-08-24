//! DDG HTML 后端(零配置兜底)。`GET https://html.duckduckgo.com/html/?q=<urlencoded>`,
//! 手写解析(零新依赖——一页两个选择器,引 DOM/scraper 依赖不值):
//!
//! - 结果标题锚:`class="result__a"`(title = 锚文本;href 是 DDG 跳转
//!   链,真实 URL 在 `uddg` 查询参数里,需 percent-decode 还原);
//! - 片段:`class="result__snippet"`(内含 `<b>` 高亮标签,剥掉)。
//! - **202 = Ratelimit 软封锁**(基于出口 IP 信誉,非请求数;与 429
//!   常规语义不同)→ `RateLimited`,**不重试**,mod.rs 渲染层出文案
//!   引导模型改用 web_fetch 直取已知 URL。
//! - 200 但解析出 0 条结果 → `Parse`(形状漂移信号;fixture 单测锁
//!   已知形状)。
//!
//! **网络现实**(2026-08-25 本机实测,research §1):DDG 在代理挂掉时
//! 直连超时死;走代理也两请内吃到 202(共享出口 IP 信誉差)。定位是
//! **零配置兜底**而非主力——auto 无 key 时才有它,失败原因可见。

use std::time::Duration;

use super::{SearchError, SearchHit};

pub(crate) const DEFAULT_BASE_URL: &str = "https://html.duckduckgo.com/html/";

/// UA 常量(design §3 / review P2-6):初始值用浏览器串。实测样本
/// n=1 且与自家直觉反向(Everlasting/0.1 → 200 / Mozilla → 202),
/// IP 信誉主导、无统计力——常量化便于 202 复发时一键翻转验证。
const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64; rv:130.0) Gecko/20100101 Firefox/130.0";

pub(crate) struct DdgClient {
    http: reqwest::Client,
    base_url: String,
}

impl std::fmt::Debug for DdgClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DdgClient")
            .field("base_url", &self.base_url)
            .finish()
    }
}

impl DdgClient {
    pub(crate) fn new(base_url: &str, connect_timeout: Duration) -> Self {
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .connect_timeout(connect_timeout)
            // 不设 per-request .timeout():整体预算在 mod.rs 外层单包。
            .gzip(true)
            .build()
            .unwrap_or_default();
        Self {
            http,
            base_url: base_url.trim_end_matches('/').to_string(),
        }
    }

    pub(crate) async fn search(
        &self,
        query: &str,
        count: u8,
    ) -> Result<Vec<SearchHit>, SearchError> {
        // 手工百分号编码(query 含 CJK / & / 空格)。不用
        // `Url::parse_with_params`:它走 form_urlencoded 序列化,空格
        // 变 `+` 而非 `%20`——DDG 两种都接受,但 `+` 形式让 httpmock
        // 的 query_param 匹配器(form 解码不含 `+`→空格)测不了。
        // `percent-encoding` 已是直接依赖(tunnel node_id 派生同款)。
        let encoded =
            percent_encoding::utf8_percent_encode(query, percent_encoding::NON_ALPHANUMERIC);
        let url = reqwest::Url::parse(&format!("{}?q={}", self.base_url, encoded))
            .map_err(|e| SearchError::Network(format!("bad base url: {e}")))?;
        let resp = self
            .http
            .get(url)
            .header("Accept", "text/html")
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    SearchError::Timeout
                } else {
                    SearchError::Network(e.to_string())
                }
            })?;

        let status = resp.status();
        if status.as_u16() == 202 {
            // DDG 软封锁:重试只会更糟,直接终态(见模块注释)。
            return Err(SearchError::RateLimited);
        }
        if !status.is_success() {
            return Err(SearchError::HttpStatus(status.as_u16()));
        }
        let html = resp
            .text()
            .await
            .map_err(|e| SearchError::Network(e.to_string()))?;
        let hits = parse_results(&html, count as usize);
        if hits.is_empty() {
            return Err(SearchError::Parse(format!(
                "200 but 0 results parsed from a {}-char page",
                html.chars().count()
            )));
        }
        Ok(hits)
    }
}

// ---------------------------------------------------------------------------
// 手写解析(fixture 锁形状)
// ---------------------------------------------------------------------------

/// 解析 DDG html 结果页。配对单位是**结果块**而非文档序索引:以容器
/// 标记 `class="result `(带尾随空格——与 `result__a` / `result__snippet`
/// 的下划线形式不相撞)切块,块内各找第一个标题锚与片段锚。纯索引
/// 配对在中间块缺 snippet 时会把片段错挂到上一条(实证:fixture 第二
/// 块无 snippet)。缺锚的块跳过,缺片段的条目片段留空,截前 `count` 条。
pub(crate) fn parse_results(html: &str, count: usize) -> Vec<SearchHit> {
    let mut hits = Vec::new();
    // `split().skip(1)`:首段是第一个块之前的页头。
    for block in html.split("class=\"result ").skip(1) {
        let Some((href, title)) = find_anchors(block, "result__a").into_iter().next() else {
            continue;
        };
        let snippet = find_anchors(block, "result__snippet")
            .into_iter()
            .next()
            .map(|(_, s)| s)
            .unwrap_or_default();
        hits.push(SearchHit {
            title,
            url: real_url(&href),
            snippet,
        });
        if hits.len() >= count {
            break;
        }
    }
    hits
}

/// 找出 class 含 `marker` 的全部 `<a>` 锚,返回 (href 原值, 内文本)。
/// 内文本过 `html_to_text`(剥 `<b>` 高亮 + 实体解码 + 空白折叠)。
fn find_anchors(html: &str, marker: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut from = 0usize;
    while let Some(rel) = html[from..].find(marker) {
        let marker_at = from + rel;
        from = marker_at + marker.len();
        // marker 落在 class 属性值里;回退到所属标签开头。
        let Some(tag_start) = html[..marker_at].rfind('<') else {
            continue;
        };
        if !html[tag_start..].starts_with("<a") {
            continue; // marker 出现在非锚元素(理论不应发生,容错跳过)
        }
        let Some(tag_end) = html[tag_start..].find('>') else {
            continue;
        };
        let tag = &html[tag_start..=tag_start + tag_end];
        let href = extract_attr(tag, "href").unwrap_or_default();
        // 内文本到闭合 </a> 为止。
        let inner_from = tag_start + tag_end + 1;
        let Some(close) = html[inner_from..].find("</a>") else {
            continue;
        };
        let text = crate::tools::web_fetch::html_to_text(&html[inner_from..inner_from + close]);
        out.push((href, text));
    }
    out
}

/// 从形如 `<a class="x" href="V" ...>` 的标签文本里取属性值(未解码
/// 的原文;实体解码交给调用方/`real_url`)。
fn extract_attr(tag: &str, attr: &str) -> Option<String> {
    let needle = format!("{attr}=\"");
    let start = tag.find(&needle)? + needle.len();
    let rest = &tag[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// 还原真实结果 URL:DDG 的 href 是跳转链,真 URL 在 `uddg` 查询参数。
/// 步骤:实体解码(`&amp;` → `&`,attribute 值是 HTML 转义的)→
/// scheme-relative `//` 补 `https:` → `query_pairs()`(自带 percent-
/// decode)取 `uddg`;非跳转链则原样返回。
pub(crate) fn real_url(href: &str) -> String {
    // 只做实体解码,不过 html_to_text(那是整页文本的空白折叠器,
    // href 不该被改动空白以外的东西;这里 6 实体子集够用)。
    let decoded = decode_entities(href);
    let parseable = if decoded.starts_with("//") {
        format!("https:{decoded}")
    } else {
        decoded.clone()
    };
    if let Ok(u) = reqwest::Url::parse(&parseable) {
        for (k, v) in u.query_pairs() {
            if k == "uddg" {
                return v.into_owned();
            }
        }
    }
    decoded
}

/// 与 `web_fetch::html_to_text` 同款 6 实体子集,但不动空白(属性值
/// 专用)。
fn decode_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&nbsp;", " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 真实形状的最小 fixture(2026-08-25 手抄结构;字段顺序/多余
    /// 属性都被解析器容忍)。锁已知形状——上游漂移时 Parse 兜底。
    const FIXTURE: &str = r#"
<div class="result results_links results_links_deep web-result">
  <h2 class="result__title">
    <a rel="nofollow" class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fdoc.rust-lang.org%2Fbook%2Fch16-00-concurrency.html&amp;rut=8f3a">Rust Book - <b>Fearless</b> Concurrency &amp; Threads</a>
  </h2>
  <a class="result__snippet" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fdoc.rust-lang.org%2Fbook%2Fch16-00-concurrency.html&amp;rut=8f3a">Threads &mdash; Rust&#x27;s approach &lt;to&gt; <b>concurrency</b>. Spawning threads.</a>
</div>
<div class="result results_links results_links_deep web-result">
  <h2 class="result__title">
    <a rel="nofollow" class="result__a" href="https://tokio.rs/tokio/tutorial">Tokio &amp; async runtime</a>
  </h2>
</div>
<div class="result results_links results_links_deep web-result">
  <h2 class="result__title">
    <a rel="nofollow" class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fcjk%20path&amp;rut=x">CJK 与空格 path</a>
  </h2>
  <a class="result__snippet" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fcjk%20path&amp;rut=x">第三个 snippet</a>
</div>
"#;

    #[test]
    fn parses_titles_snippets_and_decodes_uddg() {
        let hits = parse_results(FIXTURE, 10);
        assert_eq!(hits.len(), 3);
        // uddg percent-decode + 锚文本剥 <b> + 实体解码
        assert_eq!(
            hits[0].url,
            "https://doc.rust-lang.org/book/ch16-00-concurrency.html"
        );
        assert_eq!(hits[0].title, "Rust Book - Fearless Concurrency & Threads");
        assert!(hits[0].snippet.contains("Rust"), "{}", hits[0].snippet);
        // 直接 URL(无跳转链)原样保留
        assert_eq!(hits[1].url, "https://tokio.rs/tokio/tutorial");
        assert_eq!(hits[1].snippet, ""); // 该块无 snippet → 按下标配对留空
                                         // CJK / 空格的 percent-decode
        assert_eq!(hits[2].url, "https://example.com/cjk path");
    }

    #[test]
    fn takes_only_first_count() {
        let hits = parse_results(FIXTURE, 2);
        assert_eq!(hits.len(), 2);
    }

    #[test]
    fn real_url_handles_edge_cases() {
        // 跳转链无 uddg → 原样(scheme-relative 保持原样返回解码值)
        assert_eq!(
            real_url("//duckduckgo.com/l/?rut=x"),
            "//duckduckgo.com/l/?rut=x"
        );
        // 已是直链
        assert_eq!(real_url("https://a.b/c?x=1"), "https://a.b/c?x=1");
        // &amp; 实体在取参前解码
        assert_eq!(
            real_url("//d/l/?uddg=https%3A%2F%2Fa.b%2Fc&amp;rut=1"),
            "https://a.b/c"
        );
    }

    #[test]
    fn empty_or_garbage_page_yields_zero_hits() {
        assert!(parse_results("<html><body>anomaly page</body></html>", 10).is_empty());
        assert!(parse_results("", 10).is_empty());
    }
}
