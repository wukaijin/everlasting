# L3d Subagent Frontmatter Loader

> **设计主文档**：[`docs/subagent-loader.md`](../../../docs/subagent-loader.md)（已 grill Q1-Q10 + deepseek 审查）
> **审查报告**：[`docs/_reviews/REVIEW-l3d-subagent-loader-deepseek-v4-pro.md`](../../../docs/_reviews/REVIEW-l3d-subagent-loader-deepseek-v4-pro.md)
> **路线图**：ROADMAP §2 第三档 L3d
>
> 本 task PRD = **实施工作单元定义**：记录 v1 实施范围 + 对设计 PRD 的修订决策。
> schema 字段、错误处理表、加载优先级等细节以设计 PRD 为准，此处不重复。

## Goal

让用户通过 Markdown frontmatter 文件定义自己的 sub-agent 类型；`dispatch_subagent` tool 的 enum 自动包含 builtin + user + project，LLM 能 dispatch。builtin 维持现状（2 个硬编码）。

## What I already know（auto-context 核实，行号准确）

**SubagentDef 现状**（`agent/subagent/mod.rs`）：
- `:166-176` 字段 `name/description: &'static str` + `system_prompt: String` + `tools: &'static [&'static str]`
- `:180` `builtin_subagents() -> &'static [SubagentDef]`（OnceLock）；`:128` `definition()` enum `["researcher","general-purpose"]` serde_json 字面量硬编码
- `:351` `STRUCTURALLY_DISABLED` 无条件剥离 5 工具；`:367` `filter_tools_for_subagent(all_tools, def)` 接收外部 list
- `agent/subagent/dispatch.rs:148` dispatch 时 `filter_tools_for_subagent(crate::tools::builtin_tools(), def)`

**tool list 生命周期（关键，修正审查报告 §问题9 的假设）**：
- `state.rs:200` `AppState::load()` 启动时调一次 `builtin_tools()` → 存 `state.tools: Vec<ToolDef>`（**启动快照，非每 chat 重建**）
- `chat.rs:79` 每 `invoke("chat")` `state.tools.clone()`；`chat_loop.rs:957` 每 turn `filter_tools_for_mode(tool_defs.clone(), ...)`
- → **dispatch_subagent 的 enum 当前固化在启动快照里**；要让 .md 新增 subagent 生效，必须把 dispatch_subagent 的 ToolDef 从静态 `builtin_tools()` 拆出，改 chat/turn 时从 SubagentCache 动态拼

**加载链路径约定（与设计 PRD §4.1 冲突）**：B3/B4/B5 user 层统一 `~/.config/everlasting/`（`memory/file.rs:65` `dirs::config_dir().join("everlasting")`）。设计 PRD 写的 `~/.everlasting/agents` 错。

**frontmatter parser（设计 PRD §3.3 + 审查报告都看错文件）**：
- `resource_loader.rs:140` B3 `parse_frontmatter` private + scalar-only（不支持数组）
- **`skill/loader.rs:138` Skill `parse_frontmatter` 已支持单行 inline array**（`:247` `inner.split(',')`，strip `[]`+trim+dedup，~20 行 wrapper）；`allowed-tools: [a,b]` 与 L3d 的 `tools: [a,b]` 格式一致
- 哲学（`skill/loader.rs:82-84`）：YAGNI，手写 inline array 够用，complex/multi-line 才 graduate 到 serde_yaml_neo；Cargo.toml 无 yaml 依赖

**mtime fence 现成方案**：B3 `resource_loader.rs:376` `read_through` + B4 skill loader 都用 read-through mtime fence。`current_mtimes`（`:221`）read_dir 整目录 → path→mtime 快照；快照变化（含**新增/删除**文件）即 re-scan → 新 subagent 自动进列表。

## Decisions（ADR-lite）

| # | 决策 | 理由 |
|---|---|---|
| Q1 | **mtime fence，砍 `/reload-subagents` 命令** | SubagentCache 照搬 B3 read-through mtime fence：新增/改/删 .md 在下次 chat 自动生效（目录快照变化触发 re-scan），无需手动 reload。省掉 command palette 入口 + 前端 TriggerMenu 集成 + Arc swap。代价：dispatch_subagent ToolDef 必须从 `builtin_tools()` 启动快照拆出，改 chat/turn 时从 cache 动态拼（这步 reload 方案也得做） |
| Q2 | **tools 可选，覆盖 builtin 同名时继承其 tools** | 不填 tools = 覆盖 builtin 同名→继承其 tools；全新 agent 不填→全工具集（沿用 general-purpose `tools: &[]` 约定）。"只改 system prompt"成本为 0；隐式语义靠"是否同名 builtin"区分 + 文档说明 |
| Q3 | **SubagentDef 全 owned**（`String`/`Vec<String>`） | 单一结构最干净，builtin 2 个跟着改，owned 分配成本可忽略（启动一次）。`lookup_subagent` 返回类型 + `assemble_subagent_prompt` 签名 + `filter_tools_for_subagent` 内 `iter().copied()` + 一票测试适配 |
| Q4 | **model 字段解析但 warn ignored** | 保留 schema 字段 + `tracing::warn!` 明示未生效，用户写了能看到提示，不误以为切换成功 |

**对设计 PRD 的修订**：R1 user 路径改 `~/.config/everlasting/agents/`；R2 复用 Skill inline-array parser（非 B3）；R3 删除"YAML fail-fast"伪命题错误分支；R4 = Q1 砍 reload。

## Requirements (v1)

继承设计 PRD §2.1 + 修订：
1. user 层 `~/.config/everlasting/agents/*.md` + project 层 `<project>/.everlasting/agents/*.md` 加载
2. dispatch_subagent enum 自动含 builtin+user+project + source tag（builtin/user/project）
3. 三层优先级 project > user > builtin（last-write-wins）
4. 新增/改/删 .md 下次 chat 自动生效（mtime fence，无 reload 命令）
5. per-file isolation：单个 .md 错误 silent skip + `tracing::warn!`，不阻塞其他；builtin 永远在
6. tools 可选语义（Q2）；`name` 合法字符集 `[a-zA-Z0-9_-]+`
7. 复用 Skill inline-array parser 模式（零新依赖）
8. builtin 维持 2 个硬编码（迁 .md 是 v2 OOS）
9. model 字段解析但 warn ignored（Q4）
10. 安全不变量保持：`STRUCTURALLY_DISABLED` + `filter_tools_for_subagent` 无条件剥离；worker 继承 parent Mode

## Acceptance Criteria

- [ ] `~/.config/everlasting/agents/foo.md` 写一个 → 下条消息 dispatch 能找到（source: user）
- [ ] `<project>/.everlasting/agents/bar.md` 写一个 → dispatch 能找到（source: project）
- [ ] 新增 .md 后**不发 reload 命令**，下条消息即生效（mtime fence）
- [ ] user `researcher.md` 覆盖 builtin：不填 tools → 继承 builtin researcher 的 5 tools；填了 → 用 .md 的
- [ ] 全新 `baz.md` 不填 tools → 全工具集（减 STRUCTURALLY_DISABLED）
- [ ] `tools: [web_fetxh]` 拼错 → silent skip + warn，该 .md 不进 enum
- [ ] .md 写 `dispatch_subagent` / `update_checklist` → 仍被 STRUCTURALLY_DISABLED 剥离
- [ ] `model: foo` → warn ignored，不影响加载
- [ ] `name: a/b` 非法字符 → silent skip + warn
- [ ] 单元测试：loader scan（三层合并 + 优先级）/ lookup / enum_values / parser（数组+错误类型）/ mtime fence re-scan
- [ ] `cargo test`（带 PKG_CONFIG_PATH）+ `vue-tsc --noEmit` 全绿

## Out of Scope（继承设计 PRD §2.2）

扩展字段（permissionMode/max_turns）/ Picker UI / @mention / .claude 自动加载 / notify 监听 / builtin 迁 .md / per-agent 并发 / SKILL.md 多文件 / `/reload-subagents` 命令（Q1 砍）

## Implementation Plan（小 PR）

- **PR1 — SubagentDef owned 化**（Q3，纯重构零功能变化）：`name/description: String` + `tools: Vec<String>`；builtin 2 个跟改；`lookup_subagent`/`assemble_subagent_prompt`/`filter_tools_for_subagent` 签名 + 测试适配。独立先合，降低后续 PR 噪音。
- **PR2 — loader.rs + SubagentCache**：scan（builtin+user+project 合并 + 优先级）+ read-through mtime fence（照搬 B3）+ inline-array parser（复用 skill 模式）+ per-file isolation + source tag；单元测试覆盖 AC。
- **PR3 — 集成**：dispatch_subagent ToolDef 从 `builtin_tools()` 拆出 → chat/turn 时 `definition_with_cache(&cache)` 动态拼 enum + source tag；`dispatch.rs:148` 改 `cache.lookup(name)` 替代 `lookup_subagent`；`AppState` 加 `subagent_cache`；集成测试（端到端 dispatch / 新增自动生效）。

## Technical Notes

- 新建 `agent/subagent/loader.rs`；改 `mod.rs`（re-export + builtin owned）/ `dispatch.rs`（lookup）/ `tools/mod.rs`（dispatch_subagent 拆出）/ `state.rs`（加 cache 字段）/ `chat.rs` 或 `chat_loop.rs`（动态拼 dispatch ToolDef）
- 关键集成点：dispatch_subagent 不能再留在启动快照 `state.tools` 里（enum 会固化）；改为每 turn 拼一份最新 ToolDef 追加到 turn tool list
- SubagentCache 形态：参考 B3 `CommandCache`（`RwLock<CachedScan>` + mtime fence），非设计 PRD §8.3 的 `parking_lot::Mutex` + `Arc::swap`（那是为 reload 设计的，mtime fence 不需要）

## Research References

- [`docs/subagent-loader.md`](../../../docs/subagent-loader.md) — 设计 PRD 主文档
- [`docs/_reviews/REVIEW-l3d-subagent-loader-deepseek-v4-pro.md`](../../../docs/_reviews/REVIEW-l3d-subagent-loader-deepseek-v4-pro.md) — 审查报告（方向对，但漏了 R1/R2/R4 三个跟代码契合度的硬伤）
