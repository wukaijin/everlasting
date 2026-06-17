# Research: @文件补全的模糊匹配实现选型（Rust 后端 vs 前端 JS）

- **Query**: nucleo 现状 / Rust 替代（fuzzy-matcher / frizzy） / 前端 JS（fzf.js / fuse.js / flexsearch / fuzzysort）/ 前后端取舍 + 明确推荐
- **Scope**: external（crate / npm metadata + 官方文档） + 内部对照（TECH.md / Cargo.toml / tools/）
- **Date**: 2026-06-17

## TL;DR（明确推荐）

**前端方案：IPC 一次性拉文件列表 + 前端增量匹配，库选 `fuzzysort`（3.2 KB gzip / 0 dep / 13000 项 <1ms）。**

理由（详见 §4 权衡）：

1. **延迟**：实时键入场景下，前端本地匹配零 IPC 往返；后端方案即便 debounce 也要每次按键后串 `序列化 → IPC → Rust 匹配 → 序列化 → JS 渲染`，把"键入到面板更新"的关键路径拉长 2–10 ms 以上（IPC 序列化主导，匹配本身微秒级）。
2. **数据量**：典型项目几百～几千文件路径，序列化为 JSON 也就几十 KB～几百 KB；首次 `@` 触发时一次 IPC 拉 + 在前端 cache（+ mtime/version fence 增量刷新），后续每次按键纯本地。
3. **实现更简**：无需在 Rust 侧新增 `#[tauri::command]` + debounce 通道 + 序列化往返；前端 `fuzzysort.go(query, cachedPaths, {limit: 50})` 一行搞定，与现有 `<TriggerMenu>` 数据源注入模型一致（B3 已铺好的骨架）。
4. **nucleo 留作后续 Rust 侧能力**（如做"全局符号跳转 / 跨项目 fuzzy"这种 >10k 项或需服务多 client 的场景）再引；@文件补全不需要。

---

## 1. nucleo 现状

### 1.1 两个 crate，分清楚

| crate | 用途 | 当前版本 | 维护活跃度 |
|---|---|---|---|
| **`nucleo-matcher`** | 低层匹配器（pure matcher，无 UI / 无流式） | **0.3.1**（2024-02-20） | "core 完成，1.0 不远"，维护稳定，不活跃迭代但**不需要** |
| **`nucleo`** | 高层 picker（流式 + 多线程 + lock-free streaming） | **0.5.0**（2024-04-02） | "可用但仍会改 API"，文档欠佳 |

来源：`crates.io` API（curl + User-Agent）+ 官方 README + helix workspace `Cargo.toml` 实际 pin `nucleo = "0.5.0"`。

> README 原文："If you are looking for a replacement of the `fuzzy-matcher` crate and not a fully managed fuzzy picker, you should use the `nucleo-matcher` crate." —— **替换 `fuzzy-matcher` 一律用 `nucleo-matcher`，不要直接用 `nucleo`**。

### 1.2 维护状态

- **下游活跃使用**：helix-editor 主分支 `Cargo.toml` pin `nucleo = "0.5.0"`（核对：`https://raw.githubusercontent.com/helix-editor/helix/master/Cargo.toml`）。
- **下载量**：`nucleo-matcher` 总 240 万 / 近 90 天 80 万；`nucleo` 总 80 万 / 近 90 天 33 万。属于 Rust 生态 fzf 替代事实标准。
- **稳定性声明**：官方 README："The `nucleo-matcher` crate is finished and ready for widespread use, breaking changes should be very rare (a 1.0 release should not be far away)." —— 即便 0.x，API 冻结。
- rust-analyzer 当前**未**用 nucleo（核对 master `Cargo.toml`，无相关行）。

### 1.3 最小可用代码（`nucleo-matcher`）

来自官方 lib.rs 文档示例（`matcher/src/lib.rs`，master 分支）：

```rust
use nucleo_matcher::{Matcher, Config};
use nucleo_matcher::pattern::{Pattern, CaseMatching, Normalization};

let paths = ["foo/bar", "bar/foo", "foobar"];
let mut matcher = Matcher::new(Config::DEFAULT.match_paths()); // path 模式：路径分隔符 bonus
let matches = Pattern::parse("foo bar", CaseMatching::Ignore, Normalization::Smart)
    .match_list(paths, &mut matcher);
// 返回 Vec<(&str, score)>，按分数降序
```

API 要点：

- `Matcher` 是**可复用**的（持有预分配缓冲），多次 query 复用同一个 `Matcher` 实例。
- `Config::DEFAULT.match_paths()` 开启路径分隔符 / 文件扩展名 bonus，正是 @文件补全需要的。
- `Pattern::parse` 会把空格切分成多个 Atom（多 token AND 匹配）；要字面量用 `Pattern::new(.., AtomKind::Fuzzy)`。
- **不要**直接调 `Matcher::fuzzy_match`，效率低且坑多（文档明确警告 "Using `nucleo-matcher` directly in your ui loop will be very slow"，建议高层 `nucleo` crate——但那条建议是针对"自己实现 picker UI"，对我们做后端匹配不适用）。

### 1.4 依赖体积

`nucleo-matcher@0.3.1` 实际依赖（crates.io `/dependencies` 端点核对）：

- `memchr ^2.5` — 正常依赖
- `unicode-segmentation ^1.10` — 正常依赖（grapheme 正确性）
- `cov-mark ^1.1` — **dev-only**

纯 Rust、跨平台、无系统依赖。与现有 `memchr`（已被 reqwest 等传递引入）兼容。

---

## 2. Rust 侧替代对比

| crate | 当前版本 | 最近更新 | 90 天下载 | 算法 | 评价 |
|---|---|---|---|---|---|
| **`nucleo-matcher`** | 0.3.1 | 2024-02 | 80 万 | Smith-Waterman w/ affine gaps，fzf 同款评分 | **推荐**（如走 Rust 路线）：API 稳定、性能最强（比 skim 快 ~6×，README 基准）、helix 生产验证 |
| `fuzzy-matcher` | 0.3.7 | **2020-10**（6 年没更新） | 790 万 | SkimMatcher / Simplefuzzy / Clangd | 下载量最大但**已停滞**；`termion`（dev-only）在 Windows 上是坑；算法质量低于 nucleo；nucleo README 明确点名它是被取代的对象 |
| `frizzy` | — | — | — | — | crates.io 上**查无此 crate**（404），疑似被调研者臆测。**不要引** |

### 性能（nucleo README 基准，linux 源码全量匹配）

| Pattern | nucleo | skim (`fuzzy-matcher`) | 倍率 |
|---|---|---|---|
| `never_matches` | 2.30 ms | 17.44 ms | 7.6× |
| `copying` | 2.12 ms | 16.85 ms | 7.9× |
| `/doc/kernel` | 2.59 ms | 18.32 ms | 7.1× |
| `//.h` | 9.53 ms | 35.46 ms | 3.7× |

注意：这是 **linux kernel 全量 ~80k 文件**的基准。@文件补全典型几百～几千文件，nucleo 单次匹配**亚毫秒**；性能不是瓶颈。

### 结论

如果走 Rust 后端路线：**`nucleo-matcher`**（不是 `nucleo`、不是 `fuzzy-matcher`、没有 `frizzy`）。但请见 §4 推荐——本任务不应走 Rust 路线。

---

## 3. 前端 JS 侧对比

### 3.1 体积 & 维护（npm + bundlephobia）

| 库 | latest | 最近更新 | gzip 大小 | 0-dep | 说明 |
|---|---|---|---|---|---|
| **`fuzzysort`** | 3.1.0 | 2024-10 | **3.2 KB** | ✅ | SublimeText 风格 fuzzy；**README 自报：<1ms 搜 13000 文件**；单文件；专为"快速 fuzzy 搜很多字符串"场景设计 |
| `fzf` (npm) | 0.5.2 | **2023-04**（3 年没更） | 5.7 KB | ✅ | junegunn/fzf 算法的 JS 端口（与 Go 版 `fzf` 二进制**无关**）；维护停滞，仅 ASCII 优化 |
| `fuse.js` | 7.4.2 | 2026-06（活跃） | 9.0 KB | ✅ | 多字段 / token 搜索 / Web Worker；**面向"小到中等文档集 + 多字段加权"**，不是路径 fuzzy 的最佳形态；评分对 path 不友好（无 path-segment bonus 概念） |
| `flexsearch` | 0.8.212 | 2025-09 | 16.4 KB | ✅ | **全文检索**（倒排索引）库，**不是 fuzzy matcher**；适合"搜文档内容"，不适合"按路径模式过滤文件列表"——选型错误 |

### 3.2 适配 @文件补全场景

| 维度 | fuzzysort | fzf | fuse.js | flexsearch |
|---|---|---|---|---|
| 路径 fuzzy（`src/comp/ch/in` 匹配 `src/components/chat/ChatInput.vue`） | ✅ 原生 | ✅ 原生 | ⚠️ 能跑但无 path bonus，排序差 | ❌ 倒排索引不擅长前缀缩写匹配 |
| 13000+ 项实时过滤 | ✅ <1ms（自报） | ⚠️ 慢于 fuzzysort | ⚠️ 需 Web Worker | ✅ 索引后很快，但**建索引本身慢**且不适合"每次输入即重建" |
| 0 依赖 + 极小体积 | ✅ 3.2 KB | ✅ 5.7 KB | ✅ 9 KB | ❌ 16 KB |
| API 简洁度 | ✅ `fuzzysort.go(q, arr, {limit})` | ✅ 类似 | ⚠️ 需 `new Fuse(arr, opts)` | ❌ 需配置 IndexedDB-style 文档 schema |
| 是否高亮匹配字符 | ✅ `result.highlight()` | ⚠️ | ✅ | ❌ |

### 3.3 fuzzysort 最小代码

```js
import fuzzysort from 'fuzzysort'  // 或动态 import 按需加载

// 进入 @ 补全时一次性拿到路径列表（来自 Tauri IPC）
const cachedPaths = await invoke<string[]>('list_project_files', { cwd }) // 一次 IPC

// 每次按键（无需 debounce，fuzzysort 本身够快）
const results = fuzzysort.go(query, cachedPaths, { limit: 50, threshold: -10000 })
// results: [{ score, target, obj?, indexes? }] 已按 score 降序
// 高亮：results.map(r => r.highlight('<mark>', '</mark>'))
```

### 3.4 结论

前端首选 **`fuzzysort`**：体积最小、零依赖、为路径 fuzzy 量身设计、维护中。`fuse.js` / `flexsearch` **选型错误**（解决的是不同问题）。`fzf` npm 维护停滞，且无 path-segment bonus 概念，比 fuzzysort 弱。

---

## 4. 关键权衡（明确推荐）

### 4.1 两方案数据流

**方案 A — 前端匹配（推荐）**
```
@ 触发 ──IPC(1次)──▶ Rust 列举 currentCwd 文件树（复用 glob/list_dir + ignore）
       ◀─JSON string[]（几百～几千路径，~50–500 KB）─
前端 cache（+ 文件 mtime fence 增量刷新）

每次按键 ──▶ fuzzysort.go(query, cache, {limit:50}) ──▶ 渲染 <TriggerMenu>
          （纯本地，零 IPC）
```

**方案 B — 后端匹配**
```
@ 触发 ──IPC(1次)──▶ Rust 列举 + cache 文件树（在后端持有）

每次按键 ──IPC──▶ Rust nucleo 匹配（debounce ~30ms）
       ◀─JSON {path, score}[]──
       渲染
```

### 4.2 维度对照

| 维度 | 方案 A（前端） | 方案 B（后端） |
|---|---|---|
| 关键路径延迟（按键→面板更新） | **微秒～亚毫秒**（fuzzysort 13000 项 <1ms，无 IPC） | **2–10 ms+**（IPC marshal + nucleo 匹配 + IPC 回程 marshal；即便 nucleo 匹配本身亚毫秒，IPC 序列化主导） |
| IPC 调用次数 | **1 次**（首次 @ 触发 + 文件树变化时刷新） | **N 次**（每次按键 debounce 后 1 次，N = 用户键入字符数） |
| 大数据量传输 | 一次性传几千路径（JSON ~50–500 KB，可接受，且仅 1 次） | 每次按键传 ~50 项结果（小），但**频繁** marshal |
| 首屏冷启动 | 首次 @ 略慢（IPC 一次拉文件树，~10–50 ms 视文件数） | 同样要列文件树，**没省** |
| 实现复杂度 | **低**：前端 1 个 fuzzysort import + cache；复用 B3 `<TriggerMenu>` 数据源注入模型 | **高**：新增 `#[tauri::command]` + debounce 通道 + Rust 端持有 nucleo Matcher + 序列化往返；违反 B3 已铺的"trigger char + 数据源注入"骨架 |
| Tauri IPC 开销 | 仅一次性 | 每次按键都付 IPC 边界成本（serde + 跨进程消息） |
| Rust 端依赖 | **零新增**（不需要 nucleo / fuzzy-matcher / ignore 之外的） | +1（nucleo-matcher）+ 可能加 channel/debounce 逻辑 |
| 前端依赖 | +1（fuzzysort，3.2 KB gzip） | 无 |
| 匹配质量 | fuzzysort 评分好，有 indexes 可高亮 | nucleo 评分最优（fzf 级 + path bonus） |
| 扩展性（>10k 项、跨项目） | 几千项内无压力；上万项需 Web Worker | Rust 天然能 scale，可服务多 client |

### 4.3 决策依据（针对 B2 @文件补全的具体场景）

1. **数据规模落在前端舒适区**：典型项目几百～几千文件，fuzzysort 在 13000 项 <1ms 的自报基准足以覆盖；前端 Web Worker 都不需要。
2. **实时键入对延迟极敏感**：用户键入 `srccmpch` 期望面板**立即**重排，每次按键串 IPC 是反模式；IPC 序列化（serde_json + 跨进程）的固定开销在毫秒级，会显著拖垮手感。
3. **B3 已铺好前端骨架**：`<TriggerMenu>` 设计为"trigger char + 数据源注入"，@文件补全作为第二个 caller，最自然的就是把数据源设成"前端 fetch 后的 cached paths + fuzzysort 过滤器"，与 B3 `/command` 的数据源（builtin commands + custom commands）模型对称。方案 B 反而要绕过这个骨架。
4. **一次 IPC 拉全量可接受**：几千路径 JSON 化 ~50–500 KB，Tauri IPC 单次序列化 + 跨进程传递在 10–50 ms 量级，仅发生在**首次 @ 触发**（用户预期"打开面板"本就有感知），不在关键键入路径上。配合 mtime/version fence 做增量刷新即可。
5. **首屏冷启动两端无差**：后端方案也要列文件树，没省。

### 4.4 推荐落地

- **前端**：`pnpm add fuzzysort`，在 `@` 触发器数据源里 `invoke('list_project_files', { cwd })` 一次性拉 → 缓存（ref）→ `watch(query)` 跑 `fuzzysort.go`，结果喂 `<TriggerMenu>` 的 `items` prop。
- **Rust 侧**：新增 `#[tauri::command] list_project_files(cwd)` —— 复用 `tools/glob.rs::walk_dir` + `ignore` crate（TECH.md 候选）做 gitignore 过滤；返回 `Vec<String>`（相对路径）。**不引 nucleo**，留作后续 Rust 侧能力扩展。
- **缓存失效**：文件树变化时（notify crate 已是 TECH.md 候选 + memory 已用），刷新前端 cache；MVP 可简化为"首次 @ 拉一次 + 手动刷新按钮"，后续接 notify。

---

## Caveats / Not Found

- **基准数据均为官方自报**：nucleo 的 linux-kernel 基准来自其 README（"unscientific comparison"）；fuzzysort "<1ms 13000 文件" 来自其 README。两者都未在 Everlasting 项目内实测，但量级判断（前端足够快）即便打 10× 折扣仍成立。
- **`frizzy` 查无此 crate**：crates.io `/api/v1/crates/frizzy` 返回 404。如调研来源指的是其他 crate（如 `friz` / `fruzzy`），需澄清——目前 Rust 侧事实上只有 nucleo-matcher 值得考虑。
- **fzf npm 包身份**：npm `fzf@0.5.2` 是 junegunn/fzf 算法的 JS 端口，**与 Go 版 fzf 二进制无关**；不要混淆。
- **flexsearch 选型警示**：flexsearch 是**倒排全文检索**，不是 fuzzy matcher；适合"搜文件内容/文档"，不适合"按路径缩写过滤"。如果任务后续要做"全项目内容搜索"再考虑它。
- **未实测 Tauri IPC 单次往返成本**：基于 Tauri 文档与社区经验估为毫秒级（serde_json marshal + 消息队列入队/出队 + JS 端 unwrap），未在本项目做 micro-bench。若需精确数据，可在 PoC 阶段 `performance.now()` 包 `invoke` 测一组。
- **Web Worker 必要性**：几千项内 fuzzysort 主线程即可；若后续要支持 monorepo（>10k 文件），可切到 Web Worker 或重新评估方案 B。
