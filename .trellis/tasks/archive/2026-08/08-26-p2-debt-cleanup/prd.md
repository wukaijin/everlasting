# P2 债务清理:clippy gate + frontmatter 去重 + test_pool 去重

## Goal

打包闭合 DEBT.md 三条 P2 债(2026-08-24 harness 缺口评估发现,均低风险机械重构):

- **RULE-CI-001** — CI 缺 clippy gate,E1(07-05)留的 follow-up 未兑现;此后 +400 测试无机器把关。
- **RULE-FM-001** — frontmatter 解析器 3 份复制,一个行为变更 = 改 3 实现 + ≥3 测试。
- **RULE-TESTPOOL-001** — in-memory 测试池 `test_pool()` 15 处手写复制,migrations 变更要同步 15 处。

## Background / 已确认事实(2026-08-26 摸底)

### RULE-FM-001 复制形态

| 函数 | 位置 | 形态 |
|------|------|------|
| `parse_frontmatter` | `resource_loader.rs:160` / `skill/loader/frontmatter.rs:48` / `agent/subagent/frontmatter.rs:85` | **三份逐字相同**(仅 `Frontmatter` struct 类型不同):跳空行 → 开 fence 可选 → 逐行 `apply_kv` → 闭 fence → 返回 `(fm, body)` |
| `apply_kv` | `resource_loader.rs:195` / `skill/loader/frontmatter.rs:79` / `agent/subagent/frontmatter.rs:116` | **前半逐字相同**(trim / `#` 注释跳过 / `split_once(':')` / 平衡引号剥离),仅 `match k` 分支不同(B3: name/description/argument-hint;skill: +allowed-tools;subagent: +tools/isolation/model) |
| `parse_allowed_tools` / `parse_tools_array` | `skill/loader/frontmatter.rs:118` / `agent/subagent/frontmatter.rs:202` | **逐字相同**(strip 引号 → `[...]` 检测 → 逗号 split → trim/dedup → warn 文案前缀不同 "skills:" vs "subagent:") |
| 复制的测试 | `resource_loader.rs:558` + `skill/tests_loader.rs:55` | `apply_kv_ignores_comments_blank_unknown` 两份 |

### RULE-TESTPOOL-001 复制形态

`grep "fn test_pool"` 命中 15 处(2026-08-26 复核仍为 15):

`db/usage_tests.rs:12` / `db/sessions_tests/mod.rs:23` / `db/providers_tests.rs:21` / `db/memories_tests/mod.rs:23` / `agent/tests_common.rs:157` / `commands/tests_resolve_mode_change.rs:56` / `db/subagent_runs_tests.rs:28` / `db/search_tests.rs:18` / `db/permissions_tests.rs:24` / `db/messages_tests.rs:22` / `projects/store.rs:176` / `db/projects_tests.rs:23` / `db/trace.rs:498` / `tools/tests_merge_worker.rs:15` / `agent/subagent/tests_dispatch.rs:255`

声称的共同形态 = `SqlitePool::connect("sqlite::memory:")` + `PRAGMA foreign_keys=ON` + `migrations::run`。**注意**:个别实现可能带额外 setup 或细节差异,实施时必须逐处 diff,只替换真正等价者;有差异的保留原地并在 PRD 偏差记录。

### RULE-CI-001 现状(2026-08-26 实测)

- `.github/workflows/ci.yml` rust job 仅 `cargo fmt --check` + `cargo test --lib`。
- `cargo clippy --lib` 实测**仅 2 个 warning**,均为 `too_many_arguments`:
  - `agent/chat.rs:165` `chat_inner`(9/7 参)
  - `agent/chat_loop.rs:1280` `emit_max_turns_terminal`(8/7 参,08-19 软卡任务新增)
- 两者均属 RULE-ARGS-001(parameter object epic,单独排期、DEBT.md 有跟踪)范畴 → 本任务以显式 `#[allow(clippy::too_many_arguments)]` + 注释 `// See DEBT.md RULE-ARGS-001` 豁免,gate 落地不受阻塞;该 epic 实施时随签名收敛移除。

## Requirements

### R1 — RULE-FM-001:抽共享 frontmatter 解析(泛型/trait)

1. 在 `resource_loader.rs`(或其旁新模块,实施者裁量)抽泛型入口,形如 `parse_md_resource<T: MdResource>(content: &str) -> (T, String)` + `trait MdResource: Default { fn apply_kv(&mut self, line: &str); }`;`parse_frontmatter` 的 fence/行循环逻辑只留一份。
2. 三 loader 各自保留:`Frontmatter` struct 定义 + `apply_kv` 的 `match k` 字段分支(经 trait impl 或 thin wrapper,风格随实施)。
3. 数组解析去重:抽共享 `parse_string_array(raw, warn_prefix) -> Vec<String>`;skill 的 `parse_allowed_tools` 与 subagent 的 `parse_tools_array` 变 thin wrapper,保留各自 warn 文案("skills:" / "subagent:")。
4. 重复测试收敛:`apply_kv_ignores_comments_blank_unknown` 共享行为留一份在共享层测试;各 loader 测试只保留字段特异分支(name/description 之外的 key)。
5. **行为不变**:解析输出逐字节一致(现有全部 frontmatter 相关测试必须零改动通过,除被收敛删除的重复测试)。

### R2 — RULE-TESTPOOL-001:抽共享 test_pool

1. 新建 `app/src-tauri/src/db/test_support.rs`,`#[cfg(test)]` 门控 + `pub(crate) async fn test_pool() -> SqlitePool`(共同形态:connect in-memory + PRAGMA foreign_keys + migrations::run)。
2. 15 处调用点改为 `use crate::db::test_support::test_pool;`(或等价路径);每处替换前 diff 原实现,与共享版逐字节等价才替换。
3. 有额外 setup / 细节差异的实现保留原地,不强行统一(在 implement 记录偏差)。

### R3 — RULE-CI-001:clippy 清零 + CI gate

1. 本地跑 `cargo clippy --lib`(PKG_CONFIG_PATH 照常)清零 warning。仅剩的 2 个 `too_many_arguments`(`chat_inner` / `emit_max_turns_terminal`)按 Background 裁定显式 `#[allow]` + `// See DEBT.md RULE-ARGS-001` 注释豁免;除此之外出现的新 warning 修代码,不用 `#[allow]` 掩盖(确属误报的个别 lint 可 `#[allow]` + 一行理由)。
2. `.github/workflows/ci.yml` rust job 加 `cargo clippy --lib -- -D warnings`(与 fmt/test 同 job 顺序执行;fmt → clippy → test)。
3. 前端不受本任务影响,不动 `package.json` / 前端 job。

## Acceptance Criteria

- [x] AC1:`grep "fn parse_frontmatter" app/src-tauri/src/` 仅命中共享实现一处(generic 版);`grep "fn parse_allowed_tools\|fn parse_tools_array"` 仅 thin wrapper(或共享 `parse_string_array` 一处)。(check 裁定:三处 `parse_frontmatter` 为 R1.2 明确允许的一行 thin wrapper,泛型循环/数组逻辑仅 `resource_loader.rs` 一份)
- [x] AC2:`grep -rn "async fn test_pool" app/src-tauri/src/` 仅命中 `db/test_support.rs` 一处(+ PRD 记录的偏差豁免项)。(15 处全部逐字节等价替换,零偏差保留)
- [x] AC3:`cargo clippy --lib` 本地零 warning;`cargo test --lib` 全绿(≈1991 基线,允许已知 flaky 复跑);`cargo fmt --check` 干净。(实测 2026-08-26:clippy `-D warnings` 0 warning;test 1997 过 / 0 挂 / 1 ignored;fmt 干净)
- [x] AC4:CI yml 含 clippy gate(`-D warnings`)。
- [x] AC5:DEBT.md 三条 finding(RULE-CI-001 / RULE-FM-001 / RULE-TESTPOOL-001)删除,优先级分布表更新(P2 4→1)。

## Out of Scope

- **RULE-ARGS-001**(43 处 too_many_arguments / parameter object epic)— 量大单独排期。
- **RULE-DOC-001**(P3,注释块/CLAUDE.md 状态段)— 文档治理,不混入本任务。
- frontmatter 解析器功能增强(多行值/YAML crate 升级)— 只做形态收敛,不改行为。
- `crates/everlasting-remote*` 两 crate 的 clippy(CI 只对 app;remote crate 若有 warning 顺手清但不设 gate 范围)。
