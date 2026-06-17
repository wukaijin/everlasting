# B2 PR2 @文件内容注入

## Goal

后端 agent loop 解析 user message 里的 `@relpath` token → 读文件 → **注入内容到 context**（对齐 Claude Code @-mention 的 "directly add to context"）。PR1 已交付前端 @面板 + 插入 `@relpath` token（路径提示形态，commit `f3ac7a0`），PR2 升级为内容注入。

## Context（从 b2-at-file-completion 拆出，2026-06-17）

- **PR1 已完成**（父 task archive 于 `.trellis/tasks/archive/2026-06/06-17-b2-at-file-completion/`）：前端 `<TriggerMenu>` 第二 caller（@触发 + fuzzysort + Tab=Enter + #row slot + palette 互斥）+ 后端 `files::walk_files` + `list_files` command + 插入 `@relpath` token。当前 @token 是纯文本路径提示（LLM 收到会倾向主动读）。
- **PR2 = "渐进式"第二步**：后端 agent loop 解析 @token 注入文件内容，对齐 CC。

## Research 关键结论（CC 对齐，来自父 task `research/at-file-ux-conventions.md`）

- Claude Code 的 `@` 是**注入文件内容**到 context（非路径提示）：changelog "truncation 100→2000 lines" + "directly add them to context"。
- 大文件（>50K chars）降级为"路径 + preview"。
- token 形态：`@<相对路径>`，不加前导 `/`；多文件天然支持；不支持行号区间 `@path:10-20`。

## Requirements

- agent loop 发送前（构造 user message 时）解析 `@relpath` token（正则 `@([^\s@]+)`）。
- **越界校验**：`is_within_root`（`projects/boundary.rs`）—— 用户可能 `@../../etc/passwd`，越界拒绝/降级。
- **读文件**：小文件整内容注入；大文件（>阈值，参考 CC ~50K chars / 2000 lines）降级为"路径 + N 行 preview"。
- **坏路径**（不存在 / 不可读）：降级为路径文本 + 提示，不崩（`error-handling.md` 风格）。
- **注入位置**：与 `memory/loader.rs::build_instructions_blocks()` 同构（content block，可带 `cache_control`）。
- 单测：token 解析 / 注入 / 截断 / 降级 / 越界 / 坏路径。

## Acceptance Criteria

- [ ] user message 含 `@src/foo.ts` → agent 收到 `foo.ts` 内容（注入 context）。
- [ ] 大文件降级为路径 + preview（不爆 context）。
- [ ] 越界 `@../../etc` 拒绝/降级，不读 project 外。
- [ ] 坏路径降级（路径文本 + 提示），不崩。
- [ ] 后端单测覆盖；`vue-tsc` + `cargo check` 0 warning。

## Out of Scope

- 行号区间 `@path:10-20`（无先例）。
- 多文件（天然支持，多个 @token）。
- 前端 token 着色 / CodeMirror（独立 task）。

## References

- 父 task（PR1 + research）: `.trellis/tasks/archive/2026-06/06-17-b2-at-file-completion/`
- memory 注入参考: `app/src-tauri/src/memory/loader.rs::build_instructions_blocks()`
- agent loop: `app/src-tauri/src/agent/`
- boundary: `app/src-tauri/src/projects/boundary.rs`
