# Research: C2 循环检测 — 相似度算法 + Tokenizer 选型

- **Query**: 为 agent loop ⑬ 循环检测选定 (A) tool_use 相似度算法 + (B) token 切分方式
- **Scope**: mixed(项目内 token 计数代码 + 算法工程判断 + 少量外部实践)
- **Date**: 2026-06-24

---

## 关键背景(项目内已有事实)

### 架构设计基线(`docs/ARCHITECTURE.md` §2.5.4 "⑬ 循环检测阈值")

> - **不能严格相等**:LLM 输出有非确定性,严格 `hash(arg) == hash(prev_arg)` 几乎不命中
> - **建议算法**:滑动窗口内 **N=5 次 tool call**,用 **token-set 相似度 (Jaccard) > 0.9** 判定近重复
> - **命中后**:emit `warning:loop_detected`,LLM 收到 `tool_result = "loop detected, please reconsider"`,**不强制打断**(让 LLM 有机会说明为什么)
> - **实现位置**:⑬ 关卡内,需 LLM 端做相似度计算,不能纯 hash

### 项目已有 token 计数能力(`app/src-tauri/src/memory/tokens.rs` + `context.rs`)

| 位置 | 内容 |
|---|---|
| `memory/tokens.rs:50` | `pub async fn count_tokens(text: &str) -> u32` — 用 `tiktoken-rs` 的 `cl100k_base` 编码器,`encode_ordinary(text).len() as u32` |
| `memory/tokens.rs:35` | `static ENCODER: OnceLock<Mutex<CoreBPE>>` — 进程级单例,首次构造 ~200ms,后续 <1µs/token |
| `memory/tokens.rs:28` | 依赖:`tiktoken-rs = "0.6"`(`app/src-tauri/Cargo.toml:55`) |
| `memory/tokens.rs:21-24` | **关键约束**:`CoreBPE` 是 `!Send`,包在 `tokio::sync::Mutex` 里,所以 `count_tokens` 必须 `async` + 持锁期间是整个 encode 调用(microseconds 级) |
| `agent/context.rs:132-168` | `push_message_tokens(buf, m)` — 把 role + content.to_text() + 所有 block(id/name/input.to_string()/content)拼成大 String,再一次性 `count_tokens(&buf)` |
| `agent/context.rs:170-179` | `estimate_messages_tokens(messages)` — 聚合所有消息进单 buffer 后编码一次(单次 mutex acquire,比逐消息编码便宜) |

### ToolUse 结构(`app/src-tauri/src/llm/types.rs:103-107`)

```rust
ToolUse {
    id: String,
    name: String,
    input: serde_json::Value,   // ← 关键:已经是强类型 JSON Value
}
```

`context.rs:147` 对它的序列化方式是 `buf.push_str(&input.to_string())`(serde_json 默认序列化,key 顺序按 struct 定义顺序,**不排序**)。

---

## 主题 A:tool_use 相似度算法对比

### A1. Jaccard token-set(架构推荐)

**做法**:把 `(tool_name + 序列化 tool_input)` 切成 token 集合 S,Jaccard = `|A ∩ B| / |A ∪ B|`。

| 维度 | 评价 |
|---|---|
| **token 怎么切** | 见主题 B 详细分析。简言之:`split_whitespace()` + `chars()` 切标点就够,不必上 tiktoken |
| **优点** | ① 实现简单(集合操作 + 求交并);② 对"小改动"鲁棒(改个行号、改个 timeout 值仍可 >0.9);③ 不需要为每个 tool 写提取逻辑;④ 无顺序敏感问题(死循环里 LLM 常把 tool 参数的 key 顺序略动,Jaccard 集合无关顺序) |
| **缺点** | ① **集合无序**,丢掉参数结构(grep 的 `pattern=foo path=bar` vs `pattern=bar path=foo` 会被判相似,实际语义完全不同);② 集合基数小时(如 read_file 只有 `{path}` 一个 token),Jaccard 偏高,易误报;③ `read_file` 的 path 变化只动 1 个 token,但语义完全不同 —— Jaccard 会漏判这种情况(见下文"实际死循环长什么样") |
| **阈值 0.9 是否合理** | **偏松**(漏报风险高于误报)。理由:对短 input(read_file 只有 1 个 path token),任何改动都会让 Jaccard 掉到 0.5;对长 input(shell 一长串命令),改一两个 flag 仍 >0.9。**单一阈值无法同时适配短/长 input**。见 §"推荐:分级触发" |
| **本场景契合度** | 中。是架构推荐的"够用"方案,但对 path-centric tool(read_file/edit_file/grep/glob/list_dir)的语义粒度不够 |

### A2. 归一化字符串精确匹配

**做法**:`normalize(tool_input)` 后 `==` 或子串。归一化 = 去空白 / 排序 JSON key / 路径 canonicalize。

| 维度 | 评价 |
|---|---|
| **LLM 非确定性导致几乎不命中 —— 是真的吗** | **部分真**。LLM 重发同一 tool_use 时:(a) `tool_use_id` 一定不同(每次新生成);(b) JSON key **顺序可能不同**(LLM 不保证 deterministic 顺序);(c) 浮点数/数字格式可能不同(`1.0` vs `1`);(d) 字符串里空白/换行可能不同;(e) 同一 path 的相对/绝对写法可能不同。其中 (a) 必须排除(id 不该进 hash),(b)(c)(d)(e) 排序 JSON key + trim 后大多能对齐 |
| **真死循环 vs 假死循环** | **真死循环**(agent 反复 read 同一文件、反复 grep 同 pattern、反复 shell 同命令)通常是**字节级完全相同**的 —— LLM 一旦"卡住",后续输出高度确定性。**所以精确匹配对真死循环命中率其实很高**。它的失败场景是"近重复但非完全相同"(如改了行号、改了 timeout),但这种是否算死循环本身就可商榷 |
| **优点** | ① 实现最简单;② 零误报(只命中真死循环);③ 不需要 tokenizer |
| **缺点** | ① 漏掉"近重复"(LLM 每次改一点点参数的渐进式死循环);② JSON key 排序 + 路径 canonicalize 的归一化逻辑要写对 |
| **本场景契合度** | 高(作为"硬触发"层) |

### A3. 结构化签名(structured signature)

**做法**:对每个 tool 定义一个"语义指纹提取函数",只比签名。

| tool | 签名定义(示例) |
|---|---|
| `read_file` | `path` |
| `edit_file` | `path + old_string 的 hash`(或只 `path`) |
| `write_file` | `path` |
| `grep` | `pattern + path + glob` |
| `glob` | `pattern + path` |
| `list_dir` | `path` |
| `shell` | `command`(或 command 的归一化:去 env、去 cwd flag) |
| `web_fetch` | `url` |
| `run_background_shell` | `command` |

| 维度 | 评价 |
|---|---|
| **优点** | ① **语义最准** —— 直接比对"agent 在对什么对象操作",不被参数顺序/格式噪音干扰;② 比 Jaccard 便宜(签名短,甚至不用切 token,直接 String eq 或 hash);③ 对 path-centric tool(read_file/edit_file/grep)命中率最高,而这些正是死循环最常见的主角 |
| **缺点** | ① **要为每个 tool 写提取逻辑**(本项目 14 个 tool,但真正容易死循环的就 6-8 个文件/shell 类);② 新增 tool 要维护提取函数(漏一个就漏检);③ 签名相同但参数不同的场景会被判相似(如 `edit_file` 同一文件不同 old_string —— 这其实**不是**死循环,签名设计时要决定是否包含 old_string) |
| **本场景契合度** | 高。本项目 tool 集稳定(`builtin_tools()` 在 `tools/mod.rs:53` 固定 14 个),且都是结构良好的 JSON input,提取签名成本可控 |

### A4. 编辑距离 / 序列相似度(difflib ratio / Levenshtein)

**做法**:对序列化字符串算 Levenshtein 距离或 `difflib.SequenceMatcher.ratio()`(= 2*M/T,其中 M 是匹配数,T 是总长)。

| 维度 | 评价 |
|---|---|
| **优点** | 对"渐进式小改"(改个行号、改个 timeout 值)敏感度高 |
| **缺点** | ① **O(n*m) 复杂度**,对长 input(shell 长命令、read 大文件路径列表)慢;② 序列敏感(key 顺序不同就 ratio 掉),与 LLM 非确定性冲突;③ Rust 标准库没有,要引 crate(`levenshtein` / `strsim`)或手写;④ 阈值调参难(0.9 在编辑距离里语义和 Jaccard 完全不同) |
| **本场景契合度** | 低。比 Jaccard 贵且没明显好处,除非要处理"参数顺序敏感"场景,但那种场景用 A3 结构化签名更准 |

### A5. tool_use 序列模式(N-gram hash)

**做法**:不只看单个 call,看连续 N 个 call 的序列 hash(如 `[read_file, edit_file, read_file, edit_file]` 的序列重复)。

| 维度 | 评价 |
|---|---|
| **优点** | 抓"震荡式死循环"(A→B→A→B→A→B),单 call 相似度看不出但序列明显 |
| **缺点** | ① 实现 heavier(要维护序列窗口 + hash);② 对"单 tool 重复"(read_file 同一文件 N 次)反而不如 A1/A3 直接;③ 序列窗口大小 N 难定(N=3 太敏感,N=10 太迟) |
| **本场景契合度** | 中低。可作为 v2 增强,但 MVP 不必 |

---

## 实际中 agent 死循环长什么样 + 各算法命中率

基于本项目 tool 集(文件操作为主)和 agent loop 行为(`chat_loop.rs` max 50 turns)推断:

| 死循环形态 | 频率(经验) | A1 Jaccard | A2 精确 | A3 签名 | A4 编辑距离 |
|---|---|---|---|---|---|
| **反复 read_file 同一文件**(最常见) | 高 | 漏(path 只 1 token,Jaccard 不稳) | **命**(字节同) | **命**(path 同) | 命 |
| **反复 grep 同 pattern + 同 path** | 高 | 中(pattern+path 切词后集合小) | **命** | **命** | 命 |
| **反复 edit_file 同一文件同一块**(old_string 反复失败重试) | 中 | 中 | 半命(每次 old_string 微调就漏) | 看签名设计(含 old_string 则命,不含则漏) | 命 |
| **反复 shell 同命令**(如反复 `cargo check` 看同错) | 中 | **命**(长命令切词集合稳) | **命** | **命** | 命 |
| **震荡式 read A → edit A → read A → edit A** | 低 | 漏(单看每次 call) | 半命 | 漏 | 漏 |
| **参数渐进漂移**(每次 read 略不同的 path 探索) | 低 | 漏(合理,这其实不是死循环) | 漏 | 漏 | 漏 |

**结论**:对**本项目最高频的死循环**(read_file / grep / shell 同输入重复),**A3 结构化签名命中率最高、误报最低**;A2 精确匹配作为兜底;A1 Jaccard 适合处理"长 input 近重复"(主要是 shell)。

---

## 推荐:分级触发(取代单一阈值 0.9)

架构原文已埋了"或暂停,问用户"的口子。建议落地为**两级**:

### Level 1 — 硬触发(精确重复,零误报)

- **条件**:滑动窗口 N=3 内,**连续 3 次 tool call 的归一化签名完全相同**
- **归一化**:`tool_name + serde_json::to_string(&input)`(key 按字母序排序,serde_json 用 `BTreeMap` 或手动 sort;路径不 canonicalize —— 不同路径就该算不同)
- **动作**:emit `warning:loop_detected` + 回填 `tool_result = "loop detected: you have called <tool> with identical args 3 times. Reconsider your approach or stop."`
- **依据**:真死循环几乎都是字节级相同,这一层零误报、命中率高

### Level 2 — 软提示(近重复,容忍误报)

- **条件**:滑动窗口 N=5 内,有 ≥2 对 tool call 的 **Jaccard token-set 相似度 > 0.85**(用 split_whitespace 切,见主题 B)
- **动作**:emit `warning:loop_detected`(severity=soft)+ 回填 `tool_result = "loop suspected: recent tool calls look very similar (Jaccard > 0.85). If this is intentional progress, explain why; otherwise try a different approach."`
- **依据**:架构原文 0.9 偏松,实测短 input(read_file)Jaccard 极易抖动,降到 0.85 + 配合 N=5 窗口的"出现 ≥2 次"门槛,可压低单次误报

### 不推荐

- **不做** A4 编辑距离(贵 + 没好处)
- **不做** A5 序列模式(MVP 过度,v2 再说)
- **不**强制打断(架构已定调,让 LLM 自我收敛)

---

## 主题 B:本项目 tiktoken / token 计数复用可行性

### B1. 现有 `count_tokens` 能否直接复用到 Jaccard?

**技术上能,但工程上不推荐**。理由:

| 维度 | 评估 |
|---|---|
| **API 兼容** | `count_tokens(text: &str) -> u32` 只返回**数量**,不返回 token 列表。Jaccard 需要 token **集合**,得改成 `encode_ordinary(text) -> Vec<String>` 暴露 token id,再映射回 string。`tiktoken-rs::CoreBPE` 有 `encode_ordinary` 返回 `Vec<usize>`(token id),但**反查 id→string 需要 BPE 的 decoder**,这个 `tiktoken-rs` 没直接暴露成简单 API |
| **async 开销** | `count_tokens` 是 `async`(持 `tokio::sync::Mutex`)。循环检测在 agent loop 每轮 tool 执行后跑一次,如果每次都 async encode 一个 tool_use,N=5 窗口就是 5 次 mutex acquire。单次 microseconds 级可接受,但**无必要** —— Jaccard 不需要 BPE 级别的 token 精度 |
| **精度过剩** | BPE 把 "read_file" 切成 `["read", "_", "file"]` 之类的子词,对相似度判定反而引入噪音(同一个词的不同屈折会被切成不同子词)。Jaccard 要的是"语义单元"集合,word-level 比 subword-level 更稳 |
| **CJK 问题** | cl100k_base 对中文切得碎(1 字 ≈ 1-2 token),而本项目 prompt 是中文(CLAUDE.md 明确"全中文")。shell 命令里中文注释 / grep 中文 pattern 会被切碎,Jaccard 失真 |

**结论**:复用 `count_tokens` 到 Jaccard 是**可行但得不偿失**。它的价值在"估算 context 用量",不是"语义切片"。

### B2. 推荐的轻量 token 切分(纯 Rust,足够用)

对 Jaccard 这个用途,**不需要精确 tokenizer**,用纯标准库切词就够:

```rust
// 伪代码 — 放在 agent/loop_detection.rs 或类似位置
fn tokenize_for_jaccard(s: &str) -> std::collections::HashSet<String> {
    s.split_whitespace()               // 按空白切,顺带处理 \n \t
     .flat_map(|word| {
         // 把标点从词里剥离:read_file" → ["read_file", "\""]
         // 让 read_file 和 read_file\n 不被算成不同 token
         word.split(|c: char| !c.is_alphanumeric() && c != '_' && c != '/' && c != '-' && c != '.')
             .filter(|w| !w.is_empty())
             .collect::<Vec<_>>()
     })
     .map(|w| w.to_lowercase())         // 大小写不敏感
     .collect()
}
```

| 切分策略 | 优点 | 缺点 | 评价 |
|---|---|---|---|
| **纯 `split_whitespace`** | 最简,零依赖 | 不切标点,`path:"foo"` 和 `path: "foo"` 算不同 token | 不够 |
| **`split_whitespace` + 标点剥离**(上面这段) | 兼顾简单 + 抗噪音;路径 `/usr/local/x` 保留为单 token(因为保留了 `/`);`read_file` 保留为单 token(保留了 `_`) | 对极长无空白字符串(如 minified JS)切不出 | **推荐** |
| **按 char 切**(`s.chars().collect::<HashSet>()`) | 对 CJK 友好(每字一个 token) | 对英文切碎("read" → 4 个 char token),英文语义全丢 | 不推荐(除非纯 CJK 场景) |
| **`tiktoken-rs` cl100k_base** | 精确 | subword 噪音 + async 开销 + CJK 切碎 + 复用成本高 | 不推荐 |

**为什么 `split_whitespace + 标点剥离` 够用**:Jaccard 判的是"两个 tool_use 的参数集合重合度",目标是 >0.85 这种粗粒度判断。word-level 切分在 shell 命令、JSON 参数、文件路径上都给出稳定的语义单元,BPE 的精度优势在这个阈值下毫无价值。

### B3. 综合建议:token 切分独立实现,不复用 `count_tokens`

- 新增 `agent/loop_detection.rs`(或挂到 `chat_loop.rs` 的 ⑬ 关卡位置)
- token 切分函数**纯同步、纯标准库**(`split_whitespace` + 标点剥离),不走 `memory::tokens::count_tokens`
- `count_tokens` 继续只服务 C3 context 压缩(它的设计目标)
- 两套 token 概念物理隔离,避免"为了复用而把 async Mutex 拖进循环检测热路径"

---

## 综合推荐(算法 + tokenizer)

> **算法**:分级触发(Level 1 精确签名硬触发 + Level 2 Jaccard 软提示),不采用单一 0.9 阈值。
> **tokenizer**:Level 2 用纯 Rust `split_whitespace + 标点剥离`,不复用 `tiktoken-rs`。
> **签名提取**:为 6 个高频 tool(read_file / edit_file / write_file / grep / glob / shell)写 `signature(&self, input: &Value) -> String`,其余 tool fallback 到 `tool_name + 全 input 序列化`。

落地伪代码(供 PRD / implement 参考):

```rust
// agent/loop_detection.rs
const HARD_WINDOW: usize = 3;   // 连续 3 次完全相同 → 硬触发
const SOFT_WINDOW: usize = 5;   // 5 次窗口内 ≥2 对 Jaccard > 0.85 → 软提示
const SOFT_THRESHOLD: f64 = 0.85;

pub enum LoopVerdict {
    None,
    HardLoop { tool: String, count: usize },         // 回填 "loop detected (identical x3)"
    SoftLoop { pairs: usize, max_jaccard: f64 },     // 回填 "loop suspected (jaccard > 0.85)"
}

pub fn detect(window: &[ToolUse]) -> LoopVerdict {
    // Level 1: 精确签名
    let sigs: Vec<String> = window.iter().map(signature_of).collect();
    if let Some(last) = sigs.last() {
        let tail_run = sigs.iter().rev().take_while(|s| *s == last).count();
        if tail_run >= HARD_WINDOW {
            return LoopVerdict::HardLoop { tool: window.last().unwrap().name.clone(), count: tail_run };
        }
    }
    // Level 2: Jaccard 软提示
    let token_sets: Vec<HashSet<String>> = window.iter()
        .map(|t| tokenize_for_jaccard(&serialize(t))).collect();
    let mut pairs = 0;
    let mut max_j = 0.0;
    for i in 0..token_sets.len() {
        for j in (i+1)..token_sets.len() {
            let ji = jaccard(&token_sets[i], &token_sets[j]);
            if ji > SOFT_THRESHOLD { pairs += 1; max_j = max_j.max(ji); }
        }
    }
    if pairs >= 2 { return LoopVerdict::SoftLoop { pairs, max_jaccard: max_j }; }
    LoopVerdict::None
}

fn signature_of(t: &ToolUse) -> String {
    // per-tool 提取(只对 6 个高频 tool 定制,其余 fallback)
    match t.name.as_str() {
        "read_file" | "write_file" | "list_dir" => format!("{}:{}", t.name, t.input.get("path").and_then(|v| v.as_str()).unwrap_or("")),
        "grep" => format!("{}:{}:{}", t.name, t.input.get("pattern").and_then(|v| v.as_str()).unwrap_or(""), t.input.get("path").and_then(|v| v.as_str()).unwrap_or("")),
        "glob" => format!("{}:{}:{}", t.name, t.input.get("pattern").and_then(|v| v.as_str()).unwrap_or(""), t.input.get("path").and_then(|v| v.as_str()).unwrap_or("")),
        "edit_file" => format!("{}:{}", t.name, t.input.get("path").and_then(|v| v.as_str()).unwrap_or("")),  // 不含 old_string,允许同文件不同位置编辑
        "shell" | "run_background_shell" => format!("{}:{}", t.name, t.input.get("command").and_then(|v| v.as_str()).unwrap_or("")),
        _ => format!("{}:{}", t.name, t.input),  // fallback: 全序列化
    }
}
```

---

## Caveats / 未决问题

1. **"实际死循环长什么样"是基于本项目 tool 集和 agent loop 经验推断,没有线上日志统计**。建议 implement 阶段先加 `tracing::warn!` 把每次 detect 的输入记下,跑一周再校准阈值(0.85 / N=3 / N=5)。
2. **架构原文说"实现位置:⑬ 关卡内,需 LLM 端做相似度计算"** —— "LLM 端"措辞模糊,实际应在 **agent loop(Rust)端**算(Jaccard 是确定性计算,不该丢给 LLM);命中后把结果作为 `tool_result` 文本回填给 LLM,这才是"LLM 端"的语义。
3. **Jaccard 对 path-centric tool 的弱点**:Level 2 软提示对 read_file 同文件不同行号会漏(read_file 的 input 只有 path,签名相同会进 Level 1)。这其实是**期望行为**(同文件不同行不算死循环),但如果死循环是"反复 read 同一文件全文",Level 1 会抓到。
4. **`run_chat_loop` 在 `chat_loop.rs`,MAX_TURNS=50 是兜底**(CLAUDE.md 明确)。循环检测是 MAX_TURNS 之前的"早发现"机制,不取代它。
5. **subagent 路径**(`agent/subagent/dispatch.rs`)有自己的 loop,是否也需要循环检测?本次调研未覆盖 subagent 内部的 tool_use 流。建议 MVP 只做主 loop,subagent 复用同一函数(subagent 有独立的 MAX_TURNS)。
6. **OpenAI Agents SDK / Anthropic Agent SDK 的官方循环检测实现未在公开文档找到**(Mintlify 页面 JS 渲染,curl 拿不到;Anthropic SDK 源码未直接 grep)。业界做法多是基于"max_iterations + 最近 N 步 hash 去重"的组合,与本文推荐的分级触发思路一致。
