---
name: project-cwd-boundary
description: Contract for asserting a target path (cwd / working_directory / file path) is within a project root. Edge cases: prefix trap, symlinks, broken symlinks, nonexistent paths.
metadata:
  type: spec
  layer: backend
  applies_to: src-tauri/src/projects/boundary.rs
---

# Project CWD Boundary Spec

> 来源:[PROPOSAL §4.4 / Q6 / 评审消化 §11](../../../../docs/_archive/2026-06-3b-1/PROPOSAL-project-binding-and-top-tabs.md#44-chat-command--toolcontext-改造评审一致提)
> 实现位置:`src-tauri/src/projects/boundary.rs`
> 调用方:`tools::execute_tool` 内 `shell` / `read_file` / `write_file`;`lib.rs::chat` 命令构造 `ToolContext`

## 1. 合约

```rust
pub fn assert_within_root(root: &Path, target: &Path) -> Result<PathBuf>
// 成功:返回 target 的 canonical 绝对路径(供调用方用)
// 失败:返回 Err(anyhow!("...")),message 含 target + root + 失败原因
```

**强约束**(必须保证):
1. 必须用 **physical path**(`canonicalize` 解析 symlink),不能用 logical path
2. 必须用**末尾加 `/` 的前缀匹配**或 **component-wise 比较**,不能 `starts_with` 裸字符串
3. 不存在的路径 → 拒绝(无法判定是否在 root 内)
4. broken symlink → 拒绝(canonicalize 失败)

## 2. 7 个必测 edge case

| # | 场景 | root | target | 期望 |
|---|---|---|---|---|
| 1 | cwd == project_root | `/repo` | `/repo` | ✅ |
| 2 | cwd == project_root/subdir | `/repo` | `/repo/backend` | ✅ |
| 3 | cwd == project_root/../sibling (物理上在 root 内) | `/repo` | `/repo/../sibling` → canonicalize → `/repo/../sibling` 物理路径? | 取决于 OS |
| 4 | **前缀匹配陷阱** | `/repo/foo` | `/repo/foobar` | ❌(必须加 `/` 防护) |
| 5 | symlink 指向 root 外 | `/repo` | `/repo/link-to-etc`(symlink → `/etc`) | ❌(canonicalize 解析到 `/etc`) |
| 6 | target 不存在 | `/repo` | `/repo/no-such-dir` | ❌(canonicalize 失败) |
| 7 | broken symlink | `/repo` | `/repo/broken-link`(指向不存在目标) | ❌(canonicalize 失败) |

**关键 case 4**:`/repo/foobar` 用裸 `starts_with("/repo/foo")` 会返回 **true** — 但 `foobar` 不在 `foo` 内。必须在 canonicalize 后的字符串末尾加 `/`,或用 `Path::starts_with`(它会做 component-wise 比较,正确)。

## 3. 推荐实现

```rust
use std::path::{Path, PathBuf};
use anyhow::{anyhow, Result};

pub fn assert_within_root(root: &Path, target: &Path) -> Result<PathBuf> {
    // 1. canonicalize(target) — 解析 symlink + 物理路径
    //    不存在或 broken symlink → std::io::Error → 翻成 anyhow 错误
    let target_real = target.canonicalize()
        .map_err(|e| anyhow!("path '{}' cannot be resolved: {} (does not exist or is a broken symlink)", target.display(), e))?;
    let root_real = root.canonicalize()
        .map_err(|e| anyhow!("project root '{}' cannot be resolved: {}", root.display(), e))?;

    // 2. component-wise 前缀比较(Path::starts_with 内置,正确处理 '/repo/foobar' vs '/repo/foo')
    if target_real == root_real || target_real.starts_with(&root_real) {
        Ok(target_real)
    } else {
        Err(anyhow!(
            "path '{}' is outside project root '{}'",
            target_real.display(),
            root_real.display()
        ))
    }
}
```

**关键点**:
- 用 `Path::starts_with`(不是字符串) — Rust 的 `Path::starts_with` 做 component-wise 比较,自动处理 `/` 边界
- 不需要手动加 `/` 字符串
- `canonicalize` 失败 → 显式错误信息提示"不存在或 broken symlink"

## 4. 与 ToolContext 的集成

```rust
// lib.rs::chat 命令内
let project = db::get_project(session.project_id).await?;
let session = db::get_session(session_id).await?;

let ctx = ToolContext {
    project_root: boundary::assert_within_root(&app_data_dir, &project.path)?,  // 启动时校验一次
    cwd: boundary::assert_within_root(&project.path, &session.current_cwd)?,    // 启动时校验一次
};

// tools::execute_tool 内
let effective_cwd = input["working_directory"]
    .as_str()
    .map(Path::new)
    .unwrap_or(&ctx.cwd);
let validated_cwd = boundary::assert_within_root(&ctx.project_root, effective_cwd)?;
// ... 用 validated_cwd 跑命令
```

**turn 结束一次性写**:
```rust
// lib.rs::chat agent loop 末尾
if let Some(final_cwd) = turn_state.last_cwd {
    if final_cwd != session.current_cwd {
        db::update_session_cwd(&pool, &session_id, &final_cwd).await?;
    }
}
```

## 5. ⑨ 关 Tier 0 集成 (A2 + B7 PR1, 2026-06-13)

PR1 的 `⑨ 关 5-tier 决策层`(`app/src-tauri/src/agent/permissions::check`)
把 `assert_within_root` 集成作为 Tier 0 hard guard,放在 Tier 1
Hooks 之前:

```rust
// app/src-tauri/src/agent/chat.rs: L70-71,在 permission::check 之前
let effective_cwd = input.get("working_directory")
    .and_then(Value::as_str)
    .map(Path::new)
    .unwrap_or(&ctx.cwd);
let validated_cwd = boundary::assert_within_root(&ctx.project_root, effective_cwd)?;
```

行为:
- boundary 失败 → agent loop bail out, 不调 `permission::check` 也不
  调 `execute_tool`。这是 Tier 0 hard error (不是 ⑨ 关 Decision)。
- boundary 成功 → 走 ⑨ 关 Tier 1-6,见 `tool-contract.md §"⑨ 关
  Permission Decision Layer"`。
- 这保留了 §2 的 7 个 edge case 合约(前缀陷阱、symlink、broken
  symlink、nonexistent 等) — boundary check 是不可绕过的最后一道
  关。
- 工具内部的二次 boundary check(每个 tool 自己的 `assert_within_root` 调用):**write 族(`write_file`/`edit_file`)保留**作 defense in depth;**read 族(`read_file`/`grep`/`glob`/`list_dir`)2026-07-01 移除** —— 项目外读改由权限层 deny-list + allow-list + ask_path 受控(见 §7),避免"权限层 ask 通过后 tool 层硬卡又拒"的口径冲突。

## 6. Non-failing boolean variant: `is_within_root` (re-grill 2026-06-13)

> **Added in**: A2+B7 re-grill task `06-13-a2-b7-regrill-path-based`
> **Location**: `app/src-tauri/src/projects/boundary.rs::is_within_root`

⑨ 关 Tier 4 的 path-based 决策层需要 "is `target` inside
`root`?" 的二元判定,**不能**走 `assert_within_root` 的错误
返回契约(那个会拒绝不存在的路径,permission 层必须宽容
not-yet-existing 的 write target)。所以新增一个非失败
boolean 版本:

```rust
pub fn is_within_root(root: &Path, target: &Path) -> bool
```

### 与 `assert_within_root` 的差异

| 维度 | `assert_within_root` | `is_within_root` |
|---|---|---|
| 返回 | `Result<PathBuf>` | `bool` |
| 不存在的 target | 拒绝(`canonicalize` 失败) | 接受(lexical walk 父目录) |
| Symlink | `canonicalize` 解析 | lexical,不解析(用户视角) |
| 错误信息 | 详细 anyhow 消息 | 无 |
| 调用方 | tool 层(写前最后一道关) | permission 层(decision 辅助) |

### 关键行为

- 同样使用 `Path::starts_with` 做 component-wise 比较(防
  `/repo/foobar` vs `/repo/foo` 前缀陷阱)。
- 内部做 `..` / `.` 组件的 lexical normalize,所以
  `root/../sibling/file` 被正确归类为 "outside root"。
- 容忍 not-yet-existing write target:看其父目录是否在
  root 内(用户即将创建的文件)。
- 工具层 `assert_within_root` 仍然是 disk write 的 source
  of truth;permission 层只是辅助 decision。

### 7 个 edge case(全过)

| # | 场景 | 期望 |
|---|---|---|
| 1 | target == root | ✅ |
| 2 | target 是 root 的直接子目录 | ✅ |
| 3 | target 是 root 的多级后代 | ✅ |
| 4 | target 是 root 的 sibling | ❌ |
| 5 | target 是 root 的前缀陷阱(`/repo/foo` vs `/repo/foobar`) | ❌ |
| 6 | target 不存在但在 root 内 | ✅(parent 在 root 内) |
| 7 | target 不存在且在 root 外 | ❌ |
| 8 | empty target | ❌ |

8 个 unit test 在 `projects::boundary::tests::is_within_root_*`
锁定。

## 7. 敏感路径 deny-list / 受信 allow-list(read-side decouple, 2026-07-01)

> **Added in**: task `07-01-read-side-boundary-decouple`
> **Location**: `app/src-tauri/src/agent/permissions/sensitive.rs` + `check.rs` Tier 2.5 / Tier 4

read 族(`read_file` / `grep` / `glob` / `list_dir`)的 tool 层 `assert_within_root` 已**移除**(见 §5 末条)。项目外读改由权限层两份 static list 受控:

- **deny-list**(Tier 2.5,`check.rs`,早于 yolo bypass):项目外路径命中即硬 `Deny`、含 yolo、不可绕过。中等档 pattern:`~/.ssh/**`、`**/*.pem`/`*.key`/`*.p12`/`*.keystore`、`/etc/shadow`、`**/.env`(枚举 `.env.local`/`.production`,不挡 `.env.example`)、`**/*credentials*`/`*secret*`、`~/.aws/credentials`/`.netrc`/`.npmrc`/`.docker/config.json`。**仅项目外 lexical**生效(项目内真 `.env`/`*.pem` 信任);**项目内 symlink 逃逸**额外挡(canonicalize 后到项目外且敏感 → `Deny`,恢复原 tool 层 `assert_within_root` 的 symlink 保护)。
- **allow-list**(Tier 4 Path 分支,项目外、ask 前):两段 pattern 命中即免 `ask_path` 直接 `Allow` + 审计:
  - static 段 `~/.config/everlasting/**`(`permissions::sensitive::STATIC_TRUSTED_EXTERNAL_PATTERNS`)
  - 动态段 `<app_data_dir>/worktrees/**`(`build_trusted_external_patterns` + `init_trusted_external` 在 `AppState::load` 解析完 `app_data_dir` 后注入,覆盖 session worktree `<dir>/worktrees/<project_uuid>/<session_uuid>` + worker worktree `<dir>/worktrees/<project_uuid>/worker/<run_id>`)
  - 为什么需要动态段:worktree 是 agent 自己的运行时数据(任务上下文经常需要 cross-file 看),`~/.config/...` 同款免 ask 理由。平台差异(Linux `/root/.local/share/<bundle-id>/` / macOS `~/Library/...` / Windows `%APPDATA%\...`) + bundle id 从 `tauri.conf.json::identifier` 读,不能硬编码绝对路径。
  - Idempotent init:重复调用 `init_trusted_external` 是 no-op(first writer wins),`app_data_dir` 进程内固定。
- **优先级**:deny-list > allow-list > ask(`check.rs` 调用顺序保证)。
- **双 anchor**:`cwd` 决定 ask vs silent-Allow(历史不变);`worktree_path`(项目根)决定 deny/allow 的"项目外"触发 —— 避免 session cwd 是子目录时项目根文件被误判 outside。
- **匹配**:`globset`(`Cargo.toml` 已在依赖)+ `literal_separator(true)`(`*` 不跨 `/`、`**` 跨)+ `dirs::home_dir()` 展开 `~` + `OnceLock` 缓存编译结果。lexical(不 `canonicalize`)。
- **`~` 展开**(`projects::boundary::resolve_path`,2026-07-01):read 族 4 tool + `check.rs` 2 处 abs_path 共用此 helper,展开 `~` / `~/...` → home(`dirs::home_dir()`)。保证 LLM 的 `~/...` 写法能正确解析 + 命中 deny/allow-list(否则 `~` 被当字面目录名 → 路径错)。
- **worker 审计**:deny/allow 命中走 Tier 2 模式 `record_audit` 写父(与 `dangerous.rs` kill-list 一致),不触发 RULE-A-016(该规则仅约束 Tier 4 `ask_path` worker collapse)。
- **grep/glob 已知 gap**:deny-list 只匹配 tool 的 `path` 参数(搜索根);搜索结果里偶遇敏感文件内容不额外过滤(等同 redaction,OOS)。

write 族(`write_file`/`edit_file`)的 tool 层 `assert_within_root` **保留**(defense in depth)。

## 5. 关联

- PROPOSAL §4.4 (`chat` command + ToolContext 改造)
- PROPOSAL §4.4 (cwd 持久化时机)
- PROPOSAL §5 (前端)
- ARCHITECTURE §3 (worktree 路径)
