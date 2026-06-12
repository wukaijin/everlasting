# P0 Tool Enhancement: read_file offset/limit + shell timeout

## Goal

对标 Claude Code / Open Code / Cursor 等主流 AI coding agent，为 Everlasting 的两个核心 tool 补齐标配参数，提升 agent 对大文件和长命令的控制力。

## What I already know

### read_file 现状
- 当前只有 `path` 参数，无 offset/limit
- 大文件（>50KB）做 head+tail 截断，中间内容完全不可见
- 输出带 `cat -n` 行号（1-based, `\t<line>\t<text>`）
- ReadGuard 在 read 时记录 fingerprint

### shell 现状
- 当前有 `command` + `working_directory`(可选)
- 无 timeout 参数，靠 C1 CancellationToken 手动取消
- 30KB disk spill + 50KB inline truncation
- `sh -c` 执行，stdout+stderr 合并

### 各家对标
- **Claude Code Read**: `offset`(起始行号) + `limit`(行数，默认 2000)
- **Open Code read**: `offset`(1-indexed) + `limit`(默认 2000)
- **Cursor read_file**: `start_line_one_indexed` + `end_line_one_indexed_inclusive`(每次最多 250 行)
- **Claude Code Bash**: `timeout`(默认 120s，最大 600s) + `description`
- **Open Code bash**: `timeout`(默认 120s)

## Requirements

### R1: read_file offset + limit

1. 新增 `offset` 参数（int, 可选, 默认 1）— 起始行号，1-indexed
2. 新增 `limit` 参数（int, 可选, 默认 2000）— 返回的最大行数
3. 行号从 `offset` 开始编号（不是从 1），保持 `cat -n` 格式
4. offset 超出文件总行数 → 返回空内容（is_error: false）
5. limit 超出剩余行数 → 返回到文件末尾为止
6. 与现有截断机制的关系：offset/limit 先选取行范围，再对选取的内容做 50KB head+tail 截断
7. ReadGuard 仍然记录完整文件的 fingerprint（不受 offset/limit 影响）
8. 现有无参数调用行为不变（默认读全部，50KB head+tail）

### R2: shell timeout

1. 新增 `timeout` 参数（int, 可选, 默认 120000ms）— 命令执行超时毫秒数
2. 上限 600000ms（10 分钟），超出自动 clamp
3. 超时行为：kill 子进程，返回已收集的部分输出 + `[timeout after Nms]` 标记
4. is_error: true（超时是错误状态）
5. timeout=0 或负数 → 使用默认值 120000
6. 与 C1 cancel 的关系：timeout 到期时自动 cancel，不等用户手动取消
7. 现有无参数调用行为不变（默认 120s 超时）

## Acceptance Criteria

- [ ] read_file 带 offset=10,limit=5 时，只返回第 10-14 行，行号从 10 开始
- [ ] read_file 不带 offset/limit 时，行为与当前完全一致
- [ ] read_file offset 超出文件行数时，返回空内容（is_error: false）
- [ ] shell 带 timeout=3000 时，超过 3s 的命令自动终止并返回部分输出
- [ ] shell 不带 timeout 时，默认 120s 超时
- [ ] shell timeout=0 或负数时，使用默认 120s
- [ ] shell timeout > 600000 时，clamp 到 600000
- [ ] 现有测试全部通过
- [ ] tool-contract.md spec 更新

## Definition of Done

- `cargo test` 全部通过
- `pnpm build` 通过（前端无需改动，tool schema 变化自动传递给 LLM）
- tool-contract.md 更新
- PRD 归档

## Technical Approach

### read_file 改动（read_file.rs）

1. `definition()` 的 input_schema 新增 `offset` 和 `limit` 属性
2. `execute()` 解析 offset/limit 参数
3. 读取完整文件内容后，先按 offset/limit 切片行，再对切片结果做 add_line_numbers + truncation
4. ReadGuard 仍然基于完整文件内容（不受切片影响）

关键实现点：
```rust
// 解析参数
let offset = input.get("offset").and_then(|v| v.as_u64()).unwrap_or(1) as usize;
let limit = input.get("limit").and_then(|v| u64()).unwrap_or(2000) as usize;

// 读取文件 → 行切片 → 行号 → 截断
let lines: Vec<&str> = content.lines().collect();
let total_lines = lines.len();
let start = offset.saturating_sub(1); // 1-indexed → 0-indexed
let end = (start + limit).min(total_lines);
let sliced: String = lines[start..end].join("\n");
// 行号从 offset 开始
let output = add_line_numbers_with_offset(&sliced, offset);
```

### shell 改动（shell.rs）

1. `definition()` 的 input_schema 新增 `timeout` 属性（int, ms）
2. `execute()` 解析 timeout 参数，clamp 到 [0, 600000]
3. 在 `tokio::select!` 中加一个 `tokio::time::sleep` 分支
4. 超时触发时 kill 子进程，收集部分输出，返回超时标记

关键实现点：
```rust
// 在 select! 中加超时分支
let timeout_dur = Duration::from_millis(timeout_ms.clamp(1, 600_000) as u64);

tokio::select! {
    biased;
    _ = cancel.cancelled() => { /* 已有 C1 逻辑 */ }
    _ = tokio::time::sleep(timeout_dur) => {
        tracing::info!("shell: timeout after {}ms", timeout_ms);
        kill_and_collect(&mut child).await
        // 标记 result.timeout = true
    }
    status = child.wait() => { /* 已有逻辑 */ }
}
```

ShellResult 加 `timeout: bool` 字段，与 `cancelled` 并列。

## Out of Scope

- shell 的 `description` 参数（Claude Code 有，但仅用于日志，不阻塞 P0）
- read_file 读取图片/PDF（Claude Code 支持，属于 P2 远期）
- grep 的 multiline/type 参数（P1 小改进，独立任务）
- web_fetch / agent(task) 等 P1 全新 tool
- apply_patch（P2 远期）

## Technical Notes

- 文件: `app/src-tauri/src/tools/read_file.rs`, `app/src-tauri/src/tools/shell.rs`
- Spec: `.trellis/spec/backend/tool-contract.md`
- 调研报告: `docs/_reviews/REVIEW-tool-comparison-2026-06-12.md`
- 行号格式: 保持现有 `\t<line>\t<text>` cat -n 风格
- `add_line_numbers()` 需要扩展为 `add_line_numbers_with_offset(text, start_line)`，现有无参调用传 start_line=1
- ShellResult struct 需要新增 `timeout: bool` 字段
