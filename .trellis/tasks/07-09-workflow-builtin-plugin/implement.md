# Implement: workflow plugin 内置化

> **实现说明书**:本文件目标读者是另一个 AI 实现者。每步含确切 `file:line` 锚点、可照抄的代码骨架、验证命令。按序执行;步骤间标注依赖。
>
> **背景**:dev workflow 当前是项目级 plugin(只从 `<project>/.everlasting/workflow/dev/` 读),换项目就没了。本任务把它做成 **app 内置**:`include_str!` 把内容文件编译进二进制成 `&'static str`,loader 查不到项目层时 fallback 到内置层。优先级链 `project-plugin > builtin-plugin > project > user`。架构(分层 + merge)不变,只加一层。
>
> **统一测试命令**(WSL,见 CLAUDE.md HACKING-wsl 坑 1):
> ```bash
> cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib
> cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo check
> ```

---

## Step 0 — 内置源文件就位(纯资源,无代码)

### 0.1 建目录 + 复制

从 `.everlasting/workflow/dev/` **逐字复制**到新目录 `app/src-tauri/resources/builtin-workflow/dev/`:

```
app/src-tauri/resources/builtin-workflow/dev/
├── workflow.json                                  ← 复制自 .everlasting/workflow/dev/workflow.json
├── agents/
│   ├── researcher.md                              ← 复制自 .everlasting/workflow/dev/agents/researcher.md
│   ├── implementer.md                             ← 复制自 .../agents/implementer.md
│   └── checker.md                                 ← 复制自 .../agents/checker.md
└── skills/
    ├── wf-overview/SKILL.md                       ← 复制自 .../skills/wf-overview/SKILL.md
    ├── wf-brainstorm/SKILL.md
    ├── wf-before-dev/SKILL.md
    ├── wf-check/SKILL.md
    └── wf-update-spec/SKILL.md
```

**逐字复制,不要改内容**(frontmatter 格式必须和项目层一致,这样内置层和项目层走同一 parser)。

### 0.2 防漂移标注

- `app/src-tauri/resources/builtin-workflow/dev/` 目录下新建 `README.md`:
  ```
  # app 内置 dev workflow 源
  编译期由 `include_str!` 读入(`agent/workflow/builtin.rs`),变成二进制常量。
  这是 app 内置能力的 source of truth。
  项目级覆盖示例在仓库根 `.everlasting/workflow/dev/`,二者内容需保持同步。
  修改本目录后无需改代码(`include_str!` 路径固定),重新编译即生效。
  ```
- 不需要改 Cargo.toml / tauri.conf.json(`include_str!` 相对路径在编译期解析,不走 Tauri resource bundle)。

**验证**:`diff -r .everlasting/workflow/dev app/src-tauri/resources/builtin-workflow/dev`(应只有 README.md 差异)。

---

## Step 1 — 内置源模块 `agent/workflow/builtin.rs`

### 1.1 新建文件 `app/src-tauri/src/agent/workflow/builtin.rs`

```rust
//! W1-builtin (2026-07-09): app 内置 dev workflow 的编译期常量源。
//!
//! `include_str!` 在编译期把 `resources/builtin-workflow/dev/` 下的内容
//! 文件读成 `&'static str`,嵌进二进制。运行时 loader 查不到项目层
//! (`.everlasting/workflow/<name>/`)时 fallback 到这里。
//!
//! **为什么不用 Tauri bundle.resources**:本项目零 resource bundle 先例
//! (`tauri.conf.json` 无 `resources` 键),内置内容惯例全是硬编码 Rust 常量
//! (`builtin_subagents()` / `default_workflow()`)。`include_str!` 是该惯例
//! 的自然延伸,dev 构建即用,无运行时 resource_dir 解析依赖。详见
//! `.trellis/tasks/07-09-workflow-builtin-plugin/design.md §1`。

/// 内置 dev 的 workflow.json 文本(与 `default_workflow()` 常量逐字等价,
/// 但走 serde 解析路径,保证"内置层结构 == 项目层结构")。
pub const BUILTIN_DEV_WORKFLOW_JSON: &str =
    include_str!("../../resources/builtin-workflow/dev/workflow.json");

/// (slug, SKILL.md body) —— 内置 dev 的 5 个 wf-* skill。
/// slug 必须等于 SKILL.md 所在子目录名(skill loader 用目录名做 fallback name)。
pub const BUILTIN_DEV_SKILLS: &[(&str, &str)] = &[
    ("wf-overview", include_str!("../../resources/builtin-workflow/dev/skills/wf-overview/SKILL.md")),
    ("wf-brainstorm", include_str!("../../resources/builtin-workflow/dev/skills/wf-brainstorm/SKILL.md")),
    ("wf-before-dev", include_str!("../../resources/builtin-workflow/dev/skills/wf-before-dev/SKILL.md")),
    ("wf-check", include_str!("../../resources/builtin-workflow/dev/skills/wf-check/SKILL.md")),
    ("wf-update-spec", include_str!("../../resources/builtin-workflow/dev/skills/wf-update-spec/SKILL.md")),
];

/// (role_name, agent.md body) —— 内置 dev 的 3 个角色 agent。
/// role_name 必须等于 frontmatter 的 `name:` 字段(agent loader 要求显式 name)。
pub const BUILTIN_DEV_AGENTS: &[(&str, &str)] = &[
    ("researcher", include_str!("../../resources/builtin-workflow/dev/agents/researcher.md")),
    ("implementer", include_str!("../../resources/builtin-workflow/dev/agents/implementer.md")),
    ("checker", include_str!("../../resources/builtin-workflow/dev/agents/checker.md")),
];

/// 内置 plugin 名清单。`list_plugins` 用它做并集发现。
/// 目前只有 dev;未来加 review 时在此追加 + 加对应常量。
pub const BUILTIN_PLUGIN_NAMES: &[&str] = &["dev"];

/// 返回内置 plugin `<name>` 的 workflow.json 文本。
/// `None` = 无此内置 plugin(交由 caller 继续降级)。
pub fn builtin_workflow_json(name: &str) -> Option<&'static str> {
    match name {
        "dev" => Some(BUILTIN_DEV_WORKFLOW_JSON),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::workflow::{validate, WorkflowDef};

    #[test]
    fn builtin_dev_workflow_json_validates() {
        let def: WorkflowDef =
            serde_json::from_str(BUILTIN_DEV_WORKFLOW_JSON).expect("builtin dev JSON parses");
        assert!(validate(&def).is_ok(), "builtin dev JSON must self-validate");
        assert_eq!(def.name, "dev");
        assert_eq!(def.initial, "planning");
    }

    #[test]
    fn builtin_dev_skills_nonempty_with_frontmatter() {
        assert_eq!(BUILTIN_DEV_SKILLS.len(), 5);
        for (slug, body) in BUILTIN_DEV_SKILLS {
            assert!(!body.is_empty(), "skill {slug} body empty");
            assert!(
                body.starts_with("---\n") && body.contains("name:"),
                "skill {slug} must start with frontmatter fence + name field"
            );
        }
    }

    #[test]
    fn builtin_dev_agents_nonempty_with_frontmatter() {
        assert_eq!(BUILTIN_DEV_AGENTS.len(), 3);
        for (role, body) in BUILTIN_DEV_AGENTS {
            assert!(!body.is_empty(), "agent {role} body empty");
            assert!(
                body.starts_with("---\n") && body.contains("name:"),
                "agent {role} must start with frontmatter fence + name field"
            );
        }
    }
}
```

### 1.2 注册模块 `agent/workflow/mod.rs`

在 `mod.rs` 现有 `pub mod def;` / `task;` / `inject;` / `state;` 同级加:

```rust
/// app 内置 plugin 源(`include_str!` 编译期常量)。07-09-workflow-builtin-plugin。
pub mod builtin;
```

在 `mod.rs` 的 re-export 块(现有 `pub use def::{...}` / `pub use task::{...}` 等)加一个新的:

```rust
// Re-export 内置源常量,供 skill/subagent loader 消费。
#[allow(unused_imports)]
pub use builtin::{builtin_workflow_json, BUILTIN_DEV_AGENTS, BUILTIN_DEV_SKILLS, BUILTIN_PLUGIN_NAMES};
```

(`#[allow(unused_imports)]` 先加上,Step 3/4 接入消费后如不再有 warning 可去掉。)

### 1.3 验证

```bash
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib workflow::builtin
```

应 3 个测试全过。若 `include_str!` 路径报错,核对相对路径:从 `agent/workflow/builtin.rs` 到 `resources/builtin-workflow/dev/` 是 `../../resources/...`(上两级到 `src-tauri/`,再进 `resources/`)。

---

## Step 2 — `load_workflow` + `list_plugins` 加内置层(状态机层)

### 2.1 改 `app/src-tauri/src/agent/workflow/def.rs`

**a) 新增私有 fallback 函数**(放在 `load_workflow` 函数之前,`validate` 函数之后):

```rust
/// 尝试加载 app 内置同名 plugin(07-09-workflow-builtin-plugin)。
/// 内置源是 `include_str!` 编译期常量,理论永不出错;但走完整
/// serde + validate 路径,与项目层行为一致,出问题时仍安全降级。
/// 返回 `None` = 无此内置 plugin 或内置也坏(交由 caller 降级到 default_workflow)。
fn load_builtin(workflow_name: &str) -> Option<WorkflowDef> {
    let json = crate::agent::workflow::builtin::builtin_workflow_json(workflow_name)?;
    match serde_json::from_str::<WorkflowDef>(json) {
        Ok(parsed) if validate(&parsed).is_ok() => Some(parsed),
        Ok(_) => {
            tracing::error!(
                workflow = %workflow_name,
                "load_builtin: builtin workflow.json failed validation (this should not happen — it is a compile-time constant)"
            );
            None
        }
        Err(e) => {
            tracing::error!(
                workflow = %workflow_name,
                error = %e,
                "load_builtin: builtin workflow.json failed to parse (this should not happen — it is a compile-time constant)"
            );
            None
        }
    }
}
```

**b) 改 `load_workflow`(def.rs:623)** —— 4 个降级点(`:638` / `:647` / `:661` / `:675`)在 `return default_workflow();` **之前**插入内置尝试。

**改法**:把 4 处 `return default_workflow();` 统一替换为 `return load_builtin_or_default(workflow_name);`,并新增该辅助函数:

```rust
/// 内置层 → default_workflow() 的最终降级。load_workflow 的 4 个失败点都走这里。
fn load_builtin_or_default(workflow_name: &str) -> WorkflowDef {
    if let Some(b) = load_builtin(workflow_name) {
        return b;
    }
    default_workflow()
}
```

`load_workflow` 内部改后长这样(只展示关键结构):

```rust
pub fn load_workflow(workflow_name: &str, project_path: &str) -> WorkflowDef {
    let path = workflow_json_path(workflow_name, project_path);
    let raw = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            tracing::debug!(...);
            return load_builtin_or_default(workflow_name);   // ← 改这里
        }
        Err(e) => {
            tracing::warn!(...);
            return load_builtin_or_default(workflow_name);   // ← 改这里
        }
    };
    let parsed: WorkflowDef = match serde_json::from_str(&raw) {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!(...);
            return load_builtin_or_default(workflow_name);   // ← 改这里
        }
    };
    if let Err(errs) = validate(&parsed) {
        for err in &errs { tracing::warn!(...); }
        return load_builtin_or_default(workflow_name);       // ← 改这里
    }
    parsed
}
```

**关键语义**:项目文件存在且合法 → 用项目的(项目可覆盖);项目文件任何失败 → 先试内置 → 内置也失败才 `default_workflow()` 常量。

**c) 改 `list_plugins`(def.rs:476)** —— 返回项目扫描 ∪ 内置名。

```rust
pub fn list_plugins(project_path: &str) -> Vec<String> {
    let root = Path::new(project_path)
        .join(".everlasting")
        .join("workflow");
    let entries = match std::fs::read_dir(&root) {
        Ok(it) => it,
        Err(_) => {
            // 项目无 workflow 目录 → 只返回内置名(07-09-workflow-builtin-plugin)。
            return BUILTIN_PLUGIN_NAMES.iter().map(|s| s.to_string()).collect();
        }
    };
    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter_map(|entry| {
            let path = entry.path();
            if !path.is_dir() { return None; }
            let workflow_json = path.join("workflow.json");
            if !workflow_json.is_file() { return None; }
            entry.file_name().into_string().ok()
        })
        .collect();
    // 合并内置名,去重,字母序(07-09-workflow-builtin-plugin)。
    for builtin in BUILTIN_PLUGIN_NAMES {
        if !names.iter().any(|n| n == *builtin) {
            names.push(builtin.to_string());
        }
    }
    names.sort();
    names
}
```

需在 def.rs 顶部 import(若 `use` 块没有):`use crate::agent::workflow::builtin::BUILTIN_PLUGIN_NAMES;`

### 2.2 改既有测试 `mod.rs`

**必改的断言** —— `list_plugins_returns_empty_when_root_missing`(mod.rs:537):

```rust
#[test]
fn list_plugins_returns_empty_when_root_missing() {
    // 07-09-workflow-builtin-plugin:现在即使项目无 workflow 目录,
    // 也返回内置 plugin 名(至少 dev),不再为空。
    let proj_tmp = tempfile::TempDir::new().unwrap();
    let path = proj_tmp.path().to_string_lossy().to_string();
    assert_eq!(list_plugins(&path), vec!["dev".to_string()]);
}
```

**其余 list_plugins 测试**(mod.rs:547 `discovers_alphabetical`、:559 `ignores_dirs_without_workflow_json`):它们写的是非 dev 名(zulu/alpha/real/scratch),与内置 dev 不冲突,但 `discovers_alphabetical` 现在返回值会**多了 dev**。检查断言:
- `list_plugins_discovers_alphabetical`:原断言 `vec!["alpha", "zulu"]`,现在 dev 排序后插在 alpha 前 → 应改为 `vec!["alpha", "dev", "zulu"]`。
- `list_plugins_ignores_dirs_without_workflow_json`:原断言 `vec!["real"]`,现在变 `vec!["dev", "real"]` → 改断言。

**新增测试**(加在 list_plugins 测试组末尾):

```rust
#[test]
fn list_plugins_always_includes_builtin_dev() {
    // 空项目目录 → 只有内置 dev(项目可覆盖 + 内置 fallback 的核心行为)。
    let proj_tmp = tempfile::TempDir::new().unwrap();
    let path = proj_tmp.path().to_string_lossy().to_string();
    let plugins = list_plugins(&path);
    assert!(plugins.contains(&"dev".to_string()), "builtin dev always present: {plugins:?}");
}

#[test]
fn load_workflow_falls_back_to_builtin_when_project_missing() {
    // 项目无 dev 目录 → 内置 dev(非 default_workflow 常量路径,但二者等价)。
    let proj_tmp = tempfile::TempDir::new().unwrap();
    let path = proj_tmp.path().to_string_lossy().to_string();
    let loaded = load_workflow("dev", &path);
    assert_eq!(loaded.name, "dev");
    assert_eq!(loaded.initial, "planning");
    assert_eq!(loaded.states.len(), 4);
}
```

**核对既有 load_workflow fallback 测试**(mod.rs:363/379/433/450):
- `load_workflow_missing_file_falls_back_to_default`(363):现在命中内置 dev,断言 `name=="dev"`/`initial=="planning"`/`states.len()==4` 仍过(内置==常量逐字相同)。**无需改**。
- `load_workflow_valid_json_overrides_default`(379):用 `"review"` 名 + 项目写了 review 的 JSON → 项目命中,不走内置。**无需改**。
- `load_workflow_malformed_json_falls_back_with_warn`(433):`"broken"` 名 → 项目 JSON 坏 → 试内置 `"broken"` → 无此内置 → `default_workflow()`。断言 `name=="dev"`。**无需改**(default_workflow 常量 name 也是 dev)。
- `load_workflow_validation_failure_falls_back`(450):`"bad"` 名 → 项目 validation 失败 → 试内置 `"bad"` → 无 → default_workflow。断言 `name=="dev"`。**无需改**。

跑一遍确认:`cargo test --lib workflow`(PKG_CONFIG_PATH 见顶部)。全绿才进 Step 3。

---

## Step 3 — skill loader 加 BuiltinPlugin 层

### 3.1 `SkillSource` 加变体(`skill/loader.rs:59`)

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SkillSource {
    User,
    Project,
    Plugin,
    /// app 内置 plugin skill(`include_str!` 常量)。07-09-workflow-builtin-plugin。
    /// 优先级:`Plugin > BuiltinPlugin > Project > User`。
    BuiltinPlugin,
}
```

**注意**:SkillSource 派生了 `Serialize`(`serde rename_all lowercase`)。`BuiltinPlugin` 会序列化成 `"builtinplugin"`。若 UI / 前端对 source 字符串有枚举约束,核对 `app/src/stores/` 里有没有 match;若需 kebab-case,改派生或手写。先按 lowercase 走,前端 `PluginSelect` 不显示 source,通常无影响。

### 3.2 抽出纯解析函数(关键:复用 frontmatter parser)

现有 `load_skill_file`(`skill/loader.rs:386`)是 `async fn(&Path, dir_name, source) -> io::Result<Option<SkillResource>>`,内部先 `read_to_string` 再 `parse_frontmatter`。内置层是内存常量,**没有 Path**。

**改法**:把 `load_skill_file` 的「解析 + 构造 SkillResource」部分抽成纯函数,接收 `(&content, dir_name, source)`,不碰 IO:

```rust
/// 纯解析:从 SKILL.md 文本 + 目录名 + source 构造 SkillResource。
/// 磁盘层(load_skill_file)和内置层(builtin_plugin_skills)共用此函数,
/// 保证 frontmatter 解析行为完全一致(07-09-workflow-builtin-plugin)。
fn parse_skill_content(
    content: &str,
    dir_name: &str,
    source: SkillSource,
) -> Option<SkillResource> {
    let (fm, body) = parse_frontmatter(content);
    let name = fm
        .name
        .clone()
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| dir_name.to_string());
    if name.trim().is_empty() {
        return None;
    }
    Some(SkillResource {
        name,
        description: fm.description.unwrap_or_default(),
        body,
        // 内置层无磁盘路径,用虚拟标记(tracing 可读;locate/write 对内置层不适用)。
        // 磁盘层调用方传入真实 Path 前,需在外层覆盖此字段 —— 见 load_skill_file 改动。
        path: PathBuf::new(),
        source,
        allowed_tools: fm.allowed_tools,
    })
}
```

改 `load_skill_file`(`:386`)调用它(只保留 IO + size cap + path 覆盖):

```rust
async fn load_skill_file(
    skill_path: &Path,
    dir_name: &str,
    source: SkillSource,
) -> std::io::Result<Option<SkillResource>> {
    let meta = match tokio::fs::metadata(skill_path).await {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    if meta.len() > MAX_SKILL_FILE_SIZE {
        tracing::warn!(...);  // 保留原 warn
        return Ok(None);
    }
    let content = tokio::fs::read_to_string(skill_path).await?;
    let mut res = match parse_skill_content(&content, dir_name, source) {
        Some(r) => r,
        None => return Ok(None),
    };
    res.path = skill_path.to_path_buf();  // 覆盖虚拟 path 为真实磁盘路径
    Ok(Some(res))
}
```

### 3.3 新增内置 skill 构造函数

在 `skill/loader.rs`(skill 相关函数区,建议放 `list_skill_infos_with_workflow` 之前):

```rust
/// 构造 app 内置 plugin 的 skills(07-09-workflow-builtin-plugin)。
/// 仅 `workflow_name == "dev"` 时返回;其他返回空。
/// 不走磁盘扫描(`tokio::fs::read_dir`)—— 内置源是 `include_str!` 内存常量,
/// 用 parse_skill_content 直接解析。path 用虚拟标记。
pub(crate) fn builtin_plugin_skills(workflow_name: &str) -> Vec<SkillResource> {
    if workflow_name != "dev" {
        return Vec::new();
    }
    crate::agent::workflow::BUILTIN_DEV_SKILLS
        .iter()
        .filter_map(|(slug, body)| {
            let mut res = parse_skill_content(body, slug, SkillSource::BuiltinPlugin)?;
            res.path = PathBuf::from(format!("<builtin>/dev/skills/{slug}/SKILL.md"));
            Some(res)
        })
        .collect()
}
```

### 3.4 插入 merge 链 —— `merge_skill_layers`(loader.rs:603)

现有逻辑(后插覆盖,低优先先插):
```
user → project → (有 wf 时)project-plugin
```
改为:
```
user → project → (有 wf 时)builtin-plugin → (有 wf 时)project-plugin
```

```rust
async fn merge_skill_layers(
    cache: &SkillCache,
    project_path: Option<&str>,
    workflow_name: Option<&str>,
) -> Vec<SkillInfo> {
    let mut by_name: HashMap<String, SkillResource> = HashMap::new();
    for r in cache.list_user().await {
        by_name.insert(r.name.clone(), r);
    }
    if let Some(pp) = project_path {
        for r in cache.list_project(pp).await {
            by_name.insert(r.name.clone(), r);
        }
    }
    // 07-09-workflow-builtin-plugin: 内置 plugin 层,在 project-plugin 之前插入
    // (后插覆盖 → project-plugin 优先级高于 builtin-plugin)。
    if let Some(wf) = workflow_name {
        for r in builtin_plugin_skills(wf) {
            by_name.insert(r.name.clone(), r);
        }
    }
    if let (Some(wf), Some(pp)) = (workflow_name, project_path) {
        for r in cache.list_plugin(pp, wf).await {
            by_name.insert(r.name.clone(), r);
        }
    }
    let mut infos: Vec<SkillInfo> = by_name
        .into_values()
        .map(|r| resource_to_info(&r))
        .collect();
    infos.sort_by(|a, b| a.name.cmp(&b.name));
    infos
}
```

### 3.5 插入 find 链 —— `find_skill_in_layers`(loader.rs:669)

现有逻辑(先命中返回,高优先先查):
```
project-plugin → project → user
```
改为:
```
project-plugin → builtin-plugin → project → user
```

```rust
async fn find_skill_in_layers(
    cache: &SkillCache,
    name: &str,
    project_path: Option<&str>,
    workflow_name: Option<&str>,
) -> Option<SkillResource> {
    if let (Some(wf), Some(pp)) = (workflow_name, project_path) {
        if let Some(r) = cache.list_plugin(pp, wf).await.into_iter().find(|r| r.name == name) {
            return Some(r);
        }
    }
    // 07-09-workflow-builtin-plugin: 内置 plugin 层,在 project 之前查。
    if let Some(wf) = workflow_name {
        if let Some(r) = builtin_plugin_skills(wf).into_iter().find(|r| r.name == name) {
            return Some(r);
        }
    }
    if let Some(pp) = project_path {
        if let Some(r) = cache.list_project(pp).await.into_iter().find(|r| r.name == name) {
            return Some(r);
        }
    }
    cache.list_user().await.into_iter().find(|r| r.name == name)
}
```

**注意**:`workflow_name` 在这两个函数里进来前已被 `merge_skill_layers`/`find_skill_with_workflow` 的 `.filter(|n| !n.is_empty())` 归一化过(见 loader.rs:594/663),所以这里直接用即可,不需再判空。

### 3.6 测试(加在 `skill/loader.rs` 的 `#[cfg(test)] mod tests` 末尾)

```rust
#[tokio::test]
async fn builtin_plugin_skill_loaded_for_dev_in_empty_project() {
    // 空项目目录 + workflow=dev → wf-brainstorm 命中内置层。
    let cache = SkillCache::arc();  // 构造见现有测试 loader.rs:935
    let tmp = tempfile::TempDir::new().unwrap();
    let pp = tmp.path().to_string_lossy().to_string();
    let r = find_skill_with_workflow(&cache, "wf-brainstorm", Some(&pp), Some("dev")).await;
    let r = r.expect("builtin wf-brainstorm should load");
    assert_eq!(r.source, SkillSource::BuiltinPlugin);
    assert!(!r.body.is_empty());
    assert!(r.path.to_string_lossy().contains("<builtin>"));
}

#[tokio::test]
async fn project_plugin_overrides_builtin_skill() {
    // 项目 .everlasting/workflow/dev/skills/wf-brainstorm/SKILL.md → 项目赢(Plugin > BuiltinPlugin)。
    let cache = SkillCache::arc();
    let tmp = tempfile::TempDir::new().unwrap();
    let proj = tmp.path();
    let dir = proj.join(".everlasting/workflow/dev/skills/wf-brainstorm");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("SKILL.md"), "---\nname: wf-brainstorm\ndescription: mine\n---\nCUSTOM").unwrap();
    let pp = proj.to_string_lossy().to_string();
    let r = find_skill_with_workflow(&cache, "wf-brainstorm", Some(&pp), Some("dev")).await.unwrap();
    assert_eq!(r.source, SkillSource::Plugin, "project plugin wins over builtin");
    assert_eq!(r.body, "CUSTOM");
}

#[tokio::test]
async fn builtin_plugin_beats_project_layer_skill() {
    // 项目普通 .everlasting/skills/wf-brainstorm → 内置赢(BuiltinPlugin > Project)。
    let cache = SkillCache::arc();
    let tmp = tempfile::TempDir::new().unwrap();
    let proj = tmp.path();
    let dir = proj.join(".everlasting/skills/wf-brainstorm");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("SKILL.md"), "---\nname: wf-brainstorm\ndescription: mine\n---\nSHOULD NOT WIN").unwrap();
    let pp = proj.to_string_lossy().to_string();
    let r = find_skill_with_workflow(&cache, "wf-brainstorm", Some(&pp), Some("dev")).await.unwrap();
    assert_eq!(r.source, SkillSource::BuiltinPlugin, "builtin beats project-layer skill");
    assert_ne!(r.body, "SHOULD NOT WIN");
}
```

**核对点**:两个 Cache 都用 `::arc()` 构造(`SkillCache::arc()` loader.rs:466、`SubagentCache::arc()` loader.rs:620),返回 `Arc<Self>`,deref 后直接调方法。`find_skill_with_workflow(&cache, ...)` 传 `&Arc`(deref 生效),`cache.lookup_with_workflow(...)` 直接方法调用 —— 见现有测试 loader.rs:1393 / subagent/loader.rs:1699。

验证:`cargo test --lib skill::`(PKG_CONFIG_PATH 见顶部)。全绿进 Step 4。

---

## Step 4 — subagent loader 加 BuiltinPlugin 层

### 4.1 `SubagentSource` 加变体(`agent/subagent/loader.rs:93`)

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubagentSource {
    Builtin,
    User,
    Project,
    Plugin,
    /// app 内置 plugin agent(`include_str!` 常量)。07-09-workflow-builtin-plugin。
    /// 优先级:`Plugin > BuiltinPlugin > Project > User > Builtin`。
    BuiltinPlugin,
}
```

更新 `as_str`(loader.rs:100):

```rust
impl SubagentSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Builtin => "builtin",
            Self::User => "user",
            Self::Project => "project",
            Self::Plugin => "plugin",
            Self::BuiltinPlugin => "builtin-plugin",
        }
    }
}
```

### 4.2 抽出纯解析函数(复用 agent frontmatter parser)

现有 `load_agent_file`(`loader.rs:477`)是 `async fn(&Path, source) -> io::Result<Option<LoadedAgentFile>>`,内部 `read_to_string` + `parse_frontmatter` + size cap + name 校验。抽纯解析部分:

```rust
/// 纯解析:从 agent .md 文本构造 LoadedAgentFile。磁盘层与内置层共用(07-09)。
/// name 校验 / description fallback / tools_declared / isolation_declared 逻辑
/// 与原 load_agent_file 完全一致。
fn parse_agent_content(content: &str, source: SubagentSource) -> Option<LoadedAgentFile> {
    let (fm, body) = parse_frontmatter(content);
    let name = match fm.name.as_ref().map(|s| s.trim()).filter(|s| !s.is_empty()) {
        Some(n) => n.to_string(),
        None => {
            tracing::warn!("subagent: missing or empty `name` field, skipping");
            return None;
        }
    };
    if !is_valid_agent_name(&name) {
        tracing::warn!(name = %name, "subagent: `name` contains illegal characters, skipping");
        return None;
    }
    let description = match fm.description {
        Some(d) => d,
        None => {
            tracing::warn!(name = %name, "subagent: missing `description` field, falling back to empty string");
            String::new()
        }
    };
    let tools_declared = fm.tools.is_some();
    let isolation_declared = fm.isolation.is_some();
    let def = SubagentDef {
        name,
        description,
        system_prompt: body,
        tools: fm.tools.unwrap_or_default(),
        isolation: fm.isolation,
        model: fm.model,
    };
    Some(LoadedAgentFile {
        loaded: LoadedSubagent { def, source },
        tools_declared,
        isolation_declared,
    })
}
```

改 `load_agent_file`(`:477`)调用它(保留 IO + size cap):

```rust
async fn load_agent_file(
    path: &Path,
    source: SubagentSource,
) -> std::io::Result<Option<LoadedAgentFile>> {
    let meta = tokio::fs::metadata(path).await?;
    if meta.len() > MAX_AGENT_FILE_SIZE {
        tracing::warn!(path = %path.display(), size = meta.len(), max = MAX_AGENT_FILE_SIZE, "subagent: file exceeds size cap, skipping");
        return Ok(None);
    }
    let content = tokio::fs::read_to_string(path).await?;
    Ok(parse_agent_content(&content, source))
}
```

(原 load_agent_file 里的 name/description/tools 逻辑全部移入 parse_agent_content,行为不变。)

### 4.3 新增内置 agent 构造函数

```rust
/// 构造 app 内置 plugin 的 agents(07-09-workflow-builtin-plugin)。
/// 仅 `workflow_name == "dev"` 时返回;其他返回空。
/// 不走磁盘扫描 —— 内置源是 `include_str!` 常量。
pub(crate) fn builtin_plugin_agents(workflow_name: &str) -> Vec<LoadedAgentFile> {
    if workflow_name != "dev" {
        return Vec::new();
    }
    crate::agent::workflow::BUILTIN_DEV_AGENTS
        .iter()
        .filter_map(|(_role, body)| parse_agent_content(body, SubagentSource::BuiltinPlugin))
        .collect()
}
```

### 4.4 插入 `list_with_workflow`(loader.rs:747)

现有 layers 顺序(后插优先):
```
builtin → user → project → (有 wf 时)plugin
```
改为:
```
builtin → user → project → (有 wf 时)builtin-plugin → (有 wf 时)plugin
```

```rust
pub async fn list_with_workflow(
    &self,
    project_path: &str,
    workflow_name: Option<&str>,
) -> Vec<LoadedSubagent> {
    let wf = workflow_name.filter(|n| !n.is_empty());
    let mut layers: Vec<Vec<LoadedAgentFile>> = Vec::with_capacity(5);

    let builtin_files: Vec<LoadedAgentFile> = builtin_subagents()
        .iter()
        .cloned()
        .map(|def| LoadedAgentFile {
            loaded: LoadedSubagent { def, source: SubagentSource::Builtin },
            tools_declared: true,
            isolation_declared: true,
        })
        .collect();
    layers.push(builtin_files);
    layers.push(self.list_user_files().await);
    layers.push(self.list_project_files(project_path).await);
    // 07-09-workflow-builtin-plugin: 内置 plugin 层,在项目 plugin 之前。
    if let Some(wf) = wf {
        layers.push(builtin_plugin_agents(wf));
    }
    if let Some(wf) = wf {
        layers.push(self.list_plugin_files(project_path, wf).await);
    }
    merge_with_inheritance(layers)
}
```

(`lookup_with_workflow` 无需改 —— 它调 `list_with_workflow` find。)

### 4.5 `locate_agent_file`(loader.rs:827)加 BuiltinPlugin 分支

找到 `Plugin` 分支返回 Err 的位置(loader.rs:868 附近),加 BuiltinPlugin 同款:

```rust
// 在 locate_agent_file 的 match / if 链里:
SubagentSource::BuiltinPlugin => {
    return Err(anyhow::anyhow!(
        "builtin-plugin subagents are read-only compile-time constants; \
         to override, place a same-named .md in <project>/.everlasting/workflow/dev/agents/"
    ));
}
```

(核对 locate_agent_file 的实际写法是 match 还是 if 链,按其风格插入。)

### 4.6 测试(加在 `agent/subagent/loader.rs` tests 末尾)

```rust
#[tokio::test]
async fn builtin_plugin_agent_loaded_for_dev_in_empty_project() {
    // 空项目 + workflow=dev → researcher 命中内置 dev 角色(非 builtin researcher)。
    let cache = SubagentCache::arc();  // 构造见现有测试 loader.rs:1697
    let tmp = tempfile::TempDir::new().unwrap();
    let pp = tmp.path().to_string_lossy().to_string();
    let l = cache.lookup_with_workflow(&pp, Some("dev"), "researcher").await;
    let l = l.expect("builtin dev researcher should load");
    assert_eq!(l.def.name, "researcher");
    assert_eq!(l.source, SubagentSource::BuiltinPlugin);
    // 内置 dev researcher 的 system_prompt 含 delegation 占位符标记(来自 researcher.md)。
    assert!(!l.def.system_prompt.is_empty());
}

#[tokio::test]
async fn project_plugin_agent_overrides_builtin() {
    // 项目 .everlasting/workflow/dev/agents/researcher.md → 项目赢。
    let cache = SubagentCache::arc();
    let tmp = tempfile::TempDir::new().unwrap();
    let proj = tmp.path();
    let agents_dir = proj.join(".everlasting/workflow/dev/agents");
    std::fs::create_dir_all(&agents_dir).unwrap();
    std::fs::write(agents_dir.join("researcher.md"), "---\nname: researcher\ndescription: mine\ntools: [read_file]\n---\nCUSTOM RESEARCHER").unwrap();
    let pp = proj.to_string_lossy().to_string();
    let l = cache.lookup_with_workflow(&pp, Some("dev"), "researcher").await.unwrap();
    assert_eq!(l.source, SubagentSource::Plugin);
    assert_eq!(l.def.system_prompt, "CUSTOM RESEARCHER");
}
```

**核对点**:`SubagentCache::arc()` 构造(loader.rs:620,返回 `Arc<Self>`)。`builtin researcher`(SubagentSource::Builtin,来自 `builtin_subagents()` `subagent/mod.rs:463`)和 `builtin dev researcher`(SubagentSource::BuiltinPlugin,来自内置 workflow)是**两个不同的东西** —— 前者是 app 通用只读 agent,后者是 dev workflow 角色注入。测试断言 source 区分二者。

验证:`cargo test --lib subagent`(PKG_CONFIG_PATH 见顶部)。全绿进 Step 5。

---

## Step 5 — 全量验证 + 手动回归

### 5.1 全量测试

```bash
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib
```

全绿。若有既有测试因内置层引入而失败,逐个核对该测试是否依赖"空项目 = 无 plugin"的旧行为(Step 2.2 已处理 list_plugins 三例,其余测试同理排查)。

### 5.2 编译检查

```bash
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo check
```

无新 warning(`#[allow(unused_imports)]` 在 Step 1.2 加的,若 Step 3/4 已消费可去掉)。

### 5.3 优先级链手动核对(写个一次性测试或用 cargo test 输出)

确认四个场景:
1. 空项目 + workflow=dev → wf-brainstorm = BuiltinPlugin ✓(Step 3.6)
2. 项目 plugin 同名 → Plugin ✓(Step 3.6)
3. 项目普通 skills/ 同名 → BuiltinPlugin 赢 ✓(Step 3.6)
4. agent 三场景 ✓(Step 4.6)

### 5.4 本项目(everlasting)回归

确认本项目 `.everlasting/workflow/dev/` 仍生效(项目 plugin 层优先):在本项目跑 `cargo test`,确认 researcher/implementer/checker 仍从项目层加载(Source::Plugin,非 BuiltinPlugin)。

---

## 风险 / 回滚点

| 风险 | 护栏 | 回滚 |
|---|---|---|
| merge 顺序插错 → 优先级链错 | Step 3.6 / 4.6 的优先级断言测试 | 单步 revert 对应 step |
| 内置 frontmatter 与项目层解析不一致 | parse_skill_content / parse_agent_content 共用同一 parser | 代码已消除该风险 |
| 既有测试依赖"空=无 plugin"旧行为 | Step 2.2 列出全部需改断言;Step 5.1 全量测试 | 改断言回滚 |
| include_str! 路径错 | Step 1.3 单独跑 builtin 模块测试 | 修路径 |

每步独立可提交;出问题单 step revert。内置源文件(`resources/builtin-workflow/`)删除 + 撤销 builtin.rs 即回退到纯项目 plugin 行为。

## 完成定义(Definition of Done)

- [ ] Step 0-4 全部测试通过
- [ ] Step 5.1 全量 `cargo test --lib` 绿
- [ ] Step 5.2 `cargo check` 无新 warning
- [ ] 优先级链 4 场景测试全过
- [ ] 本项目(everlasting)仍用项目层 plugin(回归不破坏)
