## Scenario: BuiltinPlugin source layer (07-09-workflow-builtin-plugin, 2026-07-09)

**Context**: 把 `dev` workflow 三层(workflow.json / agents/ / skills/)做成 app
内置常量,新项目开箱即用。优先级链
`Project-plugin > BuiltinPlugin > Project > User`(skill + agent),
`Plugin > BuiltinPlugin > Project > User > Builtin`(agent — 比 skill 多
`Builtin` 底)。engine/content 分离不变,只加一层;项目可继续用
`.everlasting/workflow/<name>/` 覆盖。

**Builtin source** (`agent/workflow/builtin.rs`, `include_str!` 编译期常量,
非 Tauri `bundle.resources`):
- `BUILTIN_DEV_WORKFLOW_JSON` + `BUILTIN_DEV_SKILLS` (5 项:
  `wf-overview` / `wf-brainstorm` / `wf-before-dev` / `wf-check` /
  `wf-update-spec`) + `BUILTIN_DEV_AGENTS` (3 角色:
  `researcher` / `implementer` / `checker`)。
- `BUILTIN_PLUGIN_NAMES: &[&str] = &["dev"]`(目前只有 dev;未来加
  `review` 时在此追加 + 加对应常量)。
- `builtin_workflow_json(name) -> Option<&'static str>` 入口,`None` =
  交由 caller 继续降级。

**Priority chain 接入**:
- `workflow::def::load_workflow`:项目 4 个降级点(NotFound / read err /
  JSON err / validate err)统一先试 `load_builtin` → 再 `default_workflow()`。
  内置 == `default_workflow()` 常量逐字等价,故形状断言仍通过。
- `workflow::def::list_plugins`:项目扫描 ∪ `BUILTIN_PLUGIN_NAMES`,
  去重 + 字母序。
- `skill::loader`:`SkillSource::BuiltinPlugin` 加;`merge_skill_layers`
  插入顺序 = `user → project → builtin-plugin → project-plugin`(后插
  覆盖);`find_skill_in_layers` 查找顺序 = `project-plugin →
  builtin-plugin → project → user`(先命中返回)。
- `agent::subagent::loader`:`SubagentSource::BuiltinPlugin` 加;
  `list_with_workflow` 在 project-plugin 之前插入 builtin-plugin;
  `lookup_with_workflow` 调 `list_with_workflow` find,无需独立改。
- `parse_skill_content` / `parse_agent_content` 从磁盘层抽纯函数,内置层
  复用同一 frontmatter parser → 解析行为 100% 一致。
- `locate_agent_file(BuiltinPlugin)` 返回 `InvalidInput` 错误
  (只读 + 提示覆盖路径, 与 `Plugin` 同款),`commands::subagents.rs` 的
  `set_subagent_model` match 也加同款 `InvalidRequest` 分支。

**新测试覆盖**(`skill::loader::tests` / `subagent::loader::tests` / `workflow::tests`):
- 空项目 + `wf="dev"` → 内置命中 (`BuiltinPlugin` source)
- 项目 plugin 同名 → `Plugin` source 赢(项目可覆盖)
- 项目普通 `.everlasting/skills/<name>` 同名 → `BuiltinPlugin` 赢
  (workflow 语义层 > 普通 project 层)
- `list_plugins_returns_empty_when_root_missing` 改断言为 `["dev"]`
  (内置层兜底可见性);`discovers_alphabetical` / `ignores_dirs_without
  _workflow_json` 改断言含 dev
- 4 个新加测试 + 1 个 list_skill_infos_plugin_missing_dir 调整

**Out of scope**:第二个内置 plugin;Tauri `bundle.resources`;前端
`PluginSelect.vue` 改动(数据源是 IPC `list_workflow_plugins`,零改动);
内置 plugin 热更新(编译期常量,要换改项目 `.everlasting/`)。

---
