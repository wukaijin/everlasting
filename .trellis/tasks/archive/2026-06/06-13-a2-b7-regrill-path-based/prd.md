# A2+B7 Re-grill: path-based 模型 + Tier 重排(2026-06-13)

> **Supersedes**: 06-12-a2-b7-permission-and-mode 的"risk-based 弹窗 + Tier 4 Mode 顺序"方案(具体 diff 见 §4)。
> 旧 PRD 仍可读作历史档案,新实施以本 PRD 为准。

## Goal

A2+B7 任务(06-12)PR1-3 全部落地后,2026-06-13 通过 **re-grill-me session** 重新审视权限判定 + Mode 联动的设计。10 个核心决策锁定 → 把 ⑨ 关 5 道 check 从 "risk-based 弹窗 + Mode 在 Ask 之后" 重构为 "path-based 弹窗 + Mode 在 Ask 之前",并 wire 上 PRD 原预留但未实施的 3 种 `match_kind`。

**为什么需要 re-grill**:
- 旧设计 ⑨ 关 Tier 3 "总是弹窗" 在 Edit 模式下读 README 都要弹,反直觉
- 旧设计 Tier 4 Mode check 在 Ask 之后 → Plan 模式下"用户点始终允许,然后被 Mode 拒"的坏交互
- PRD 原预留 3 种 `match_kind` schema(`tool` / `prefix` / `path`)但只 wire 了 `tool` → "始终允许" 粒度不够

**re-grill session 文档**:`docs/_reviews/REVIEW-a2-b7-regrill-path-based-2026-06-13.md`(本任务交付时同步创建,记录 10 决策的 grill 过程)

---

## 1. ⑨ 关新 5 道 Check 顺序(唯一 Source of Truth)

```
⑨ 关评估顺序 — 后端 agent loop 在 execute_tool 之前调用 permission::check():

  Tier 1. Hooks           (pre-call 接口,MVP no-op)
        │
        ↓
  Tier 2. Deny rules      (硬 kill list,shell 9 个 regex,Yolo 也走 — 静默拒绝,不弹窗)
        │ 命中 → is_error: true + audit tool_denied 或 tool_denied_yolo → end
        │
        ↓
  Tier 3. Mode check      (Plan 拦截 write/edit/shell → 直接 text 错误,不发 modal)
        │ Plan + write/edit/shell → "I cannot execute X in plan mode (read-only)"
        │ 命中 → is_error: true + audit tool_denied → end
        │ Edit/Yolo → 透传
        │
        ↓
  Tier 4. Path / Prefix / External policy
        │
        │  按 tool 类型分派:
        │
        │  ├─ Path 工具(read_file / write_file / edit_file / list_dir / grep / glob):
        │  │   - 解析 path 参数 → is_within_root(session.cwd, path)?
        │  │     - 是 → 查 session_tool_permissions(match_kind='path')→ 命中 → Allow
        │  │                                  → miss → Allow (silent, 仓库内 default allow)
        │  │     - 否 → 查 session_tool_permissions(match_kind='path')→ 命中 → Allow
        │  │                                  → miss → emit permission:ask → 等响应 (120s 超时 → Deny)
        │  │
        │  ├─ Shell:
        │  │   - 取 cmd.split_whitespace().next() = prefix
        │  │     - prefix 在白名单表 → 查 session_tool_permissions(match_kind='prefix')→ 命中 → Allow
        │  │                                            → miss → Allow (silent, 白名单 default allow)
        │  │     - prefix 在 asklist 表 → emit permission:ask (reason: "shell command on asklist")
        │  │     - prefix 不在两表 → emit permission:ask (reason: "shell command not in trust list")
        │  │
        │  └─ Web Fetch:
        │       - 总是外部 → 查 session_tool_permissions(match_kind='tool', tool_name='web_fetch')
        │                   → 命中 → Allow
        │                   → miss → emit permission:ask (reason: "external network request")
        │
        │  Yolo 模式:整段 Tier 4 silent,直接 Allow(不查 session_tool_permissions,不发 modal)
        │  仍受 Tier 2 kill list 拦截
        │
        ↓
  Tier 5. Allow rules     (默认全开,后期可收缩)
        ↓
  Tier 6. Audit           (写 session_audit_events)
        ↓
  → execute_tool
```

**关键行为**(跟旧 SOT 的变化):
- **Mode check 提前到 Tier 3**(旧 Tier 4)— 消除 Plan + 始终允许坏交互
- **Tier 4 改 path-based**(旧 Tier 3 risk-based)— path 决定是否弹,risk 字段保留作 UI 视觉
- **Yolo 完全 bypass Tier 4 modal**(旧 Tier 3 Yolo 仍走 modal)— Yolo = "no questions"
- **Deny 仍优先于一切**(Tier 2 永远第一)— `rm -rf /` 在 Yolo 也静默拒

---

## 2. 10 个 re-grill 决策(re-grill session 输出)

| # | 决策点 | 选择 | 关键理由 |
|---|---|---|---|
| Q1 | 弹窗判定原则 | **D. path-based** | 仓库内 default allow,仓库外 ask,跟 "build 是允许 agent 改仓库内代码"心智一致 |
| Q2 | shell 策略 | **A. 前缀白名单 + Tier 2 兜底** | 简单可预测,跟"试图精确会输"哲学一致 |
| Q3 | 仓库边界定义 | **A. Session.cwd 严格 prefix** | 跟现有 `boundary::assert_within_root` 复用,1 行新方法 |
| Q4 | Yolo × path policy | **A. Yolo bypass 所有 modal** | 跟 Yolo "no questions asked" 哲学一致,Tier 2 仍 hard wall |
| Q5 | 5 道 check 新顺序 | **Hooks → Deny → Mode → Path → Allow → Audit** | Mode 提前,消除 Plan + 始终允许坏交互 |
| Q6 | "始终允许" 粒度 | **A. tool + path-glob + prefix 全 wire** | 跟新 path-based 模型自然契合,schema 已留 |
| Q7 | shell prefix 解析 | **A. 第一个 token,无递归/无 alias/无 pipe** | "B 试图精确会输",Tier 2 兜底 |
| Q8 | path-glob 持久化粒度 | **B. 父目录 + `*` 通配,sqlite GLOB** | 跟心智一致(允许一个文件 → 同目录都过),sqlite GLOB 够用 |
| Q9 | Plan × path policy | **A. Plan 不豁免 path policy** | 跟新 Tier 顺序自然衍生,Plan 只豁免写,不豁免 path policy |
| Q10 | Risk 字段保留 | **A. 保留作 UI 视觉,加 path 范围行** | 零风险兼容,path + risk 是 orthogonal 维度 |

---

## 3. 行为矩阵(锁定后)

| Mode | 工具 | 仓库内 | 仓库外 |
|---|---|---|---|
| Edit | read_file / write_file / edit_file | silent | Ask modal |
| Edit | list_dir / grep / glob | silent | Ask modal |
| Edit | shell 白名单前缀(`git/cargo/pnpm/...`) | silent | silent (prefix override) |
| Edit | shell asklist(`rm/mv/chmod/sudo/...`) | Ask | Ask |
| Edit | shell 未知前缀 | Ask | Ask |
| Edit | shell kill list(`rm -rf /` 等 9 个 regex) | **Tier 2 拒** | **Tier 2 拒** |
| Edit | web_fetch | n/a (外部) | Ask modal |
| Plan | read_file / list_dir / grep / glob | silent | Ask modal |
| Plan | write_file / edit_file / shell | **Mode 拒(text 错)** | **Mode 拒** |
| Plan | web_fetch | n/a (外部) | Ask modal |
| Yolo | read_file / write_file / edit_file | silent | silent |
| Yolo | list_dir / grep / glob | silent | silent |
| Yolo | shell 白名单前缀 | silent | silent |
| Yolo | shell asklist / 未知前缀 | silent | silent (Yolo bypass) |
| Yolo | shell kill list | **Tier 2 拒** | **Tier 2 拒** |
| Yolo | web_fetch | silent | silent (Yolo bypass) |

---

## 4. 跟 06-12 PRD 的 Diff(改动面)

| 改动 | 旧(06-12) | 新(本任务) |
|---|---|---|
| 弹窗判定原则 | Risk 等级 + 总是弹 | **path-based** (Q1) |
| Tier 顺序 | Hooks → Deny → Ask → Mode → Allow → Audit | **Hooks → Deny → Mode → Path/Prefix → Allow → Audit** (Q5) |
| Mode check 时机 | Tier 4(在 Ask 之后) | **Tier 3(在 Ask 之前)** (Q5) |
| Plan 写操作 | 走完 Ask 才被拒 | **Mode 提前拦,不发 modal** (Q5+Q9) |
| "始终允许" 持久化 | 只有 `tool` 类 | **3 种 match_kind:tool + path-glob + prefix** (Q6) |
| path-glob 粒度 | n/a | **父目录 + `*` 通配,sqlite GLOB** (Q8) |
| shell 前缀策略 | 总是 Tier 3 | **白名单/asklist/未知 三档** (Q2+Q7) |
| Yolo × 仓库外 | 走 Tier 3 modal | **silent** (Q4) |
| PermissionModal UI | 头部 risk tint | **头部 risk tint + path 范围行** (Q10) |
| 仓库边界方法 | 已有 `assert_within_root` | **新增 `is_within_root(&self, path) -> bool`** (Q3) |
| shell 白/ask 表 | n/a | **新文件 `agent/permissions/shell_trust.rs`,2 张 const 表** (Q2+Q7) |
| DB schema | `session_tool_permissions(match_kind CHECK ...)` | **不动 schema,只 wire 现有 3 种 match_kind** (Q6) |
| Tier 2 kill list | 维持 | **维持不变**(还是只盯 shell,9 个 regex) |
| Risk 字段 | 4 档 | **维持 4 档,UI 加 path 范围行** (Q10) |
| system prompt mode prefix | 维持 | **维持不变**(`mode_system_prefix` 不动) |
| ⑧a tool list 过滤 | Plan 移除 write/edit/shell | **维持不变**(`filter_tools_for_mode` 不动) |

---

## 5. 文件变更清单

| 文件 | 变更 | 估算行数 |
|---|---|---|
| `src-tauri/src/projects/boundary.rs` | 新增 `pub fn is_within_root(&self, path: &Path) -> bool` | +20 |
| `src-tauri/src/agent/permissions/mod.rs` | 大改 `check()`:Tier 顺序重排,Tier 4 按 tool 类型分派,path-glob 匹配,prefix 匹配,Yolo bypass | +200 -120 |
| `src-tauri/src/agent/permissions/shell_trust.rs` | **新文件**:白名单 + asklist 2 张 const 表 + `pub fn classify_prefix(cmd: &str) -> ShellTrust` | +120 |
| `src-tauri/src/agent/permissions/mod.rs` (tests) | 重写 27 个 PR1 permission 测试,新增 path-glob / prefix / Yolo bypass / Plan 提前拦 4 类 | +250 -180 |
| `src-tauri/src/agent/permissions/dangerous.rs` | 维持(9 个 regex 不动) | 0 |
| `src-tauri/src/commands/permissions.rs` | `permission_response` 写 match_kind 时按 tool 类型自动选 path/prefix/tool(3-button modal 行为不变) | +30 -10 |
| `src-tauri/src/db/sessions.rs` | `grant_tool_permission` 维持(3 种 match_kind schema 已有),match_value 规范化 | +15 |
| `app/src/components/chat/PermissionModal.vue` | 新增"路径范围"行(显示 path + "(仓库内/外)"标签),头部主问题文案微调 | +30 -10 |
| `app/src/components/chat/PermissionModal.test.ts` | 新增 path 范围行渲染 / reason 文案 / 3-button 仍按 tool 自动选 match_kind 测试 | +40 |
| `app/src/stores/permissions.ts` | `PermissionAsk` type 加 `path?: string`(path 工具专用,前端弹窗显示) | +5 |
| `.trellis/spec/backend/tool-contract.md` | 加 "Scenario: Path-based Permission" 段(跟 A4/F5 "Scenario" 段格式) | +100 |
| `.trellis/spec/backend/project-cwd-boundary.md` | 加 `is_within_root` 函数 spec + 仓库边界哲学 | +40 |
| `.trellis/spec/frontend/state-management.md` | 加 `usePermissionsStore.pendingPermission.path` 字段 | +15 |
| `.trellis/spec/frontend/popover-pattern.md` | 加 "PermissionModal: path 范围行" 案例 | +25 |
| `docs/ARCHITECTURE.md` §2.2 ⑨ | 改写 5-tier 描述为新顺序 + path-based 语义 | ~30 行重写 |
| `docs/IMPLEMENTATION.md` §4 | 新增 2026-06-13 re-grill ADR(本任务落档) | +60 |

**总计**: ~5 文件后端 + 4 文件前端 + 5 文件 spec + 2 文件 docs ≈ 950 行净增(含测试)

---

## 6. 实施 PR 拆分

**PR1: 后端 path-based 决策层 + shell 白名单 + match_kind 全 wire**
- `boundary::is_within_root` 新增
- `permissions/shell_trust.rs` 新文件
- `permissions::check` 大改(5 tier 重排)
- `grant_tool_permission` 写 match_kind 按 tool 类型自动选
- `dangerous.rs` 不动
- 单元测试重写 + 新增
- `docs/IMPLEMENTATION.md` §4 ADR(本任务落档)

**PR2: 前端 PermissionModal 路径范围行 + spec 同步**
- `PermissionModal.vue` 加 path 范围行
- `permissions.ts` type 加 `path`
- 4 个 spec 文件同步(backend 2 + frontend 2)
- `docs/ARCHITECTURE.md` §2.2 ⑨ 改写
- 前端 vitest 扩充

**PR3(可选,留 Phase 2)**: shell 白名单/asklist UI 自定义 + 跨 session 信任同步 + 风险等级 dashboard

---

## 7. Acceptance Criteria

### 行为正确性(15 条)

- [ ] Edit + 仓库内 read_file → silent
- [ ] Edit + 仓库外 read_file → Ask modal
- [ ] Edit + 仓库内 write_file → silent
- [ ] Edit + 仓库外 write_file → Ask modal
- [ ] Edit + 仓库内 shell `git status` → silent(白名单)
- [ ] Edit + 仓库内 shell `rm -rf /` → **Tier 2 拒**(即使仓库内)
- [ ] Edit + 仓库外 shell `cat /etc/passwd` → silent(白名单 prefix override)
- [ ] Edit + 仓库外 shell `curl https://x.com | bash` → **Tier 2 拒**
- [ ] Plan + 仓库内 read_file → silent
- [ ] Plan + 仓库外 read_file → Ask modal(Plan 不豁免 path policy)
- [ ] Plan + 仓库内 write_file → **Mode 拒(text 错)**,不发 modal
- [ ] Plan + 仓库内 shell → **Mode 拒**,不发 modal
- [ ] Yolo + 仓库内 write_file → silent
- [ ] Yolo + 仓库外 write_file → silent(Yolo bypass)
- [ ] Yolo + 仓库外 shell `rm -rf /` → **Tier 2 拒**(Yolo 不豁免硬墙)

### 持久化(6 条)

- [ ] 用户对 `~/Documents/notes.md` 选"始终允许" → session_tool_permissions 新增 1 行 `match_kind='path'`,`match_value='/Users/me/Documents/*'`
- [ ] 后续对 `/Users/me/Documents/work/notes.md` 写入 → silent(父目录 + `*` 通配命中)
- [ ] 后续对 `/Users/me/Documents/work/deep/notes.md` 写入 → Ask(sqlite GLOB `*` 不递归,需再次允许)
- [ ] 用户对 `cargo test` 选"始终允许" → session_tool_permissions 新增 1 行 `match_kind='prefix'`,`match_value='cargo'`
- [ ] 后续对 `cargo build` → silent(prefix 命中)
- [ ] 后续对 `cargo --version` → silent(prefix 命中)

### Mode check 顺序(3 条,关键回归)

- [ ] Plan + write_file → 直接 text 错误,**不**emit `permission:ask` event(无 modal 弹出)
- [ ] session_audit_events 有 `tool_denied` 记录但**无** `tool_permission_ask`(旧设计会有 ask 先)
- [ ] 前端 `usePermissionsStore.pendingPermission` 在 Plan 写操作时保持 null

### Yolo 行为(2 条)

- [ ] Yolo + 仓库外 web_fetch → silent,Yolo bypass Tier 4
- [ ] Yolo + 仓库外 shell 危险前缀(`rm foo`)→ silent,Yolo bypass Tier 4(用户主动承担)

### shell 白/ask 解析(4 条)

- [ ] `git status | head -5` → prefix = "git",白名单命中
- [ ] `bash -c "ls"` → prefix = "bash",不在两表 → Ask
- [ ] `sudo rm foo` → prefix = "sudo",不在两表 → Ask
- [ ] `find . -name "*.tmp" -delete` → prefix = "find",白名单命中(silent,即使 -delete 副作用)

### 测试 & 检查(4 条)

- [ ] `cargo test --lib` 全过(原 398 + 新增 20 个 = ~418)
- [ ] `pnpm vitest run` 全过(原 153 + 新增 8 个 = ~161)
- [ ] `pnpm vue-tsc --noEmit` exit 0
- [ ] `pnpm build` 干净

---

## 8. Definition of Done

- 5 文件后端 + 4 文件前端 + 5 文件 spec + 2 文件 docs 改动
- 27 PR1 permission 测试重写 + 20 新增 path-based / prefix / Yolo bypass / Plan 提前拦 测试
- 8 前端 vitest 新增
- 4 spec 文件同步更新
- `docs/ARCHITECTURE.md` §2.2 ⑨ 改写
- `docs/IMPLEMENTATION.md` §4 新增 2026-06-13 re-grill ADR
- 30 条 AC 全部通过
- 2 PR(PR1 + PR2)顺序合入 main

---

## 9. Out of Scope(本任务不做)

- **shell 白名单/asklist UI 自定义**:让用户在 Settings 增删 — 留 PR3+
- **跨 session 信任同步**:每 session 独立 `session_tool_permissions` 表 — 留 future
- **path-glob `**` 递归支持**:sqlite GLOB 不支持 `**`,用户允许子目录要再次点 — 留 PR3+ 考虑自己写 matcher
- **prefix 通配符**:`match_value='cargo'` 字面匹配,不接 `cargo *` glob — 留 PR3+
- **风险等级 dashboard**:C4 接走(第二档 C4)
- **Background Mode 启用**:enum 留位置,UI 不提供(同 06-12 决策)
- **web_fetch per-domain 始终允许**:本任务 web_fetch 始终允许 = 整 tool(`match_kind='tool'`);per-domain 留 PR3+ 增 `match_kind='domain'`
- **"始终允许" 撤销 UI**:让用户在 Settings 看 + 删 session_tool_permissions 行 — 留 PR3+

---

## 10. Technical Notes

### Tier 4 path 工具判定代码骨架

```rust
// agent/permissions/mod.rs::check() Tier 4 分支
match classify_tool(tool_name) {
    ToolKind::Path => {
        let path = parse_path_arg(tool_input)?;
        let inside = ctx.session_cwd.is_within_root(&path);
        if inside {
            // 查 session_tool_permissions(match_kind='path', match_value glob 命中)
            check_path_grant(db, ctx, &path).await
                .unwrap_or(Decision::Allow) // miss → silent
        } else {
            // 查 session_tool_permissions 后 emit permission:ask
            check_path_grant(db, ctx, &path).await
                .unwrap_or_else(|| ask_for_path(...))
        }
    }
    ToolKind::Shell => {
        let prefix = cmd.split_whitespace().next().unwrap_or("");
        match shell_trust::classify_prefix(prefix) {
            ShellTrust::Allow => Decision::Allow,
            ShellTrust::Ask => ask_for_shell(...),
        }
    }
    ToolKind::WebFetch => {
        check_tool_grant(db, ctx, "web_fetch").await
            .unwrap_or_else(|| ask_for_web_fetch(...))
    }
}
// Yolo 模式下:整段 bypass,直接 Decision::Allow(除非 Tier 2 早拦)
if ctx.mode == Mode::Yolo { return Decision::Allow; }
```

### Tier 4 path-glob 匹配算法

```rust
// 用 sqlite GLOB 语法,无新依赖
// match_value='/Users/me/Documents/*' 匹配 /Users/me/Documents/notes.md
// 不匹配 /Users/me/Documents/work/notes.md (sqlite GLOB * 不跨 /)
async fn check_path_grant(db, ctx, path) -> Option<Decision::Allow> {
    let row = sqlx::query("SELECT match_value FROM session_tool_permissions
                           WHERE session_id = ? AND tool_name = ? AND match_kind = 'path'")
        .bind(&ctx.session_id)
        .bind(tool_name).fetch_all(db).await?;
    for r in row {
        let glob = r.match_value;
        if sqlite_glob_match(glob, path) { return Some(Decision::Allow); }
    }
    None
}
```

### "始终允许" 写 match_value 算法

```rust
// 3-button modal "始终允许" 触发时
match tool_kind {
    ToolKind::Path => {
        // 父目录 + /* 通配
        let parent = path.parent()?;
        let glob = format!("{}/*", parent.display());
        grant_tool_permission(db, sid, tool, "path", &glob).await?;
    }
    ToolKind::Shell => {
        // 第一个 token,字面字符串
        let prefix = cmd.split_whitespace().next()?;
        grant_tool_permission(db, sid, tool, "prefix", prefix).await?;
    }
    ToolKind::WebFetch => {
        // 整 tool
        grant_tool_permission(db, sid, "web_fetch", "tool", NULL).await?;
    }
}
```

### Mode check 提前的回归风险

旧设计中 Plan 模式用户在 PermissionModal 可以点"始终允许",新设计中 Plan 写操作根本不会弹 modal。回归点:
- 旧测试 `permissions::tests::plan_mode_write_file_asks_user` 需改写为 `plan_mode_write_file_returns_text_error_no_modal`
- 前端 `PermissionModal.test.ts` 加 Plan 写操作 → modal 不弹的断言

---

## 11. Research References

- `docs/_reviews/REVIEW-a2-b7-regrill-path-based-2026-06-13.md`(本任务交付时创建)— re-grill session 10 决策 grill 过程
- `docs/_reviews/REVIEW-a2-b7-permission-mode-plan-2026-06-13.md` — 06-12 旧 PRD audit(4/5)
- `docs/_reviews/REVIEW-tool-comparison-2026-06-12.md` — Claude Code / OpenHands 权限模型对比(本任务 path-based 模型的灵感来源)
- `.trellis/tasks/archive/2026-06/06-12-a2-b7-permission-and-mode/prd.md` — 旧 PRD(被本任务 supersede)
- `.trellis/spec/backend/tool-contract.md` — 9 关决策合约(将扩写新 Scenario)
- `.trellis/spec/backend/project-cwd-boundary.md` — boundary check(将扩写 `is_within_root`)

---

## 12. Audit Reference

本任务由 re-grill-me session(2026-06-13)输出 10 决策,无独立 audit doc(grill 过程本身就是 audit)。

---

## 13. Decision(ADR-lite)

**Context**: A2+B7 任务(06-12)落地后,实际跑起来发现 ⑨ 关 Tier 3 "总是弹窗" 在 Edit 模式下读 README 都要弹,反直觉。Tier 4 Mode check 在 Ask 之后,Plan + 写操作有"用户点始终允许白点"的坏交互。re-grill session(2026-06-13)锁定 10 个核心决策,把"risk-based 弹窗 + Mode 在 Ask 之后"重构为"path-based 弹窗 + Mode 在 Ask 之前"。

**Decision** (10 项,见 §2 表格)

**Consequences**:
- 行为差异: Edit 模式仓库内写/edit 静默,Plan 写不发 modal,Yolo 仓库外全 silent
- 实现: 1 新模块 `shell_trust.rs`,`permissions::check` 大改,~950 行净增(含测试)
- DB: 不改 schema,wire 现有 3 种 match_kind
- 前端: PermissionModal 加 path 范围行
- Spec: 5 文件同步,ARCHITECTURE §2.2 ⑨ 改写
- ADR: 落 IMPLEMENTATION.md §4
- 旧 06-12 PRD 顶部加 Superseded 标记(信息隔离,便于回溯)
- 跟 C4 审计日志(第二档)无缝对接 — 10 类 AuditKind 复用,新 path-based 决策路径产生的 audit 行 kind 命名不变

**Alternatives**(已 grill 否决):
- B/Q1: Risk-based 弹窗(每 risk 等级 → 弹 / 不弹)— Edit 模式读文件要弹,反直觉
- B/Q2: 解析 shell 命令路径 token 判定"是否在仓库内"— 试图精确,会输(pipe / env 变量 / cd 切换)
- B/Q4: Yolo 仓库外仍 ask— 跟 Yolo "no questions" 哲学矛盾
- B/Q5: 维持旧 Tier 顺序— Plan + 始终允许坏交互保留
- B/Q6: 只 wire `tool` match_kind— path 工具想信任 `~/Documents` 没辙,跟新模型脱节
- B/Q7: 递归解析 shell("sudo X" → 跳到 X)— 跟"试图精确会输"哲学冲突,`bash -c $(curl)` 解析不了
- A/Q8: 最小精确(只记 path 自身)— path 工具太严,同目录 10 个文件弹 10 次
- B/Q9: Plan 豁免 path policy(读外部文档 silent)— 跟"仓库外一律 ask"模型冲突
- C/Q10: 废弃 risk 字段— UI 改动大,跟现有 UX 偏离
