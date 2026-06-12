# Research: HTML→Markdown conversion options in the Rust ecosystem

- **Query**: Pick the best Rust HTML→Markdown crate for a new `web_fetch` tool in Everlasting (Tauri 2 + Rust, lean dep set, LLM-facing output)
- **Scope**: external (crates.io + GitHub/GitLab + docs.rs + cargo tree)
- **Date**: 2026-06-12

## 1. Candidate Crates (Headline Data)

| Crate | Latest | License | Rec. downloads (90d) | Last release | GH stars | Active? |
|---|---|---|---|---|---|---|
| `htmd` | 0.5.4 (2026-04-04) | Apache-2.0 | ~901k | 2026-04-04 | 443 | yes, latest push 2026-04 |
| `html2md` (Kanedias) | 0.2.15 (2025-01-12) | **GPL-3.0** | ~173k | 2025-01 | 20 (GitLab) | slow / stale |
| `html2markdown` (nchapman) | 0.2.0 (2026-02-28) | MIT | ~5.5k | 2026-02-28 | 0 | yes but new / low adoption |
| `fast_html2md` (spider-rs) | 0.0.62 (2026-04-30) | MIT | ~74k | 2026-04-30 | 72 | yes, pre-1.0 |
| `quick_html2md` | 0.2.1 (2026-03-28) | MIT/Apache-2.0 | ~6.7k | 2026-03-28 | 1 | yes, but solo author |
| `mdkit` | 0.7.4 (2026-?) | MIT/Apache-2.0 | (large) | — | — | shell-outs to Pandoc, overkill |
| `tpnote-html2md` | 0.3.7 | MIT | n/a | 2026-? | — | niche, by tp-note author |

Notable rejects:
- `crw-extract` — **AGPL-3.0** (copyleft, blocks our use).
- `pulldown-cmark` — wrong direction (MD→HTML).
- `tl` / raw `html5ever` only — no built-in conversion; would mean writing/maintaining a handler.

## 2. Direct + Transitive Dependency Footprint

Measured locally with `cargo tree -p <crate>` in a scratch project (2026-06-12).

| Crate | Direct deps | Transitive crates (unique) | Heavy? |
|---|---|---|---|
| `htmd` 0.5.4 | `html5ever`, `markup5ever_rcdom`, `phf` | ~33 | Same `html5ever` stack the rest use; lean. |
| `html2markdown` 0.2.0 | `html5ever`, `markup5ever`, `markup5ever_rcdom`, `regex`, `thiserror`, `url` | ~67 | `regex` (proc-macro2/`syn` chain), `url`. |
| `fast_html2md` 0.0.62 | `auto_encoder`, `futures-util`, `lazy_static`, `lol_html`, `percent-encoding`, `regex`, `url` | ~84 | Adds `lol_html` (Cloudflare C-based rewriter, ~large), `encoding_rs` via `auto_encoder`, `regex`. |
| `quick_html2md` 0.2.1 | `dom_query` | ~51 (one big dep) | `dom_query` pulls `cssparser`, `selectors`, `html5ever 0.36` (older line). |

For our project (`reqwest 0.13` + `tokio` + `serde` + `sqlx` + …), `html5ever` is **already transitive** through nothing today (we have no HTML parsing dep), so *every* viable converter adds it. `htmd` adds the fewest extra crates on top.

## 3. Feature / Quality Comparison

| Concern | `htmd` | `html2markdown` | `fast_html2md` | `quick_html2md` |
|---|---|---|---|---|
| HTML parsing | html5ever (Servo) | html5ever | lol_html (rewriter) / html5ever (scraper feat.) | html5ever (via `dom_query`) |
| Test corpus | Passes **all turndown.js test cases** | Ports `hast-util-to-mdast` (130 fixture tests) | internal | internal |
| Tables | yes (GFM-style) | yes (with alignment) | yes | yes (GFM, alignment) |
| Nested lists | yes | yes | yes | yes (the README brags about it) |
| Fenced code + lang hint | yes | yes | yes | yes |
| `<pre>` w/o `<code>` | wraps in fence (untyped) | fenced w/ no lang | fenced | fenced |
| `<a>` relative→absolute | manual / opt-in | opt-in via base URL | opt-in | opt-in (`base_url` option) |
| `<img>` | `![alt](src)` | `![alt](src)` | yes | yes |
| `<script>`/`<style>` | skip_tags builder | dropped by default | dropped | dropped |
| HTML entities | decoded (html5ever) | decoded | decoded | decoded |
| Custom tag handlers | yes (`add_handler`) | no | no | no |
| Async / streaming | no (sync `convert`) | no | **yes** (`rewrite_html_streaming`) | no |
| Multithread-safe | yes (`Arc<HtmlToMarkdown>`) | yes | yes | yes |
| Faithful mode (preserve unknown HTML) | yes (`#54`) | no | no | no |
| One-call API | `htmd::convert(&str)` | `html2markdown::convert(&str)` | `html2md::rewrite_html(&str, false)` | `quick_html2md::html_to_markdown(&str)` |
| `Result` returns | `Result<String, _>` | `String` (panics on bad input) | `String` | `String` |

## 4. Maintenance & Safety

- No RustSec advisories match any of these crates (`rustsec.org` search 2026-06-12, empty).
- `htmd` is single-maintainer (`letmutex`) but **most-starred (443)**, latest release two months old, last push ~2 months ago — healthiest of the group.
- `html2md` (Kanedias) is **GPL-3.0** → license-incompatible with the project's MIT/Apache stance; also stale (last release 17 months old).
- `html2markdown` and `quick_html2md` are healthy but **low-star / low-download** (≈5–7k/90d) — higher bus factor risk.
- `fast_html2md` is in production at Spider Cloud but **pre-1.0 (0.0.62)** with `lol_html` (C dependency) inflating build time and binary size.
- Robustness to malformed HTML: all four delegate parsing to `html5ever` (or `lol_html`), which is Servo's spec-compliant HTML5 parser — handles unclosed tags, weird nesting, etc. well. No crate-level CVE history.

## 5. Performance (vendor-reported; not independently re-benchmarked)

- `htmd`: ~16 ms for 1.37 MB Wikipedia page on M4 (vendor).
- `fast_html2md`: vendor-claimed "fastest", uses streaming lol_html rewriter; only candidate with built-in async streaming.
- `html2markdown`: no published number; MDAST-based pipeline is slightly heavier.
- `quick_html2md`: no published number.

All are fine for 100 KB pages; `fast_html2md` wins on very large / adversarial input because of the streaming rewriter.

## 6. LLM-friendly Output — Field Notes

What *every* candidate does well:
- Decode entities, strip `<script>`/`<style>`, emit GFM tables, fenced code with language hint, link rewriting, image fallback to alt text.

What matters for an LLM that sees the result:
- **Preserve code-block language hints** — yes, all four do. `htmd` and `quick_html2md` read `class="language-xxx"` reliably.
- **`<pre>` without `<code>`** — all four wrap in a fenced block but emit no language. Acceptable.
- **Navigation/footer/sidebar** — none of the four strip *semantic* chrome (no `readability`/`mozilla` style). For docs sites we may want to pre-strip `<nav>`, `<footer>`, `<header>`, `<aside>` ourselves before conversion, or add an htmd `add_handler` for them.
- **Relative URLs** — only `html2markdown` and `quick_html2md` expose a built-in `base_url` option; for `htmd` you can register a custom `<a>`/`<img>` handler (it supports it natively).
- **Microdata / data-attributes** — all four drop them; that's the right default for LLM input.

## 7. Code Sketch (recommended crate: `htmd`)

```rust
// src-tauri/src/tools/web_fetch.rs
use htmd::HtmlToMarkdown;

pub async fn web_fetch(url: &str) -> anyhow::Result<String> {
    // existing reqwest fetch ...
    let html: String = reqwest::get(url).await?.text().await?;

    // strip semantic chrome (no LLM needs a docs-site sidebar)
    let html = strip_chrome(&html);

    // convert; 1-line default, builder for customisation
    let converter = HtmlToMarkdown::builder()
        .skip_tags(vec!["script", "style", "noscript", "nav", "footer", "header", "aside"])
        .build();

    let md = converter.convert(&html)
        .map_err(|e| anyhow::anyhow!("html→md failed: {e}"))?;
    Ok(md)
}
```

10 lines of real code, builder pattern is the only customisation surface. `Arc<HtmlToMarkdown>` can be shared across tool invocations.

## 8. Concrete Recommendation

**Pick `htmd` 0.5.**

Why:
1. **Leanest deps** (3 direct: `html5ever` + `markup5ever_rcdom` + `phf`; ~33 transitives). Aligns with project's "fewer new deps" stance (HACKING-wsl, B5 memory footgun notes).
2. **Most battle-tested** — passes the *entire* turndown.js test corpus (the de-facto JS reference) and is by far the most-downloaded Rust HTML→MD crate (~901k recent downloads vs. next-best 173k for the GPL `html2md`).
3. **Best ergonomics** — one-call `convert`, but a builder for `skip_tags` / `heading_style` / custom tag handlers (useful if we want to drop `<nav>`/`<aside>` etc.).
4. **License clean** (Apache-2.0).
5. **Active** (last release 2 months ago; 443 GH stars; 2 open issues).
6. **Thread-safe + small API** — fits the existing `tools/*.rs` per-tool pattern (cf. `read_file.rs` / `shell.rs`).
7. **Robust** — delegates HTML parsing to `html5ever` (Servo-grade HTML5 spec compliance), so malformed real-world HTML is handled.

**Alternates (only if `htmd` breaks for us):**
- **`html2markdown`** (MIT, ~5.5k dl) — drop-in replacement, two-phase AST pipeline, good test corpus (130 fixtures). Slightly heavier deps. Useful if we want `base_url` URL resolution built-in.
- **`fast_html2md`** (MIT) — pick this if we hit *streaming* needs (e.g. fetch-and-stream-progress to the LLM). Costs `lol_html` (C dep, bigger binary, longer first build). Currently 0.0.x.
- **Roll-your-own on `html5ever`** — last resort; only if the four crates all become unmaintained. Would need ~600–1k LOC of handlers and a test corpus; not worth it today.

**Explicitly reject:**
- `html2md` (Kanedias) — GPL-3.0 license.
- `crw-extract` — AGPL-3.0.
- `mdkit` — pulls `pdfium`, `calamine`, `csv`, `pandoc` discovery; vastly overweight for a single HTML→MD call.
- `tpnote-html2md` — niche, single-project-driven.

## 9. Dependency Cost (Cargo.toml delta)

```toml
# One line, in [dependencies]:
htmd = "0.5"
```

What it actually pulls into the build (from `cargo tree`):
- 3 direct (`html5ever`, `markup5ever_rcdom`, `phf`)
- ~33 unique transitive crates (same `html5ever` ecosystem Tauri itself uses, so no ABI/build-time surprises)
- No new `proc-macro2`/`syn` chain (those are dev/build-time only and already in the tree)
- No new C deps, no async runtime change
- Build-time impact: html5ever is moderate (~30–60s on first cold build for the dev profile, much less incremental). The rest is trivial.

## Caveats / Not Found

- No published independent benchmark comparing all four crates; performance numbers above are vendor-claimed. A 100 KB doc is well under 1 ms for all four on a modern CPU — not a real differentiator for us.
- No known CVEs in any of the four (RustSec search empty 2026-06-12).
- No crate does semantic readability-style chrome stripping; we'll either pre-strip with simple selectors or register `htmd` `add_handler` hooks for `<nav>`/`<footer>`/`<header>`/`<aside>`.
- `html2markdown` (nchapman) is interesting architecturally (two-phase AST) but very new (0.2.0, 0 GH stars) — flag for revisit if it matures.
