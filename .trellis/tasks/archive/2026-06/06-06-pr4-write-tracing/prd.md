# PR4: write_file tool 加 tracing 诊断偶发失败

> Source spike: [`docs/spikes/2026-06-06-feature-requests.md`](../../../../../docs/spikes/2026-06-06-feature-requests.md) 第 3 条
> 父 task: `06-06-spike-005-follow-up`
> 父 prd: [../06-06-spike-005-follow-up/prd.md](../06-06-spike-005-follow-up/prd.md) (PR4 段)
> Priority: P2 (诊断性, 5 行)

## Goal

spike 第 3 条: write tool 偶发 LLM 一直失败, 跟参数有关, 偶发无法复现。
本 PR **不改业务逻辑**, 只在 `write_file.rs::execute()` 入口和失败点加 `tracing::debug!`, 让下次复现时能拿到完整 trace。

## What I already know

- `app/src-tauri/src/tools/write_file.rs:46-156` `execute()` 入口有 5 个 is_error 出口
- spike 提到 "参数有关" — 关键诊断信息: 实际收到什么 path / content_len / 是否 existing / tail_components
- 不动 Cargo.toml (无新依赖)
- 不改业务逻辑 (跟 PR4 SPEC §Requirements 一致)
- 父 prd 锁定: ~5 行

## Requirements

- `app/src-tauri/src/tools/write_file.rs::execute()` 入口:
  - 现有: `let raw_path = match input.get("path").and_then(|v| v.as_str()) { ... }`
  - 加 `tracing::debug!({ path = %raw_path, content_len = content.len(), is_existing = requested.exists(), tail_components = ?tail }, "write_file called");` (raw_path 在 tail 变量之前用, 需要 reorder)
  - 或者更早: 在函数入口就 debug!, 用最原始的 input
- 失败点: 5 个 is_error=true 分支, 每个加 `tracing::debug!({ path = %raw_path, error = %error_msg }, "write_file failed")`
  - 或者用 `tracing::warn!` 区分: 业务预期的失败 (路径在 root 外) 是 warn, IO 错误是 error
  - 选 `tracing::debug!` 一致 (production 默认 info 级别, debug 不会输出; 用户跑 RUST_LOG=debug 才看到)
- 不动现有 cargo test
- 加 1 个 cargo test 验证 tracing 行为: 用 `tracing_test` 或 `tracing-subscriber` 捕获, 验证 debug! 被调 (optional, 复杂度高, 可省)

## Acceptance Criteria

- [ ] 手动跑 `RUST_LOG=debug` 触发 write_file, 日志包含 path / content_len / is_existing
- [ ] 业务逻辑零变化 (原 cargo test 全过, 4 个现有 test + 5 个 PR2 + 5 个 PR5 = 98 个仍 pass)
- [ ] `cargo check` 通过
- [ ] pnpm build + pnpm test 通过
- [ ] 新代码 ~5-10 行

## Definition of Done

- 修改 1 个文件 (`write_file.rs`)
- 跑完 standard Trellis 流程到 archived
- 视觉验证: 不需要 (无 UI 改动)

## Out of Scope

- 改 write_file 业务逻辑 (预校验 / 重构) — 留 v2, 等下次复现有 trace 后再决定
- `tracing_test` crate 引入 (写 unit test 捕获 tracing) — 复杂度 > 价值
- 其他 tool (read_file / shell) 加 tracing — 暂不, write_file 是 spike #3 唯一指向的
- 文件锁 / atomic write (write_file 当前是覆盖式) — 独立问题, 不在本 PR

## Technical Notes

- 改动文件: `app/src-tauri/src/tools/write_file.rs` (+5-10 行)
- 风险: 极低 (tracing 是 no-op 在默认级别)
- 风险: 用户跑 `RUST_LOG=debug` 会看到额外日志, 性能开销忽略
- 关联: PR5 / PR2 的 `lib.rs` 已有 `tracing::warn!` / `tracing::info!` 模式, 风格一致

## Decision (ADR-lite)

- **决策**: 用 `tracing::debug!` 而非 `tracing::warn!` / `tracing::error!`
  - **理由**: 默认 info 级别不输出, 用户主动开 RUST_LOG=debug 才有, 不污染生产日志
  - **后果**: 复现问题时需要重启 dev server with `RUST_LOG=debug pnpm tauri dev`
