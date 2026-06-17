# B2 PR2 @文件内容注入

## Goal

后端 agent loop 解析 user message 里的 `@relpath` token → 读文件 → **注入内容到 context**（对齐 Claude Code / opencode / Aider / Cline 的"注入文件内容"语义），并对**非文本文件**（图片/PDF/Office/二进制）做**占位降级**。PR1 已交付前端 @面板 + 插入 `@relpath` token（路径提示形态，commit `f3ac7a0`），PR1.5 交付 CodeMirror 着色（commit `8e7c975`），PR2 升级为后端内容注入。

## Context（从 b2-at-file-completion 拆出，2026-06-17）

- **PR1 已完成**（父 task archive 于 `.trellis/tasks/archive/2026-06/06-17-b2-at-file-completion/`）：前端 `<TriggerMenu>` 第二 caller（@触发 + fuzzysort + Tab=Enter + #row slot + palette 互斥）+ 后端 `files::walk_files` + `list_files` command + 插入 `@relpath` token。当前 @token 是纯文本路径提示（LLM 收到会倾向主动读，但非确定性 + 多一个 tool turn）。
- **PR1.5 已完成**：ChatInput 迁移 CodeMirror 6 + `@file`/`/command` token 着色。
- **PR2 = "渐进式"第二步**：后端 agent loop 解析 @token 注入文件内容，对齐 CC；并对非文本文件降级。

## Research 关键结论

### CC 对齐（来自父 task `research/at-file-ux-conventions.md`）

- Claude Code 的 `@` 是**注入文件内容**到 context（非路径提示）：changelog "truncation 100→2000 lines" + "directly add them to context"。
- 大文件（>50K chars）降级为"路径 + preview"。
- token 形态：`@<相对路径>`，不加前导 `/`；多文件天然支持；不支持行号区间 `@path:10-20`。

### 6 家 coding agent 横向调研（2026-06-17，详见 [`docs/research/at-file-injection-coding-agents-survey.md`](../../../docs/research/at-file-injection-coding-agents-survey.md)）

调研 opencode / Aider / Cline / pi / PearAI / Cursor 六家后端注入与降级，**5 家共识**：

1. **注入内容，不是路径** — 5 家全部。PR1「纯文本路径提示」是业界唯一被判最差形态。
2. **二进制必须检测，不能乱码喂 LLM** — 4 家检测，**pi 不检测（utf-8 乱码直接喂）是公认短板，Everlasting 反面教材**。检测算法收敛到：扩展名黑名单 + null byte + 30% 非打印比。
3. **降级而非崩溃** — 占位串（Cline）/ fail（opencode）/ 静默丢（Aider）三选一；**无一家降级为路径文本**。
4. **截断默认 2000 行 / 50KB**（opencode、pi、CC 一致）。
5. **类型判定做独立 dispatch 层**（media 优先于 binary 判定）。

## Decision（已决 2026-06-17，基于 6 家调研定稿）

| 决策 | 定稿 | 依据 |
|---|---|---|
| **降级哲学** | **Cline 占位串**（不 fail、不静默丢） | 纯文本通道全 fail 太激进；占位串不阻塞 agent loop 且用户可见处理结果 |
| **二进制检测** | **扩展名黑名单 + 内容嗅探（null byte / 30% 非打印比）** | opencode/Cline 共识；pi 无检测是反面教材 |
| **图片/PDF/Office** | **一律占位降级**（纯文本通道做不了多模态） | 需 ContentBlock 加 Image/Document variant + wire base64 + 缩放，属 B1（第三档）范围 |
| **截断** | **复用 `read_file` 的 50KB head+tail** | opencode/pi 共识值；复用避免两套截断逻辑 |
| **注入形态** | **注入内容 = `read_file` 输出格式**（cat -n 行号） | opencode 核心直觉（内容 vs 工具输出格式统一，模型不困惑） |
| **越界** | **复用 `projects::boundary::assert_within_root`** | opencode 拒绝 `..`/绝对/symlink 三连；已有 5-tier 权限层 |
| **占位引导** | PDF→`pdftotext`、Office→`pandoc`（shell 工具转换） | 零新依赖，复用已有 shell 工具 |

**Why 占位串而非 fail**：@ 是用户主动引用，fail（opencode 模式）会打断 agent loop 且不可恢复；纯文本通道下图片/PDF/Office 全 fail 太激进。占位串"不阻塞 + 用户可见 + 可引导替代方案"最平衡。

**Why 不引 PDF/Office 解析器**：Cline 用 mammoth/exceljs/pdf-parse 是因定位通用 agent；Everlasting 是自研 harness 学习项目，引 Rust 解析 crate（docx-rs/calamine/pdf-extract）体积大、维护成本高，违背"PR2 小步收口"。占位串引导用户用 shell 工具（pdftotext/pandoc）转换，零新依赖。

## Requirements

### 核心注入流程（agent loop 构造 user message 时）
- 解析 `@relpath` token（正则 `@([^\s@]+)`），对每个 token 走"类型 dispatch"：
  1. **越界校验**：`assert_within_root`（`projects/boundary.rs`）—— `@../../etc/passwd` 越界 → **保留原始 `@token` 不替换**（不读 project 外）。
  2. **类型判定**：扩展名黑名单 + 内容嗅探（null byte / 30% 非打印比），分类为 text / image / pdf / office / binary。
  3. **注入**：text → 复用 `read_file` 截断逻辑（50KB head+tail，cat -n 行号）注入；非 text → 占位串降级。
- **无效路径**（不存在 / 不可读 / 越界）：**保留原始 `@token` 文本不替换**（非占位），避免 email `a@b.com` 误伤 + typo 友好。只有"路径有效但非文本"（图片/PDF/Office/二进制）才占位降级。
- **注入位置**：与 `memory/loader.rs::build_instructions_blocks()` 同构（user message content block，可带 `cache_control`）。

### 二进制检测（新增模块，参考 opencode `tool/read.ts:182-227`）
- **扩展名黑名单**：`.zip .tar .gz .exe .dll .so .doc .docx .xls .xlsx .ppt .pptx .odt .ods .odp .bin .dat .obj .o .a .lib .wasm .pyc .pyo .class .jar .war .7z`（命中即判二进制，不读内容）。
- **内容嗅探**：读前 4096 字节 → 含 `\x00` → 二进制；否则非打印字符占比 > 30%（`\t \n \r` 不计）→ 二进制；空文件 → 非二进制。
- ⚠️ **扩展名表与嗅探表对齐**（opencode bmp 误杀教训）：图片扩展名判定和 magic byte 嗅探要同步，避免 `.bmp` 掉进二进制分支。

### 分层降级占位文案
| 类型 | 处理 | 占位文案 |
|---|---|---|
| text | ✅ 注入内容（复用 read_file 截断） | — |
| image | 占位 | `[image: <name> — 当前为纯文本通道，不支持图片注入（B1 计划）]` |
| pdf | 占位 | `[binary: <name> — 二进制文档未注入；可 shell 运行 pdftotext 转文本后引用]` |
| office | 占位 | `[binary: <name> — 二进制文档未注入；可 shell 运行 pandoc <name> -t plain 转文本后引用]` |
| binary | 占位 | `[binary: <name> — 二进制文件，无法注入文本内容]` |

### 单测覆盖
- token 解析（单/多 token、token 前后文本保留、相邻 token）。
- 注入（text 整内容 / cat -n 行号格式正确）。
- 截断（>50KB head+tail，UTF-8 char boundary 不 panic — 参考 read_file RULE-E-009）。
- 二进制检测（扩展名黑名单 / null byte / 30% 比例 / 空文件 / 边界）。
- 降级（image / pdf / office / binary 各一类占位文案）。
- 越界（`@../../etc` / 绝对路径 / symlink）。
- 无效路径（不存在 / 不可读 / 越界）→ 保留原 token；email 不误伤。

## Acceptance Criteria

- [ ] user message 含 `@src/foo.ts` → agent 收到 `foo.ts` 内容（cat -n 行号格式注入 context）。
- [ ] 大文件 >50KB → head+tail 截断注入（不爆 context，UTF-8 边界不 panic）。
- [ ] 二进制文件（`.exe`/`.zip` 等）→ 占位串降级，不把乱码喂 LLM，不崩。
- [ ] 图片（`.png` 等）/ PDF / Office → 占位串降级（纯文本通道不注入多模态）。
- [ ] 越界 `@../../etc` / 绝对路径 → 保留原始 `@token`，不读 project 外。
- [ ] 无效路径（不存在/不可读/越界）→ 保留原始 `@token` 文本（不替换、不崩）；email `a@b.com` 不误伤。
- [ ] 后端单测全覆盖（解析/注入/截断/检测/降级/越界/坏路径）；`cargo check` 0 warning；`vue-tsc` 0 错误（PR2 纯后端，前端无改动）。

## Out of Scope

- **图片 / PDF / Office 内容注入**（需 multimodal 基础设施：ContentBlock 加 Image/Document variant + 双 provider wire base64 + 缩放）— 属 **B1（第三档）**，PR2 仅占位降级。
- **docx/xlsx/pdf 文本提取**（不引解析 crate，占位串引导用户用 shell `pdftotext`/`pandoc` 转换）。
- 行号区间 `@path:10-20`（无先例，CC 也不支持）。
- 多文件（天然支持，多个 @token）。
- 前端 token 着色 / CodeMirror（PR1.5 已完成）。
- references 别名系统 `@alias/path`（opencode 重型特性，个人工作台 over-engineering）。

## References

- **6 家调研定稿**: [`docs/research/at-file-injection-coding-agents-survey.md`](../../../docs/research/at-file-injection-coding-agents-survey.md)
- 父 task（PR1 + research）: `.trellis/tasks/archive/2026-06/06-17-b2-at-file-completion/`（含 `research/at-file-ux-conventions.md` CC UX 约定）
- memory 注入参考: `app/src-tauri/src/memory/loader.rs::build_instructions_blocks()`
- 截断复用: `app/src-tauri/src/tools/read_file.rs::truncate_output()`（50KB head+tail + cat -n + RULE-E-009 UTF-8 边界）
- agent loop: `app/src-tauri/src/agent/`（user message 构造处接入）
- boundary: `app/src-tauri/src/projects/boundary.rs::assert_within_root`
