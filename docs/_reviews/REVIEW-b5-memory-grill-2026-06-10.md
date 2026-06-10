# B5 Memory 设计复审 — grill 诊断报告

> 复审日期：2026-06-10
> 复审方式：grill-me 逐题访谈（9 题全决议）
> 复审范围：`app/src-tauri/src/memory/` 全部代码 + agent loop 注入点 + 前端 `Memory*` 组件 + `.trellis/spec/backend/memory.md`
> 触发原因：B5 实现完成但感觉"Memory 的设计思路不太对劲"

---

## 一、核心诊断

**一句话：B5 实现的不是 Memory，是 System Instruction Injection。**

当前代码做了三件事：
1. 读 4 个固定 Markdown 文件（CLAUDE.md + AGENTS.md × User + Project）
2. 把全文拼到 `system_prompt` 头部
3. **每轮** agent loop（最多 20 轮）重复注入

这里的根本矛盾是：**"Memory" 天然是动态的、可查询的、按需的，但 B5 实现的是静态的、强制推送的、每轮重复的指令注入。**

把它跟 `build_system_prompt()` 里硬编码的 "You are a coding agent..." 对比——两者本质上是同一件事（给 LLM 的持久化指令），只不过一个硬编码在 Rust 里，一个从文件读。

---

## 二、发现的问题（逐项）

### 2.1 注入频率：每轮都发 → 浪费 token

```rust
// app/src-tauri/src/agent/chat.rs:304-352
let system_prompt = memory_banner + memory_layers_block + base_prompt;  // 构造一次
for turn in 1..=MAX_TURNS {
    provider.send(Some(system_prompt.clone()), messages, tools);  // 每轮 clone
}
```

100KB × 4 文件 = 400KB 上限 ≈ 100K tokens。在 200K 窗口占 50%。LLM 第 3 轮只做一个 `grep` 也要带全部 instructions。

**应该**：只注入首轮，后续轮次 instructions 留在 history 中自然携带。

### 2.2 注入位置：system prompt 前缀 → 与行业反着走

| 工具 | 注入位置 | 每轮重复？ |
|---|---|---|
| **Claude Code** | User message（messages 数组） | 否，一次注入留 history |
| **Aider** | User message（messages 数组） | 否，一次注入留 history |
| **Everlasting（当前）** | System prompt 前缀 | **是，每轮 clone** |

两个主要工具都走 user message → messages 数组 → 一次性注入。Everlasting 是唯一走 system prompt + 每轮重复的。

### 2.3 概念膨胀：把 Instructions 当 Memory 做

4 层 Memory 设计（User / Project / Session / Runtime）是合理的整体框架，但 B5 把前 2 层做成了"把文件全文灌进 prompt"，而不是"Agent 可以主动查询的知识库"。

- **Instructions**（CLAUDE.md / AGENTS.md）：静态，用户/项目给 agent 的持久化指令
- **Memories**（未来的 Runtime 层 SQLite + FTS5）：动态，agent 在对话中通过 `use_memory` tool 主动读写

B5 是 Instructions，不是 Memories。PRD 说 "V2 2 期才会做 `use_memory` tool + FTS5"，但即使还没做到那一层，也不该把 Instructions 的行为做成 Memory 的样子。

### 2.4 4 文件的语义一直模糊

CLAUDE.md 和 AGENTS.md 在当前代码中**完全等价**——都走 `load_layer` 读进来、拼到 prompt，没有任何语义区分。

但用户的心智是：
- `CLAUDE.md` → Claude Code 的指令文件
- `AGENTS.md` → Reasonix (Everlasting) 的指令文件

两者存在是为了**工具互操作**：同一个 repo 下 Claude Code 读 CLAUDE.md，Everlasting 读 AGENTS.md，切换工具时指令不丢失。

**当前代码没有体现 AGENTS.md 的优先级**——对 Everlasting 而言，AGENTS.md 是专门写给它的，权重应 > CLAUDE.md。

### 2.5 Watcher + debounce 存在隐形 race

```rust
// watcher.rs: 收到事件 → 标记 pending → 1s debounce → invalidate
```

但 `load_for_session` 和 `apply_invalidation` 之间没有同步。用户在 debounce 窗口内发消息，可能读到旧缓存。虽然窗口仅 1s，但"编辑器保存 → 下一条消息立即生效"的承诺是概率性的。

---

## 三、决议记录（9 题，逐题确认）

| # | 问题 | 决议 | 理由 |
|---|---|---|---|
| 1 | 这到底是不是 Memory？ | **否，是 System Instruction Injection。** 拆成 Instructions（静态文件）+ Memories（未来 Runtime 层），C 方案。 | 功能语义和实现不匹配 |
| 2 | Instructions 每轮注入还是只首轮？ | **只首轮。** 注入到 messages 数组，后续轮次 history 自然携带。 | 省 token，对齐 Claude Code/Aider |
| 3 | 4 文件（CLAUDE.md + AGENTS.md × 2）还成立吗？ | **保留 4 文件。** CLAUDE.md / AGENTS.md 是工具互操作桥梁，不是冗余。 | 用户用 Claude Code 和 Everlasting 切换时指令不丢失 |
| 4 | 两个文件冲突时谁优先？ | **AGENTS.md 优先。** 注入时 AGENTS.md 放前面、标注 `<primary instructions>`；CLAUDE.md 标注 `<reference>`。 | AGENTS.md 是专门写给 Reasonix 的 |
| 5 | Watcher 变更后怎么通知 LLM？ | **下一条 user message 生效。** Agent loop 中途不打断（选 A）。 | 对齐 Claude Code（编辑 CLAUDE.md 需 `/compact` 或新 session） |
| 6 | Instructions 怎么注入才"只首轮"？ | **Synthetic user message 前置到 messages 数组。** `system_prompt` 只保留轻量基础指令。 | 与 Claude Code/Aider 对齐；未来 Runtime Memory 的注入也走 messages |
| 7 | 业界参考验证？ | **Claude Code 和 Aider 都走 user message 注入，一次注入留在 history。** Everlasting 当前做法与业界背道。 | 调研确认（见 `sa_20260610_180411_000000000_69d6ac1058f0`） |
| 8 | In-place 重构 vs 新 task？ | **A：In-place 重构。** 改 `agent/chat.rs` 注入逻辑 + `memory/loader.rs` 去掉每轮假设，其他不动。 | B5 代码质量没问题，只是注入位置/频率不对；改动 ~40 行 |
| 9 | 命名策略？ | **C：用户面保留 "Memory"，内部加 instructions 前缀，前端显示文本调整。** 功能名（Tab/aria-label）保留 "Memory" 对齐 Claude Code；文件本体叫"指令文件"；注入行为描述从"每轮上下文构造"改为"session 启动时注入"。 | 最小改动量，用户零迁移成本，内部语义清晰 |

---

## 四、前端显示文本变更清单

> 原则：功能名 "Memory" 不动（Tab 标签、aria-label），文件/行为描述改称"指令文件"。

| 文件 | 当前 | 改为 |
|---|---|---|
| `ChatPanel.vue` button title | `查看项目 memory (CLAUDE.md / AGENTS.md)` | `查看项目指令文件 (CLAUDE.md / AGENTS.md)` |
| `MemoryModal.vue` DialogTitle | `Project Memory` | `项目指令文件` |
| `MemoryPreview.vue` headerTitle(user) | `User Memory` | `用户指令文件` |
| `MemoryPreview.vue` headerTitle(project) | `Project Memory` | `项目指令文件` |
| `MemoryPreview.vue` headerTitle(all) | `Memory` | `指令文件` |
| `MemoryPreview.vue` headerHint(all) | `所有 memory 文件(2 层 × 2 文件)` | `用户 + 项目,共 4 个指令文件` |
| `MemoryPreview.vue` error | `Memory 暂不可用:...` | `指令文件暂不可用:...` |
| `MemoryPreview.vue` empty | `请先选择一个项目以查看 memory 文件。` | `请先选择一个项目以查看指令文件。` |
| `MemoryPreview.vue` loading | `加载 memory 文件中…` | `加载指令文件中…` |
| `MemoryPreview.vue` empty-layer | `该层下没有 memory 文件。` | `该层下没有指令文件。` |
| `MemoryPreview.vue` footer | `Memory 文件每 1 秒自动监听变更; 新建 memory 文件需重启 session 生效。` | `指令文件每 1 秒自动监听变更; 新建文件需重启 session 生效。` |
| `MemoryTab.vue` intro | `您的个人 memory 文件 — 由 agent 在每个 chat 请求的 ⑤a 上下文构造 阶段自动加载...` | `您的个人指令文件 — 在 session 启动时自动注入到对话上下文中...` |

---

## 五、不变的部分

以下不动：

- **Rust `memory/` 目录名** — 保留为 Memory 系统的顶层模块（未来 Session/Runtime 层也在此模块下）
- **Rust types** — `MemoryKind` / `MemorySource` / `MemoryLayer` / `MemoryCache` 等名称不动（类型体系是稳定的）
- **Tauri IPC** — `read_memory_layers` / `read_memory_content` / `open_memory_in_editor` 命令名不动
- **前端 Pinia store** — `useMemoryStore` 名不动
- **前端组件名** — `MemoryPreview.vue` / `MemoryModal.vue` / `MemoryLayerItem.vue` / `MemoryTab.vue` 文件名不动
- **CSS class** — 所有 `.memory-*` 前缀不动
- **Settings Tab 标签** — "Memory" 不动
- **ChatPanel 按钮 aria-label** — "Memory" 不动
- **`.trellis/spec/backend/memory.md`** — 文件名不动
- **`notify` watcher** — 监听逻辑整体不动

---

## 六、需要改的部分

### Backend（~40 行）

1. **`agent/chat.rs:304-325`**：Instructions 注入位置从 `system_prompt` 前缀改为 `messages` 数组头部（两条 synthetic message：instructions 内容 + assistant acknowledgment）
2. **`agent/chat.rs:342-352`**：去掉 `system_prompt.clone()` 中携带 instructions 的部分（去掉即实现"只首轮"）
3. **`memory/loader.rs`**：`build_banner` 和 `build_layers_block` 保留但调用点从 agent loop 移到 messages 构造阶段；函数签名不变
4. **注入优先级**：`AGENTS.md` 在 banner 中标注 `<primary>`，`CLAUDE.md` 标注 `<reference>`；AGENTS.md 的 block 排在 CLAUDE.md 前面
5. **函数名加 instructions 前缀**（可选，约 10 个 identifier）：如 `load_instructions_for_session`、`build_instructions_injection`

### Frontend（12 处显示文本，见 §四）

---

## 七、为什么不在 B5 就做对

B5 PRD 的 grill-with-docs 阶段（9 题）已经锁定了范围，但当时的问题集中在"怎么做 Memory 的 UI、Watcher、Token 计数"，没有先问"这真的是 Memory 吗？"。

这暴露了一个 **grill 流程的薄弱点**：PRD 阶段的问题偏向实现细节（文件名、加载时机、UI 入口），缺少概念层面的质疑（"你实现的东西和你叫它的名字匹配吗？"）。

---

## 八、对后续的启示

1. **Instructions 和 Memories 是两个系统，不是同一系统的前后两个版本。** V2 2 期做 Runtime Memory 时，应该作为独立模块（或 `memory/` 子模块），复用文件路径解析和 token 计数，但不复用注入逻辑（Instructions 是首轮注入到 messages，Memories 是 `use_memory` tool 按需查询）。
2. **Grill PRD 时优先问"是什么"，再问"怎么做"。** 本次复审发现 PRD 阶段跳过了概念验证环节。
3. **业界参考是 PRD 的必选步骤。** 如果 B5 PRD 阶段先查了 Claude Code 对 CLAUDE.md 的注入方式（user message，一次注入），就不会设计出"system prompt 每轮 clone"的方案。

---

## 九、后续行动

参见此诊断得出的实施计划：in-place 重构 B5 memory → instructions injection，composite of §六所有改动。

---

> 本报告基于 2026-06-10 grill-me interview，9 题全决议。代码状态为 B5 archived 后的 main 分支。
