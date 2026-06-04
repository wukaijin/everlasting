# spike-002: reqwest + Anthropic Messages API + SSE 流式

**日期**: 2026-06-XX(待填)
**状态**: 待执行 / 通过 / 失败-回退 / 失败-终止
**依赖**: 无(可独立跑,跟 spike-001 并行)
**预估耗时**: 30-60 分钟

## 目标

验证 Rust 端能调通 Anthropic Messages API、解析 SSE 流、正确分类错误。这是步骤 1 实施前的技术就绪检查,**跟 spike-001 并行**。

## 通过标准(分项独立,任一不通过可走软退路)

- ✅ 能 stream 收 token(到 stdout 可见)
- ✅ `content_block_delta` / `message_delta` / `message_stop` 顺序不乱
- ✅ 错误分类正确(401 / 429 / 400 / 网络错误)

---

## 执行步骤

### 1. 创建 Rust 项目(预估 5 分钟)

```bash
mkdir -p ~/sse-spike && cd ~/sse-spike
cargo init --name sse-spike
cargo add reqwest --features stream,json
cargo add tokio --features full
cargo add serde --features derive
cargo add serde_json
cargo add futures-util
cargo add anyhow
```

### 2. 写最小测试

替换 `src/main.rs`:

```rust
use anyhow::Result;
use futures_util::StreamExt;
use reqwest::Client;
use serde_json::json;
use std::env;

#[tokio::main]
async fn main() -> Result<()> {
    let api_key = match env::var("ANTHROPIC_API_KEY") {
        Ok(k) => k,
        Err(_) => {
            eprintln!("❌ ANTHROPIC_API_KEY not set");
            std::process::exit(1);
        }
    };

    let client = Client::new();
    let body = json!({
        "model": "claude-haiku-4-5",
        "max_tokens": 256,
        "stream": true,
        "messages": [{"role": "user", "content": "用一句话介绍 Rust 的所有权"}]
    });

    println!("📡 发送请求...");
    let resp = client.post("https://api.anthropic.com/v1/messages")
        .header("x-api-key", &api_key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await?;

    let status = resp.status();
    println!("📊 HTTP {}", status);

    if !status.is_success() {
        let err_text = resp.text().await?;
        eprintln!("❌ 错误响应: {}", err_text);
        std::process::exit(1);
    }

    let mut stream = resp.bytes_stream();
    let mut event_type = String::new();
    let mut data_buf = String::new();
    let mut token_count = 0;

    while let Some(chunk) = stream.next().await {
        let bytes = chunk?;
        let text = std::str::from_utf8(&bytes)?;

        for line in text.lines() {
            if let Some(rest) = line.strip_prefix("event: ") {
                event_type = rest.to_string();
            } else if let Some(rest) = line.strip_prefix("data: ") {
                data_buf.push_str(rest);
            } else if line.is_empty() && !data_buf.is_empty() {
                // SSE event 完成
                match event_type.as_str() {
                    "message_start" => println!("▶ message_start"),
                    "content_block_start" => println!("▶ content_block_start"),
                    "content_block_delta" => {
                        // 解析 delta
                        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&data_buf) {
                            if let Some(delta) = v.get("delta").and_then(|d| d.get("text")) {
                                print!("{}", delta.as_str().unwrap_or(""));
                                token_count += 1;
                            }
                        }
                    }
                    "content_block_stop" => println!("\n▶ content_block_stop"),
                    "message_delta" => println!("\n▶ message_delta"),
                    "message_stop" => println!("⏹ message_stop"),
                    _ => {}
                }
                data_buf.clear();
            }
        }
    }

    println!("\n✅ 完成,共 {} 个 delta", token_count);
    Ok(())
}
```

### 3. 跑成功用例

```bash
export ANTHROPIC_API_KEY=sk-ant-...
cargo run
```

**期望看到**:
- `📡 发送请求...`
- `📊 HTTP 200`
- 一段中文输出("Rust 的所有权是...")
- `▶ message_start` → `▶ content_block_start` → 多个 `content_block_delta` → `▶ content_block_stop` → `▶ message_delta` → `⏹ message_stop`
- `✅ 完成,共 N 个 delta`(N > 0)

### 4. 验证错误分类(4 个用例)

**A. 401 鉴权失败**:
```bash
ANTHROPIC_API_KEY=sk-ant-wrong-key cargo run
# 期望:HTTP 401,错误响应是 JSON 含 "type": "authentication_error"
```

**B. 400 请求过大**:
临时改 `max_tokens: 999999`,跑:
```bash
cargo run
# 期望:HTTP 400,错误响应含 "type": "invalid_request_error"
```

**C. 400 空消息**:
临时把 `content` 改成空字符串:
```bash
cargo run
# 期望:HTTP 400 或类似
```

**D. 网络断开**:
```bash
unset ANTHROPIC_API_KEY
# 改 URL 到不存在的 host
# 期望:连接错误,不是 JSON 错误
```

---

## 失败 → 走哪个回路(软,都允许)

| 现象 | 回退 |
|------|------|
| SSE event 解析乱(丢事件、重复) | 退路 1:换 `eventsource-stream` crate(标准库级 SSE 解析) |
| 401 错误描述不清 | 退路 1:忽略,实施时再细化 |
| 网络层有问题(代理 / 防火墙) | 退路 1:切 Anthropic 兼容服务(自建转发 / cloudflare worker 中转) |
| Anthropic API 协议变更 | 退路 1:跳过手写 reqwest,直接进步骤 3 用 rig-core(它内置协议适配) |
| 全部不行 | 退路 2:用 `claude` CLI / `aider` 间接调用(纯验证目的,实施时再切回直连) |

> 注:本 spike 失败**不阻塞** MVP,只决定"步骤 1-2 用手写 reqwest 还是直接用 rig-core"。

---

## 跑完后贴给 Claude

- 成功用例的 stdout 完整输出
- 4 个错误用例的 HTTP 状态码 + 错误响应 body
- 如果失败:**失败现象 + 已尝试的回退**

---

## 关联文档

- [TECH §2 rig-core](./../TECH.md#2-决策rig-core-作为-llm-抽象层)
- [IMPLEMENTATION §2.1 步骤 1](./../IMPLEMENTATION.md#21-步骤-1--骨架与-llm-直连-mvp)
- [ARCHITECTURE §2.2 ⑥ LLM 请求 / ⑦ SSE 解析](./../ARCHITECTURE.md#2-harness-设计从用户输入到文件变更的-16-道关卡)
- [ARCHITECTURE §2.5.7 LLM Provider 限流](./../ARCHITECTURE.md#257-llm-provider-限流)
