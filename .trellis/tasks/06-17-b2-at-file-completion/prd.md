# B2 @文件补全

## Goal

输入框输入 `@`（at-start）触发**文件路径补全面板**（对齐 Claude Code at-mention），选中后插入 `@relpath` token；PR2 起 agent loop 解析 token 注入文件内容。复用 B3 已建好的 `<TriggerMenu>` 骨架。

**为什么**：输入层三件套（图片 / @文件 / /command）的第二件，B4（skill）之前最后一次骨架验证。

## Decision (ADR-lite)

**Context**: @文件选中后 agent 收到什么 + 横切议题（着色/CodeMirror）。Research 实锤 CC 注入文件内容（非路径提示）。

**Decision (2026-06-17)**:
- **渐进式**：PR1 前端 @面板 + fuzzysort + 插 `@relpath` token（路径提示可用形态）；PR2 后端 agent loop 解析 @token 注入文件内容 + 大文件截断/降级，升级到完整 CC 体验。
- **触发条件 at-start**（对齐 CC，复用 TriggerMenu trigger-char；多行看光标所在行，同 B3）。
- **token 着色 / CodeMirror 不做**——独立架构改动，留后续独立 task（@token 纯文本形态完全可用）。
- **fuzzy 匹配位置** — 改 `<TriggerMenu>` 加 `fuzzy` prop（true 时 fuzzysort 替换内置 prefix filter；默认 false 保持 B3 行为），B2/B4 共享，需回归测 B3 command palette 不退化。

**Consequences**: PR1 独立可交付、风险低；PR2 触及 chat/agent loop 核心路径，风险隔离在后端单测；最终对齐 CC。着色延期不影响功能可用性。

## What I already know

### 前端（已 inspect）
- **`ChatInput.vue`** B3 接入点清晰，B2 对称复制一套：`onKeydown`（line 263）加 `filePaletteOpen` 路由分支（与 command palette **互斥**）；`syncCommandPalette`/`detectCommandTrigger`/`currentLineInfo`/`onCommandSelect`（line 389-587）是 @版本模板；`triggerMenu` ref（line 372）当前被 command palette 占用，B2 需第二个 ref 或合并调度。
- **`TriggerMenu.vue`** 为 B2 预置到位：`trigger="@"` / `items` / `#row` slot（文件图标+相对路径）/ `headerLabel="文件"` / `triggerEl`；⚠️ 内置 prefix filter（line 131-135），fuzzy 需协调（见 Q3）。`TriggerMenuItem`：`key`(绝对路径)/`name`(相对路径)。

### 后端（方向已明）
- tool `list_dir`/`glob`/`grep` + boundary `projects/`（is_within_root）可复用。
- PR2 注入与 `memory/loader.rs::build_instructions_blocks()` 同构。

### Research（见 Research References）
- CC @ = at-start + **注入文件内容**（truncation 100→2000 lines）+ `@relpath` 不加 `/` + 多文件 + 不支持行号。
- 模糊匹配：前端 **fuzzysort（3.2KB/0dep）** 一次 IPC 缓存 + 本地匹配（<1ms@几千文件）；nucleo 留后续 Rust 侧。`frizzy` 查无；`fuzzy-matcher` 停更；`flexsearch` 非同类。

## Open Questions

1. ~~选中后语义~~ ✅ 渐进式。2. ~~触发条件~~ ✅ at-start。3. ~~着色/CodeMirror~~ ✅ 不做，留独立 task。4. ~~fuzzy 位置~~ ✅ 改 TriggerMenu 加 `fuzzy` prop。

> 所有决策已收敛（2026-06-17）。

## Requirements

- 输入 `@`（at-start）触发文件补全面板，复用 `<TriggerMenu>`；与 `/command` palette 互斥。
- 后端 `list_files` 列举 `currentCwd` 文件树（gitignore 过滤 + node_modules/.git/target/dist 默认排除 + 深度/数量上限）+ mtime fence 缓存。
- 前端 fuzzysort 模糊匹配（首次 IPC 拉取缓存，按键本地匹配 top-N）。
- 键盘 ↑↓/Enter（插 `@relpath` token）/Esc，不冲突 Enter 发送/IME。
- PR2：agent loop 解析 `@relpath` → 注入文件内容（大文件截断/降级路径+preview）。

## Acceptance Criteria

- [ ] 输入 `@` 弹文件补全面板，fuzzysort 模糊匹配，结果有数量上限。
- [ ] 键盘 ↑↓/Enter/Esc 正确，与 `/command` palette 互斥，不冲突 Enter/IME。
- [ ] 复用 `<TriggerMenu>`（第二个 caller，验证骨架）；fuzzy 改动回归测 B3 command palette 不退化。
- [ ] 后端 `list_files`（gitignore + 上限 + mtime fence）单测。
- [ ] （PR2）@token 解析注入 + 大文件截断降级 + 越界/坏路径降级 + 单测。
- [ ] `vue-tsc --noEmit` + `cargo check` 0 warning。

## Technical Approach

### 前端 PR1
- `<TriggerMenu>` 第二 caller：`trigger="@"` `headerLabel="文件"`，`#row` slot 渲染文件图标 + 相对路径（monospace）。
- ChatInput 加 `filePaletteOpen` + `detectFileTrigger()`（at-start `@`，复用 `currentLineInfo`）+ `syncFilePalette()` + `onFileSelect()`（插 `@relpath`，光标定位 token 后，保留 token 前后文本）。
- `onKeydown` 路由：两 palette 互斥（同时只一 open），各自拦截 ↑↓/Enter/Esc。
- fuzzysort：@ 触发首次 `list_files` IPC 拉列表缓存，按键本地匹配 top-N（~50）。

### 后端 PR1（已实现）
- 新 Tauri command `list_files(project_id)`（`commands/files.rs`）→ 顶层 `files::walk_files`（`tokio::task::spawn_blocking`：std::fs + git2 同步遍历）。
- 排除：默认排除列表（`.git`/`node_modules`/`target`/`dist`/`build`/`out`/`.next`/`.cache`/`__pycache__`/`coverage`/`.DS_Store` 等）+ git2 `is_path_ignored`（git 项目 honors `.gitignore`；非 git 降级仅默认排除）。
- 上限：`MAX_DEPTH=15` + `MAX_FILES=5000`（达上限停止）。返回 root-relative `/` 路径，字母序。8 单测通过。
- **实现修正（偏离 brainstorm 假设）**：不做后端 mtime fence 缓存——源码树变化频繁，B3 `CommandCache`（少文件精确 mtime）模式不适用；前端每次 @ 打开拉新更合适 + 无缓存简单可靠。e2e 验证延迟，慢再加 TTL。
- 越界：list_files 只遍历 project root 内（无用户输入路径），无需 `is_within_root`；越界校验留给 PR2 @token 解析。

### 后端 PR2
- agent loop 发送前解析 user message `@relpath`（正则）→ is_within_root 校验 → 读文件 → 小文件整内容注入，大文件（>阈值）降级"路径 + N 行 preview"。坏路径/越界降级为路径文本 + 提示（不崩）。与 memory synthetic message 同构。

## Implementation Plan

- **PR1**（前端面板 + 后端列举）：`list_files` 命令 + 单测；TriggerMenu `fuzzy` prop（fuzzysort，默认 false 保持 B3 prefix）+ 回归测 B3；ChatInput @触发 + 插 token + 互斥路由 + `#row` slot；type-check + e2e 手测。
- **PR2**（后端注入）：@token 解析注入 + 截断/降级 + 单测；ROADMAP §1.2 条目。

## Definition of Done

- 后端单测（list_files / @token 注入）+ 前端 type-check 绿；PR1/PR2 分别 e2e 手测。
- `docs/ROADMAP.md` §1.2 补 B2 落地条目 + commit hash。
- `<TriggerMenu>` 复用验证；fuzzysort 引入 / TriggerMenu fuzzy prop 同步 TECH.md + 组件 spec。

## Out of Scope

- B4 Skill（第三类 trigger）；B1 图片粘贴。
- 行号区间 `@path:10-20`（无先例）；目录级 @（`@src/` 逐级筛选，PR1 文件级足够）。
- **token 着色 / CodeMirror 6**（独立 task，记 BACKLOG；@token 纯文本可用）。

## Research References

- [`research/at-file-ux-conventions.md`](research/at-file-ux-conventions.md) — CC @ at-start + 注入文件内容 + `@relpath` 不加 `/` + 多文件 + 不支持行号。
- [`research/fuzzy-matching-impl.md`](research/fuzzy-matching-impl.md) — 前端 fuzzysort（3.2KB/0dep）一次 IPC 缓存 + 本地匹配；nucleo 留后续。

## Status（2026-06-17 session 收尾）

- **PR1 完成**：commit `f3ac7a0`（前端 `<TriggerMenu>` 第二 caller：@触发 + fuzzysort + Tab=Enter + #row slot + 互斥路由；后端 `files::walk_files` + `list_files` command；8 单测；`vue-tsc` 0 错误 + `cargo test --lib` 526 passed + e2e 通过）。实现修正：不做后端 mtime 缓存（源码树频繁变化，前端每次 @ 拉新更合适）。
- **PR2 拆出独立 task** → `.trellis/tasks/06-17-b2-pr2-at-file-injection/`（后端 agent loop 解析 `@token` 注入文件内容，对齐 CC；下次 session 继续）。
- 本 task（PR1 范围）archive。
