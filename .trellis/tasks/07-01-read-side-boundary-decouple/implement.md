# Implement — read 侧路径边界解耦

> 配套 `design.md`。按 Step 顺序执行;每个 Step 是独立 commit / 回滚点。

## 前置核查(已完成)

- ✅ `globset 0.4` + `dirs 5` 已在 `app/src-tauri/Cargo.toml`(:44 / :61)。无需加依赖。
- ✅ read 族 4 处 `assert_within_root` 调用点已定位:`read_file.rs:120` / `grep.rs:150` / `glob.rs:84` / `list_dir.rs:76`。
- ✅ 权限层 check 顺序已确认:`chat_loop.rs:2001` check → `:2105` execute_tool。

## Steps

### Step 1 — 新建 `agent/permissions/sensitive.rs`(独立,无依赖)

- `SENSITIVE_PATH_PATTERNS: &[&str]`(中等档清单,见 design §3)。pattern 用 `~/` 占位。
- `TRUSTED_EXTERNAL_PATTERNS: &[&str] = &["~/.config/everlasting/**"]`。
- `fn build_globset(patterns) -> GlobSet`:`~` → `dirs::home_dir()` 展开后编译 Glob。lazy static(`once_cell::sync::Lazy` 或 `std::sync::OnceLock`;看项目惯例)缓存编译结果。
- `pub fn is_sensitive_path(abs_path: &Path) -> bool` / `pub fn is_trusted_external(abs_path: &Path) -> bool`。lexical 匹配(不 canonicalize)。
- `mod tests`:私钥 pattern 命中、`.env` 命中、`.env.example` **不**命中、`~/.config/everlasting/**` 命中、项目内 `.env` 也能命中(pattern 不区分内外,触发与否由 caller 用 worktree_path 决定)。
- **Review gate**:单测全绿才进 Step 3。

### Step 2 — `PermissionContext` 加 `worktree_path`

- `agent/permissions/types.rs`:`struct PermissionContext` 加 `pub worktree_path: std::path::PathBuf`。
- `agent/chat_loop.rs` 所有构造 `PermissionContext` 的点(production + 测试 stub):填入 `current_ctx.worktree_path.clone()`(或测试用 tempdir canonicalize)。
- grep 确认所有 `PermissionContext {` 字面量都已补字段(cargo check 会强制)。
- **回归点**:仅加字段,不改任何现有判断逻辑 — cargo test 应全绿。

### Step 3 — `check.rs` Tier 2.5 插入 deny-list 检查

- 位置:`check.rs` Tier 2(`dangerous::is_kill_listed`)之后、Tier 3(Mode)之前。
- 条件:`classify_tool(tool_name) == Path` **且** tool ∈ {read_file, grep, glob, list_dir}(用新 helper `is_read_path_tool` 或在 classify_tool 旁加判定;write_file/edit_file 不进)。
- 逻辑:extract path → abs_path(复用 `extract_path_arg` + ctx.cwd join);`if !is_within_root(&ctx.worktree_path, &abs_path) && sensitive::is_sensitive_path(&abs_path)` → `record_audit(ToolDenied / ToolDeniedYolo by mode)` + `return Decision::Deny { reason: "path blocked: matches sensitive-path deny-list ...", critical: true }`。
- **Review gate**:`cargo test --lib permissions` 全绿;新增"sensitive path 在 yolo 也 Deny"测试。

### Step 4 — `check.rs` Tier 4 Path 分支插 allow-list

- 位置:现有项目外侧(`check.rs:210-216` `ask_path` 调用前)。
- 逻辑:`if sensitive::is_trusted_external(&abs_path)` → `record_audit(ToolAllowed)` + `return Decision::Allow`。
- **Review gate**:新增"项目外 `~/.config/everlasting/**` 免 ask Allow"测试。

### Step 5 — 删 read 族 4 处 `assert_within_root`

- `read_file.rs:120` / `grep.rs:150` / `glob.rs:84` / `list_dir.rs:76`:删 `match assert_within_root(...)` 块,`validated` 直接 = `requested`(abs path)。
- 保留对 `tokio::fs` 的调用(读不存在文件自然 IO error,见 AC6)。
- `use crate::projects::boundary::assert_within_root;` 若变 unused → 删 import(cargo check 会警告)。
- **回归点**:`write_file.rs` / `edit_file.rs` 的 `assert_within_root` **不动**(AC7)。

### Step 6 — 改 read 族现有"项目外 reject"测试语义

- `read_file.rs:401/415/438`、`list_dir.rs:271`、`glob.rs:358`、`grep.rs` 对应:`path_outside_root_rejected` 类断言现已失效(tool 层不再 reject)。
- 改为:`path_outside_root_no_longer_rejected_by_tool_layer` — 验证 tool 层 `execute()` 对项目外路径**返回内容**(不再 is_error)。边界由权限层单测覆盖。
- **Review gate**:cargo test 全绿。

### Step 7 — 更新 spec `.trellis/spec/backend/project-cwd-boundary.md`

- §5 "工具内部二次 boundary check" 段:注明 read 族(tool 层)**移除** defense-in-depth;write 族保留。
- 新增 §"敏感路径 deny-list / 受信 allow-list(权限层维度)"小节:列中等档 pattern、allow-list `~/.config/everlasting/**`、优先级 deny > allow > ask、双 anchor(cwd 决定 ask、worktree_path 决定 deny/allow 触发)。

### Step 8 — 手动验证 AC

- 启 `pnpm tauri dev`,在 edit / plan / yolo 三模式各验:AC1-AC6(read 族项目外、敏感 deny、everlasting allow、不存在 IO error)、AC7(write 仍拒)、AC10(everlasting 免弹窗)。

## Validation commands

```bash
# 编译 + 单测(WSL 需 PKG_CONFIG_PATH,见 CLAUDE.md)
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo check
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib
# 针对 permissions + boundary + read 族
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib permissions:: boundary:: tools::read_file tools::list_dir tools::glob tools::grep
# 前端 type-check(read 族无前端改动,保险跑)
cd app && pnpm build
```

## 回滚点

- Step 1 / 3 / 4 / 5 / 6 各自独立 commit;任一 Step 出问题可单独 revert。
- 全量回滚 = `git revert` 本 task 所有 commit;`sensitive.rs` 删除 + `PermissionContext.worktree_path` 字段移除。

## 风险文件

- `agent/permissions/check.rs` — 改动核心(Tier 2.5 + Tier 4),勿误伤 Shell / WebFetch / GitMutation 分支。
- `agent/permissions/types.rs` — `PermissionContext` 加字段,所有构造点要同步(否则 cargo check 失败,属强约束安全网)。
- `tools/write_file.rs` / `edit_file.rs` — **只读不改**,作为 AC7 回归基线。
