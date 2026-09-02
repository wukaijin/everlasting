<!-- Spec for builtin workflow plugins. Captured from 07-26-review-plugin-pack (C3) +
     07-09-workflow-builtin-plugin. Read before adding a new builtin workflow plugin
     or touching builtin.rs / the two builtin_plugin_* loaders. -->

# Builtin Workflow Plugin 内置化机制

> **Source**: `.trellis/tasks/archive/2026-07/07-09-workflow-builtin-plugin` (dev plugin 内置化) +
> `.trellis/tasks/07-26-review-plugin-pack` (C3, review plugin 内置化)。
>
> **何时读本文**:新增 builtin workflow plugin、改 `builtin.rs`、改 `skill/loader.rs::builtin_plugin_skills` 或
> `agent/subagent/cache.rs::builtin_plugin_agents`(08-07 拆分)、或排查「新 plugin 在 PluginSelect 不可见 / skill 没加载 /
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
镜像范围 = workflow.json + agents/ + skills/；`dev/README.md` 仅存在于 builtin 侧、不参与镜像
（`diff -r --exclude=README.md` 为验收口径）。

> workflow.json ↔ Rust 镜像的等价性（2026-08-27 起部分机制化）：builtin 层 dev JSON 反序列化后与
> `default_workflow()` 全字段相等由单测 `builtin_dev_json_equals_default_workflow_constant`
> （builtin.rs，WorkflowDef derive PartialEq/Eq）守护 —— 改 JSON 漏改 def.rs 会被测试拦下；
> 但 JSON 文本级逐字一致与 agents/*.md 的镜像仍纯靠人工（diff -r 是唯一防线）。
> 另：**Rust 测试可能锚定提示词字面量**（workflow/mod.rs 曾断言模板 `contains("cargo test")`）——
> 改内置提示词文案前先 grep src 里对旧文案的测试断言（08-27 任务即被迫连带修了一处）。

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
| 7 | `agent/subagent/cache.rs::builtin_plugin_agents`(08-07 拆分) | 同 #6，返回 `BUILTIN_<PLUGIN>_AGENTS` |
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

## 提示词内容约定（08-27-builtin-agent-prompt-generalize 落地）

loader/frontmatter 之外的**内容级**强约束 —— builtin 插件的 agent.md / SKILL.md / delegation_templates
是编译进二进制发给所有用户的提示词：

1. **栈中立**：不得硬编码特定技术栈的可执行命令（`cargo test --lib` / `cargo clippy` / `pnpm test` 等）。
   写探测指引：项目 `AGENTS.md`/`CLAUDE.md` 记载的验证命令 → 清单文件推断（`Cargo.toml` 注意 workspace
   default-members 陷阱、`package.json` 由 lockfile 定包管理器、`pyproject.toml`/`go.mod`/`pom.xml`/
   `build.gradle.*`）→ 找不到全量套件则按改动文件做最小验证并显式标注。报告模板用 `<test cmd>: X passed`
   占位字段。本仓库自己的特化命令只活在根 `AGENTS.md`——**不进 builtin，也不进项目层镜像**
   （"项目层可保留仓库特化"是误解：dev 插件曾据此错误分叉出 implement/check 旧状态词汇表）。
2. **无 dogfood 泄漏**：session id、内部 DB 字段名、commit hash、"见 Q7"式内部引用不得进 builtin 提示词。
   教训类内容收编时先去标识化、只留纪律本身（反例与正例都在 review/wf-review-method 的 model 漏传教训段）。
3. **不承诺 ask 行为**：子代理约束清单未声明 ask 类工具（checker 还只读 + 禁 dispatch），提示词不得写
   "问用户"；向用户澄清一律由主 LLM 依据子代理报告发起。

## dev 插件内容契约(2026-09-02 wf-trellis-alignment 起)

dev workflow 不再是纯线性三态:`transitions` 含回环边 `in_progress→planning`(requires_user_confirm,
先例:review 的 revising→reviewing);`done` 无出边是**有意设计**(返工应新建任务,防 done↔in_progress
振荡)。回滚走 `request_task_state_transition` → `set_task_state` 的 no-op hook 分支(零 marker);
重入 `planning→in_progress` 的 preflight / spec-distillation marker 均幂等(`has_marker` 短路)。

### `{relevant_specs}` 按任务策展(sidecar 优先)

- planning 阶段主 LLM 写 `.everlasting/tasks/<slug>/relevant-specs.jsonl`,每行
  `{"file": "<repo 相对路径>", "reason": "<为什么相关>"}`(spec + research 文件)。
- 注入契约:`resolve_relevant_specs(project_path, task_slug: Option<&str>)`(inject.rs)——
  sidecar 存在且有好行 → 输出 `file — reason` 列表;坏行跳过;空文件 / 全坏 / 缺失 / slug None →
  **逐字回退旧全树罗列**(含 `(auto-detect via wf-before-dev)` 兜底)。改 fallback 构造 = 破坏兼容,
  tests_inject 有对拍用例。
- 已知路径分叉:调用方传入的 `project_path` 是 dispatch 的 `current_ctx.worktree_path`
  (subagent/dispatch/parse.rs),`current_task` 则源自 DB `project.path` —— session 跑在
  session worktree 时策展查找 miss,fallback 兜底(有意不为此引入 DB 依赖,改前先读 inject.rs 函数 doc)。

### Gotcha:tasks/ 文件对隔离 worker 结构性不可见

> `.everlasting/tasks/` 整体 gitignored(根 `.gitignore`),而 `isolation: true` 的角色(implementer)
> 跑在 parent HEAD 检出的真 git worktree(`create_worker`,tests_worktree 钉死)→ research/、
> relevant-specs.jsonl、task.json 对它**永久不可见,commit 也救不了**(目录根本不进 git)。
> checker / researcher 无隔离(共享 cwd),不受影响。
> **唯一可靠通道 = delegation 文本**:派 implementer 前把关键调研结论摘要进 delegation
> message(wf-brainstorm 已写明);implementer 提示词要求 research 路径 read 不到时写 Known issues,
> 禁止臆测内容。

### 提示词工具指向

builtin 提示词(skills / delegation_templates / 工具 description)凡涉及「切 state」一律指向
`request_task_state_transition` —— 它是唯一翻转 `task.json.status` 的通道(Allow 后 IPC handler
落盘);`ask_user_question` 只提问不落盘。wf-overview 里保留的 ask_user_question 字样只能是
「它不落盘」的对比澄清。

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
- ❌ **不要**在 builtin 提示词硬编码技术栈命令 / dogfood 细节 / "问用户"承诺（见上文「提示词内容约定」三条）。
- ❌ **不要**以为 workflow.json ↔ def.rs 靠"注释声明等价"就安全 —— 现在有测试兜底
  （`builtin_dev_json_equals_default_workflow_constant`），漏改会被拦；agents/*.md 没有这层，diff -r 别省。

## 已知前置：C0 TaskStatus / C1 resume

若新 plugin 需要**自定义 task state**（如 review 的 intake/reviewing/revising/reported）或 **subagent resume 续接**，
那是独立基建，前置任务：
- 自定义 state → `TaskStatus::Custom(String)`（C0, `workflow/task.rs` / `def.rs`）。
- resume 续接 → `dispatch_subagent` 的 `resume_from` + `resume_clarification`（C1, commit 703ab7d,
  `agent/subagent/dispatch.rs` + `agent/subagent/mod.rs` schema）。
