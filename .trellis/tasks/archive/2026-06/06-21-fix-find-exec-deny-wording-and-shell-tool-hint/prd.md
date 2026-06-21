# PRD: fix find -exec deny wording and shell tool hint

## 背景
sub-agent 执行 `find . -name '*.ts' ... -exec wc -l {} +` 被 Tier 2 硬 kill list 拦截（`dangerous.rs:76-79`），deny reason 写的是 `"find -exec is denied: runs an arbitrary command per match"`。但 `{} +` 是**批量**模式（等价 `wc -l f1 f2 f3...`），不是 per-match；只有 `{} \;` 才是 per-match。文案对 `+` 的描述不准，且没给被拒的 LLM 任何出路，造成一次浪费的 round-trip。

规则本身**合理**：`-exec` 是任意命令执行通道，静态正则无法区分 `wc -l`（无害）vs `rm -rf`（破坏），kill list 一刀切是对的，与 `curl|bash` / `find -delete` 同类取舍。本任务只修文案 + 加源头引导，**不动 regex 逻辑**。

## 方案
1. **`dangerous.rs` 文案 + 注释**（rule 不变）：
   - reason：`"find -exec is denied: runs an arbitrary command per match"` → `"find -exec is denied: find becomes an arbitrary-command runner — use -print0 | xargs -0 instead"`（去掉对 `+` 不成立的 "per match"，并直接给被拒的 LLM 替代写法）。
   - 注释补一句：`\;`（per-match）和 `+`（batch）都是任意命令通道，都拦；正确替代是 `-print0 | xargs -0`。
2. **`shell.rs` description 加引导**：在 tool description 里说明 `find -exec` / `-execdir` 会被 kill list 拦，改用 `-print0 | xargs -0`（顺带处理含空格文件名），让 LLM 从源头不生成被拦命令 —— 与既有 timeout/output/env 引导同风格。
3. **测试**：加回归 guard `kill_list_find_exec_reason_suggests_xargs`，断言 find -exec 的 deny reason 含 `"xargs"`（防 copy-edit 丢引导，风格对齐既有 `definition_documents_timeout_guidance`）。

## 验收
- `dangerous.rs` regex **不变**（`kill_list_blocks_find_delete_and_exec` 继续过）。
- 新文案不含 `"per match"`；含 `"xargs"` 引导词。
- `shell.rs` description 含 `-print0 | xargs -0` 引导。
- `cargo test`（PKG_CONFIG_PATH 注入）全绿。
- **顺手修 spec 计数**：`permission-layer.md` 写「9 regex」但实际 10 条（`find -delete` + `find -exec` 各一），改成 10。

## Out of scope
- 不放宽/收紧 kill list 正则。
- 不改其他 deny pattern 的文案。
- 不引入 memory-file-driven 自定义 deny 规则（`dangerous.rs:16-19` 注释里提到的 future PR）。
