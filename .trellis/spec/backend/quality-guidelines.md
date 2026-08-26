# Quality Guidelines

> Code quality standards for backend development.

---

## Overview

<!--
Document your project's quality standards here.

Questions to answer:
- What patterns are forbidden?
- What linting rules do you enforce?
- What are your testing requirements?
- What code review standards apply?
-->

(To be filled by the team)

---

## Forbidden Patterns

<!-- Patterns that should never be used and why -->

(To be filled by the team)

---

## Required Patterns

<!-- Patterns that must always be used -->

### Convention: `cargo clippy` CI gate(`-D warnings`),零告警是硬标准

**What**: app crate(`app/src-tauri`)在 CI 跑 `cargo clippy --lib -- -D warnings`(`.github/workflows/ci.yml`,顺序 fmt → daemon build → clippy → test)。任何新 warning 直接红 CI,不允许用 `#[allow]` 掩盖。

**Why**: RULE-CI-001(2026-08-24 摸底):E1(07-05)的 "先本地清 warning 再加 gate" follow-up 一直未兑现,此后 +400 测试无机器把关。gate 落地于 2026-08-26(`.trellis/tasks/08-26-p2-debt-cleanup`),当时基线已清零。

**How**: 本地等价命令(WSL 需 `PKG_CONFIG_PATH`,同 test):

```sh
cargo clippy -p everlasting --lib -- -D warnings
```

确属误报的个别 lint 才允许 `#[allow]`,必须带一行理由注释。已知存量豁免仅 2 处 `too_many_arguments`(`agent/chat.rs` 的 `chat_inner`、`agent/chat_loop.rs` 的 `emit_max_turns_terminal`),均注明 `// See DEBT.md RULE-ARGS-001`,该 epic 收敛签名时移除。注意 gate 只覆盖 `--lib`,`#[cfg(test)]` 代码不在 clippy 范围内(测试文件的 import 卫生靠 review)。

**相关**: gate 步骤注释另说明 clippy 的 check 编译同样触发 build.rs externalBin 校验,故排在 daemon sidecar build 之后。

---

### Convention: markdown 资源 frontmatter 解析只用共享 `parse_md_resource`,禁止复制解析循环

**What**: 任何带 frontmatter 的 markdown 资源(B3 command / skill / subagent agent 文件)的解析,一律走 `resource_loader.rs` 的泛型入口 `parse_md_resource<T: MdResource>(content) -> (T, String)` 与共享 `parse_string_array(raw, warn_prefix)`;新资源类型只需定义自己的 `Frontmatter` struct + `apply_kv` 字段分支 + `impl MdResource`。禁止在 loader 里手写 fence 检测 / 逐行 `key: value` 循环 / 数组解析。

**Why**: RULE-FM-001(2026-08-26 闭合):此前同一段 fence/行循环逻辑 3 份逐字复制,一个行为变更 = 改 3 实现 + ≥3 测试。收敛后行为变更只改 `resource_loader.rs` 一处;各 loader 只保留字段差异(`match k` 分支)。

**How**: 字段分支示例(以 subagent 为例,数组字段走共享解析、warn 前缀区分资源):

```rust
"tools" => fm.tools = Some(parse_tools_array(&v)),   // thin wrapper: parse_string_array(v, "subagent: `tools`")
```

**Wrong vs Correct**: Wrong = 新 loader 复制 `parse_frontmatter` 的 fence/行循环;Correct = 实现 `MdResource` trait(一个 `apply_kv`)后一行 `parse_md_resource(content)`。多行值 / YAML 全集仍不在支持范围(升级走受维护 YAML crate 的决定不变,见 `resource_loader.rs` 注释)。

---

### Convention: `cargo fmt` 必须通过,提交前自动拦截

**What**: 任何 `app/src-tauri/**/*.rs` 改动,提交时必须通过 `cargo fmt --check`(无 `rustfmt.toml`,全默认风格)。

**Why**: 2026-07-27 一次 push 让 CI 的 `cargo fmt --check` 失败(~50 个 `.rs` 在提交前没跑 fmt,大多是 `use` 排序与长行折行)。fmt 是机器能 100% 判定的检查,不该靠人记,也不该等 CI 兜底——本地提交时就拦。

**How (已被自动化)**: `lefthook.yml` 的 `pre-commit.cargo-fmt` job 对暂存的 `.rs` 跑 `cargo fmt --check`,失败即拦截。开发机执行 `pnpm install` 时由 `prepare` 脚本自动 `lefthook install`,clone 即生效。手动修复命令:

```sh
cargo fmt --manifest-path app/src-tauri/Cargo.toml
git add -u   # 重新 add 格式化后的文件
```

> hook 故意用 `--check` 而非直接 `cargo fmt`:避免 hook 偷偷改暂存内容、制造混乱。失败时提示作者手动跑。

**相关**: 见仓库根 `lefthook.yml`;CI 的 `cargo fmt --check` 步骤在 `.github/workflows/ci.yml`。

---

## Testing Requirements

<!-- What level of testing is expected -->

### Convention: 测试需要 in-memory DB 池时用 `db::test_support::test_pool`,禁止手写三件套

**What**: `app/src-tauri` 测试里需要 SQLite in-memory 池时,一律 `use crate::db::test_support::test_pool;`(模块经 `#[cfg(test)]` 门控注册在 `db/mod.rs`)。禁止再手写 `SqlitePool::connect("sqlite::memory:")` + `PRAGMA foreign_keys=ON` + `migrations` 三件套。

**Why**: RULE-TESTPOOL-001(2026-08-26 闭合):此前 15 处手写复制,migrations 变更要同步 15 处。收敛后 migrations 语义变更只改 `db/test_support.rs` 一处。

**How**: 测试簇 hub(sessions_tests / memories_tests)用 `pub(super) use crate::db::test_support::test_pool;` 再导出,簇内子文件的 `use super::test_pool;` 不用动。

**注意**: 共享版形态 = connect `sqlite::memory:` + `PRAGMA foreign_keys=ON` + `run_migrations`。若某测试需要额外 setup(不同 PRAGMA、预置数据),在该测试内自行补充,**不要**往共享 `test_pool()` 加参数分支——共享函数保持零参数。

---

## Code Review Checklist

<!-- What reviewers should check -->

(To be filled by the team)
