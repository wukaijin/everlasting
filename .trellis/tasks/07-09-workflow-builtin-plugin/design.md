# Design: workflow plugin 内置化

> 设计围绕 PRD「app 内置 dev workflow + 项目可覆盖」+ 已决策优先级链
> `project-plugin > builtin-plugin > project > user`。

## 1. 技术选型:`include_str!` 而非 Tauri `bundle.resources`

| 维度 | `include_str!`(选) | Tauri `bundle.resources` |
|---|---|---|
| 本项目先例 | ✅ 硬编码常量惯例的延伸(`builtin_subagents` / `default_workflow`) | ❌ 零先例 |
| dev 构建可用性 | ✅ 编译期常量 | ⚠️ `pnpm tauri dev` resource_dir 解析,WSL 有坑 |
| 运行时依赖 | 无 | 需 `app.path().resource_dir()` |
| 只读性 | ✅ 天然只读 | 需额外约束 |
| 新 crate | 无 | 无 |

内置内容是只读常量,`include_str!` 完全够用。源文件放 `app/src-tauri/resources/builtin-workflow/dev/`,编译期读入。

## 2. 内置源文件布局

```
app/src-tauri/resources/builtin-workflow/dev/
├── workflow.json                              # 复制自 .everlasting/workflow/dev/workflow.json
├── agents/
│   ├── researcher.md
│   ├── implementer.md
│   └── checker.md
└── skills/
    ├── wf-overview/SKILL.md
    ├── wf-brainstorm/SKILL.md
    ├── wf-before-dev/SKILL.md
    ├── wf-check/SKILL.md
    └── wf-update-spec/SKILL.md
```

**源材料** = 现有 `.everlasting/workflow/dev/` 下的同名文件(逐字复制)。二者保持同步(见 §6)。

## 3. 模块改动

### 3.1 新模块 `agent/workflow/builtin.rs`(内置源真值层)

```rust
//! app 内置 dev workflow 的编译期常量源。

/// 内置 dev 的 workflow.json(与 default_workflow() 常量逐字等价,
/// 但走 serde 路径,保证"内置层 == 项目层结构")。
pub const BUILTIN_DEV_WORKFLOW_JSON: &str =
    include_str!("../../resources/builtin-workflow/dev/workflow.json");

/// (slug, SKILL.md body) —— 内置 dev 的 5 个 wf-* skill。
pub const BUILTIN_DEV_SKILLS: &[(&str, &str)] = &[
    ("wf-overview",   include_str!("../../resources/builtin-workflow/dev/skills/wf-overview/SKILL.md")),
    ("wf-brainstorm", include_str!("../../resources/builtin-workflow/dev/skills/wf-brainstorm/SKILL.md")),
    ("wf-before-dev", include_str!("../../resources/builtin-workflow/dev/skills/wf-before-dev/SKILL.md")),
    ("wf-check",      include_str!("../../resources/builtin-workflow/dev/skills/wf-check/SKILL.md")),
    ("wf-update-spec",include_str!("../../resources/builtin-workflow/dev/skills/wf-update-spec/SKILL.md")),
];

/// (role_name, agent.md body) —— 内置 dev 的 3 个角色 agent。
pub const BUILTIN_DEV_AGENTS: &[(&str, &str)] = &[
    ("researcher",  include_str!("../../resources/builtin-workflow/dev/agents/researcher.md")),
    ("implementer", include_str!("../../resources/builtin-workflow/dev/agents/implementer.md")),
    ("checker",     include_str!("../../resources/builtin-workflow/dev/agents/checker.md")),
];

/// 内置 plugin 名清单(目前只有 dev)。list_plugins 用它做并集发现。
pub const BUILTIN_PLUGIN_NAMES: &[&str] = &["dev"];

/// 返回内置 plugin `<name>` 的 workflow.json 文本,None = 无此内置 plugin。
pub fn builtin_workflow_json(name: &str) -> Option<&'static str> {
    match name {
        "dev" => Some(BUILTIN_DEV_WORKFLOW_JSON),
        _ => None,
    }
}
```

单测:解析 `BUILTIN_DEV_WORKFLOW_JSON` 通过 `validate()`;每个 skill/agent body 非空且 frontmatter 含 `name`。

### 3.2 改 `workflow/def.rs::load_workflow`(加内置 fallback)

新 fallback 链(失败链,逐级降级):

```
项目 workflow.json ─失败─▶ 内置同名 workflow.json ─失败─▶ default_workflow() 常量
```

具体:`load_workflow` 现有的 4 个 `return default_workflow()` 点(NotFound / read err / JSON err / validate err,`def.rs:638/647/661/675`),在它们之前插入内置层尝试:

```rust
// 任何项目侧失败后,先试内置
if let Some(json) = crate::agent::workflow::builtin::builtin_workflow_json(workflow_name) {
    if let Ok(parsed) = serde_json::from_str::<WorkflowDef>(json) {
        if validate(&parsed).is_ok() {
            return parsed;  // 内置命中
        }
    }
    // 内置也坏(不该发生,编译期常量)→ 继续降级
}
return default_workflow();  // 最终兜底
```

实现上把内置尝试抽成一个私有 `fn load_builtin(workflow_name) -> Option<WorkflowDef>` 放 def.rs,load_workflow 的 4 个降级点统一调它,避免重复。

**`list_plugins` 改动**(`def.rs:476`):返回 `项目扫描 ∪ BUILTIN_PLUGIN_NAMES`,去重 + 字母序。

**`default_workflow()` 保留**:最终兜底语义不变,6 个原 fallback 测试(它们用 `load_workflow("dev", empty_tmp)` 期望 fallback)需要调整断言 —— 现在会命中**内置 dev**(返回内置 WorkflowDef,而非 default_workflow() 常量)。但因为内置 == default_workflow() 常量逐字相同,断言 `name=="dev"` / `initial=="planning"` / 4 states 仍通过。**需要逐个核对这些测试是否断言了"来自 default_workflow()"的副作用**(目前看没有,只断言形状)。这点在 implement 阶段验证。

### 3.3 改 `skill/loader.rs`(加 BuiltinPlugin 层)

`SkillSource` 加变体:
```rust
pub enum SkillSource {
    User, Project, Plugin,
    /// app 内置 plugin skill(include_str! 常量)。优先级:Plugin > BuiltinPlugin > Project > User。
    BuiltinPlugin,
}
```

**关键实现约束**:skill loader 的 plugin 层用 `tokio::fs::read_dir` + mtime 缓存(`current_mtimes`/`scan_skill_dir`,`loader.rs:327/351`)。内置层是内存常量,**不走目录扫描**,需要一个并行构造路径:

新增 `fn builtin_plugin_skills(workflow_name: &str) -> Vec<SkillResource>`:
- 仅 `workflow_name == "dev"` 时返回;其他返回空。
- 对 `BUILTIN_DEV_SKILLS` 每个 `(slug, body)`,用现有 frontmatter parser 解析 `name`/`description`/`allowed_tools`(复用 `parse_frontmatter`,与磁盘 skill 同一解析路径 → 行为一致,包括 researcher.md frontmatter 里的 `#` 注释行)。
- 构造 `SkillResource { source: BuiltinPlugin, path: PathBuf::from("<builtin>/dev/skills/<slug>/SKILL.md"), ... }`。
- 不进 mtime 缓存(常量无 mtime;每次调用便宜,且数量固定 5 个)。

**插入 merge 链**(`merge_skill_layers` `loader.rs:603` + `find_skill_in_layers` `loader.rs:669`):在 plugin 层(project)之后、project 层之前插入 builtin 层。两个函数的 merge 顺序:
- `merge_skill_layers`(by_name 后插覆盖):插入顺序 = user → project → **builtin-plugin** → project-plugin(后插者赢)。
- `find_skill_in_layers`(先命中返回):查找顺序 = project-plugin → **builtin-plugin** → project → user。

两处都要加 `Some(wf), Some(pp)` 门控(同 plugin 层),且 `wf == "dev"` 才查内置(未来 review 等内置时再扩)。

### 3.4 改 `agent/subagent/loader.rs`(加 BuiltinPlugin 层)

镜像 skill:`SubagentSource` 加 `BuiltinPlugin`(`loader.rs:93` 那个 enum)。

新增 `fn builtin_plugin_agents(workflow_name: &str) -> Vec<SubagentDef>`(或返回 loader 内部用的中间结构,取决于 `list_with_workflow` 的 merge 形态 —— 需 implement 时核对 `loader.rs:747` 的 merge 细节):
- 仅 `workflow_name == "dev"`。
- 对 `BUILTIN_DEV_AGENTS` 每个 `(role, body)`,用现有 frontmatter parser 解析(复用 agent frontmatter 解析,name/description/tools/model 等)。
- 构造 `SubagentDef { source: BuiltinPlugin, ... }`。
- `isolation`/`model` 等 frontmatter 字段按现有 plugin agent 解析逻辑(researcher.md 里有 `isolation` 注释说明只读不隔离 —— 需确认 frontmatter 是否显式声明 isolation,还是默认;见 implement 核对)。

**优先级插入**:`list_with_workflow`(`loader.rs:747`)和 `lookup_with_workflow`(`loader.rs:786`)的 merge,Plugin 之后、Project 之前插入 BuiltinPlugin。

**`locate_agent_file`**(`loader.rs:827`):`BuiltinPlugin` 分支返回 Err(只读),与现有 `Plugin` 分支(`loader.rs:868`)一致。

## 4. 不动的部分

- `default_workflow()` 常量、`validate()`、`WorkflowDef` 结构 —— 不变。
- `build_workflow_ctx`(`inject.rs:143`)、state.rs hooks、task.json 体系 —— 不变(只消费 `WorkflowDef`/`load_workflow`)。
- 注入 seam、prompt cache breakpoint —— 不碰。
- 前端 `PluginSelect.vue` —— 零改动(数据源是 IPC)。

## 5. 兼容性 / 回归

- **原 6 个 `load_workflow` fallback 测试**(`mod.rs:363/379/433/450` 等):它们对空 tmp 目录调 `load_workflow("dev", path)`,现在会命中内置 dev 而非 default_workflow() 常量。因内置 == 常量逐字相同,形状断言(name/initial/states)仍通过。**风险点**:若有测试断言了"fallback 发生"的 tracing warn 或特定路径,需调整。implement 阶段逐个跑确认。
- **list_plugins 测试**(`mod.rs:537/547/559`):`list_plugins_returns_empty_when_root_missing` 现在会返回 `["dev"]`(内置)而非 `[]`。**此测试断言需改**为 `["dev"]`,并加注释说明内置层。这是**唯一需要修改断言的既有测试**。

## 6. 内置源 vs 项目示例同步(防漂移)

两份文件:`app/src-tauri/resources/builtin-workflow/dev/`(内置源,只读)与 `.everlasting/workflow/dev/`(项目示例,可改)。

约定:
- 内置源是 **source of truth**。
- `.everlasting/workflow/dev/` 顶部加注释:「本目录是 dev plugin 的项目级覆盖示例;app 内置源在 `app/src-tauri/resources/builtin-workflow/dev/`,二者需保持同步」。
- 不做自动同步脚本(YAGNI;内置内容变更频率低,人工同步 + 本任务文档锚点足够)。
- 内置源每个文件顶部加注释指回项目示例路径。

## 7. 风险与注意

- **内置 skill/agent 走独立构造路径**(非目录扫描),需确保 frontmatter 解析行为与磁盘路径完全一致(同一 parser 函数)。researcher.md frontmatter 含 `#` 注释行,确认 parser 能容忍(现有项目路径已验证可加载,内置复用同 parser 则等价)。
- **优先级链插错位置**会导致"项目可覆盖"失效 —— merge_skill_layers 的"后插覆盖"与 find_skill_in_layers 的"先命中返回"方向相反,两处都要按 §3.3 写。
- **`default_workflow()` 常量现在变成"全失败兜底"**,实际几乎不会触达(内置 dev 常量编译期就合法)。保留它是为了 `load_workflow("不存在的名字", path)` 这类调用的安全网。
