# L2 follow-up: RULE-A-013 — is_parallel_eligible 加 path-outside-root 检测,保持并发集合绝对 silent

## Goal

DEBT RULE-A-013(P2 open)的推荐 fix 方案 a:**`is_parallel_eligible` 加 path-outside-root 检测** —— 任一 out-of-root read tool 拉回串行批,保留"并发集合绝对 silent"不变量。低成本、纯函数扩展、零并发结构变化。

## What I already know

- **当前 `is_parallel_eligible`**(`chat_loop.rs:1486`):签名 `(tool_calls: &[(String, String, serde_json::Value)]) -> bool`,只看 tool name,不看 path。
- **DEBT RULE-A-013**(`DEBT.md:582-594`):并发集合 `{read_file,grep,glob,list_dir,use_skill}` 假设全部 silent Allow,但当 path 解析到仓库外 + 无 path-glob grant 时,`permissions::check` 走 `ask_path` → emit `permission:ask`。并行 batch 里多个 out-of-root read tool 会**并发弹多个 PermissionModal**(UX 乱)。无数据损坏,仅 UX 后果。fix 推荐方案 a = `is_parallel_eligible` 加 path-outside-root 检测。
- **`is_within_root(root: &Path, target: &Path) -> bool`**(`projects/boundary.rs:86`):非失败版,已存在,完整单测(8 个 case 覆盖 prefix-trap / nonexistent / empty / 等)。**直接复用**。
- **Path 解析约定**(`permissions/mod.rs:565-571`):`if absolute → as-is; if relative → ctx.cwd.join(p)`,然后 `is_within_root(&ctx.cwd, &abs_path)`。**直接镜像**到新逻辑,保持与权限层一致语义。
- **Caller**(chat_loop.rs:997):`is_parallel_eligible(&tool_calls)`。改成 `is_parallel_eligible(&tool_calls, &permission_ctx.cwd)`(`permission_ctx.cwd = session_cwd`,line 237)。
- **`use_skill`** 无 path arg(ToolKind::Other → Tier 5 default-allow)→ 不需要 path 检查,只要 name-eligible 即可。
- **现有 13+ 测试**(`tests.rs:2803-2844`)全部 `is_parallel_eligible(&batch(&[...]))`,签名改动需全部更新为 `is_parallel_eligible(&batch(&[...]), &root)`,`batch` helper 加 root 参数化。
- **DEBT 推荐方案 b**(两阶段 check-then-execute):更复杂,改 task 结构,本轮不采用。DEBT 描述里也明示 a 比 b 简单。

## Assumptions (temporary)

- 假设 `permission_ctx.cwd` 与 `current_ctx.cwd` 在 L2 并行批入口处一致(都是 session_cwd;L2 是 read-only,不会触发 cwd 变更)。如果后续 task model 引入 batch 内 cwd 变更,需要重新评估。
- 假设 `is_within_root` 现有 8 个测试覆盖足够,不需要为本次新增 boundary 单测。
- 假设 `extract_path_arg(tool_name, tool_input)`(permissions/mod.rs)拿到的 path 与 tool 实际执行时拿到的 path 是一致的(同 source)。**轻验风险点**:如不一致,会出现"谓词说 in-root 但 tool 实际读到 out-of-root"的偏差 → 仍走 `ask_path` 路径,但只会单 modal(不再是并发),UX 退化但不破。接受。

## Open Questions

(无 blocking 决策点;方案 a 是 DEBT 推荐 + 已有 `is_within_root` API + 最小改动,3 个 vector 全 OK。)

## Requirements

- R1: `is_parallel_eligible` 签名扩展 `root: &Path`,增加 path-outside-root 检测 —— 任一 path tool(`read_file`/`grep`/`glob`/`list_dir`)的 input `path` 解析到 root 之外 → 返回 false。
- R2: `use_skill` 保留 name-eligible 即可,不参与 path 检查(无 path arg,Tier 5 default-allow 永远 silent)。
- R3: path 解析约定与 `permissions/mod.rs:565-571` 完全一致:absolute → as-is;relative → `root.join(p)`;None → 当作 eligible(沿用 permissions 层的"无 path 走 Allow"约定,tool layer schema validation 兜底)。
- R4: caller `chat_loop.rs:997` 同步更新,传 `&permission_ctx.cwd`。
- R5: 现有 13+ 测试全部更新签名;`batch` helper 参数化 root;**新增** path-outside-root 场景的 5 个测试 case(in-root absolute/relative、out-of-root absolute、out-of-root relative `../foo`、path tool 无 path arg、use_skill + 任意 path 共存)。
- R6: `docs/ARCHITECTURE.md §2.5.9` 更新 Q2 rationale —— 谓词从"tool name 白名单"升级为"name 白名单 + path-in-root",并行集合绝对 silent 不变量证明完整。
- R7: `DEBT.md RULE-A-013` 在 PR 收尾时 closed,记 commit hash + Closed At。

## Acceptance Criteria

- [ ] `cargo check` 0 warning 0 error
- [ ] `cargo test --lib` 全部 pass(629 + 新增 5 个 path-outside-root case = 634)
- [ ] `is_parallel_eligible` 14 个老测试 call 全部更新签名,无 compile error
- [ ] 新测试覆盖:absolute in-root / relative in-root / absolute out-of-root / relative `../foo` out-of-root / path tool 无 path arg / use_skill + 任意 path 共存(共 5 个新 case,可放在 1 个 `#[test]` 或拆开)
- [ ] DEBT.md RULE-A-013 status → closed,记 commit hash
- [ ] ARCHITECTURE.md §2.5.9 Q2 rationale 描述更新

## Definition of Done

- 代码改动:`chat_loop.rs` 谓词 + caller + `tests.rs` 14 老 call + 5 新测试
- 文档:`ARCHITECTURE.md §2.5.9` 更新 + `DEBT.md RULE-A-013` closed
- 验证:`cargo check` + `cargo test --lib` 全绿
- 收尾:commit (fix 改 + docs 改可一并)→ 归档 task → 记 journal

## Technical Approach

**核心改动 1 处 + 调用点 1 处 + 测试 2 处**:
1. `app/src-tauri/src/agent/chat_loop.rs:1486-1496` — `is_parallel_eligible` 签名加 `root: &Path`,增加 path 检查循环(对 `read_file`/`grep`/`glob`/`list_dir` 解析 input.path,与 permissions/mod.rs 同样的 absolute/relative 约定)。
2. `app/src-tauri/src/agent/chat_loop.rs:997` — caller 传 `&permission_ctx.cwd`。
3. `app/src-tauri/src/agent/tests.rs:2803-2844` — 14 个老 call 全部更新;`batch` helper 加 `paths: &[&str]` 参数(每个 tool 的 path,空串 = 无 path);新增 1 个 `is_parallel_eligible_boundary_silent` 测试覆盖 5 个 path 场景。

**逻辑伪代码**:
```rust
pub(crate) fn is_parallel_eligible(
    tool_calls: &[(String, String, serde_json::Value)],
    root: &Path,
) -> bool {
    const NAME_ELIGIBLE: &[&str] = &["read_file", "grep", "glob", "list_dir", "use_skill"];
    const PATH_TOOLS: &[&str] = &["read_file", "grep", "glob", "list_dir"];
    if tool_calls.is_empty() {
        return false;
    }
    for (_, name, input) in tool_calls {
        if !NAME_ELIGIBLE.contains(&name.as_str()) {
            return false;
        }
        if PATH_TOOLS.contains(&name.as_str()) {
            // Mirror permissions/mod.rs:565-571 path resolution
            if let Some(p) = input.get("path").and_then(|v| v.as_str()) {
                if !p.is_empty() {
                    let abs = if Path::new(p).is_absolute() {
                        PathBuf::from(p)
                    } else {
                        root.join(p)
                    };
                    if !is_within_root(root, &abs) {
                        return false;
                    }
                }
            }
            // None / empty path → treat as eligible (tool layer validates)
        }
    }
    true
}
```

**注释更新**:谓词上方 doc 段需补一段说明 path check 的存在 + 与 permissions 层 path 解析的一致性保证。

## Decision (ADR-lite)

**Context**: DEBT RULE-A-013 描述 L2 并行批的并发集合"假设全部 silent"在 path-outside-root 场景下破裂 —— 多 modal 并发弹出导致 UX 乱。DEBT 给出方案 a(谓词扩展)与方案 b(两阶段 check-then-execute)。

**Decision**: 采用方案 a(DEBT 推荐)。`is_within_root` API 已有,单测覆盖充分,签名扩展 + 循环检查成本 ~15 行,串行路径完全不动。

**Consequences**:
- ✅ 并发集合 silent 不变量补齐
- ✅ 串行路径行为完全不变(被拉回串行的 batch 走老路径,逐个 ask 弹 modal)
- ✅ 公共 API 复用 `is_within_root`,无重复实现
- ⚠️ `is_parallel_eligible` 不再是"纯 name 谓词"—— doc 需更新
- ⚠️ 测试 helper `batch` 需参数化路径(轻微 churn)
- 暂不采用方案 b:两阶段 check-then-execute 改 task 结构,需要 cache 决策 + 重排 await,影响面大且与 L2 MVP 目标"最小并行结构"冲突

## Out of Scope

- 方案 b(两阶段 check-then-execute)—— 复杂,改 task 结构
- L2 谓词的 path-glob grant 探测 —— Tier 4.1 path grant 命中会 silent Allow,但本轮只防御 out-of-root 场景(grant 命中后 `is_within_root` 也会通过,所以 in-root + grant 路径已隐式覆盖;out-of-root + grant 的窄场景暂不专门优化)
- `is_within_root` 本身的新增测试 —— 8 个老 case 足够,本轮不重复
- `web_fetch` 重新评估 —— DEBT 未点名,且 URL 不在 `is_within_root` 语义内
- worktree path vs cwd path 的差异讨论 —— 两者在 L2 入口一致
- 前端 modal 队列化(解决"多 modal 叠加"作为对照方案)—— 是 UX 层的另一条路,DEBT 选的是谓词层;不并列做

## Technical Notes

- **核心文件**:
  - `app/src-tauri/src/agent/chat_loop.rs:1486-1496`(谓词)
  - `app/src-tauri/src/agent/chat_loop.rs:997`(caller)
  - `app/src-tauri/src/agent/tests.rs:2803-2844`(14 老 call + batch helper)
  - `app/src-tauri/src/projects/boundary.rs:86`(`is_within_root` 复用)
  - `app/src-tauri/src/agent/permissions/mod.rs:560-571`(path 解析约定对齐)
- **DEBT.md**:RULE-A-013 在 § 580-594,closed 时需改 status + Closed At + Related Task
- **ARCHITECTURE.md §2.5.9**:L2 架构小节,Q2 rationale 描述需补 path check
- **journal**:session 42 记录本轮:fix+docs 一并 commit,DEBT RULE-A-013 closed
- **commit 风格**:按 memory `trellis-task-finish-commit-pattern`,本轮是单 fix+docs,可一 commit 收口(`fix(l2): RULE-A-013 — is_parallel_eligible 加 path-outside-root 检测,保持并发集合 silent`)
- **影响面**:仅 chat_loop.rs 谓词 + caller + tests.rs,无 PR 拆分需求(总改动 ~30 行,文档 2 处)
