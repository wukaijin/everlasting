# Skill 系统 stretch: allowed-tools + 用户手动 /skill 入口

> 状态:**待规划**(未 activate)。本任务只产出 PRD,不实现。
> 来源:B4 MVP(`06-18-skill-system`,commit `96b6f93`)Out of Scope 的两个 stretch。

## Goal

B4 skill 系统 MVP 已于 2026-06-18 落地(纯 LLM 自动触发,`use_skill` 虚拟 tool + 三层渐进披露)。本任务规划其控制层的两个 stretch:

1. **`allowed-tools`** — skill frontmatter 声明该 skill 可用的 tool 子集(范围/安全控制)
2. **用户手动 `/skill` 入口** — 复用 B3 `<TriggerMenu>`,让 skill 也能被用户强制触发(不只依赖 LLM 按 description 自动命中)

两者**正交**,可独立分 PR 实现。

## Background

- B4 MVP PRD + 决策:`.trellis/tasks/archive/2026-06/06-18-skill-system/prd.md`
- 调研:`docs/research/skill-system-survey.md`(§3.3 Claude `allowed-tools` / §5.3 skill 是 command superset / §5.5 parser 升级决策 / §6.1 MVP stretch 清单)
- MVP 明确把这两项放 Out of Scope(MVP 先验证 L0→L1→L2 数据流跑通,frontmatter 最小集 `name`+`description`,不做用户手动入口)

---

## Stretch 1:`allowed-tools`

### What

SKILL.md frontmatter 加数组字段:
```yaml
---
name: review-pr
description: review 一个 PR / diff
allowed-tools: [read_file, grep, git_diff]
---
```

### Why

- **范围/安全控制**:限制 skill 激活后只动用其声明的 tool,防止 skill 指令引导 LLM 调越权 tool(尤其 project/untrusted skill)
- **对齐业界**:Claude Code `allowed-tools` / `disallowed-tools`;agentskills.io 标准 `allowed-tools`(实验性)
- 让 skill 成为可信任的"能力封装"(声明它需要什么)

### Decisions (grill 收敛)

- **✅ 语义 = 声明性**:frontmatter 只记录/提示,不强制。skill 正文自然引导 LLM 用某些 tool,实际 tool 访问控制靠现有 ⑨ 5-tier 权限层(已落地)。零新机制——无"激活期"状态机、无 tool list 动态化、无 ⑩ 拦截。对齐 Claude Code 默认(`allowed-tools` 配合权限规则,非硬限)。
  - ⇒ **作用域问题消失**:无强制 → 无"激活期"概念,allowed-tools 只是静态元数据
  - ⇒ **与权限系统关系**:allowed-tools **不融入** ⑨,只是 `SkillResource` 字段(供 UI/诊断/L0 清单提示);真正的 tool 访问控制仍走 ⑨ 现有 5-tier

- **✅ parser = 手写扩展**:现有 `parse_frontmatter`(B3 手写)加单行数组解析(`allowed-tools: [a, b, c]` → 剥 `[` `]` + 逗号 split + trim),~20 行零新依赖。声明性下 frontmatter 字段少(name/description/allowed-tools),够用;未来加复杂字段(多行/嵌套)再升级 `serde_yaml_neo`。

> **Stretch 1 grill 完成**(语义/作用域/权限关系/parser 全收敛)。落地清单:`SkillResource` 加 `allowed_tools: Vec<String>` 字段 + `parse_frontmatter` 扩展单行数组 + `build_skill_listing_block` 可选展示 allowed-tools 提示。

### 依赖

- 若选强制白名单:skill 正文注入后的 tool list 过滤机制(per-skill tool subset)

---

## Stretch 2:用户手动 `/skill` 入口

### What

输入框行首 `/` 触发 `<TriggerMenu>`(复用 B3 第二 caller,像 B2 `@文件`),列出可用 skill;用户选中 `/skill-name` 后,该 skill 正文作为 user message 发送(强制触发,不等 LLM 自动调 `use_skill`)。

### Why

- **用户主动控制**:明确要 LLM 用某 skill(不依赖模型 description 匹配是否命中)
- **对齐业界双轨触发**:Claude Code(`/skill-name` 手动 + `Skill` tool 自动)/ Hermes(skill 自动注册成 slash command)
- **BACKLOG §2 原设计**含此入口("双入口:用户 `/skill` + LLM `use_skill`")

### Decisions (grill 收敛)

- **✅ 触发后行为 = 正文作 user message 走 `send()`**:复用 B3 command body 展开路径(`get_skill_body` IPC 拿 body → 前端当 user message 发 `send()`)。零改 agent loop,与 `/command` 完全同构。语义对齐:用户选 skill = 用户要 LLM 用它(跟 command 一致)。
  - ⇒ 前端数据源:`list_skills` + `get_skill_body` IPC(对齐 `list_commands` / `get_command_body`)

- **✅ TriggerMenu = 合并面板**:`/` 触发一个面板同时列 command(builtin + custom)+ skill,带 source 标签(builtin / command / skill)。选中路由区分:builtin→客户端动作;custom command→`get_command_body`;skill→`get_skill_body`。符合 survey §5.3 skill 是 command superset(Claude Code / 列所有)。UX 统一,代价是 TriggerMenu 数据源合并 + 路由区分(后端合并 list 更干净)。

- **✅ 跨类型同名优先级 = builtin command 始终胜出 + 同名 skill skip+warn;其余同名 skill 覆盖 custom command(superset)**:保留 B3 builtin 不可被覆盖保护(skill-system 0.5 决策延续);同类型(project/user)照 B3 precedence 覆盖。实现:后端合并 list IPC(`list_panel_items` 或扩展 `list_commands` 返 source 标签)做去重 + 优先级 + warn。

> **Stretch 2 grill 完成**(触发行为/TriggerMenu/同名优先级全收敛)。落地清单:前端 `<TriggerMenu>` 合并数据源 + 路由区分 + source 标签;后端合并 IPC + 同名 warn(对齐 B3 builtin 保护扩展到跨类型)。

### 依赖

- B3 `<TriggerMenu>` 第二/第三 caller 机制(B3 设计时已预留为共享组件)
- skill listing/body IPC(对齐 `list_commands` / `get_command_body`)

---

## Requirements

### Stretch 1 — allowed-tools(声明性)
- `SkillResource` 加 `allowed_tools: Vec<String>` 字段(空 Vec 表示未声明)
- `parse_frontmatter` 扩展单行数组解析:`allowed-tools: [a, b, c]` → 剥 `[` `]` + 逗号 split + trim(忽略空元素 + 重复;非数组格式如多行/嵌套 → 该字段空 + warn,对齐 B3 bad-file skip 容错)
- `build_skill_listing_block` 可选展示:`"  (tools: read_file, grep)"` 跟在 description 后(让模型在 L0 看到声明的 tool 倾向,信息性)
- **不**修改 `permissions/mod.rs`(声明性,无 ⑨ 融入;无 ⑩ 拦截)
- **不**修改 `tool_defs` / `execute_tool`(无 tool list 动态化,无强制拦截)

### Stretch 2 — 用户手动 /skill 入口(合并面板)
- 后端:新增合并 list IPC(建议 `list_panel_items(project_id)`,或扩展 `list_commands` 返 source 标签)→ 合并 `list_commands` + `list_skills`,按同名优先级去重:
  1. builtin command 始终胜出
  2. 其余同名 skill 覆盖 custom command(superset)
  3. 同类型 project 覆盖 user(B3 precedence)
  4. 跨类型同名(builtin vs skill):builtin 胜出,同名 skill skip+warn(对齐 B3 builtin 不可被覆盖)
- 后端:新增 `get_skill_body(skill_name, project_id)` IPC(对齐 `get_command_body`),返 `SkillResource.body`(用于前端作 user message)
- 前端:`<TriggerMenu>` 数据源合并(builtin command + custom command + skill)+ source 标签(builtin / command / skill)+ 选中路由:
  - builtin → 客户端动作(沿用 B3 `executeCommand`,如 /clear /new /help)
  - custom command → `get_command_body` → 作 user message 发 `send()`(B3 已有)
  - skill → `get_skill_body` → 作 user message 发 `send()`(对齐 B3 command 路径)
- 前端:`ChatInput` 的 `/` 触发逻辑不变(已接 TriggerMenu);TriggerMenu 数据源加 skill
- **不**改 agent loop / `run_chat_loop` / `use_skill`(手动 /skill 走 user message 路径,不走 use_skill 虚拟 tool)

## Acceptance Criteria

### Stretch 1
- [ ] SKILL.md 含 `allowed-tools: [read_file, grep]` → `SkillResource.allowed_tools = ["read_file", "grep"]`
- [ ] SKILL.md 无 `allowed-tools` → `SkillResource.allowed_tools = []`
- [ ] `allowed-tools: []` 或 `allowed-tools: not_an_array` → 空 + warn(容错)
- [ ] 包含重复/空格的数组 → 去重 + trim(`[a, a, b ]` → `["a", "b"]`)
- [ ] `build_skill_listing_block` 在 description 后展示 `  (tools: a, b)`(仅当 `allowed_tools` 非空)
- [ ] `cargo check` 零 warning,`cargo test` 无回归

### Stretch 2
- [ ] `/` 触发面板同时列 builtin command + custom command + skill,带 source 标签
- [ ] 选中 builtin command 走客户端动作(不调用 IPC)
- [ ] 选中 custom command 走 `get_command_body` → user message 发送
- [ ] 选中 skill 走 `get_skill_body` → user message 发送
- [ ] builtin command 与 skill 同名 → 面板只显示 builtin(同名 skill 被后端 skip + warn)
- [ ] custom command 与 skill 同名 → 面板只显示 skill(superset 覆盖)
- [ ] 同类型 skill(project vs user)同名 → project 胜出(B3 precedence)
- [ ] `cargo check` 零 warning,`cargo test` 无回归

## Definition of Done

- 单元测试覆盖 `parse_frontmatter` 数组解析 + `SkillResource.allowed_tools` 字段
- 集成测试覆盖合并面板 IPC 的同名优先级(builtin skip skill / skill 覆盖 custom / project 覆盖 user)
- 前端 TriggerMenu 路由区分(若项目有前端测试则单测,否则手动验证)
- `PKG_CONFIG_PATH=... cargo test --lib` 全绿
- ROADMAP 第三档 B4 stretch 划掉 + IMPLEMENTATION §4 ADR

## Technical Approach

### Stretch 1 数据流
```
SKILL.md (frontmatter 含 allowed-tools: [a, b])
  → parse_frontmatter 扩展单行数组解析
  → SkillResource { allowed_tools: ["a", "b"], ... }
  → build_skill_listing_block (description 后追加 "  (tools: a, b)")
  → L0 清单 message 注入
```
零新机制;仅 SkillResource 字段 + parser 扩展 + listing 渲染。

### Stretch 2 数据流(手动 /skill)
```
用户在 ChatInput 输入 "/"
  → TriggerMenu 显示(builtin command + custom command + skill 合并,带 source 标签)
  → 选中 skill "review-pr"
  → 前端调 get_skill_body("review-pr") → SkillResource.body
  → 前端把 body 作 user message 发 send()(对齐 B3 command body 展开)
  → agent loop 正常走 messages → provider.send
```
零 agent loop 改动;纯前端 + 后端 IPC 增。

### 精确接入点(预估,实施时确认)
- **Stretch 1**:`app/src-tauri/src/skill/loader.rs`(parser 扩展 + `SkillResource` 加字段 + listing 渲染)
- **Stretch 2 后端**:`app/src-tauri/src/commands/` 新增 `panel.rs` 或扩展 `command_palette.rs`(合并 list + `get_skill_body` IPC)+ `app/src-tauri/src/commands/mod.rs` + `lib.rs` 注册 IPC
- **Stretch 2 前端**:现有 `app/src/components/` `<TriggerMenu>` 数据源加 skill + source 标签 + 路由 dispatch(builtin→客户端 / custom→get_command_body / skill→get_skill_body)+ `app/src/stores/` chat store 加 skill body 发送

---

## Out of Scope(本任务也不做)

- `disable-model-invocation` / `user-invocable` 开关(Claude Code 细粒度触发控制)
- L0 清单预算制(1% 窗口)
- skill 自我创建(Hermes `skill_manage`)
- 条件激活(Hermes `requires_toolsets` / `fallback_for_toolsets`)
- skill 与 command 泛型 `ResourceLoader<Kind>` 合并

## Implementation Plan(待 brainstorm 后定)

两个 stretch 正交,建议:
- **PR1** = `allowed-tools`(含 parser 升级前置 + 语义决策落地)
- **PR2** = 用户手动 `/skill` 入口(前端 TriggerMenu + IPC)

PR 顺序可换;`allowed-tools` 偏后端/安全,`/skill` 偏前端/UX。

## Research References

- [`docs/research/skill-system-survey.md`](../../../docs/research/skill-system-survey.md) — §3.3(Claude `allowed-tools` 字段)/ §5.3(skill 是 command superset + 同名优先级)/ §5.5(parser 升级 A/B/C 决策)/ §6.1(MVP stretch 清单)
- [`.trellis/tasks/archive/2026-06/06-18-skill-system/prd.md`](../archive/2026-06/06-18-skill-system/prd.md) — B4 MVP,Out of Scope 记录这两项 + 4 个已定决策

## Technical Notes

- B4 MVP 代码:`app/src-tauri/src/skill/loader.rs`(`SkillResource` / `parse_frontmatter` 当前只解 name+description 标量)+ `tools/use_skill.rs`
- B3 复用基准:`app/src-tauri/src/resource_loader.rs`(`CommandCache` / `list_commands` / `get_command_body` IPC 模式)+ 前端 `<TriggerMenu>`
- parser 升级口子:`resource_loader.rs:9-16` 注释已预留("Future Skill loaders … graduate to `serde_yaml_neo`")
- 权限层:`app/src-tauri/src/agent/permissions/mod.rs`(⑨ 5-tier,`classify_tool` / `filter_tools_for_mode`)
- BACKLOG §2 双入口原设计:`docs/BACKLOG.md`
