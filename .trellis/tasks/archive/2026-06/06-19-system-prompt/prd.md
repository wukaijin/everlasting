# system-prompt 改造

## Goal

为 Everlasting agent 补上缺失的「行为准则」system prompt，并修复工具清单硬编码漂移（RULE-E-013），使 agent 在语气 / 主动性 / 工具使用 / git 安全等维度获得可感知的行为指导。基础 = `docs/research/system-prompt-research.md §7` 评审 + `DEBT.md RULE-E-013`。

## Requirements

- **R1（RULE-E-013）**：删除 `build_system_prompt` 硬编码的 7 个工具枚举，改通用表述（"tools defined in this request"），保留路径相对性说明
- **R2（behavior_prompt）**：新增 `agent/behavior_prompt.rs` 常量，含 7 段行为准则（见 Technical Approach）
- **R3（拼接顺序）**：`chat_loop.rs:327` 从 `mode_prefix + base_prompt` 改为 `behavior_prompt + mode_prefix + base_prompt`（稳定层在前）
- **R4（语言）**：behavior_prompt 主体英文，末尾约束 "Reply in the user's language"

## Acceptance Criteria

- [ ] `build_system_prompt` 不再出现硬编码工具名清单（grep 无 `read_file, write_file` 字面量）
- [ ] system prompt 含 7 段行为准则，引用 `update_checklist`（非 TodoWrite）
- [ ] 拼接顺序单测断言：`behavior_prompt` 在 `mode_prefix` 与 `base_prompt` 之前
- [ ] `cargo check` 0 warning；`PKG_CONFIG_PATH=... cargo test --lib` 现有测试不回归
- [ ] RULE-E-013 在 DEBT.md 标 closed + 填 commit hash
- [ ] `docs/research/system-prompt-research.md §7.8` 推荐表标记已实施项

## Definition of Done

- 单测：工具清单不再硬编码（反向断言）+ 拼接顺序断言 + behavior_prompt 非空
- `cargo check` + `cargo test --lib` 绿
- DEBT.md RULE-E-013 closed + commit hash
- research §7.8 + spec 更新

## Technical Approach

### R1 — `build_system_prompt` 改通用表述（system_prompt.rs:79-89）

删除 `You have access to tools (read_file, write_file, edit_file, shell, grep, glob, list_dir).`，改为：

```
You are a coding agent. You have access to the tools defined in this
request. All file paths in tool inputs are relative to the session's
working directory.
```

（保留后半句路径说明 + tool result envelope 说明；删工具枚举）。工具可见性完全由 `tools[]` 决定，与 mode filter 自动一致，彻底消除漂移。

### R2 — `behavior_prompt.rs` 草案（英文主体 + 语言约束）

```text
# Tone and style
- Be concise, direct, and to the point.
- Answer the user's question directly without elaboration unless asked.
- Use emojis only if the user explicitly requests it.
- Do not add code-explanation summaries unless requested.

# Professional objectivity
- Prioritize technical accuracy and truthfulness over validating the
  user's beliefs.
- Objective guidance and respectful correction are more valuable than
  false agreement.

# Task management
- For complex tasks (3+ steps), use the update_checklist tool to plan
  and track progress.
- Mark items as completed as soon as you are done — do not batch
  completions.

# Tool usage
- Batch independent tool calls into a single response to reduce
  round-trips.
- Prefer specialized tools over shell: read_file over cat, edit_file
  over sed, grep over shell grep.
- Do not use shell echo or comments to communicate — output text
  directly.

# Code conventions
- Before changing a file, study its existing conventions and mimic them.
- Never assume a library is available without checking
  imports/dependencies first.
- Do not add comments unless asked.

# Finishing work
- When asked to build, run, or verify something, the deliverable is a
  working artifact backed by real tool output — not a description of one.
- Keep working until the task is actually complete, then verify.

# Git safety
- Never run destructive git commands (push --force, hard reset) unless
  the user explicitly asks.
- Never commit changes unless the user explicitly asks.

# Language
- Reply in the user's language (Chinese by default for this user).
```

### R3 — 拼接（chat_loop.rs:327）

```rust
let system_prompt = format!(
    "{}\n\n{}\n\n{}",
    behavior_prompt::DEFAULT_BEHAVIOR_PROMPT,  // 稳定常量
    mode_prefix,                               // 较稳定（session mode）
    base_prompt,                               // 每次变（cwd/head_sha）
);
```

分层（Q4 判定）：`mode_prefix` = 权限兜底（系统强制），`behavior_prompt` 的 git safety = 行为自觉（模型主动），两者维度不同不冲突，都保留。

## Decision (ADR-lite)

- **D1 scope = P0 + P1**：model_family_guidance(P2) 与 mini-eval 作 follow-up
- **D2 工具清单 = 不列具体名**：比"动态生成"更治本（零维护 + mode filter 自动一致）。RULE-E-013 Fix 方向随之从「动态生成」改为「删除枚举」，实现时同步更新 DEBT 描述
- **D3 语言 = 英文 + 末尾语言约束**：兼顾指令遵循度 + 中文输出
- **D4 MVP 不加 system cache_control**：system 保持 String。behavior_prompt 是稳定常量，加 cache_control breakpoint 是真·缓存优化，但需把 system 改成 block 数组结构，超 MVP 范围 → follow-up

## Out of Scope

- 方案 B 3 层架构重构
- 指令文件从 user message 迁移到 system
- Custom system prompt UI（方案 C）
- `model_family_guidance`(P2) → follow-up task
- mini-eval 评估集 → follow-up task
- system 字段 cache_control 优化 → follow-up（D4）

## Implementation Plan (small PRs)

- **PR1**：R1 RULE-E-013 Fix — 改 `build_system_prompt` 通用表述 + 反向单测（grep 无硬编码工具名）。独立 bug fix，闭合 RULE-E-013
- **PR2**：R2+R3 behavior_prompt — 新增 `behavior_prompt.rs` 常量 + 改拼接顺序 + 拼接顺序单测 + 语言约束。收尾：DEBT RULE-E-013 closed + research §7.8 更新 + spec

## Technical Notes

- 代码：`system_prompt.rs:56-96` / `chat_loop.rs:327` / `tools/mod.rs:45`(builtin_tools) / `permissions/mod.rs:1321`(mode_system_prefix)
- 参考：`docs/research/system-prompt-research.md`（§4.1 草案、§7 评审、§7.8 推荐表）
- DEBT：RULE-E-013（P2/Tools）
- 测试命令：`cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib`
