# feat(tools): web_fetch — agent 自主抓取外部文档/API 参考 (P1)

## Goal

给 agent 加一个 `web_fetch` 工具,让 LLM 自主获取外部文档/错误信息/API 参考(对比当前只能依赖训练数据或用户粘贴)。这是 P1 任务,目标工具数从 7 → 8,接近 Claude Code 核心高频工具的 95%。

**对标参考**:`docs/_reviews/REVIEW-tool-comparison-2026-06-12.md` §5.1 (P1 建议) + §3.1-3.2 (各家 web_fetch 参数)。

## What I already know (代码 + 文档事实)

**架构基线**:
- `app/src-tauri/src/tools/<name>.rs` — 每个 tool 一个文件,导出 `definition()` 返回 `ToolDef` + `execute(input, ctx, ...)` 异步函数
- `tools/mod.rs` — 在 `builtin_tools()` 注册新 tool,在 `execute_tool_inner` 派发
- `ToolDef { name, description, input_schema: serde_json::Value }` — JSON Schema 描述参数
- 所有 tool 走 `tokio::select!` 的 cancel 包装 (`CancellationToken` 注入)
- `ToolContext { worktree_path, cwd }` — 每个 tool 都拿到,但 web_fetch 不需要边界检查
- `ReadGuard` / `session_id` 是可选参数,只有 `read_file` / `edit_file` 用

**HTTP 客户端现状**:
- `reqwest 0.13` 已在 `Cargo.toml` (`features = ["stream", "json", "rustls"]`)
- `anthropic.rs:209-219` 是当前模式:`Client::builder().timeout(60s).connect_timeout(10s)`,每个 provider 各自构造
- **没有共享 client** — web_fetch 跟同样模式即可

**多 provider 影响**:
- `web_fetch` 完全是 Rust 后端做 HTTP,跟 provider 协议无关
- **不需要改 wire 层 / Provider trait / ToolDef 序列化**

**LLM 错误分类**:
- `LlmError` 5 类 (Auth / RateLimit / InvalidRequest / Server / Network) 是给 LLM 调用用的
- `web_fetch` 的错误走 `is_error: true` 工具结果通道,不是 LLM 调用错误 → 不复用 `LlmError`

**已有 spec 文档**:
- `.trellis/spec/backend/tool-contract.md` — 现有 7 个 tool 的契约,web_fetch 落地后需要补一节

## Research 总结 (3 份已落 `research/`)

1. **`research/web-fetch-api-design.md`** — 主流实现横向对比
   - **推荐 Candidate A** (OpenCode 风格):`url` (必填) + `format` (markdown | text | html, 默认 markdown) + `timeout` (秒, 默认 30, 最大 120)
   - 不做 extraction prompt(本地无独立小模型,主模型直接看 markdown 即可)
   - 通用约定:GET only / HTTP→HTTPS auto-upgrade / markdown 默认输出 / 响应大小限制 / `is_error: true` + 人类可读错误

2. **`research/html-to-markdown-rust.md`** — Rust 生态选型
   - **推荐 `htmd` 0.5.4**:依赖最少 (3 direct / ~33 transitive) / 跑通 turndown.js 全部 test / 901k 下载 / Apache-2.0 / 仍在维护
   - 一行依赖:`htmd = "0.5"`
   - 首次 cold `cargo check` 多 ~30-60s(增量几乎无开销)

3. **`research/web-fetch-security.md`** — 安全威胁建模
   - **T2a-c (SSRF 私网/loopback/link-local/cloud-metadata) 是 High severity,MVP 必须挡**
   - **纠正了 PRD 初始假设 #7**:"MVP 不做 SSRF" 被 research 反对 — LLM 驱动的本地 agent 一旦可扫 `169.254.169.254`,等于把 IDE 变成网络扫描器
   - 7 类工具错误:`InvalidUrl` / `BlockedAddress` / `TooLarge` / `HttpStatus { code }` / `Timeout` / `Tls` / `Network`
   - 实现 ~80 行 stdlib 代码,**不需要新 crate**
   - 必加项:scheme allowlist (http/https) / max 5 redirects (每跳都重新验 IP) / 5 MiB body cap / 30s timeout / 严格 TLS / `User-Agent: Everlasting/<ver>`

## Requirements (收敛版 MVP)

**Tool schema** (Candidate A):
```json
{
  "name": "web_fetch",
  "input_schema": {
    "type": "object",
    "properties": {
      "url": { "type": "string", "description": "..." },
      "format": { "enum": ["markdown", "text", "html"], "default": "markdown" },
      "timeout": { "type": "integer", "description": "秒,默认 30,最大 120" }
    },
    "required": ["url"]
  }
}
```

**行为**:
1. 校验 URL scheme ∈ {http, https} → 否则 `InvalidUrl`
2. DNS 解析 host → 全部 IP 在私网/loopback/link-local/云 metadata 段 → `BlockedAddress`
3. `reqwest::Client` 配 `.timeout(timeout).connect_timeout(10s).redirect(Policy::limited(5))` + rustls 严格 TLS
4. **每跳 redirect 都重新走 IP 检查** (T2f)
5. GET 请求,带 `User-Agent: Everlasting/<ver>` + `Accept: text/markdown;q=1, text/html;q=0.9, text/plain;q=0.8, application/json;q=0.5, */*;q=0.1` + `Accept-Encoding: gzip, br, deflate`
6. Body 上限 5 MiB,超过则截断并追加 `[truncated, original was N MiB]`
7. 按 `format` 处理:
   - `markdown` (默认) / `text/html` / 含 `<html` → `htmd` 转 markdown
   - `text` → `htmd` 转 markdown 后再做 strip(去 markdown 标记)
   - `html` → 原文返回
   - `application/json` / 其他 → 原文 pretty-print(if JSON) / 否则 raw
8. 5 类结果前缀:`<!-- fetched: <url> at <RFC3339> · status <code> · <bytes> bytes · content-type <ct> -->\n\n`
9. 输出总长超过 ~100 KB → 头 50 KB + `<truncated: omitted N bytes>` + 尾 50 KB(参考 `read_file` 策略)
10. `tokio::select!` 包装 + `CancellationToken` 注入,用户 Stop 能中断

**错误映射** (返回 `is_error: true` + 人类可读字符串):
- `InvalidUrl` → "URL must be http or https"
- `BlockedAddress` → "refusing to fetch private/loopback/link-local address (URL resolves to {ip})"
- `TooLarge` → "response body exceeds 5 MiB cap; truncated"
- `HttpStatus { code }` → "HTTP {code} {reason}"
- `Timeout` → "request timed out after {n}s"
- `Tls` → "TLS error: {msg}"
- `Network` → "network error: {msg}"

## Acceptance Criteria

- [ ] `definition()` 返回正确 schema
- [ ] `builtin_tools()` 注册 web_fetch
- [ ] `execute_tool_inner` 派发 web_fetch
- [ ] `execute(input, ctx, ...)` 实现完整链路 + 7 类错误处理
- [ ] 单元测试:成功 fetch HTML→MD / text passthrough / json passthrough / 404 / 5xx / timeout / cancel / private IP 拒绝 / scheme 拒绝 / redirect 跨 host 拒绝 / redirect 到私网拒绝
- [ ] 集成测试:启动 mock HTTP server (`httpmock` 或 `wiremock`),覆盖重定向链 + 大 body + 5xx
- [ ] `cargo test --lib` 全过
- [ ] `vue-tsc --noEmit` 全过(无前端改动)
- [ ] `.trellis/spec/backend/tool-contract.md` 补一节 web_fetch
- [ ] ROADMAP.md 第一档更新
- [ ] 一次 commit 提交

## Definition of Done

- 代码落地 (`tools/web_fetch.rs` + `mod.rs` 注册 + 派发)
- 单元测试 + 集成测试全过
- `cargo check` + `cargo test --lib` 全过
- `vue-tsc --noEmit` 全过
- tool-contract.md 补 spec
- ROADMAP.md 更新
- 一次 commit

## Out of Scope (MVP 明确不做)

- POST / PUT / DELETE method(只做 GET)
- `web_search` (P2 任务)
- `prompt` extraction param(本地无独立小模型)
- JS 渲染(Playwright/CDP)— 静态 HTML only
- 图像 / PDF / 二进制内容
- 抓取结果缓存
- Cookies / session 管理
- robots.txt 遵守
- 域名权限门(Claude Code 风格的 "first time per host")— PRD 标记后续
- DNS rebinding 完整防御(单次解析 + 接受风险)— follow-up spec
- 审计日志 UI
- 可配置 IP blocklist(硬编码,MVP 不暴露)

## Technical Notes

**参考文件**:
- `app/src-tauri/src/tools/read_file.rs` (594 行,含详尽单测,可参照风格)
- `app/src-tauri/src/tools/shell.rs` (timeout 模式参考)
- `app/src-tauri/src/tools/mod.rs` (注册 + 派发点)
- `app/src-tauri/src/llm/provider/anthropic.rs:209-219` (reqwest client 构造)

**新增依赖**:
- `htmd = "0.5"` (HTML→MD)

**复用依赖**:
- `reqwest 0.13` / `tokio` (含 `time` / `net` / `sync`) / `serde` / `serde_json` / `tracing`
- 不需要 `ipnetwork` / `dns-lookup` 等新 crate,IP 比较用 stdlib

**测试用**:
- 候选 mock server:`httpmock` (更简单) / `wiremock` (更强大)— 倾向 `httpmock`,轻量
- 候选断言库:仍用 `tokio::test` + plain `assert!`

## Decision (ADR-lite) — MVP 关键决策

**Context**:
- 工具集需要"agent 自主拿外部信息"能力(P1)
- 主流实现横向对比 + Rust 生态调研 + 安全威胁建模 已完成
- 需要在"最小可用"和"安全基线"之间找平衡

**Decisions**:
1. **API 形状** = Candidate A (OpenCode 风格,3 参数)。理由:本地无独立小模型,extraction prompt 无意义;url-only 跟 LLM 思维模型最贴。
2. **SSRF 防护进 MVP**。理由:research 明确指出 LLM 驱动的本地 agent 一旦可访问 169.254/192.168/127.0.0.1,等于变成网络扫描器 — 修复成本极低 (~80 行),漏掉的成本极高 (隐私+合规+用户信任)。纠正 PRD 初始假设 #7。
3. **HTML→MD 选 `htmd` 0.5**。理由:依赖最少 / 跑通全部 turndown 测试 / Apache-2.0 / 仍在维护。
4. **响应体 5 MiB hard cap,markdown 输出 100 KB head+tail 截断**。理由:5 MiB 是 OpenCode 上限,100 KB head+tail 是 read_file 的成熟策略,直接复用体感。
5. **timeout 默认 30s,最大 120s**。理由:对标 OpenCode + 跟 shell 的 120s 持平。
6. **GET only,scheme allowlist http/https,strict TLS**。理由:通用约定 + 防御 SSRF 基础。
7. **Honest User-Agent `Everlasting/<ver>`**。理由:不假装是浏览器;撞 Cloudflare 403 后续再加 retry。

**Consequences**:
- 工具数 7 → 8,接近 Claude Code 核心高频 95%
- `web_fetch` 落地后为 B6 (agent/task) 的"agent 自主查外部信息"打基础
- 后续可扩展:web_search / domain permissions / image attachments / extraction prompt
- ~80 行 IP block 代码 + ~50 行 IP helper 测试 = 增加 130 行左右代码量

## Research References

- [`research/web-fetch-api-design.md`](research/web-fetch-api-design.md) — 6 家 web_fetch 横向对比,推荐 Candidate A
- [`research/html-to-markdown-rust.md`](research/html-to-markdown-rust.md) — Rust HTML→MD 选型,推荐 htmd
- [`research/web-fetch-security.md`](research/web-fetch-security.md) — 7 类威胁建模 + MVP 默认值 + IP block 代码 sketch
