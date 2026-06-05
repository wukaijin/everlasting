---
name: project-cwd-boundary
description: Contract for asserting a target path (cwd / working_directory / file path) is within a project root. Edge cases: prefix trap, symlinks, broken symlinks, nonexistent paths.
metadata:
  type: spec
  layer: backend
  applies_to: src-tauri/src/projects/boundary.rs
---

# Project CWD Boundary Spec

> 来源:[PROPOSAL §4.4 / Q6 / 评审消化 §11](../../../../docs/PROPOSAL-project-binding-and-top-tabs.md#44-chat-command--toolcontext-改造评审一致提)
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

## 5. 关联

- PROPOSAL §4.4 (`chat` command + ToolContext 改造)
- PROPOSAL §4.4 (cwd 持久化时机)
- PROPOSAL §5 (前端)
- ARCHITECTURE §3 (worktree 路径)
