# Step 4 follow-up 修复: diff 空 / attach 冲突 / system prompt 未注入

## 背景

上一个任务"step 4 follow-up: worktree attach/detach/delete opt-in + LLM transparency"上线后,3 个用户场景发现 bug,全部与 worktree 解耦后未充分考虑边界有关。本任务修复。

## Bug 1: UI diff 显示空 (LLM 编辑后)

### 症状
- LLM 编辑文件后,UI 点击 "diff" 弹层显示空(没文件变更)
- 但在 bash 里 `git diff` 能看到变更

### 根因分析(2 个可能原因)

**根因 1(高置信度)**: `git::diff::diff_worktree` 用 `repo.diff_tree_to_workdir_with_index(...)`,libgit2 这个 API **不包含 untracked 文件**。如果 LLM 用 `write_file` 创建新文件,libgit2 看不到,UI 显示空;bash `git diff` 默认也不显示 untracked,所以两边都是空 — 但 `git status` 会显示"Untracked files: <path>"。如果用户的 `write_file` 是改已 tracked 文件,理论上两边都该看到。

**根因 2(中置信度)**: LLM 实际编辑到了**项目根目录**(project.path),而不是 worktree。`attach_worktree` 后,`ctx.worktree_path` 应该指向 worktree,但有几种可能让 LLM 编辑错地方:
- (a) attach 之前 LLM 已经编辑,那时 `worktree_path` 是 NULL,工具写到 project.path
- (b) 工具内部用 `assert_within_root(worktree_path, ...)` 检查,如果 worktree_path 还没设就 fallback 到 project.path
- (c) write_file 接受绝对路径时,LLM 自己拼了 project.path 绝对路径

**根因 3(待用户确认)**: libgit2 二进制与 rust toolchain 在 worktree 上的兼容性问题,导致 workdir tree 解析失败,返回空 diff 而非 error(理论上不该静默)。

### 修复方案

主修根因 1 + 根因 2,根因 3 用 `tracing::warn!` + 端到端验证覆盖:

1. **`git::diff::diff_worktree` 改成两段式**:
   - 第一段: `diff_tree_to_workdir_with_index` (现有行为,捕获 modified/added/deleted tracked)
   - 第二段: 单独扫 `repo.statuses()` 的 untracked 文件,对每个 untracked 文件生成一个 `Delta::Added` 合成 entry,`diff_text` 是整文件 + 标记
2. **加 `tracing::info!` 记录** `worktree_path` / `worktree_state` / `file_count` / `untracked_count`,方便现场排查根因 3
3. **文档化**: `docs/HACKING-worktree.md` 加一节"diff 命令契约",说明 untracked 文件包含 + 新文件 (added) 与 bash `git diff` 的差异

## Bug 2: attach worktree 失败 "worktree already exists"

### 症状
```
attach worktree 失败: attach_worktree: worktree creation failed: worktree already exists at
/home/carlos/.local/share/everlasting/worktrees/087ea743-f0af-4b71-8e78-01d8c248e352/f43b757b-e772-4564-99e1-7b8ad56d78aa
```

错误是 libgit2 抛的(不是我们的 pre-check),说明 `repo.worktree(session_id, &worktree_path, Some(&opts))` 在第 161 行失败。

### 根因

`git::worktree::create` (worktree.rs:93-170) 有 3 个边界没处理:

1. **worktree 目录存在但不是 git worktree**: `worktree_path.exists()` 通过了 pre-check(返回 `Err`)。但 libgit2 是基于 `.git/worktrees/<session_id>/` metadata 判断的 — 如果 metadata 还在,libgit2 报 "worktree already exists"。
2. **branch `session/<id>` 已经存在**: `repo.branch(&branch_full, &head_commit, false)` 在 line 156 — `force=false`,如果 branch 存在,libgit2 报错。
3. **`.git/worktrees/<session_id>/` metadata 还在**: 上次 `delete_worktree` 失败,或 libgit2 状态不一致。

### 修复方案

修改 `git::worktree::create` 在 3 个边界做 self-healing:

1. **stale worktree metadata**: `repo.worktrees()` 列出所有 metadata name,如果包含 `session_id`,`Worktree::prune(None)` 清掉(best-effort,log warn)
2. **stale worktree dir**: 如果 `worktree_path.exists()` 但不是 .git 内的 dir(只可能是孤儿 dir),先 `git::worktree::prune` 再 `std::fs::remove_dir_all`(最暴力,但场景是 stale)
3. **stale branch**: `repo.find_branch(&branch_full, BranchType::Local)` 找到的话先 `branch.delete()`,再 `repo.branch(...)` 创建(或者干脆复用已有 branch 不删,但 force=true 重新指向 HEAD)

注意:这 3 步必须发生在 `check_clean` 之后(避免误删 dirty 用户数据)— 但 attach 流程里 check_clean 是 project root,不是 worktree,所以先后顺序 OK。

写 unit test 覆盖 3 种 stale 状态。

## Bug 3: system prompt 未告诉 LLM 在 worktree

### 症状
- attach worktree 之后,问 LLM "你的系统提示词有没有提到当前在 worktree",LLM 答 "没有"
- 用户期望:LLM 在每次请求时都应该知道自己在 worktree

### 根因

**`llm/client.rs:172` `system: None`** — 请求体里 `system` 字段是 `None`,LLM 根本**没有 system prompt**。

所谓"worktree event" 是通过 `db::insert_system_event` 以 `role='user'` 写进 messages 表,作为一条用户消息出现在 conversation history 里。但 LLM 看到的是 "user said: [worktree event] attached: ..." — 它知道这条消息是用户说的,不是系统。

LLM 的诚实回答"系统提示词没提到"是正确的 — 因为 system prompt 字段本来就是 None,worktree info 放错了地方。

### 修复方案

加一个**真正的 system prompt**,在每次 chat 请求时由 backend 构造:

```
You are a coding agent. You have access to tools (read_file, write_file, edit_file,
shell, grep, glob, list_dir). All file paths in tool inputs are relative to the
session's working directory.

Session context:
- Session ID: <session_id>
- Project: <project.name> (<project.path>)
- Working directory: <ctx.worktree_path>          <-- 或 project.path
- Worktree: ACTIVE on branch 'session/<id>'     <-- 或 NONE (project root)
- Branch: <session/<id>>  (HEAD at <commit SHA>)
```

构造规则:
- `WorktreeState::Active` → 显示 worktree path + branch + HEAD SHA
- `WorktreeState::Detached` / `None` → 显示 "Working directory: project.path" + "No worktree"
- 每次 chat 命令起手就 `db::load_session` + `db::get_project` + `repo.head().peel_to_commit()` 拿 HEAD SHA
- 通过 `chat_stream_with_tools` 的新参数 `system: Option<String>` 传进去(改 client.rs 的 signature)
- spec `llm-contract.md` Scenario 7 加一段 system prompt 契约

注意:tool result envelope 里 `cwd` 字段还要保留(给 LLM 在多 worktree 切换时识别当前 cwd),system prompt 是"持久声明",cwd 是"运行时数据"。

## 测试矩阵

| Bug | 测试 | 类型 |
|---|---|---|
| 1 | `diff_worktree::includes_untracked` | cargo test |
| 1 | `diff_worktree::modified_tracked` | cargo test |
| 2 | `worktree::create::prunes_stale_metadata` | cargo test |
| 2 | `worktree::create::deletes_stale_branch` | cargo test |
| 2 | `worktree::create::cleans_orphan_dir` | cargo test |
| 3 | `chat::builds_system_prompt_with_worktree` | cargo test |
| 3 | `chat::system_prompt_none_when_no_worktree` | cargo test |
| 3 | `chat_request::system_field_serializes_when_some` | cargo test |

## 风险

- **Bug 3 修 system prompt** 会让 LLM 看到 tool 描述之外的额外上下文,可能影响 token 计费 / 模型行为。需要 A/B 测一下(简单聊一次 vs 修后聊一次)。
- **Bug 1 加 untracked** 会让 `git diff` 弹层更"丰富" — 用户可能误以为是 LLM 错误(其实是写新文件)。需要在弹层里 status 标签区分。
- **Bug 2 self-heal** 必须 `tracing::warn!` 清楚,避免静默吃掉用户数据。建议在 toast 里也提示"清理了 stale 状态"。

## 范围外

- merge worktree (另一个 task 在 backlog)
- 持久化 system prompt (目前每次 chat 重新构造)
- LLM 透明度的"主动播报" — system prompt 是被动声明,如果 LLM 不主动读,仍然会"不知道"
