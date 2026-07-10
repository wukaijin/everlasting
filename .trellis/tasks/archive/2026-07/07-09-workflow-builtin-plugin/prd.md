# workflow plugin 内置化:app 内置 dev workflow,新项目开箱即用

## Goal

让 `dev` workflow 成为 **app 内置能力**,任何新项目开箱即用:切到新项目能看到 `dev` plugin 可选,选中后状态机 + wf-* skills + plugin agents 三者齐备,不依赖项目目录里有 `.everlasting/workflow/dev/`。同时**保留 engine/content 分离 + 项目可换 plugin** 架构(PRD 07-08-workflow-integration 核心):项目 `.everlasting/workflow/<name>/` 仍是第一优先级,内置层只做 fallback。

## 背景

上个开发(任务 07-08-workflow-integration)按 PRD 实现了 workflow,engine/content 分离架构没问题,但**三层内容只有状态机有内置 fallback,skills 和 plugin agents 没有**:

| 内容层 | 项目有 `.everlasting/workflow/dev/` | 别的项目(无该目录) |
|---|---|---|
| 状态机 `WorkflowDef` | 读项目 `workflow.json` | ✅ fallback `default_workflow()` 常量(`def.rs:369`) |
| wf-* skills(5个) | 读项目 `skills/` | ❌ 完全消失 → `use_skill("wf-brainstorm")` 失败 |
| plugin agents(3角色) | 读项目 `agents/` | ❌ 完全消失 → fallback 到 builtin researcher(语义不同) |

`list_plugins(project_path)` 只扫项目目录(`def.rs:476`)→ 别的项目 popover 空(`PluginSelect.vue:110`)→ 观感"这功能是项目专属 plugin 而非 app 内置"。`default_workflow()` 常量(`def.rs:369-440`)与 `.everlasting/workflow/dev/workflow.json` 逐字相同,状态机部分内置其实已等价。

### 代码证据(file:line)

- `app/src-tauri/src/agent/workflow/def.rs:623` `load_workflow()` 只读项目文件,失败 fallback 到 `default_workflow()` 常量。
- `def.rs:476` `list_plugins(project_path)` 只扫项目目录,无内置发现。
- `app/src-tauri/src/skill/loader.rs:618` `merge_skill_layers` 的 `SkillSource::Plugin`(`loader.rs:67`)只读项目目录;skill 扫描走 `tokio::fs::read_dir`(`loader.rs:327/351`)。
- `app/src-tauri/src/agent/subagent/loader.rs:396` `plugin_agents_dir` 同理;`SubagentSource::Plugin`(`loader.rs:93`),merge 走 `layers: Vec<Vec<_>>` 后插优先(`loader.rs:747/781`)。
- 本项目零 Tauri bundled-resource 先例(`tauri.conf.json` 无 `resources`,无 `include_str!`),内置内容惯例全是硬编码 Rust 常量(`builtin_subagents()` `subagent/mod.rs:463` / `default_workflow()`)。
- 前端 `PluginSelect.vue` 数据源是后端 `list_workflow_plugins` IPC,改后端即可,前端零改动。

## 已决策

- **优先级链(2026-07-09 确认)**:`project-plugin > builtin-plugin > project > user`。同名时项目 `.everlasting/workflow/<name>/` 压过内置;内置压过普通 project/user 层 skill(wf-* 是 workflow 语义的一部分,用户随手在 project/user 写同名 skill 不应意外盖掉 workflow 行为;要覆盖只能在项目 plugin 目录放同名文件)。影响:`BuiltinPlugin` 层插在 `Plugin` 之后、`Project` 之前。
- **技术选型**:`include_str!` 编译进二进制(非 Tauri `bundle.resources`)—— 符合本项目惯例 + 避免 WSL/dev resource_dir 坑 + 天然只读。
- **内置源位置**:`app/src-tauri/resources/builtin-workflow/dev/`,源材料逐字复制自 `.everlasting/workflow/dev/`。

## Requirements

### 功能需求

1. **内置 `dev` 三层齐备**:app 二进制内置 `workflow.json` + `agents/{researcher,implementer,checker}.md` + `skills/{wf-overview,wf-brainstorm,wf-before-dev,wf-check,wf-update-spec}/SKILL.md`。
2. **两层查找,项目优先**(`project-plugin > builtin-plugin > project > user`):engine 查 plugin 内容先查项目 `.everlasting/workflow/<name>/`,缺失/失败 fallback 到内置。
3. **新项目开箱即用**:`list_plugins(任意项目)` ≥ 含 `dev`;`load_workflow("dev", 任意项目)` 有效 `WorkflowDef`;workflow session 内 `use_skill("wf-*")` + dispatch researcher/implementer/checker 命中内置层。
4. **架构保留**:engine/content 分离 + 项目可换 plugin 不变;`default_workflow()` 常量作为最终兜底语义不变。
5. **只读内置**:内置层不可写(write_file / locate_agent_file 返回 Err)。

### 非功能需求

- 不引入新 crate(`include_str!` stdlib)。
- 不破坏 prompt cache breakpoint(不动注入 seam,只改 loader 查找层)。
- 复用现有分层 + 优先级 merge 模型,只加一层。

## Acceptance Criteria

### 状态机层(workflow/def.rs)
- [ ] 新增内置源模块,`include_str!` 加载 dev `workflow.json`;解析通过 `validate()`。
- [ ] `load_workflow(name, path)`:项目失败时先查内置同名 plugin,命中且合法返回内置 `WorkflowDef`;仍失败才 fallback `default_workflow()`。
- [ ] `list_plugins(path)`:项目扫描 ∪ 内置 plugin 名(去重,字母序)。
- [ ] 原 fallback 测试通过;`list_plugins_returns_empty_when_root_missing` 断言改为 `["dev"]`(内置层);新增空项目目录 + dev → 内置 WorkflowDef 测试。

### Skill 层(skill/loader.rs)
- [ ] `SkillSource` 加 `BuiltinPlugin`;workflow session 内 `find_skill_with_workflow("wf-brainstorm", 空项目, "dev")` 命中内置 body。
- [ ] 优先级链测试三断言:project-plugin 同名 → 项目赢;project 层同名 → 内置赢;user 层同名 → 内置赢。
- [ ] 内置 skill `path` 虚拟标记(`<builtin>/dev/skills/...`)。

### Agent 层(agent/subagent/loader.rs)
- [ ] `SubagentSource` 加 `BuiltinPlugin`;workflow session 内 dispatch researcher/implementer/checker 在空项目命中**内置 dev 角色**(非 builtin researcher,语义不同)。
- [ ] `locate_agent_file` 对内置层返回 Err(只读)。

### 集成
- [ ] `cargo test --lib` workflow + skill + subagent 全绿;`cargo check` 通过。
- [ ] 手动:空临时项目目录 → list_plugins=["dev"];load_workflow 有效;find_skill 命中内置;lookup agent 命中内置 dev 角色。

## Out of Scope

- ❌ 第二个内置 plugin(`review` 等)—— 机制就位即可。
- ❌ Tauri `bundle.resources` 动态资源解析 —— 选 `include_str!`。
- ❌ 改前端 `PluginSelect.vue` —— 数据源是 IPC,零改动。
- ❌ 改注入 seam / state hooks / task.json —— 只消费 `WorkflowDef`/`load_workflow`,不碰。
- ❌ 内置 plugin 热更新 —— 编译期常量;要换只能改项目 `.everlasting/`(即"项目可覆盖")。

## Notes

- 技术设计见 `design.md`;执行清单见 `implement.md`。
- 借鉴 Trellis 架构(机制 + 可定制性),不复制其打包方式。
