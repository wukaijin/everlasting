# 06-19 — LLM streaming 总超时改 per-chunk read_timeout + 流错误补 tracing

## Context

2026-06-18 17:56:52 一条 session (`request_id=mz8s3hqwx6rmqjswgte`, `messages.seq=37`) 在 thinking 流中途被静默切断,前端只看到 `[生成出错中断]` toast,Rust 日志无任何 WARN/ERROR。

**根因诊断(DB + 代码对照)**:
- DB `messages.seq=37`:`text="[生成出错中断]"`,`content` 只含 thinking block(无 text delta),在"尝试 1"中途被截
- 用户 prompt `seq=36` 时间 `17:55:52.251`,partial turn 落库 `17:56:52.654`,间隔 **60.403s**
- `anthropic.rs:210` / `openai.rs:425` 配的是 `.timeout(Duration::from_secs(60))` —— **reqwest 总 deadline**(从 connect 开始到 body EOF)
- 该 session 用的 model 是 `MiniMax-M3`(provider `Carlos-API`,base_url `https://api.wukaijin.com`),3rd-party Anthropic-compat 代理 + `thinking_effort=high`(默认填),extended thinking 在慢代理上拖到 60s+
- `chat_loop.rs:655-661` 把 `Err(err)` 静默包成 `ChatEvent::Error`,**不打 tracing**,所以 grep 不到任何线索

## 修复

### A. Provider reqwest 总超时 → per-chunk read_timeout

参考 [`seanmonstar/reqwest` 源码注释 `async_impl/client.rs:1448-1459`](https://github.com/seanmonstar/reqwest/blob/master/src/async_impl/client.rs#L1448):

> `.timeout()`: "The timeout is applied from when the request starts connecting until the response body has finished. Also considered a total deadline."
>
> `.read_timeout()`: "The timeout applies to each read operation, and resets after a successful read. This is more appropriate for detecting stalled connections when the size isn't known beforehand."

SSE streaming = 响应大小未知 = `.read_timeout()` 的标准场景。

```diff
 let client = match reqwest::Client::builder()
-    .timeout(Duration::from_secs(60))
+    .read_timeout(Duration::from_secs(60))   // per-chunk,resets per SSE event
     .connect_timeout(Duration::from_secs(10))
     .build()
```

**不变**:`.connect_timeout(10s)` 仍卡握手;代理真要无限吐 chunk(活着但慢),`read_timeout` 也会兜住(连续 60s 无 chunk 视为卡死,触发错误路径)。

### D. chat_loop.rs:657 加 tracing::warn

把 silent 包装改造成:

```rust
Err(err) => {
    tracing::warn!(
        request_id = %rid,
        turn,
        category = err.category(),
        error = %err,
        "chat: LLM stream errored"
    );
    ChatEvent::Error {
        message: err.user_message(),
        category: err.category(),
    }
}
```

## 行业参照

| 项目 | 默认 | 区分 streaming? |
|---|---|---|
| LiteLLM | `timeout=600s` | ✅ `httpx.Timeout(timeout=, connect=, read=, pool=)` |
| Anthropic SDK (Python) | `DEFAULT_TIMEOUT=httpx.Timeout(5.0)` | ✅ 暴露 `Timeout(connect=, read=, write=, pool=)` |
| OpenAI SDK (Python) | 同上 | ✅ |
| Aider | 委托 LiteLLM | — |
| Continue | AbortSignal 用户取消 | ❌ |
| **reqwest** | 无 | ✅ **三独立 API**:`timeout` / `read_timeout` / `connect_timeout` |

## 落地

- **A**:`app/src-tauri/src/llm/provider/anthropic.rs:209-211` + `app/src-tauri/src/llm/provider/openai.rs:424-426`
- **D**:`app/src-tauri/src/agent/chat_loop.rs:657`
- **Spec**:`.trellis/spec/backend/error-handling.md` 新增 "RULE-A-011 (2026-06-19) — reqwest per-chunk read_timeout + stream-error tracing" 段
- **ADR**:`docs/IMPLEMENTATION.md` §4 加 2026-06-19 条目
- **DEBT**:`.trellis/reviews/DEBT.md` 加 RULE-A-011
- **Journal**:`.trellis/workspace/Carlos-home/journal-2.md` 追加 summary

## Out of Scope(留待未来)

- B. 把总超时抬到 600s(LiteLLM 同款):当前 `read_timeout=60s` 已能 cover 慢代理 streaming;真要触发说明代理真的死了,这时让用户看到错误反而是对的。**暂不动**。
- C. providers / models 表加 `request_timeout_secs` 列:等真有多 provider 用户被不同代理掐脖子再上,**不在本次范围**。