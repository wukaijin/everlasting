# P0 — web_fetch redirect 自定义策略 + 重做 IP 校验

## Goal

关闭 RULE-E-003 P0 安全漏洞:`web_fetch` tool 当前 `Policy::limited(MAX_REDIRECTS)` 只数 depth,不在每个 redirect target 上重做 host → IP 解析 + blocklist 校验。攻击路径:`attacker.com → 301 → 169.254.169.254/latest/meta-data/iam/security-credentials/` 绕 SSRF 打云 metadata 泄漏 IAM 凭证。

## Decisions (ADR-lite)

**Context**: 当前 `web_fetch.rs:385` `.redirect(redirect::Policy::limited(MAX_REDIRECTS))` 简单 5 跳限制,初始 URL 走 `resolve_public` + `is_blocked`(`:372`),但 redirect 后的 host 不再校验。SPEC 自相矛盾:docstring `:17` 写"each redirect target",但 §5 security notes 又写"not implemented"(即 DRIFT-002)。

**Decisions**:

1. **改用 `Policy::custom`**: `reqwest::redirect::Policy::custom(F)` 接受 `FnMut(Attempt) -> Action`,每 redirect 在 callback 里能拿到新 URL,在 callback 内做 IP check 决定 Follow/Stop/Error
2. **同步 IP 解析**: callback 是 `FnMut` (非 async),用 `std::net::ToSocketAddrs` 同步解析 host,代价 ~50ms 一次可接受。不引 `tokio::task::block_in_place` 复杂度
3. **复用现有 `is_blocked` + blocklist**: 不重写 IP check,提取 `resolve_and_check(host, port, allow_private) -> Result<SocketAddr, WebFetchError>` 给 callback 用(sync 版本)
4. **失败行为**: redirect target 是 private IP / 不可解析 / 解析超时 → `Action::Stop` + 保留已收内容(若 status 是 3xx,告诉 LLM "redirect refused by SSRF guard"),**不**返回 Error 让 LLM 看到 `private address`
5. **测试路径**: `execute_for_test` 仍 bypass 初始 URL IP check,新增的 redirect IP check 在测试路径**也**生效(redirect 本身的 SSRF 防护不能 bypass),所以测试用 mock server + `allow_private=true` 仍可走完初始 URL,redirect target 用 `127.0.0.1` mock
6. **同步修 DRIFT-002**: web_fetch.rs:17 docstring 改成"每个 redirect target 都做 IP check"或删除该行,§5 security notes 改为"implemented, see `Policy::custom` callback"

**Consequences**:
- `attacker.com → 169.254.169.254` 在第二次 redirect 被拒
- `attacker.com → internal.company.lan` 同样被拒
- 合法多跳 redirect(`http → https`)正常 follow
- DNS rebinding 在初始 URL 已通过 `.resolve(host, public_ip)` pin IP,redirect target 走新解析 + block,两阶段防护

---

## Requirements

### R1 — 自定义 redirect Policy

* 替换 `web_fetch.rs:385` `.redirect(redirect::Policy::limited(MAX_REDIRECTS))` 为 `.redirect(redirect::Policy::custom(check_redirect))`
* 实现 `fn check_redirect(attempt: redirect::Attempt) -> redirect::Action`:
  - 从 `attempt.url()` 取 host + port
  - 调 `resolve_and_check_sync(host, port, allow_private)`
  - Ok → `Action::Follow`
  - Err(BlockedAddress(_)) → `Action::Stop`(终止 redirect 链)
  - Err(Network(_)) → `Action::Stop`
* 提取 `fn resolve_and_check_sync(host: &str, port: u16, allow_private: bool) -> Result<SocketAddr, WebFetchError>`,内部用 `std::net::ToSocketAddrs` 解析
* `reqwest::redirect::Attempt` 提供 `.previous()` 取上一次 URL,记录已访问 host 防 redirect loop(可选,本期不实施,Policy 自带循环检测)

### R2 — 同步修 DRIFT-002

* `web_fetch.rs:17` docstring:`"a hard-coded IP blocklist is applied to the initial URL AND each redirect target"` — 这本来就是规范,只是实现缺失,改为"implemented, see `Policy::custom`"
* `web_fetch.rs` §5 注释:删除"not implemented",加"implemented via `Policy::custom(check_redirect)` callback"

### R3 — 测试覆盖

* 新单测 `redirect_to_blocked_address_is_refused`:
  - MockServer A (受信任 attacker): `GET / → 301 Location: http://127.0.0.1:PORT2/metadata`
  - MockServer B (loopback,模拟 cloud metadata): `/metadata → 200`
  - 调用 `execute_for_test` 拿初始 URL,断言 `is_error == true` + 错误含 "redirect refused" / "private" / "stopped"
  - 断言 B 的 mock **未**被 hit(redirect 被拦)
* 新单测 `redirect_chain_follows_when_public`:
  - MockServer A: `/page → 301 Location: /page2`(同 server,公开 IP)
  - MockServer B(`/page2`): `200 OK`
  - 断言 B 被 hit,内容返回
* 新单测 `redirect_to_rfc1918_blocked`:
  - MockServer A: `→ 301 Location: http://10.0.0.1/admin`
  - 断言 `is_error`,错误含 "private"

### R4 — 文档

* `web_fetch.rs` 顶部 module docstring "SSRF protection in MVP" 段补 "via `Policy::custom` callback"
* ARCHITECTURE.md §web_fetch 引用 RULE-E-003

---

## Acceptance Criteria

* [ ] `web_fetch.rs` 用 `Policy::custom` 替换 `Policy::limited`
* [ ] callback 内对每个 redirect target 走 `resolve_and_check_sync` + `is_blocked`
* [ ] Blocked redirect target 返回 `Action::Stop` 而非 `Action::Follow`
* [ ] DRIFT-002 docstring 矛盾同步修复
* [ ] 至少 3 个新单测覆盖 redirect SSRF
* [ ] 现有 17+ web_fetch 单测全部仍 pass
* [ ] `cargo test --lib tools::web_fetch` green
* [ ] `cargo check` 无新增 warning

---

## Definition of Done

* 上述 Acceptance Criteria 全 ✅
* PR merge 后更新 `docs/_reviews/DEBT.md` 中 `RULE-E-003` 条目:`Status: closed`
* `SPEC-DRIFT.md` 中 `DRIFT-002` 标 `resolved by PR #N`

---

## Out of Scope

* DNS rebinding 在 redirect 路径上的双向 pin(本期仅初始 URL pin,redirect 走新解析 + block 已足够)
* HTTPS redirect 到 HTTP 降级检测(现代浏览器都拒,LLM agent 可类似行为,留 P2)
* Redirect 循环显式追踪(Policy 自带次数限制 + 同 host 复用,本期不优化)
* User-agent / Cookie / Header 重写(MVP 不动)

---

## Technical Approach

### 实施步骤

**Step 1: 抽同步 resolve helper**

```rust
/// Sync version of `resolve_public` for use inside the redirect
/// Policy callback (which is `FnMut`, not async).
///
/// Uses `std::net::ToSocketAddrs`; ~50ms per call on a healthy
/// network — acceptable since redirect hops are bounded to 5.
fn resolve_and_check_sync(
    host: &str,
    port: u16,
    allow_private: bool,
) -> Result<SocketAddr, WebFetchError> {
    let addrs: Vec<SocketAddr> = (host, port)
        .to_socket_addrs()
        .map_err(|e| WebFetchError::Network(format!("DNS lookup failed: {}", e)))?
        .collect();
    if addrs.is_empty() {
        return Err(WebFetchError::Network("DNS lookup returned no addresses".into()));
    }
    for addr in &addrs {
        if !is_blocked(addr.ip(), allow_private) {
            return Ok(*addr);
        }
    }
    Err(WebFetchError::BlockedAddress(addrs[0].ip()))
}
```

**Step 2: 改 redirect Policy**

```rust
let redirect_policy = redirect::Policy::custom(move |attempt| {
    let url = attempt.url();
    let host = match url.host_str() {
        Some(h) => h,
        None => return redirect::Action::Stop,
    };
    let port = match url.port_or_known_default() {
        Some(p) => p,
        None => return redirect::Action::Stop,
    };
    match resolve_and_check_sync(host, port, allow_private) {
        Ok(_) => redirect::Action::Follow,
        Err(WebFetchError::BlockedAddress(ip)) => {
            tracing::warn!(
                host = host,
                ip = %ip,
                "web_fetch: redirect target refused (SSRF block)"
            );
            redirect::Action::Stop
        }
        Err(e) => {
            tracing::warn!(
                host = host,
                error = %e,
                "web_fetch: redirect target DNS failed"
            );
            redirect::Action::Stop
        }
    }
});
```

**Step 3: 加 error variant**

`WebFetchError` 加 `RedirectBlocked { from: String, to: String }` 变体,`fetch_and_process` 收到 `Action::Stop` 后续响应时映射到此(返回 Error 给 LLM)。

实际上 reqwest 在 Action::Stop 后会把"已收到的 redirect 响应"作为最终响应返回,我们应在 step 5 之后检查 `response.url() != original_url`,若是且 status 是 3xx,说明是 redirect 被拦,转 `RedirectBlocked`。

**Step 4: 测试**

按 R3 加 3 个新单测。MockServer A/B 是 `httpmock::prelude::*` 的 `MockServer::start()`,用 `server.address()` 拼 URL。

---

## Technical Notes

### 关键文件

* `app/src-tauri/src/tools/web_fetch.rs:372` — 初始 URL resolve(已存在,保留)
* `app/src-tauri/src/tools/web_fetch.rs:385` — redirect Policy(改)
* `app/src-tauri/src/tools/web_fetch.rs:154-211` — `is_blocked` + blocklist(已存在,复用)
* `app/src-tauri/src/tools/web_fetch.rs:642-963` — tests 块,加新单测
* `SPEC-DRIFT.md` — DRIFT-002 同步修

### 同步 DNS 解析的代价

`std::net::ToSocketAddrs` 在 Linux 上调 `getaddrinfo`,~10-100ms 健康网络,可接受。Redirect 上限 5 跳,理论最多 500ms 额外延迟。

### 测试 mock 的限制

`httpmock` 默认绑 `127.0.0.1`,**初始 URL** 走 `allow_private=true` 才能用 mock server。**redirect target** 同样需要 `allow_private=true`(否则会拒 loopback)。但本修复的目的是阻止 redirect 到 private,**测试时应区分**:
- 测试 1: 初始 URL 公开(用 mock),redirect target 是 `10.0.0.1`(RFC 1918),应拒
- 测试 2: 初始 URL mock,redirect 同 server mock(公开 host 走 resolve,但 mock 绑 loopback),应能 follow

**更简单的测试策略**:在测试 1 中让 mock A 重定向到一个**字面量** RFC 1918 地址(如 `http://10.0.0.1/foo`),`resolve_and_check_sync("10.0.0.1", 80, true)` 仍会通过 `is_blocked` check——因为 `allow_private=true`。**这表明 redirect IP check 必须用 `allow_private=false` 才有效**,与 `execute_for_test` 的 `allow_private=true` 矛盾。

**解决方案**:
- 把 `allow_private` 参数也传给 `resolve_and_check_sync`,但 redirect 路径**强制**用 `false`,不管调用方传什么
- 或者:`web_fetch.rs` 在 redirect callback 里 hardcode `allow_private = false`(安全优先)
- 测试时:**redirect target 用真实 RFC 1918 地址但 `allow_private=false`**,mock server 在 callback 拒 redirect 之前已经被初始 URL 请求过一次,断言 B 未 hit

最终方案:callback 内 hardcode `allow_private = false`,与生产路径一致。测试 redirect-to-blocked 时,期望 redirect 链在第一次 hop 被拒,目标 mock 未被 hit。

### 与 RULE-E-001/E-002 的关系

完全独立(不同文件)。本 task 可与 shell P0 修复并行。

---

## Research References

* `.trellis/reviews/DEBT.md` — RULE-E-003 + DRIFT-002
* `docs/_reviews/REVIEW-agent-loop-full-audit-2026-06-14.md` — §2.5 原始论据
* reqwest `redirect::Policy::custom` API docs
* OWASP SSRF Cheat Sheet — redirect 校验章节
* CWE-601: URL Redirection to Untrusted Site