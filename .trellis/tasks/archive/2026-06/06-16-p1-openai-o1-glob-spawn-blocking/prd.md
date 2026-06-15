# p1-openai-o1-glob-spawn-blocking

> 关联 DEBT.md：RULE-D-002 (P1) + RULE-E-004 (P1)
> 合并理由：两条都是 ~15-20 行的局部小修 + **active bug**(非清理性质),scope 清晰、子系统独立、可同一 PR review。
> **不并入 B-001/B-002**:permission store 重构(~50 行+测试,清理隐性依赖性质,DEBT 自述"实际不泄漏")单独开 `06-16-p1-permission-asks-cleanup`。

## Goal

修两条 active bug:

- **RULE-D-002**:OpenAI provider 硬编码 `max_tokens`,o1/o3/o4-mini reasoning 模型要求 `max_completion_tokens`,发错字段 400。用户配 o1 后**每条 chat 必崩**。
- **RULE-E-004**:`glob` tool 的 `walk_dir` 用 sync `std::fs::read_dir`,被 async `execute` 直接调,大 repo(Chromium / Linux kernel)glob **卡死 tokio worker**,拖累同 runtime 并发 session。

## What I already know

### RULE-D-002 — `openai.rs:243-248`

```rust
let mut body = json!({
    "model": config.model,
    "max_tokens": config.max_tokens,   // ← o1 family 拒绝,要求 max_completion_tokens
    "stream": true,
    "messages": msgs,
});
```

项目**无现成 o1 family helper**(grep 确认)。`reasoning_effort` 字段(`:266-270`)已按 config 条件 emit,但 max_tokens 无分支。body 本就不发 `temperature`(o1 也不支持,无需处理)。

o1 family model id 前缀:`o1` / `o1-mini` / `o1-preview` / `o1-pro`、`o3` / `o3-mini` / `o3-pro`、`o4-mini`。

### RULE-E-004 — `glob.rs:115` + `walk_dir:205-226`

`walk_dir`(sync,`std::fs::read_dir` + 手动 stack)被 async `execute` 直接 `for entry in walk_dir(...)`(`:124`)。其他 IO tool 用 `tokio::fs`,glob 是异类。glob.rs 当前**无** `tokio::task::spawn_blocking` import,无 sibling tool 参照(全 tools 模块仅 glob 这一处 sync walk)。

## Decisions (resolved)

- **[D-002] o1 判断**:新增 `fn is_o1_family(model: &str) -> bool`,前缀判断 `o1` / `o3` / `o4`(to_lowercase 容错)。true → body 用 `max_completion_tokens`,false → `max_tokens`。**值都取 `config.max_tokens`**(key 换,值不变,语义对齐 OpenAI 文档:两字段都是输出上限)。
- **[D-002] scope 只改字段名**:o1 family 其他差异(system message 角色、`temperature`、`system_fingerprint`)不在 D-002,留作后续。当前 body 不发 temperature,o1 的 system→user 转换是独立债(未记 DEBT,本 task 不触)。
- **[E-004] spawn_blocking 范围**:walk + glob match + mtime collect 整体进 `spawn_blocking`(read_dir / metadata / glob match 都是 blocking);返回 `(Vec<Match>, usize truncated)`。sort + 输出格式化留在 async 侧(纯 CPU,非 blocking)。
- **[E-004] pattern 校验位置**:`Glob::new` 保留在 async 侧快速失败(`:96-104`,错误友好 + 不阻塞);`spawn_blocking` 闭包内**重新 compile matcher**(pattern 短,重 compile 成本可忽略,规避 matcher 跨 `'static` 闭包 Send/borrow 认知负担)。
- **[E-004] 错误处理**:`spawn_blocking` join 失败(panic)→ 透传为 `(content, is_error=true)`;`walk_dir` io 失败 → 透传(同原行为)。

## Requirements (locked)

- **D-002**:`is_o1_family(&config.model)` true → body `"max_completion_tokens" = config.max_tokens`;false → `"max_tokens" = config.max_tokens`。补单测覆盖 `o1-preview` / `o3-mini` / `o4-mini`(true)+ `gpt-4o` / `gpt-4.1`(false)。
- **E-004**:walk + match + collect 进 `spawn_blocking`;现有行为不变 —— 100 cap、mtime desc sort、truncation hint、path boundary 校验、relative 显示。现有 7 个 glob 单测全 pass。

## Acceptance Criteria (locked)

- [ ] **D-002**:`is_o1_family` 单测覆盖 o1/o3/o4 前缀 true + 非 o1 false;body 构造单测断言 o1 走 `max_completion_tokens`、非 o1 走 `max_tokens`。
- [ ] **E-004**:glob.rs execute 的 read_dir 路径在 `spawn_blocking` 内;7 个 glob 单测 pass。
- [ ] `cargo test --lib`(带 PKG_CONFIG_PATH)全套 pass,`cargo check` 0 warning。

## Out of Scope (explicit)

- o1 system message 角色转换(独立债,未记 DEBT)。
- glob 改 gitignore 感知 / 换 `walkdir` / `ignore` crate。
- RULE-B-001 / RULE-B-002(permission store 重构)→ `06-16-p1-permission-asks-cleanup`。
- RULE-D-004(`WireRequest.reasoning_effort` dead field)/ RULE-D-005(`supports_reasoning_effort` hardcode true)→ 相关 Provider 债,本 task 只碰 max_tokens 一处。

## Technical Notes

- **改动文件**:`app/src-tauri/src/llm/provider/openai.rs`(D-002)+ `app/src-tauri/src/tools/glob.rs`(E-004)。零前端改动。
- **验证**:`cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib`(见 CLAUDE.md WSL 坑 1)。
- **回归风险**:低 —— D-002 仅 o1 family 分支(非 o1 路径 byte-for-byte 不变);E-004 spawn_blocking 是纯并发包装,walk/match 逻辑不变。
