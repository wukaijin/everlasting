# spike-002: reqwest + Anthropic Messages API + SSE 流式

**日期**: 2026-06-04
**状态**: 通过(手写 reqwest 路径可走,GLM 兼容层有 3 处差异,见下方)
**依赖**: 无(可独立跑,跟 spike-001 并行)
**预估耗时**: 30-60 分钟 — 实际 25 分钟(build 1min + 5 个用例各 1-2min)

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

## 实际执行 / 结论 / 后续动作

### 实际执行(2026-06-04)

**环境**:
- 平台:同 spike-001(WSL 2 + Ubuntu 22.04)
- Rust:cargo 1.96.0(linuxbrew 装的,1.83 太老,已升级)
- 项目:`~/sse-spike/`,release build 1m00s

**重要前提**:本次跑的**不是真的 Anthropic Claude API**,而是 **智谱 GLM-4.7 通过 Anthropic 兼容协议**转发的服务(用户决定用 wukaijin.com 转发)。所以:
- SSE 协议理论上等价(都走 Anthropic 协议)
- model 写 `GLM-4.7`(不是 `claude-haiku-4-5`)
- 错误响应 / HTTP 状态码可能跟真 Claude 不完全一致(GLM 兼容层有差异)
- API key 是智谱风格 `sk-g4HcGHnrq...`,env 一次性注入,未落盘

**代码关键改动**(vs spike 文档里的最小测试):
- 支持 `argv[1]` 切模式:`success` / `401` / `400-too-big` / `400-empty`
- BASE_URL 从 `ANTHROPIC_BASE_URL` env 读,空时 fallback `https://api.anthropic.com`
- 事件顺序用 `Vec<String>` 记录,跑完打印,方便核对
- 未知事件打印 `▶ <name> (unhandled)`,不崩

**5 个用例实测**:

| 用例 | 期望(spike 文档) | 实际 | 评价 |
|------|------------------|------|------|
| 成功 | HTTP 200 + 6 事件顺序 + 流式中文 | ✅ HTTP 200,`message_start → ping → content_block_start → 49×content_block_delta → content_block_stop → message_delta → message_stop`,输出完整中文"Rust 的所有权是...内存管理机制..." | **完美** |
| A 401 错 key | HTTP 401 + `type: authentication_error` | HTTP 401,error=`{"code":"","message":"Invalid token (request id: 202606040746161667876668268d9d6oumb0OUc)","type":"new_api_error"}` | ⚠️ 状态码对,type 是 `new_api_error` 而非 `authentication_error`(GLM 差异) |
| B 400 max_tokens=999999 | HTTP 400 + `type: invalid_request_error` | **HTTP 200,正常 stream 50 个 delta** | ❌ GLM 不验证 max_tokens 上限,**该用例不适用 wukaijin 转发** |
| C 400 content 空串 | HTTP 400 或类似 | **HTTP 500**,error=`{"error":{"type":"invalid_request_error","message":"[1213][未正常接收到prompt参数。][20260604154619bcfec0cd2f094b81]"},"type":"error"}` | ⚠️ 状态码 500(不是 4xx),但内层 `type: invalid_request_error` 语义对 |
| D 网络断开 | 连接错误,非 JSON | `error sending request for url (...)`,被 catch → `eprintln!("❌ 网络/连接错误: {}")` + `exit(3)` | ✅ 网络层独立识别,不走错误响应 JSON 解析路径 |

**通过标准 3 项,逐条核对**:
- ✅ 能 stream 收 token(49 个 delta 完整)
- ✅ `content_block_delta` / `message_delta` / `message_stop` 顺序不乱(中间多一个 GLM 特有的 `ping` 心跳事件,无关顺序)
- ⚠️ 错误分类正确(401 / 400 / 500 / 网络 4 类可区分),**但 GLM 兼容层有 3 处差异**:
  1. 401 的 `error.type` 字段是 `new_api_error` 不是 `authentication_error`
  2. 400 类错误有时返 5xx(空消息 → 500)
  3. 不严格验证 `max_tokens` 上限(999999 通过)

### 结论

**spike-002 通过(手写 reqwest 路径可走),但需注意 GLM 兼容层 3 处差异**。

支撑的下游决策:
- 步骤 1-2 可以**手写 reqwest + SSE 解析**实施,不强制用 rig-core(避免一层抽象)
- 错误处理要写一个 "error 归一化" 层,把 `new_api_error` / `invalid_request_error` / 5xx-but-语义-4xx 等 GLM 风格都映射到内部统一错误类型(不是只靠 HTTP status code)
- 如果未来要切真 Claude API,需要重测错误分类(可能 rig-core 更省事,因为它内置 Anthropic 协议适配)

**软退路(都没走)**:
- ~~换 `eventsource-stream` crate~~:手写解析器够用,6 事件顺序 100% 对
- ~~切 rig-core~~:多一层抽象换不来实际收益(已知 GLM 兼容层差异,rig-core 也要做适配)
- ~~切 Anthropic 兼容服务~~:已经在用 wukaijin(GLM 兼容)

### 后续动作

- ✅ spike-002 通过 → 步骤 1-2 可用 reqwest 手写 SSE,不必上 rig-core
- ⏳ 步骤 1 实施时,LLM 客户端模块要:
  - 支持 BASE_URL env(便于切 wukaijin / 真 Claude / 其他)
  - 支持 model env(便于切 GLM-4.7 / Claude / 其他)
  - 错误归一化(把 GLM 兼容层差异吸收掉)
  - 未知 SSE 事件不崩(unhandled 但继续)
- ⏳ 真切到 Anthropic Claude 时,本 spike 文档要重测 4 错误用例(可能 GLM 差异是兼容层特有的,真 Claude 是 401/`authentication_error`、400/`invalid_request_error`、400 而非 500、严格 max_tokens 上限)
- 📝 把"GLM 兼容层 3 处差异"写进 `docs/HACKING-llm.md`(新建,跟 HACKING-wsl.md 配对)

---

## 关联文档

- [TECH §2 rig-core](./../TECH.md#2-决策rig-core-作为-llm-抽象层)
- [IMPLEMENTATION §2.1 步骤 1](./../IMPLEMENTATION.md#21-步骤-1--骨架与-llm-直连-mvp)
- [ARCHITECTURE §2.2 ⑥ LLM 请求 / ⑦ SSE 解析](./../ARCHITECTURE.md#2-harness-设计从用户输入到文件变更的-16-道关卡)
- [ARCHITECTURE §2.5.7 LLM Provider 限流](./../ARCHITECTURE.md#257-llm-provider-限流)
