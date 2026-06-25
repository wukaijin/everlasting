# Subagent Frontmatter Loader — L3d

> **状态**: 待实施(v1 范围已 grill 锁定 2026-06-25)
> **路线图位置**: [ROADMAP §2 第三档 L3d](./ROADMAP.md#2-v2-路线图分类)
> **范围**: 让用户通过 Markdown frontmatter 文件定义自己的 sub-agent 类型;builtin 维持现状
> **设计依据**: grill-me Q1-Q10,详见 [附录 A](#附录-a-q1-q10-决策摘要)

---

## 1. 背景与动机

### 1.1 当前 sub-agent 架构(2026-06-25 快照)

`app/src-tauri/src/agent/subagent/mod.rs` 已经内置 2 个 sub-agent:

- `researcher` — 只读工具集(`read_file` / `grep` / `glob` / `list_dir` / `web_fetch`)
- `general-purpose` — 全工具集(减去 `dispatch_subagent` / `update_checklist` / 3 个 L1a shell 工具)

定义形式是 Rust `&'static [SubagentDef]` 硬编码(`mod.rs:180`)。

代码注释里**已经明确预留了扩展路径**(第 153-156 行):

> "MVP ships 2 ... a future PR will load these from Markdown frontmatter (.everlasting/agents/*.md, mirroring .claude/agents/*.md)"

### 1.2 为什么做这个

**核心论点**: 用户自定义 sub-agent 类型 > 不断增加 builtin 数量。

1. **OpenCode 印证 2 个 builtin 够用** — `build`/`plan` mode + 1 个 `general` subagent 已经覆盖绝大多数场景。
2. **Claude Code 验证配置化路径** — `~/.claude/agents/*.md` frontmatter 让用户能自由扩展,是事实标准。
3. **OpenFang 给出完整参考** — `HAND.toml` + 系统 prompt + `SKILL.md` 三件套;我们不需要 `SKILL.md`(已有 memory cache 替代),但 manifest 模式值得借鉴。
4. **builtin 维护负担递增** — 每加一个 builtin = 加一个 schema enum 值 + 加一组测试 + 加一段文档;边际成本高,边际价值低。

### 1.3 行业调研参考

| 项目 | 内置 sub-agent 数 | 配置文件 | 触发方式 |
|------|------------------|----------|---------|
| OpenCode ([sst/opencode](https://github.com/sst/opencode)) | 2 + 1 (`general`) | 代码内置 | `@general` @mention |
| Hermes ([nousresearch/hermes-agent](https://github.com/nousresearch/hermes-agent)) | 不明(README 未提) | Runtime 注册 | LLM dispatch |
| OpenFang ([RightNow-AI/openfang](https://github.com/RightNow-AI/openfang)) | 7 (`Hands`) | `HAND.toml` + system prompt + `SKILL.md` | `hand activate <name>` + schedule |

**借鉴**: OpenFang 的 manifest 模式(但不需要 `SKILL.md` 也不需要 schedule);OpenCode 的极简 builtin 数量(2+1 是合理上限,印证"不加 builtin"决策)。

---

## 2. 目标与非目标

### 2.1 v1 必交付

- [ ] 用户在 `~/.everlasting/agents/*.md` 写一个 markdown 文件,定义 sub-agent;LLM 能 dispatch 它
- [ ] 用户在 `<project>/.everlasting/agents/*.md` 同上
- [ ] `dispatch_subagent` tool 的 enum 自动包含 builtin + user + project 所有 sub-agent
- [ ] 每个 sub-agent 有 source tag(`user` / `project` / `builtin`),在 tool description 中标注
- [ ] `~/.everlasting/agents/<name>.md` 覆盖 builtin 同名 sub-agent(完全覆盖,详见 §4.3)
- [ ] `<project>/.everlasting/agents/<name>.md` 覆盖 user + builtin 同名
- [ ] 提供 `/reload-subagents` 命令,当前 session 立即生效新 .md
- [ ] 单个 .md 错误不阻塞其他 sub-agent 加载(per-file isolation)
- [ ] 所有警告错误通过 `tracing::warn!` 落日志

### 2.2 明确不做(v1 OOS)

- ❌ 扩展字段(`permissionMode` / `max_turns` 等)— 推迟到 v2
- ❌ Picker UI / 命令面板 sub-agent 浏览器 — 推迟到 v2
- ❌ @mention 触发 — OpenCode 风格,推迟到 v3
- ❌ `.claude/agents/*.md` 自动加载 — 让用户符号链接即可,避免双源冲突
- ❌ Notify 监听 — sub-agent 改动频率低,启动 + reload 已覆盖
- ❌ builtin 从 Rust 迁到 .md — 破坏性变更,推迟到 v2
- ❌ Per-sub-agent 并发 / 调度策略 — L3a concurrent 分支是全局逻辑,不 per-agent
- ❌ `SKILL.md` / 多文件 sub-agent — system prompt 单文件即可

---

## 3. Schema 设计

### 3.1 字段定义(主字段对齐 Claude Code)

**Markdown 文件结构**:

```markdown
---
name: <subagent-id>
description: <when to dispatch this sub-agent>
tools: [<allowlist of tool names>]
model: <model-id>          # 可选,v1 解析但不切换
---

<system prompt — markdown body,跟 Claude Code 一致>
```

**字段说明**:

| 字段 | 必填 | 类型 | 说明 |
|------|------|------|------|
| `name` | ✅ | string | sub-agent 标识符(LLM dispatch_subagent 用),project + user + builtin 全局唯一 |
| `description` | ✅ | string | LLM dispatch 时的语义提示;写"何时用我" |
| `tools` | ✅ | string[] | 工具 allowlist;**空数组 = 全工具集**(类似 builtin `general-purpose`) |
| `model` | ⚠️ 可选 | string | 模型 id,v1 解析但**不切换**(`Provider` trait 用单实例模型) |
| body | ⚠️ 可选 | markdown | system prompt;空 body 视为空 string |

### 3.2 已知限制

- `model` 字段 v1 解析但**不切换模型** — 我们的 `Provider` trait 用单一模型实例;支持 per-call model 需要 V2 路线图独立 PR
- `description` 缺失时 fallback 到空字符串(降级,不算错误,见 §6.2)

### 3.3 解析

- 复用 `resource_loader.rs::parse_frontmatter`(B3 已落地的手写 YAML parser,零依赖)
- markdown body 用 `---` 分隔 frontmatter 之后的所有内容
- 解析失败 → 详见 §6 错误处理决策表

### 3.4 完整例子

`~/.everlasting/agents/quick-lookup.md`:

```markdown
---
name: quick-lookup
description: |
  轻量级只读代码探索,适合查 API 用法 / 函数签名 / 模块结构。
  比 researcher 更快(只走 file/grep,不发 web_fetch)。
tools: [read_file, grep, glob, list_dir]
---

You are a quick-lookup subagent. Answer the question in 1-3 sentences.
Don't run web_fetch — if you need external docs, ask the parent.
Reply in the user's language.
```

---

## 4. 加载路径 + 优先级

### 4.1 路径

两层加载,跟 [B5 Memory 加载约定](#)一致(`CLAUDE.md` 提到的 4 文件 user/project 双层模式):

| 层 | 路径 | 用途 |
|----|------|------|
| **user-level** | `~/.everlasting/agents/*.md` | 个人常用,跨项目共享 |
| **project-level** | `<project>/.everlasting/agents/*.md` | 项目专用,可提交进 git |

`<project>` 取自 session 关联的 worktree / 项目根(与 `MemoryCache` 的 project_id 同源)。

### 4.2 加载顺序 + 优先级

**加载顺序**: builtin → user → project(后加载覆盖先加载,**last-write-wins**)

**优先级**: `project > user > builtin`

| 场景 | 生效来源 |
|------|---------|
| builtin `researcher` 唯一(无 .md) | builtin |
| builtin `researcher` + user `researcher.md` | user |
| builtin `researcher` + project `researcher.md` | project |
| builtin + user + project 同名 `researcher.md` | project |
| user `foo.md` + project `bar.md`(无冲突) | user `foo` + project `bar` |

跨层冲突 → silent skip + `tracing::warn!`(见 §6.2 第 10 行决策)。

### 4.3 builtin 覆盖语义(关键 UX)

**`.md` 完全覆盖 builtin 同名 sub-agent**(包括 `tools` / `system prompt` / `model`),**不字段 merge**。

**用户写 `~/.everlasting/agents/researcher.md` 时**:
- ✅ **必须显式列全**想要的 `tools`(不能省略字段)
- ❌ 不能从 builtin 继承未填字段(没有"未填则 fallback"语义)

如果想"在 builtin 基础上扩展"(如 builtin 是 5 个 read tools,你想加 `web_fetch`),必须:
1. 复制 builtin 的 5 个 tools
2. 追加 `web_fetch`
3. 写完整 system prompt(或省略 body 走空字符串)

**示例**(`~/.everlasting/agents/researcher.md` 在 builtin 基础上加 `web_fetch`):

```markdown
---
name: researcher
description: <完整复制 builtin description>
tools: [read_file, grep, glob, list_dir, web_fetch]
model: claude-sonnet-4-6
---

<完整复制 builtin system prompt,或自定义>
```

**为什么这么设计**:
- 跟 Q5 last-write-wins 一致
- merge 语义复杂、调试难(用户改了一个字段不知道是否合并了 builtin 的其他字段)
- v2 可以加"从 builtin 继承 fields"模式,如果用户反馈复制繁琐

### 4.4 builtin 列表(v1 不变)

Rust `builtin_subagents()` 继续返回 2 个: `researcher` + `general-purpose`(详见 `agent/subagent/mod.rs:180`)。

**迁移到 .md 是破坏性变更**,推迟到 v2。

---

## 5. 前端暴露

### 5.1 v1 暴露组件(最小集合)

| 组件 | 必选 | 实现 |
|------|------|------|
| **A. `dispatch_subagent` schema enum 自动扩展** | ✅ 必选 | `builtin_tools()` 返回 `Vec<ToolDef>` 时,从 `Arc<SubagentCache>` 取最新 enum 拼接 |
| **B. 当前激活 sub-agent 来源 tag** | ✅ 必选 | tool description 末尾追加 `Available subagents: <name> (source: <tag>), ...` |

### 5.2 显式不做(v1 OOS)

- ❌ Picker UI(侧栏 / 菜单 / 浏览器)— 推迟到 v2
- ❌ 命令面板集成 `/subagents list` — 推迟到 v2
- ❌ @mention 触发 — OpenCode 风格,推迟到 v3

### 5.3 tool description 格式示例

**v1 之前**:
> "Dispatch a worker subagent to run a sub-task ... Two built-in subagents are available: `researcher` (read-only: ...) and `general-purpose` (...)"

**v1 之后**:
> "Dispatch a worker subagent to run a sub-task ... Available subagents: researcher (source: builtin), general-purpose (source: builtin), quick-lookup (source: user), db-migrator (source: project)."

LLM 看到 source tag 后知道每个 sub-agent 来自哪里(对 dispatch 决策不影响,但对 debug 有用)。

### 5.4 用户可发现性

- 用户在 IDE 写 .md 文件 = 主动行为,无需 UI 提示
- source tag 在 tool description 里 = 用户 hover 即可看
- 想"列出所有 sub-agent" — v2 加 picker UI 或命令面板

---

## 6. 错误处理(per-file isolation)

### 6.1 核心哲学: silent skip + warn

**单个 .md 错误不阻塞其他 .md 加载**。用户写 5 个 .md,第 4 个错,1/2/3/5 都还能用。

builtin 永远加载,不受 .md 错误影响。

### 6.2 错误处理决策表

| 错误类型 | 严重性 | 处理 |
|---------|--------|------|
| YAML frontmatter 解析失败 | **严重** | fail-fast 启动报错 |
| `name` 字段缺失 | **严重** | fail-fast 启动报错 |
| `name` 含非法字符(`/` `\` `:` 等) | **严重** | fail-fast 启动报错 |
| `name` 重复(同一层内) | **严重** | fail-fast 启动报错 |
| `tools` 字段值不是数组 | **警告** | silent skip 该 .md + `tracing::warn!` |
| `tools` 包含不存在的 tool 名 | **警告** | silent skip 该 .md + `tracing::warn!`(不允许"忽略未知 tool"的容错,因为会掩盖拼写错误) |
| `description` 字段缺失 | **警告** | fallback 空字符串 + `tracing::warn!` |
| `model` 字段值不识别 | **警告** | 忽略该字段值 + `tracing::warn!`(v1 不切换模型) |
| markdown body 为空 | **警告** | 空 system prompt 继续加载 + `tracing::warn!` |
| user/project 跨层 name 冲突 | **警告** | 按 §4.2 优先级覆盖 + `tracing::warn!` |
| .md 跟 builtin name 冲突 | **警告** | 按 §4.2 优先级覆盖 + `tracing::warn!` |

### 6.3 严重 vs 警告的边界

**严重错误**(fail-fast): "配置文件完全无法理解" → 必须修复才能启动
- YAML 解析失败
- 必填字段缺失(`name`)
- name 重复或非法

**警告错误**(silent skip): "配置文件可读但配置不健康" → 跳过这个 .md,其他继续
- 类型错误、字段值缺失、tool 不存在
- 跨层冲突 — 按优先级覆盖,无需用户决策

### 6.4 audit log 集成(预留)

silent skip 的 warn 事件**未来**可在 audit log 显示(走 `⑯ 审计日志` 现有 10 类 AuditKind)。
v1 先把 warn 落到 `tracing` 日志(用户用 `RUST_LOG=warn pnpm tauri dev` 看到)。
audit log UI 集成推迟到 v2(等 C4 AuditLogModal 加 `subagent_loader` 类别)。

---

## 7. 加载时机 + reload

### 7.1 加载时机

**启动一次性扫描 + 内存缓存**。

- session 启动时:`SubagentCache::scan()` 扫 builtin + user + project,合并生成 `Vec<LoadedSubagent>`
- 缓存存为 `Arc<SubagentCache>`,通过 `AppState` 共享
- `.md` 改动后需要触发 reload 才生效(详见 §7.2)

### 7.2 Reload 命令

**`/reload-subagents`** 命令触发重新加载。

实现:
1. B3 已落地 `/command` 命令面板系统(详见 `agent/commands/command_palette.rs`)
2. v1 加一条 **builtin 命令** `/reload-subagents`:
   - 调用 `SubagentCache::scan()` 重新扫描
   - 清空旧缓存 + 替换新缓存(用 `Arc::swap` 原子替换引用)
   - 触发 `dispatch_subagent` enum 重新生成(锁 + 替换 `Arc<Vec<String>>`)
   - 返回 reload 结果给前端(成功条数 / 失败条数 / 警告列表)

### 7.3 schema enum 拼接

**启动一次性 + reload 重新生成**。

- `dispatch_subagent` tool 的 enum 是 `Arc<Vec<String>>`
- 启动时生成一次;reload 时 lock + 替换引用
- dispatch 端只做 O(1) 查名字,无 IO、无解析

### 7.4 reload 时机

- 用户主动 `/reload-subagents` 触发
- **不在 dispatch 时自动 reload**(避免性能抖动)
- 不监听 .md 文件变化(notify 是 v2 候选)

### 7.5 builtin 在 reload 行为

- builtin 不需要重新加载(已经在内存里)
- reload 只扫 user + project 两个目录

---

## 8. 关键文件 / 实现要点

### 8.1 新建文件

| 路径 | 用途 |
|------|------|
| `app/src-tauri/src/agent/subagent/loader.rs` | `SubagentCache` 类型 + `scan()` / `reload()` / `lookup()` / `enum_values()` 方法 |

### 8.2 改动文件

| 路径 | 改动 |
|------|------|
| `app/src-tauri/src/agent/subagent/mod.rs` | re-export `SubagentCache`;`builtin_subagents()` 保留(builtin 不变) |
| `app/src-tauri/src/tools/mod.rs` | `builtin_tools()` 接收 `&Arc<SubagentCache>` 参数,动态拼接 `dispatch_subagent` enum |
| `app/src-tauri/src/agent/subagent/dispatch.rs` | `run_subagent` 从 `Arc<SubagentCache>` 查 subagent,不再依赖 `builtin_subagents()` 静态表 |
| `app/src-tauri/src/state.rs` | `AppState` 加 `subagent_cache: Arc<SubagentCache>` 字段 |
| `app/src-tauri/src/commands/command_palette.rs` (B3 已落地) | 加 builtin 命令 `/reload-subagents` 的 Tauri IPC 入口 |
| `app/src/components/chat/...` (chat input 或 command palette 集成) | `/reload-subagents` 前端触发(走 B3 `<TriggerMenu>` 已有的 `/` 触发器) |

### 8.3 关键数据结构

```rust
/// 加载后的单个 sub-agent 定义 + 来源 tag
pub struct LoadedSubagent {
    pub def: SubagentDef,
    pub source: SubagentSource,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubagentSource {
    Builtin,
    User,
    Project,
}

impl SubagentSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Builtin => "builtin",
            Self::User => "user",
            Self::Project => "project",
        }
    }
}

/// 缓存:所有 builtin + user + project 合并后的列表 + enum 切片
pub struct SubagentCache {
    inner: parking_lot::Mutex<SubagentCacheInner>,
}

struct SubagentCacheInner {
    /// 合并后的完整列表(按 §4.2 优先级覆盖)
    loaded: Vec<LoadedSubagent>,
    /// dispatch_subagent enum 缓存(避免每次拼接)
    enum_values: Arc<Vec<String>>,
}

impl SubagentCache {
    pub fn scan() -> Arc<Self> { ... }
    pub fn reload(&self) -> ReloadResult { ... }
    pub fn lookup(&self, name: &str) -> Option<&LoadedSubagent> { ... }
    pub fn enum_values(&self) -> Arc<Vec<String>> { ... }
}

pub struct ReloadResult {
    pub loaded_count: usize,
    pub builtin_count: usize,
    pub user_count: usize,
    pub project_count: usize,
    pub skipped: Vec<SkippedFile>,  // { path: PathBuf, reason: String }
}
```

### 8.4 builtin 保留(短期)

`mod.rs::builtin_subagents()` 继续返回 Rust 硬编码 2 个;`SubagentCache::scan()` 第一步调用它,把 builtin 加入合并列表。

迁移 builtin 到 .md 是 v2 OOS。

### 8.5 跟现有架构的集成点

| 集成点 | 改动 |
|--------|------|
| `AppHandle::state()` | 加 `subagent_cache` 字段 |
| `run_chat_loop` 闭包 | 不需要改 — `SubagentCache` 通过 `AppState` 共享 |
| `dispatch.rs::run_subagent` | 改用 `cache.lookup(name)` 替代 `lookup_subagent(name)` |
| `tools/mod.rs::builtin_tools` | 接受 `&Arc<SubagentCache>`,动态生成 enum |
| B3 command palette | 加 builtin 命令 `/reload-subagents` |

---

## 9. 测试策略

### 9.1 单元测试(`cargo test`)

| 测试目标 | 覆盖点 |
|---------|--------|
| `loader::scan` | user / project / builtin 三层合并;§4.2 优先级正确 |
| `loader::reload` | 清空重建;enum 重新生成 |
| `parse_frontmatter` (复用) | 8 种警告错误类型(§6.2 决策表逐条) |
| `loader::lookup` | 存在 / 不存在 / 跨层同名按优先级 |
| `loader::enum_values` | 去重、顺序稳定 |

### 9.2 集成测试(`cargo test --test` 或 vitest)

| 场景 | 覆盖点 |
|------|--------|
| reload 端到端 | 创建临时 user .md → 调用 reload → dispatch 能找到 |
| dispatch 端到端 | builtin + user + project 各有一个 .md → LLM dispatch 全部能成功 |
| source tag 显示 | tool description 末尾包含 `Available subagents: ... (source: builtin/user/project)` |
| builtin 被 .md 覆盖 | user 写 `researcher.md` 替换 tools → dispatch 用 user 版本,tools 列表符合 .md |

### 9.3 手工测试(开发期)

- [ ] `~/.everlasting/agents/quick-lookup.md` 实际写一个 → dispatch 成功
- [ ] `<project>/.everlasting/agents/db-migrator.md` 实际写一个 → project source tag 生效
- [ ] user 写 `researcher.md` 覆盖 builtin → dispatch 用 user 版本,tools 列表符合 .md
- [ ] `tools` 字段拼错 `web_fetxh` → silent skip,该 .md 不出现在 enum;tracing warn 可见
- [ ] `/reload-subagents` 命令 → 修改 .md 后调用 → 新 sub-agent 立即可用
- [ ] session 重启 → user .md 仍然加载(无状态丢失)

---

## 10. 风险 + 未来扩展

### 10.1 v1 已知风险

| 风险 | 概率 | 影响 | 缓解 |
|------|------|------|------|
| 用户写 `researcher.md` 不知道要复制 builtin tools | 中 | 中 | PRD §4.3 明确写;v2 加 builtin inspect 命令 |
| `.md` 解析失败阻塞启动(严重错误) | 低 | 中 | fail-fast + tracing error;用户删 .md 即可恢复 |
| reload 命令误触发 | 低 | 低 | reload 是幂等的,无副作用 |
| enum 拼接性能差(几百个 sub-agent) | 低 | 低 | enum 是 `Arc<Vec<String>>` 一次性生成;实测可忽略 |
| source tag 让 LLM dispatch 决策变复杂 | 极低 | 极低 | tag 是纯文本描述,LLM 忽略 |

### 10.2 v2 候选(用户反馈后再排)

- **`permissionMode` 扩展字段** — worker 独立声明权限模式(plan/edit/yolo),默认继承 parent
- **`max_turns` 扩展字段** — 覆盖默认 200 turn 预算
- **`SKILL.md` 多文件 sub-agent** — system prompt 分层(L0 摘要 / L1 全文 / L2 reference)
- **Picker UI** — sub-agent 浏览器(类似 Skill browser)
- **builtin 迁移** — 把 2 个 Rust 硬编码 builtin 迁到 .md 文件(`~/.everlasting/agents/researcher.md` + `general-purpose.md`)
- **`.claude/agents/*.md` 自动加载** — 双目录支持,通过符号链接或显式配置
- **AuditLogModal 集成** — subagent_loader warn 事件可视化
- **Notify 监听**(如果用户高频改 .md 反馈多)— 启动 + reload 不够用再加

### 10.3 v3+ 候选

- **@mention 触发** — OpenCode 风格,用户在消息里 @subagent
- **HAND.toml 多文件 manifest** — OpenFang 风格的"系统 prompt + tools + guardrails + schedule"分离
- **Per-sub-agent 并发配置** — L3a concurrent 分支 per-agent 化

---

## 附录 A: Q1-Q10 决策摘要

| # | 问题 | 锁定 |
|---|------|------|
| Q1 | 完成定义 | A: 仅 frontmatter 加载器,builtin 维持 |
| Q2 | schema 兼容性 | C: 主字段对齐 Claude Code + 支持扩展 |
| Q3 | 扩展字段 v1 | 0 扩展(纯 Claude Code 4 字段) |
| Q4 | 加载路径 | C: user + project 两层 |
| Q5 | 优先级 | A: project > user > builtin,last-write-wins |
| Q6 | 前端暴露 | A+B: schema enum 自动扩展 + source tag |
| Q7 | 错误处理 | A: silent skip + warn,per-file isolation |
| Q8 | 加载时机 | C: 启动扫描 + 命令面板 reload |
| Q9 | 交付物 | PRD `docs/subagent-loader.md` + ROADMAP §2 第三档 L3d |
| Q10 | 实现细节 | builtin 完全覆盖 / enum 启动一次性 / 三层测试覆盖 |

## 附录 B: 参考资料

- OpenFang `HAND.toml` manifest 模式: <https://github.com/RightNow-AI/openfang>
- OpenCode 极简 sub-agent 模型: <https://github.com/sst/opencode>
- Hermes self-improving agent: <https://github.com/nousresearch/hermes-agent>
- Claude Code `.claude/agents/*.md` 约定(参考性,待核实官方文档)
- 现有 subagent 代码: `app/src-tauri/src/agent/subagent/`
- 现有 command palette: `app/src-tauri/src/agent/commands/command_palette.rs`(B3)
- 现有 frontmatter parser: `app/src-tauri/src/resource_loader.rs`(B3)
- 现有 MemoryCache 双层加载: `app/src-tauri/src/memory/`(B5)

## 附录 C: grill-me 决策日志

> 2026-06-25 通过 grill-me skill 锁定 10 个核心设计决策,详见 [附录 A](#附录-a-q1-q10-决策摘要)。每个决策在 PRD 对应章节有明确语义。
>
> 不写入 `IMPLEMENTATION.md §4 决策日志`(那是代码实施后的 ADR 归档位置);PRD 阶段的决策放在这里,实施时如有调整再迁到 §4。
