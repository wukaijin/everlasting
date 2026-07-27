<!-- Spec for builtin workflow plugins. Captured from 07-26-review-plugin-pack (C3) +
     07-09-workflow-builtin-plugin. Read before adding a new builtin workflow plugin
     or touching builtin.rs / the two builtin_plugin_* loaders. -->

# Builtin Workflow Plugin 内置化机制

> **Source**: `.trellis/tasks/archive/2026-07/07-09-workflow-builtin-plugin` (dev plugin 内置化) +
> `.trellis/tasks/07-26-review-plugin-pack` (C3, review plugin 内置化)。
>
> **何时读本文**:新增 builtin workflow plugin、改 `builtin.rs`、改 `skill/loader.rs::builtin_plugin_skills` 或
> `agent/subagent/loader.rs::builtin_plugin_agents`、或排查「新 plugin 在 PluginSelect 不可见 / skill 没加载 /
> agent 角色没生效」时。

## 机制概览

Builtin workflow plugin = 编译期内置进二进制的 workflow 内容包（workflow.json + agents + skills），
作为项目层（`.everlasting/workflow/<name>/`）缺失时的 fallback。优先级链：
`project-plugin > builtin-plugin > project > user`（见 `agent/subagent/loader.rs` Source 枚举）。

内容用 `include_str!` 在编译期读入（非 Tauri `bundle.resources` —— 本项目零 resource bundle 先例，
`include_str!` 是硬编码常量惯例的自然延伸，dev 构建即用，无运行时 resource_dir 解析依赖）。

## 文件布局（source of truth）

```
app/src-tauri/resources/builtin-workflow/<plugin>/
├── workflow.json
├── agents/<role>.md
└── skills/<skill-slug>/SKILL.md
```

**source of truth = builtin 源目录**。项目示例 `.everlasting/workflow/<plugin>/` 是 byte-identical 镜像
（人工同步，不写脚本 —— 同 dev 约定）。改内容先改 builtin 源，再同步镜像。

## 新增一个 builtin plugin 必须改的清单（N 处，缺一不可）

> ⚠️ 这是 07-09 dev 内置化时留下的、C3 (review) 又踩到的坑：**两处 loader 曾硬编码 `if workflow_name != "dev"`**。
> 加新 plugin 时务必同步扩这两处 match，否则 plugin 在 PluginSelect 可见（`list_plugins` 发现）但 skill/agent 不加载。

| # | 文件 | 改动 |
|---|---|---|
| 1 | `resources/builtin-workflow/<plugin>/` | 新建 workflow.json + agents/*.md + skills/*/SKILL.md |
| 2 | `agent/workflow/builtin.rs` | 加 `BUILTIN_<PLUGIN>_WORKFLOW_JSON` + `BUILTIN_<PLUGIN>_SKILLS` + `BUILTIN_<PLUGIN>_AGENTS` 常量组（照 `BUILTIN_DEV_*` 模式） |
| 3 | `agent/workflow/builtin.rs` | `BUILTIN_PLUGIN_NAMES` 追加 `"<plugin>"` |
| 4 | `agent/workflow/builtin.rs` | `builtin_workflow_json(name)` match 加 `"<plugin>" => Some(BUILTIN_<PLUGIN>_WORKFLOW_JSON)` |
| 5 | `agent/workflow/mod.rs` | re-export 新的 `BUILTIN_<PLUGIN>_*` 常量（照 `BUILTIN_DEV_*` 的 re-export 行） |
| 6 | `skill/loader.rs::builtin_plugin_skills` | 从 `if != "dev"` / match 扩成覆盖新 plugin，返回对应 `BUILTIN_<PLUGIN>_SKILLS` |
| 7 | `agent/subagent/loader.rs::builtin_plugin_agents` | 同 #6，返回 `BUILTIN_<PLUGIN>_AGENTS` |
| 8 | `agent/workflow/builtin.rs` tests mod | 加 3 个单测（照 dev 模式）：workflow.json 过 `validate()` + skills body 非空 + frontmatter 含 name |
| 9 | `.everlasting/workflow/<plugin>/` | 同步 byte-identical 镜像 |

**清单 #6/#7 是隐藏依赖**：`list_plugins`（def.rs）只读 `BUILTIN_PLUGIN_NAMES`（#3），所以 plugin 名会出现在 PluginSelect；
但 skill/agent 加载走单独的 `builtin_plugin_skills` / `builtin_plugin_agents`，若没扩这两处，plugin 可选但内容空。
C3 的 review 就是因为 07-09 把这两处写死 "dev" 而必须扩 match。

## 强约束（loader / frontmatter 要求）

- **skill slug == 子目录名 == frontmatter `name:` 字段**（三者必须一致；skill loader 用目录名做 fallback name，
  frontmatter name 做正式 name）。C3 review 的单测断言了这点。
- **agent role_name == frontmatter `name:` 字段**（agent loader 要求显式 name）。
- **frontmatter fence 必须是 `---\n`**（dev/review skills 均用 `---\nname: ...\n...`）。
- **agent frontmatter `model:` 留空 / 缺省**当该角色靠 per-dispatch override 主导模型时（如 review 的 reviewer）。
  C3 review 的单测 programmatically 断言 reviewer.md 无 `model:` 实键。
- **workflow.json 必须过 `validate()`**（def.rs:548）。`states` 数组无重复；`transitions` 的 from/to 必须在 states 里；
  空 `roles_by_state` 合法；回环 transition（from==某 state 已出现过）合法（review 的 revising→reviewing 依赖此）。
  C3 design.md §1 起草时手抖把 "reviewing" 写了两次，靠 `validate()` + 单测兜底 —— **永远靠单测兜底 states 唯一性**。

## 验证命令

```bash
cd app/src-tauri
cargo build --lib                          # include_str! 路径正确
cargo test --lib builtin                   # 新 plugin 的 3 个单测 + 现有 dev 不破坏
cargo test --lib list_plugins              # PluginSelect 发现新 plugin
cargo test --lib loader                    # skill/subagent loader 路径
cargo clippy --lib --tests -- -D warnings  # 零新增 warning（pre-existing 不归本任务）
diff -r app/src-tauri/resources/builtin-workflow/<plugin> .everlasting/workflow/<plugin>  # 镜像同步
```

## 反模式

- ❌ **不要**在新 plugin 的 revising/写产物 skill 里指引主 LLM 调一个不存在的专用工具（如 C3 design §4 曾提议的
  `emit_review_state_updated`）。除非该工具已实现并注册，否则用通用 `write_file`。C3 最终砍掉该工具，用 write_file。
- ❌ **不要**改 `write_file` 全局行为来满足单 plugin 的原子写需求 —— 那是全局改动，应另立 task。
- ❌ **不要**只改 `BUILTIN_PLUGIN_NAMES` 就以为内置化完成 —— 漏 #6/#7 会导致 plugin 可选但内容空。
- ❌ **不要**让 skill slug 与 frontmatter name 不一致 —— loader 会用错名字或解析失败。

## 已知前置：C0 TaskStatus / C1 resume

若新 plugin 需要**自定义 task state**（如 review 的 intake/reviewing/revising/reported）或 **subagent resume 续接**，
那是独立基建，前置任务：
- 自定义 state → `TaskStatus::Custom(String)`（C0, `workflow/task.rs` / `def.rs`）。
- resume 续接 → `dispatch_subagent` 的 `resume_from` + `resume_clarification`（C1, commit 703ab7d,
  `agent/subagent/dispatch.rs` + `agent/subagent/mod.rs` schema）。
