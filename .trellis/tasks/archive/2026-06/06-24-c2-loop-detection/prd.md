# C2 — Agent Loop ⑬ 循环检测

## Goal

实现 agent loop 第 ⑬ 关卡"循环检测(防死循环)"。当前 `chat_loop.rs` 里**零实现**(grep 无任何 loop/repeat/fingerprint 逻辑),唯一的"防死循环"兜底是 `MAX_TURNS=200` 粗粒度 turn 硬卡(1903 行 `max turns reached`)——在真正触发前 agent 可能已烧掉大量 token。C2 要做的是**基于行为重复的早发现机制**,在 MAX_TURNS 之前拦截 LLM 陷入"反复调同一 tool 同样参数"的死循环。

与项目"学习 harness 工程"目标高度契合:⑬ 是 ARCHITECTURE §2.2 16 关之一,架构已画好但代码未写,是典型的"补齐架构预留关卡"任务。

## What I already know

- **⑬ 是架构预留、代码绿地的关卡**:ARCHITECTURE §2.2 第 ⑬ 关 + §2.5.4"⑬ 循环检测阈值"给了算法建议,但 `chat_loop.rs` 零实现。
- **架构 §2.5.4 算法建议**:滑动窗口 N=5 次 tool call + Jaccard token-set 相似度 > 0.9 + 命中后**软提示不硬打断**(emit `warning:loop_detected` + tool_result "loop detected, please reconsider")。
- **审计边界已定(§2.5.8)**:循环检测触发次数只 `tracing::info!`,**不落 AuditKind 表**(收益低,C4 OOS)。
- **MAX_TURNS 已 50→200**(2026-06-22):粗粒度兜底,循环检测是其上层的"早发现",不取代它。
- **cancel 机制(RULE-A-004 / C1)成熟**:用户主动取消走 CancellationToken;循环检测是**系统自动检测**,两者互补。
- **B6 subagent 复用同一个 `run_chat_loop`**(nested call):⑬ 关卡加在 `run_chat_loop` 内,worker 自动继承检测(worker 有独立 `max_turns=Some(20)`)。
- **`ToolUse.input` 已是 `serde_json::Value`**(`llm/types.rs:107`):签名提取可直接 `.get("path").as_str()`,无需重新解析。
- **现有 `memory/tokens.rs::count_tokens` 不复用**:它是 async + 持 `tokio::sync::Mutex`(`CoreBPE` 是 `!Send`),且 cl100k_base 对中文切碎、subword 噪音。循环检测 token 切分独立实现,物理隔离避免 async Mutex 进热路径。

## Research References

- [`research/similarity-algorithm-and-tokenizer.md`](research/similarity-algorithm-and-tokenizer.md) — 推荐**分级触发**(Level1 精确签名硬触发 N=3 + Level2 Jaccard 软提示 N=5/0.85)取代架构单一 0.9;token 切分纯 Rust `split_whitespace + 标点剥离`;6 个高频 tool 签名提取 + 落地伪代码。

## Research Notes

### 核心结论(替代架构原文单一 0.9 阈值)

单一 Jaccard > 0.9 **偏松**:对短 input(read_file 只 1 个 path token)Jaccard 极易抖到 0.5 漏判,对长 input(shell 长命令)改一两个 flag 仍 >0.9 误报。**单一阈值无法同时适配短/长 input**。故采用分级:

- **Level 1 — 硬触发(精确重复,零误报)**:滑动窗口 N=3 内**连续 3 次 tool call 归一化签名完全相同**。真死循环几乎都是字节级相同(见下表),这一层命中率最高、误报为零。归一化 = `tool_name + 序列化 input`(key 按字母序排,路径不 canonicalize)。
- **Level 2 — 软提示(近重复,容忍误报)**:窗口 N=5 内有 ≥2 对 Jaccard token-set > 0.85。措辞更试探("loop suspected... if intentional, explain why")。

### 实际死循环形态 × 算法命中率(research §"实际中 agent 死循环长什么样")

| 死循环形态 | 频率 | L1 精确 | L2 Jaccard |
|---|---|---|---|
| 反复 read_file 同一文件(最常见) | 高 | ✅ 命 | 漏(path 只 1 token) |
| 反复 grep 同 pattern+path | 高 | ✅ 命 | ✅ 命 |
| 反复 edit_file 同文件同块(old_string 反复失败) | 中 | 半命(old_string 微调漏) | 中 |
| 反复 shell 同命令(反复 cargo check) | 中 | ✅ 命 | ✅ 命(长命令集合稳) |
| 震荡式 read A→edit A→read A | 低 | 半命 | 漏(v2 序列模式才能抓) |

**结论**:对最高频死循环(read/grep/shell 同输入),L1 结构化签名命中率最高、误报最低;L2 Jaccard 处理长 input 近重复(主要是 shell)。

### Tokenizer 选型

纯 Rust `split_whitespace + 标点剥离`(保留 `_/-.` 让 `read_file` / `/usr/local/x` 各为单 token),大小写不敏感。**不复用** tiktoken(async + CJK 切碎 + subword 噪音)。

### Caveats(research 已标)

1. "实际死循环长什么样"无线上日志统计 → implement 阶段先 `tracing::warn!` 记每次 detect 输入,跑一周校准阈值(0.85 / N=3 / N=5)。
2. 架构原文"实现位置:需 LLM 端做相似度计算"措辞有歧义 → Jaccard 是确定性计算,在 **Rust agent loop 端**算,命中后把结果作为 `tool_result` 文本回填给 LLM 才是"LLM 端"的真正语义。
3. `edit_file` 签名**不含 old_string**(允许同文件不同位置编辑,不算死循环)。
4. L1 对 read_file 同文件不同行号会漏 → 期望行为(同文件不同行不是死循环)。

## Feasible Approaches

### Approach A — 两层软提示,无硬打断(架构原意 / research 主体推荐)

- **How**:L1 硬触发 + L2 软提示两层判定,但**动作都软**——emit `warning:loop_detected` + 回填 `tool_result` 文本(L1 措辞确定 / L2 措辞试探),完全不终止 loop,靠 LLM 自我收敛,最终兜底仍是 MAX_TURNS。
- **Pros**:① 完全符合架构 §2.5.4"不强制打断";② 无状态机(不需要追踪"已提示过几次");③ 最小侵入 `run_chat_loop`;④ 与 RULE-A-010(cancel 单次即终止的 MVP 简化)风格一致。
- **Cons**:LLM 可能无视软提示继续循环,MAX_TURNS=200 兜底前仍烧 token(个人用 API key 是真金白银)。

### Approach B — 两层软提示 + 升级硬打断(更强资源保护)

- **How**:在 A 基础上,若 L1 硬触发后**再**收到完全相同的 tool call(第 4、5 次),则真终止 loop(像 `max_turns` 那样 emit terminal `Done`)。需维护"硬触发后已重复次数"状态机。
- **Pros**:① 对"LLM 无视软提示"有最终保护,省 token;② 更接近成熟 agent(Claude Code 等)的兜底语义。
- **Cons**:① 多一个 per-request 状态字段(状态机);② 终止语义要和 RULE-A-010 cancel 的"一次即终止"协调(走哪个 Done 路径);③ MVP 复杂度 +1。

### Approach C — 先观察版(只 tracing,不动作)

- **How**:先只上 detect 逻辑 + `tracing::warn!` 记录每次命中(含窗口/签名/Jaccard),**不回填 tool_result、不打断**。跑一段时间校准阈值,确认无误报后再上 Approach A/B 的动作。
- **Pros**:① 零误报风险(不影响 LLM 行为);② 直接落地 research caveat #1 的校准诉求;③ 最小变更。
- **Cons**:① MVP 期间**无实际拦截效果**(只是埋观测);② 要二次迭代才真正生效。

## Requirements (evolving)

- 新增 `agent/loop_detection.rs`:纯函数 `detect(window: &[ToolUse]) -> LoopVerdict`,含 `LoopVerdict { None, HardLoop, SoftLoop }` + `tokenize_for_jaccard` + `jaccard` + per-tool `signature_of`(6 个高频 tool 定制 + fallback)。
- 在 `run_chat_loop` 的 ⑬ 关卡位置(tool 执行后、回填 tool_result 前)调用 `detect`,维护一个 per-request 滑动窗口(`VecDeque<ToolUse>` 容量 SOFT_WINDOW=5)。
- 命中时 emit `warning:loop_detected` 事件 + 构造 synthetic `tool_result` 回填 LLM(措辞分级)。
- 不落 AuditKind(§2.5.8 已定),只 `tracing::warn!`。
- token 切分独立实现,不动 `memory/tokens.rs::count_tokens`。
- 单元测试覆盖:`detect` 的 L1/L2 命中 + 不命中边界、`signature_of` 6 个 tool、`tokenize_for_jaccard` 标点剥离 + CJK、`jaccard` 边界(空集 / 相同 / 不相交)。

## Acceptance Criteria (evolving)

- [ ] `loop_detection.rs` 纯函数 + 单测全绿
- [ ] `run_chat_loop` 接入 ⑬ 关卡,主 loop + worker(继承)均生效
- [ ] L1 硬触发:连续 3 次相同签名 → 命中
- [ ] L2 软提示:窗口内 ≥2 对 Jaccard > 0.85 → 命中
- [ ] `cargo test --lib` 全绿,无新 warning
- [ ] 现有 cancel / max_turns / parallel-tool 路径无回归(`tests_agent_loop.rs` / `tests_cancellation.rs`)
- [ ] ARCHITECTURE §2.5.4 标注"已实施" + DEBT/ROADMAP §1.2 追加 C2 条目

## Definition of Done

- 单测覆盖 detect/signature/tokenize/jaccard
- `cargo test --lib` + `cargo check` 0 warning
- 文档同步(ARCHITECTURE §2.5.4 + §2.5.8 ⑬ 行 + ROADMAP §1.2 + IMPLEMENTATION §4 ADR)
- 命中动作的语义在 spec 里固化为可执行 contract

## Out of Scope (explicit)

- **不做** 序列模式检测(震荡式 A→B→A→B,v2 / A5 序列 hash)
- **不做** 前端独立"loop detected" UI(待 Q3 决策,默认仅 tool_result 文本)
- **不做** 循环检测落 AuditKind 表(§2.5.8 已定,只 tracing)
- **不做** 阈值运行时可配(MVP 硬编码常量,校准靠改代码)
- **不**改 `memory/tokens.rs::count_tokens`
- **不**取代 MAX_TURNS=200 兜底

## Decision (ADR-lite)

**Context**: ⑬ 循环检测是架构预留、代码零实现关卡。架构 §2.5.4 给了"Jaccard > 0.9 单一阈值 + 软提示不硬打断"的建议;research 指出单一阈值无法适配短/长 input,改推荐分级触发。命中动作的"重量"和前端呈现是两个未定 scope。

**Decision**:
- **算法**:采用 research 推荐的**分级触发**(L1 精确签名硬触发 N=3 + L2 Jaccard 软提示 N=5/0.85),取代架构单一 0.9。
- **命中动作(原 Q1)**:**Approach A — 两层软提示,无硬打断**。L1/L2 命中都 emit `warning:loop_detected` + 回填 synthetic `tool_result` 文本(L1 措辞确定 / L2 措辞试探),不终止 loop,靠 LLM 自收敛 + MAX_TURNS=200 兜底。符合架构 §2.5.4"不强制打断",无状态机,与 RULE-A-010 cancel 风格一致。
- **前端 UI(原 Q2)**:**仅 tool_result 文本**,不新增前端组件/独立事件。命中提示作为 synthetic tool_result 回填,用户在 tool 卡片里看到。
- **范围**:⑬ 关卡加在 `run_chat_loop` 内,B6 subagent worker(nested run_chat_loop)自动继承检测。

**Consequences**:
- ✅ 最小侵入 `run_chat_loop`,无状态机,无前后端协议变更。
- ✅ L1 零误报抓真死循环(最高频 read/grep/shell 同输入),L2 容忍近重复(shell 长命令微调)。
- ⚠️ LLM 可能无视软提示继续循环,MAX_TURNS 兜底前仍烧 token —— **升级硬打断(Approach B)留作 follow-up**,若线上观测到"软提示后仍循环"高频出现再上。
- ⚠️ 阈值 0.85 / N=3 / N=5 无线上日志支撑 —— implement 阶段 `tracing::warn!` 记每次 detect 输入,跑一段时间校准。

## Implementation Plan (small PRs)

- **PR1** — 新增 `agent/loop_detection.rs` 纯函数 + 完整单测(`detect` / `LoopVerdict` / `signature_of` / `tokenize_for_jaccard` / `jaccard`),不接入 `run_chat_loop`。纯增量,可独立 review + 测试。
- **PR2** — `run_chat_loop` 接入 ⑬ 关卡:维护 per-request 滑动窗口(`VecDeque<ToolUse>` 容量 5),tool 执行后调 `detect`,命中 emit `warning:loop_detected` + synthetic `tool_result` 回填;+ `tests_agent_loop.rs` 加 L1/L2 命中集成用例 + cancel / max_turns / parallel-tool 无回归。
- **PR3** — 文档同步:ARCHITECTURE §2.5.4 + §2.5.8 ⑬ 行标"已实施" + ROADMAP §1.2 追加 C2 条目 + IMPLEMENTATION §4 ADR + spec(命中动作的可执行 contract)。

## Technical Notes

- 关键文件:`app/src-tauri/src/agent/chat_loop.rs`(⑬ 挂载点,~2110 行)+ 新增 `agent/loop_detection.rs`
- `ToolUse` 定义:`app/src-tauri/src/llm/types.rs:103-107`(`input: serde_json::Value`)
- 现有 token 能力(不复用):`app/src-tauri/src/memory/tokens.rs:50`(`count_tokens` async + Mutex)
- 架构基线:`docs/ARCHITECTURE.md` §2.2 ⑬(L531)+ §2.5.4(L641)+ §2.5.8(L687 "⑬ 只 tracing 不落表")
- MAX_TURNS:200(§2.5.5 L661,2026-06-22 提)
- research 伪代码已含 `LoopVerdict` enum + `detect` + `signature_of` 完整骨架,见 `research/similarity-algorithm-and-tokenizer.md` L216-263
- **edit_file 签名修正(实现时发现 research caveat #3 逻辑反向)**:`signature_of` 对 edit_file **含 old_string**(非 research 说的"不含")。理由:不含 old_string 会让正当的同文件多块编辑签名相同→误判 loop;含 old_string 才让同文件不同块编辑保持区分,同时仍能抓"反复失败同一 old_string"的真死循环。
- **tokenize 修正(实现时发现)**:research 的 `split + 保留 _/-.` 让 `--flag` / `...` 整体成 token;实现改为 `trim 首尾标点 + 保留内部连接符`(CLI flag `--` 剥离、路径 `/usr/x.rs` 保留、纯标点 `...` 丢弃)。
