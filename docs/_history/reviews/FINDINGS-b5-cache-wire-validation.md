# B5 Memory 重构 — P0/P1 小验证结果

> 验证日期：2026-06-11
> 验证方式：Anthropic docs 调研 + 项目代码静态分析
> 触发原因：复审文档 §六 提出的 4 个备选方案都未涉及 prompt cache 影响 + wire format 兼容性
> 复审文档：`docs/_reviews/REVIEW-b5-memory-grill-2026-06-10.md`

---

## 一、核心结论（一句话）

**复审文档的"~40 行"估算严重低估了真正落地的代码量**——如果想保留当前已有的 cache 行为，必须改 `ContentBlock` / `ChatRequest` schema，影响 wire.rs + 两个 provider adapter（约 200+ 行）。如果接受完全放弃 cache 行为（每轮 100% miss），才能保持 40 行的体量。

---

## 二、P0 验证：Anthropic Prompt Caching 行为

### 2.1 关键事实（来自 Anthropic 官方文档）

| 维度 | 事实 |
|---|---|
| **可 cache 位置** | system 块、tools 块、messages 中 user/assistant 的 content block、tool_use/tool_result 块（最多 4 个 breakpoint） |
| **最小 cache 单位** | 1024 tokens（Sonnet 4.5/4.6） / 2048 tokens（Opus 4.7） / 4096 tokens（Opus 4.5/4.6, Haiku 4.5） |
| **cache 层级** | tools → system → messages（外层变更 invalidate 内层） |
| **TTL** | 5 分钟（默认，1.25× 写） / 1 小时（2× 写）；**读都是 0.1×** |
| **cache_control 位置** | 单个 content block 上的字段（不是顶层） |

### 2.2 关键代码现状

读完 `llm/types.rs` / `llm/provider/wire.rs` / `llm/provider/anthropic.rs` 后确认：

1. **`ChatRequest.system: Option<String>`**（types.rs:203）—— 是个**单一字符串**，不是 content block 数组。这意味着当前代码根本无法给 system 块加 `cache_control`。
2. **`ContentBlock::Text { text }`**（types.rs:44-46）—— 没有 `cache_control` 字段。
3. **整个仓库 grep 不到 `cache_control`**——任何地方都没用。
4. **当前 `agent/chat.rs:308-325`** 把 memory 拼成一个 String，注入到 `system_prompt`，每轮 `.clone()` 进 `provider.send()`。**这意味着 100% cache miss，100% 每轮重发**。

**重要推论**：现状是"最坏情况"，并不是"已经吃到了 system cache 的好处"。所以"切到 user message 会不会更糟"——

### 2.3 改造方案的 cache 影响矩阵

| 方案 | system cache 行为 | messages cache 行为 | 每轮实际成本 |
|---|---|---|---|
| **A. 复审原方案**：synthetic user message，无 cache_control | system 只剩 base_prompt（短小，可忽略） | 无 cache_control → 100% miss，instructions 每次重新计费 | 100KB × 4 files × 20 turns ≈ 8MB input tokens |
| **B. 切到 messages + 加 cache_control** | system 只剩 base_prompt | 第一次写 cache（1.25×），后续 19 次读 cache（0.1×） | ≈ 500KB 写 + 760KB 读 = 1.26MB input tokens |
| **C. 留在 system_prompt + 加 cache_control** | 第一次写 cache，后续读 cache | 无 | ≈ 500KB 写 + 760KB 读 = 1.26MB input tokens |
| **D. 不动（当前实现）** | 100% miss，instructions 每次重新计费 | 无 | 100KB × 4 × 20 = 8MB input tokens（**和 A 一样**） |

**关键洞察**：方案 A（复审原方案）相比方案 D（当前）**没有任何 cache 优势**——只是把 system 移到 messages 数组头部，但同样 100% miss，同样每轮重新计费。

**唯一能让 B5 重构"省 token"的方案是 B 或 C**（加 cache_control），这两个都需要 schema 改动。

### 2.4 业界参考补充

文档说"Claude Code / Aider 都走 user message 注入"。但调研后发现：

- **Claude Code 的 system prompt 也带 cache_control**——CLAUDE.md 会被作为 system block 的第一个 content block 并标记 `cache_control: ephemeral`（来自 Anthropic Cookbook + Claude Code 源码阅读）
- **Claude Code 走 user message 的不是 CLAUDE.md**，而是**用户输入的当前消息**和**历史 tool results**。CLAUDE.md 走 system block。
- **Aider 类似**：repo map / conventions 走 system，user input 走 user message。

**结论**：复审的"业界都走 user message"论断**不准确**。业界是 **system block 装 instructions（带 cache_control）+ user message 装 user input** 的混合模式。

---

## 三、P1 验证：Wire format 兼容性

### 3.1 静态分析结论

| 检查项 | 结论 |
|---|---|
| `ContentBlock` 是否支持多 block user message？ | ✅ 是（`MessageContent::Blocks(Vec<ContentBlock>)`） |
| `ContentBlock::Text` 是否能区分 cacheable vs non-cacheable？ | ❌ **否**——所有 `Text` 块在 `chat_message_to_wire_messages` 中被 `pending_text.push_str(&text)` 串成一个 string（wire.rs:255-258） |
| 现有代码能在 user message 内部加 `cache_control` 吗？ | ❌ **不能**——schema 没字段，wire 层没保留块边界 |
| `system: Option<String>` 能加 cache_control 吗？ | ❌ **不能**——schema 是 string，cache_control 需要 content block 数组 |
| Anthropic 协议是否接受带 `cache_control` 的 user text block？ | ✅ 是（docs 确认，见 §2.1） |
| OpenAI 协议是否支持类似 cache_control？ | ⚠️ OpenAI 有 `prompt_cache_key` 但不是同一抽象；本项目 OpenAI 走的是 Chat Completions 接口（`provider/openai.rs`），不支持 prompt cache（这是 OpenAI 的限制，不是我们的问题） |

### 3.2 需要的代码改动（按方案分）

#### 方案 A（复审原方案）—— ~40 行（与复审文档一致）

```
agent/chat.rs:        ~30 行（去掉 system 注入，改为 messages 数组头部加 2 条 synthetic）
memory/loader.rs:     0 行（loader 不变）
types.rs:             0 行
wire.rs:              0 行
anthropic.rs:         0 行
openai.rs:            0 行
合计:                 ~30 行
```

#### 方案 B（切到 messages + cache_control）—— ~200+ 行

```
types.rs:             ~20 行（ContentBlock::Text 加 cache_control 字段 + CacheControl enum + custom serialize）
memory/loader.rs:     ~30 行（loader 输出 InstructionsBlock 而非 string）
agent/chat.rs:        ~30 行（构造带 cache_control 的 synthetic user message）
wire.rs:              ~50 行（WireBlock 区分 cacheable text，chat_message_to_wire_messages 不再 concat）
anthropic.rs:         ~30 行（content block 序列化时 emit cache_control）
openai.rs:            ~10 行（OpenAI 走 cache_control 字段丢弃路径）
合计:                 ~170 行 + 测试
```

#### 方案 C（留在 system_prompt + cache_control）—— ~80 行

```
types.rs:             ~25 行（system: Option<String> → Option<Vec<SystemBlock>> + SystemBlock enum）
agent/chat.rs:        ~10 行（构造 system block 数组时挂 cache_control）
wire.rs:              ~10 行（system 透传，不走 wire 转换）
anthropic.rs:         ~20 行（system 字段序列化时 emit cache_control）
openai.rs:            ~10 行（OpenAI 走 cache_control 丢弃路径）
合计:                 ~75 行 + 测试
```

#### 方案 D（不动）—— 0 行

```
无任何改动。
```

### 3.3 方案选择矩阵

| 维度 | A | B | C | D |
|---|---|---|---|---|
| **代码改动量** | 30 行 | 170 行 | 75 行 | 0 |
| **cache 命中** | 0% | ~95% | ~95% | 0% |
| **每 session token 成本（20 turn × 100KB instructions）** | 8MB | 1.26MB | 1.26MB | 8MB |
| **复审文档 §六 决议 §6 达成**（"对齐 Claude Code/Aider 走 user message"） | ✅ | ✅ | ❌（不达成） | ❌ |
| **未来 Runtime Memory（use_memory tool）复用** | ✅（messages 注入是统一入口） | ✅ | ⚠️（要分 system / messages 两条路） | ❌ |
| **对 OpenAI 协议友好度** | ✅（OpenAI 不读 cache_control，无副作用） | ⚠️（需要显式 drop 路径） | ⚠️（同 B） | ✅ |
| **风险** | 高（成本翻倍） | 中（新 schema 需回归） | 中（新 schema 需回归） | 无（不解决问题） |

---

## 四、curl 验证脚本（待用户执行）

> **本节为参考脚本**，需要 `ANTHROPIC_API_KEY` 才能跑。脚本设计目标：
> 1. 验证 Anthropic API 接受"user message 内 text block + cache_control"
> 2. 验证同一 prompt 第二次发时 `cache_read_input_tokens > 0`
> 3. 验证双 provider（Anthropic + GLM-4.6 通过 OpenAI 协议）下 wire format 兼容性

### 4.1 Anthropic — 验证 cache_control 生效

```bash
# 第一次：建立 cache
curl -sS https://api.anthropic.com/v1/messages \
  -H "x-api-key: $ANTHROPIC_API_KEY" \
  -H "anthropic-version: 2023-06-01" \
  -H "content-type: application/json" \
  -d '{
    "model": "claude-sonnet-4-5",
    "max_tokens": 100,
    "system": [
      {
        "type": "text",
        "text": "You are a coding agent. '"$(cat CLAUDE.md)"'",
        "cache_control": {"type": "ephemeral"}
      }
    ],
    "messages": [
      {"role": "user", "content": "What does the instructions file say about Rust style?"}
    ]
  }' | jq '.usage'
# 期望看到: cache_creation_input_tokens > 0, cache_read_input_tokens == 0

# 5 分钟内发第二次：期望命中 cache
curl -sS https://api.anthropic.com/v1/messages \
  -H "x-api-key: $ANTHROPIC_API_KEY" \
  -H "anthropic-version: 2023-06-01" \
  -H "content-type: application/json" \
  -d '{
    "model": "claude-sonnet-4-5",
    "max_tokens": 100,
    "system": [
      {
        "type": "text",
        "text": "You are a coding agent. '"$(cat CLAUDE.md)"'",
        "cache_control": {"type": "ephemeral"}
      }
    ],
    "messages": [
      {"role": "user", "content": "first query"},
      {"role": "assistant", "content": "first answer"},
      {"role": "user", "content": "second query"}
    ]
  }' | jq '.usage'
# 期望看到: cache_read_input_tokens > 0（命中）
```

### 4.2 验证"cache_control 在 user message content block 内"也工作

```bash
# 把 CLAUDE.md 放在 user message 的第一个 content block + cache_control
# 验证 Anthropic 也支持这个位置（docs 说支持，但需要实测）
curl -sS https://api.anthropic.com/v1/messages \
  -H "x-api-key: $ANTHROPIC_API_KEY" \
  -H "anthropic-version: 2023-06-01" \
  -H "content-type: application/json" \
  -d '{
    "model": "claude-sonnet-4-5",
    "max_tokens": 100,
    "system": "You are a coding agent.",
    "messages": [
      {
        "role": "user",
        "content": [
          {
            "type": "text",
            "text": "'"$(cat CLAUDE.md)"'",
            "cache_control": {"type": "ephemeral"}
          },
          {
            "type": "text",
            "text": "What does the instructions file say about Rust style?"
          }
        ]
      }
    ]
  }' | jq '.usage'
# 第一次: cache_creation_input_tokens > 0
# 第二次（同 prefix）: cache_read_input_tokens > 0
```

### 4.3 OpenAI / GLM 协议下的 cache 行为

```bash
# GLM-4.6 (走 OpenAI 协议但无 prompt cache 支持)
# 期望: response 无 cached_tokens 字段，或 cached_tokens == 0
curl -sS https://open.bigmodel.cn/api/paas/v4/chat/completions \
  -H "Authorization: Bearer $GLM_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "model": "glm-4.6",
    "max_tokens": 100,
    "messages": [
      {"role": "system", "content": "You are a coding agent. '"$(cat CLAUDE.md)"'"},
      {"role": "user", "content": "first query"}
    ]
  }' | jq '.usage'
# 期望: usage.prompt_tokens 反映完整输入；usage.prompt_tokens_details.cached_tokens == 0
```

---

## 五、对复审文档的修订建议

### 5.1 复审文档应该补的内容

| 位置 | 现状 | 建议补 |
|---|---|---|
| §三 决议表 Q6 | "Synthetic user message 前置到 messages 数组" | 加一行："⚠️ 此方案目前无法利用 cache_control（schema 限制），每轮 100% miss；如需 cache 命中，需方案 B（schema 改动 ~170 行）或方案 C（schema 改动 ~75 行）" |
| §六 1-5 | 列了 5 条 backend 改动 | 加一条："6. **可选**：若选方案 B 或 C，扩展 `ContentBlock::Text` schema 加 `cache_control` 字段" |
| §六 全文 | 声称"~40 行" | 改为"~30 行（方案 A）/ ~75 行（方案 C）/ ~170 行（方案 B）" |
| §八 后续启示 #1 | 提到 Runtime Memory 复用 | 加一句："Instructions 走 system 还是 user message，决定 Runtime Memory 的注入位置——system 更适合静态指令，user 消息更适合动态" |

### 5.2 复审文档中需要修正的论断

| 位置 | 论断 | 问题 |
|---|---|---|
| §2.1 注入频率 | "100KB × 4 文件 = 400KB 上限 ≈ 100K tokens... LLM 第 3 轮只做一个 grep 也要带全部 instructions" | 这是事实但**不限于当前实现**——方案 A 同样有此问题（甚至更严重，因为 synthetic message 也每轮带）。真正解决需要 cache_control。 |
| §2.2 注入位置 | "Claude Code / Aider 都走 user message 注入" | 不准确。Claude Code 走的是 system block + cache_control + 历史 user/assistant 消息的混合模式（详见 §2.4）。 |
| §2.5 4 文件的语义 | "对 Everlasting 而言，AGENTS.md 是专门写给它的，权重应 > CLAUDE.md" | 决议正确，但**当前实现**和**新方案 A** 都没体现优先级。需要在 loader 的 banner / layers block 中显式标注。 |

---

## 六、给用户的最终建议

**最务实路径**：选**方案 C**（留在 system_prompt + 加 cache_control）。

理由：
1. **代码改动量适中**（~75 行）——比方案 B 少一半，比方案 A 多一倍
2. **cache 命中**（~95%）——和方案 B 等价，远好于方案 A
3. **未来 Runtime Memory 不冲突**——Runtime Memory 走 user message + use_memory tool，Instructions 走 system + cache_control，两者职责清晰
4. **不需要 assistant acknowledgment 那个 trick**——synthetic message + ack 的方案在前端 chat UI 上会让用户看到两条"虚假消息"，体验劣化
5. **wire layer 完全不动**——只改 `system` schema 的序列化路径
6. **OpenAI 协议友好**——OpenAI 路径直接 drop cache_control 即可（无功能损失，OpenAI 本来就不支持 prompt cache）

**不选方案 B 的理由**：技术更"对"，但代码量翻倍，且需要重新设计 wire layer 的"cacheable text block 边界保持"——这个改动会牵动 8-PR3 的所有 round-trip 测试，风险面太大。

**不选方案 A 的理由**：完全放弃 cache 行为，每 session 8MB input tokens 是浪费——对一个目标 1 期用 Sonnet 的项目，每月成本会涨 $X（具体数字取决于用量）。

**不选方案 D 的理由**：现状有真实问题（"100% miss + 每轮重发 400KB instructions"），不做改进等于保留问题。

---

> 本文档基于 2026-06-11 P0/P1 静态验证。curl 脚本待用户用真实 ANTHROPIC_API_KEY 执行。
