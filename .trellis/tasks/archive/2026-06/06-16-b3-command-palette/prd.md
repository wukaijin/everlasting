# B3 /command 命令面板

## Goal

输入框输入 `/` 触发命令自动补全面板。支持两类命令：

1. **内置命令**(`clear` / `model` / `mode` / `help` 等) — 由前端 action / 现有 Tauri command 执行,不发给 LLM。
2. **用户自定义命令**(`.everlasting/commands/*.md` frontmatter + Markdown 模板) — 模板展开后**作为 user message 走 `send()`**。

**为什么**:对齐 Claude Code 的 slash command 体验,把"常做的几步"固化为可复用入口;为 B2(@文件)/B4(skill) 预置共享的 `<TriggerMenu>` 触发器骨架。

## What I already know

### 前端(自助探索)

- `app/src/components/chat/ChatInput.vue`:`input.value` v-model(line 82)、`textareaEl` ref、`onKeydown`(Enter 发送 / Shift+Enter 换行 / IME 安全)、`submit()`(line 296)→ `emit("send", text)`。
- `app/src/stores/chat.ts`:`send(text)`(line 943)、`requestSetMode(sessionId, mode)`(line 1076,带 Yolo 门控)、`renameSession`(line 696)、`setSessionColor`(line 702)、`deleteSession`(line 677)。**无"清空消息保留 session"** —— 只有删 session 重建。
- Mode 切换:`ModeSelect.vue`(ChatInput 左侧)→ `requestSetMode` → `invoke("set_session_mode")`。
- Model 切换:`ModelSelect.vue` → `invoke("update_session_model_id")`,per-session override。
- reka-ui 当前**仅**用 Tooltip + DropdownMenu(`SessionList.vue` 右键菜单),**无 Combobox/Command/Popover 列表**可复用 → 命令面板需新建。
- Markdown 渲染:`utils/markdown.ts`(`marked` + `DOMPurify`,GFM + breaks)。
- session 重命名/8 色:右键 DropdownMenu → `startEditing`/`commitEdit` + `COLOR_PALETTE`。

### 后端(自助探索)

- `app/src-tauri/src/commands/mod.rs`:29 个命令注册中心。
- `sessions.rs`:`list/create/load/delete/rename_session`、`set_session_color`、`update_message_latency`、`record_tool_duration`。
- `permissions.rs`:`set_session_mode`(line 106)、`permission_response`、`grant_tool_permission`、`list_session_audit_events`。
- `memory/file.rs`:4 文件路径约定(`~/.config/everlasting/{CLAUDE,AGENTS}.md` + `<project.path>/{CLAUDE,AGENTS}.md`),`load_file_inner` **直接读 UTF-8,不解析 frontmatter**。
- `.everlasting/` 现状:`.everlasting/outputs/`(shell spillover)已存在;`.everlasting/commands/` **未实现,需新建**。
- `Cargo.toml`:**无** `serde_yaml`/`serde_yml`。
- `tools/mod.rs`:`builtin_tools()` 8 个 tool。command 系统**不与 tool 注册耦合**(command 是用户手动触发的文本展开,不是 LLM 可调 tool)。
- `AuditKind`:10 类,command 执行**不需要写审计**(审计只针对 ⑨ 关决策路径)。
- 路径:`user_dir()` = `~/.config/everlasting/`;项目根 = `project.path`。

## Assumptions (temporary)

- 内置命令复用现有 store action / invoke(mode/model/rename/color/delete),不发 LLM。
- 用户命令 = YAML frontmatter(`name` + `description`,可选 `argument-hint`)+ Markdown body;body 展开后作为 user message 走 `send()`。
- 目录约定:project `<project.path>/.everlasting/commands/*.md` + user `~/.config/everlasting/commands/*.md`,project 覆盖 user 同名(优先级待定)。
- frontmatter 解析:倾向 `serde_yml`(BACKLOG §1.3 已定 `serde_yaml` 弃用迁移到此分叉);待定是否手写 `---` 分割更轻。

## Open Questions

> 只列 Blocking / Preference;其余自行查代码已得。

1. ~~**MVP scope**~~ — ✅ **已决(2026-06-16)**:内置 + 用户自定义(基础版);不做参数补全/插值。
2. ~~**内置命令集合**~~ — ✅ **已决(2026-06-16)**:`/help` `/clear` `/new`(聚焦"补缺失动作";不做 `/mode` `/model` 已有按钮的别名,不做 `/compact`)。
3. ~~**/clear 语义**~~ — ✅ **已决(2026-06-16)**:清空消息保留 session(保留 title/color/mode/model/project/created_at)。新后端 command `clear_session_messages`(`DELETE FROM messages WHERE session_id = ?`)+ 前端取消 inflight + 清 permission asks;token usage 是否同步重置待实现时定。
4. ~~**frontmatter 解析**~~ — ✅ **已决(TECH.md §1.4 + §5 锁定)**:用 `serde_yml`(轻量纯 Rust);后端建轻量通用 `ResourceLoader`(解析 frontmatter + 扫描目录),`/command` 为首个 caller,结构预留 Skill/Memory/Role 复用(§5 契约),不过度抽象。

## Requirements (evolving)

- **MVP scope(2026-06-16 决)**:内置命令 + 用户自定义命令(基础版);**不做**参数补全、**不做**模板插值(后置 v2)。
- 输入框 **行首 `/` 且当前行为空**时触发命令面板(多行只看光标所在行);自动补全 + 键盘上下选 + 回车执行 + Esc 关闭;对齐 Claude Code。
- 内置命令至少 `help`(列出所有可用命令)。
- 内置命令首版:**`/help`**(列全部命令,含用户命令)、**`/clear`**(清空当前 session 消息)、**`/new`**(新建 session)。
- 用户自定义命令从 `.everlasting/commands/` 加载。

## Acceptance Criteria (evolving)

- [ ] 输入 `/` 弹出命令面板,列出内置 + 用户命令,模糊匹配前缀。
- [ ] 键盘 ↑↓ 选中、Enter 执行、Esc 关闭、不与现有 Enter 发送冲突。
- [ ] 内置命令执行不发给 LLM;用户命令 body 展开后作为 user message 发送。
- [ ] 用户命令 frontmatter 解析(`name`/`description`),坏格式文件优雅降级(跳过 + 提示)。
- [ ] `vue-tsc --noEmit` + `cargo check` 0 warning;后端单测覆盖 frontmatter 解析 + 目录扫描。

## Definition of Done

- 后端单测(frontmatter 解析 / 目录扫描 / 坏格式降级)+ 前端 type-check 绿。
- `docs/ROADMAP.md` §1.2 补 B3 落地条目 + commit hash。
- `<TriggerMenu>` 组件为 B2/B4 留扩展位(文档注释说明)。

## Out of Scope (explicit)

- B4 Skill 系统(LLM 可调 `use_skill` tool)。
- B2 @文件补全(共享 TriggerMenu 骨架但功能独立,另起 task)。
- 用户命令复杂插值(`{{selection}}` / `{{args}}`)— scope 决策移出,后置 v2。
- 命令子参数补全(`/model <tab>` 列出 model 列表)— scope 决策移出,后置 v2。
- 用户命令写审计。
- 内置命令 `/mode` `/model`(已有专门按钮,是别名)、`/compact`(首版不做)。
- **输入框内 token 着色**(`/command` / `@file` / skill 高亮) — 横切视觉增强,实现需 textarea overlay(~150–250 行 + 光标/IME 对齐坑)或引入 CodeMirror 6(TECH.md §1.2 候选,需重构 ChatInput);与 B3 核心流程解耦,建议随 **B2(@文件,引入第二类 token)同期做**,一次改造覆盖多 token,并在那时评估 CodeMirror 6 是否正式引入。BACKLOG §1 输入层留候选。

## Technical Approach

### 后端
- **不加 serde_yml**(`serde_yml`/`serde_yaml` 均已 deprecated);通用 `ResourceLoader` 内置手写 frontmatter parser(split `---` + `key:value` 单行标量,~30 行)。
- 新建轻量通用 `ResourceLoader`:解析 frontmatter(`name`/`description`/可选 `argument-hint`)+ 扫描目录。`/command` 首个 caller;结构预留 Skill/Memory/Role 复用(TECH.md §5),不过度抽象(YAGNI)。
- 目录约定:project `<project.path>/.everlasting/commands/*.md` + user `~/.config/everlasting/commands/*.md`;**project 覆盖 user 同名**(对齐 memory project 覆盖 user 原则);内置命令优先,用户同名需 `/custom:` 前缀隔离(BACKLOG §1.3)。
- 新 Tauri command:`list_commands(project_id)` → 返回内置 + 用户命令元数据;`clear_session_messages(session_id)` → `DELETE FROM messages WHERE session_id` + 级联清理(参考现有 `delete_session`)。
- mtime fence 缓存(复用 RULE-C-001 模式)避免每次 `/` 重扫目录。

### 前端
- 新建 `<TriggerMenu>` 组件(reka-ui Popover/Listbox primitives 或手写,参考 ModeSelect 手写 popover 模式);为 B2(@文件)/B4(skill) 留扩展位(文档注释)。
- ChatInput 监听 `/` 触发(触发条件见 Q&A),键盘 ↑↓ + Enter 执行 + Esc 关闭。
- 内置命令分发:`/help` → 展开面板全列表;`/clear` → invoke `clear_session_messages` + 清前端内存;`/new` → `createSession`。
- 用户命令:body 展开后作为 user message 走 `send()`(命令名不进消息)。

### 安全
- 模板只插值不 exec(BACKLOG §3.3)。
- 命令名冲突:内置优先,用户同名需前缀。
- 坏格式 frontmatter:跳过 + 前端提示,不崩。

## Decision (ADR-lite)

**Context**: B3 /command 面板,涉及触发器骨架复用、frontmatter 解析、目录约定、/clear 语义。
**Decision (2026-06-16)**:
- MVP = 内置(`/help` `/clear` `/new`)+ 用户自定义(`.everlasting/commands/*.md` 基础模板,无参数补全/无插值)。
- frontmatter **手写 parser**(split `---` + `key:value`,~30 行零依赖)——`serde_yml`/`serde_yaml` 均已 deprecated(TECH.md §1.4 过时,待更新);通用 `ResourceLoader` 内置解析,§5 契约仍成立;未来字段复杂化再上 maintained YAML 库(如 `serde_yaml_neo`)。
- `/clear` = 清空消息保留 session(新 `clear_session_messages` command)。
- 触发器 `<TriggerMenu>` 为 B2/B4 复用预留。
**Consequences**: `serde_yml` 一次性投资惠及后续 Skill/Memory/Role;通用 loader 需克制避免过度抽象;参数补全/插值后置 v2 意味 `/model` `/mode` 暂不进内置(已有专门按钮)。

## Technical Notes

- **⚠️ serde_yml 废弃发现(2026-06-16)**:`serde_yml`(TECH.md §1.4 锁定项)+ `serde_yaml`(前代)均已 deprecated(crates.io 标注 "Deprecated YAML library")。决策改为手写 frontmatter parser(B3 字段简单)。TECH.md §1.4 需更新(标 serde_yml 废弃 → 手写/B3 内置,未来复杂字段上 maintained fork 如 serde_yaml_neo)。
- BACKLOG §1.3 已有技术评估:frontmatter + Markdown,命令注册表启动扫描,内置优先/用户覆盖用 `/custom:commit` 命名隔离,模板 ≤4KB。
- BACKLOG §1 输入层三者(图片/@/command)共享 `<TriggerMenu>` 组件 —— B3 先建此骨架。
- 安全:模板**只插值不 exec**(§3.3);命令名冲突内置优先。
- spec 索引:`.trellis/spec/backend/`、`.trellis/spec/frontend/`(待 Phase 2 curate)。

## Research References

> 待 research-first(若 Q&A 收敛到需要):frontmatter 解析方案对比(serde_yml vs 手写)、slash command UX 约定(Claude Code / Cursor / GitHub Copilot)。
