> ## ⚠️ Superseded by 06-13-a2-b7-regrill-path-based (2026-06-13)
>
> 本 PRD 是 06-12 实施时的设计档案,记录 "risk-based 弹窗 + Tier 4 Mode 顺序" 方案的落地过程。
> 2026-06-13 通过 re-grill-me session 重新审视,锁定 10 个核心决策,把方案重构为:
> - 弹窗判定: risk-based → **path-based**(仓库内 default allow,仓库外 ask)
> - Tier 顺序: Hooks → Deny → Ask → Mode → Allow → Audit → **Hooks → Deny → Mode → Path → Allow → Audit**(Mode 提前)
> - "始终允许" 粒度: 只有 `tool` 类 → **3 种 match_kind 全 wire**(tool / path-glob / prefix)
> - Yolo × 仓库外: 走 modal → **silent**(Yolo bypass)
> - shell 策略: 总是 Tier 3 → **前缀白名单 + asklist 三档**
>
> 新设计完整 PRD 参见: **[`.trellis/tasks/06-13-a2-b7-regrill-path-based/prd.md`](../../06-13-a2-b7-regrill-path-based/prd.md)**
> 实施时以新 PRD 为准;本文保留作历史档案,便于回溯 06-12 commit (`442fb3d` / `db0f762` / `3a50212` 等)。
>
> ADR 落档: [`docs/IMPLEMENTATION.md §4 — 2026-06-13 Re-grill path-based 模型`](../../../../docs/IMPLEMENTATION.md#4-决策日志)

# A2 + B7 权限系统 + 多模式(合并工作组)

## Goal

实现 ROADMAP 第二档合并工作组 **A2(⑨ 关权限基础架构)+ B7(前端 Mode 切换 UI)**。后端在 agent loop ⑨ 关接入统一的权限决策层,前端在 ChatInput 改 flex 布局加 Mode 切换器,联动 ⑨ 关行为。

**为什么这两项必须合并做**:B7(Mode UI)不是独立功能,是 A2(权限基础设施)的 UX 层(见 [ROADMAP §4.2/4.3](./docs/ROADMAP.md))。前端 Mode 切换会驱动后端 ⑧a Mode 检查 + ⑨ 权限检查的联动,拆开做会二次重构。

**审计 review**: `docs/_reviews/REVIEW-a2-b7-permission-mode-plan-2026-06-13.md` 已审计本文档(评分 4/5)。本版本采纳全部 P0/P1/P2 反馈,见各段 "audit-feedback-applied" 标注。

---

## ⚠️ ⑨ 关 5 道 Check 顺序(唯一 Source of Truth)

> **本段为唯一 source of truth**,任何其他地方描述 ⑨ 关顺序的都以本段为准。audit 报告 §1 指出的三套顺序矛盾已统一。

**Claude Code 风格(permissions.md 原文)**,应用到我们的 `agent/permissions.rs`:

```
⑨ 关评估顺序 — 后端 agent loop 在 execute_tool 之前调用 permission::check():

  Tier 1. Hooks           (pre-call 接口,留空给后期扩展)
        │ 命中 hook override? → 用 hook 决定(本期不实现,直透)
        ↓
  Tier 2. Deny rules      (硬 kill list)
        │ 命中 → 直接返回 is_error: true(不弹窗)
        │ Yolo 模式也走这步 — 静默拒绝
        │ → 到 Tier 6 记录 audit event(tool_denied),end
        ↓
  Tier 3. Ask rules       (用户确认)
        │ 查 session_tool_permissions:
        │   有"始终允许"(match_kind='tool') → 跳过弹窗,直接 Tier 6
        │   无 → emit permission:ask,等用户响应(120s 超时 → 自动 deny,is_error=true,内容"permission timed out after 120s")
        │ 收到响应:
        │   allow_once → 放行,本次不写表
        │   allow_always → 放行 + INSERT INTO session_tool_permissions
        │   deny → 同 Tier 2,is_error: true,内容"user denied"
        │   timeout → 同 deny,但内容区分("permission timed out after 120s")
        ↓
  Tier 4. Mode check      (Plan/Review 拦截)
        │ Plan/Review 模式下 tool 在黑名单(write_file/edit_file/shell) → 返回 text
        │ 不影响 read 类 tool
        ↓
  Tier 5. Allow rules     (白名单,默认全开)
        │ tool 不在白名单 → Deny(本期默认全开,后期可收缩)
        ↓
  Tier 6. Audit hook      (record)
        │ 记录 tool_allowed / tool_denied / tool_permission_ask / permission_granted
        │ 到 session_audit_events 表(C4 接走)
        ↓
  → 放行 execute_tool
```

**关键行为**:
- **Deny 优先于 Ask**(即使 Yolo):`rm -rf /` 在 Yolo 下也是静默拒绝
- **拒绝(用户主动/超时) ≠ Cancel 整轮**:拒绝只跳该 tool_use,LLM 收到 `is_error: true` 可自决;C1 cancel 才是整轮终止
- **超时跟主动 deny 在 audit log 区分**:`reason` 字段不同("user denied" vs "permission timed out after 120s")

---

## What I already know

### 现状(从代码 + ARCHITECTURE.md 确认)

- **⑨ 关现在是空架子**:`agent/chat.rs:971` 直接 `execute_tool(name, input, ...)`,**完全无 ⑨ 关权限检查**。ARCHITECTURE §2.2 §2.5 描述的 5 道检查(白名单 / 参数 schema / 路径 / 用户确认 / 危险操作)一行都没实现。
- **隐式 boundary check**:`projects::boundary::assert_within_root` 已经在 `chat.rs:279` 把 session cwd 验过,工具内部也用 `ToolContext.worktree_path` 再验一次。**这是当前唯一的"权限"防线**。
- **Mode 概念完全没落地**:ARCHITECTURE §2.2 ⑧a 描述了 Mode,但代码里没有 `Mode` enum、`Session.mode` 字段、或任何 mode 分支。
- **审计日志完全没落地**:ARCHITECTURE §2.5.8 已规划,代码里没有 audit 写入。
- **C1 cancel 已落地**(2026-06-11):`execute_tool` 用 `tokio::select!` 包了 cancel token,可作为 ⑨ 关的协同样板。
- **C3 context 压缩已落地**(2026-06-12,`5e7f948`):`agent/context.rs` 已成熟,后续 B7 在 Plan 模式下"想清楚但不做"时可以参考。
- **B5 memory 已落地**(2026-06-10/11):4 文件加载 + cache_control,后续 A2 ⑨ 关的"危险操作"判定可能要参考 user/project memory 里的自定义规则。
- **C3 token 预算已就位**:为 A2 ⑨ 关"per-tool token 上限"(例 `read_file` 一次性最多吃多少)提供基础设施。

### 已有 spec 索引(相关)

- `.trellis/spec/backend/llm-contract.md` — LLM 协议,后续 Mode 行为要在这里更新
- `.trellis/spec/backend/tool-contract.md` — tool 接口,⑨ 关检查要在这里描述
- `.trellis/spec/frontend/state-management.md` — Pinia store + stream 模式,Mode store 要融入
- `.trellis/spec/frontend/popover-pattern.md` — B7 切换 UI 可参考

### BACKLOG §4.2 已有的 Mode 设计(参考,5 档中 MVP 砍到 4 档)

| Mode       | Tool 调用?     | 用户确认?  | MVP? |
|------------|----------------|------------|------|
| Chat       | 是             | 危险动作   | ✅   |
| Plan       | 否(只看)      | 计划确认   | ✅   |
| Review     | 否(只读 tool) | —          | ✅   |
| Background | 是             | 危险动作   | ❌ MVP 移除,enum 位置留 |
| Yolo       | 是             | 无         | ✅   |

---

## Assumptions(部分已 grill,部分作废)

> audit 反馈:原 §Assumptions L42 的 5 道顺序跟新 SOT(本 PRD 顶部)冲突,**作废**,统一以顶部 SOT 为准。

1. ~~**⑨ 关检查顺序**:静态白名单 → 路径 → 参数 schema → 危险模式 → 用户确认~~ → **作废,见顶部 SOT**
2. **"危险操作"判定**:硬编码规则(路径匹配、命令前缀)+ memory 文件里 user 自定义规则(可选扩展,放后期)
3. **Yolo 默认关** + **进入 Yolo 二次确认** + **Yolo 操作进审计** + **Yolo 仍走 Deny 静默拒绝** 四件套
4. **B7 UI 入口**:ChatInput flex 布局,左侧 ModeSelect(per-session override),全局 Shift+Tab 快捷键

---

## Open Questions

1. ~~**Mode 持久化粒度**~~ → ✅ **Per-session 绑定**(`sessions.mode TEXT` nullable,跟 `model_id` 同模板)(2026-06-12)
2. ~~**⑨ 关 check 顺序与是否每条都做**~~ → ✅ **全跑 5 道**(见顶部 SOT,Yolo 也走 Deny 静默拒绝)(2026-06-12 + 2026-06-13 audit 确认)
3. ~~**用户确认 modal 形式**~~ → ✅ **3-button: 始终允许 / 仅一次 / 拒绝**(Claude Code 风格,后端新表 `session_tool_permissions` 存"始终允许"列表)(2026-06-12)
4. ~~**Yolo 安全护栏**~~ → ✅ **全上 4 件套**(硬 kill list + tracing::warn! 审计 hook + 拒 root + per-session)(2026-06-12)
5. ~~**⑨ 关拒绝跟 C1 cancel 联动**~~ → ✅ **拒绝 = 跳过该 tool_use**(回 `is_error: true` 给 LLM,LLM 自决;不触发 CancellationToken)(2026-06-12)
6. ~~**B7 UI 位置**~~ → ✅ **ChatInput flex 布局 + 左侧 ModeSelect 点击切换 + 全局 Shift+Tab 快捷键**(2026-06-12)
7. **Yolo 下 Deny 命中行为** → ✅ **静默拒绝**(`is_error: true`,不弹窗,跟 Claude Code 一致)(2026-06-13 audit + 用户拍板)
8. **Background Mode 去留** → ✅ **MVP 移除**(enum 位置留,UI 不提供)(2026-06-13 audit + 用户拍板)
9. **IPC 超时时长** → ✅ **120s 超时自动 deny**(`is_error: true`,内容"permission timed out after 120s, treat as denied"提醒 LLM 是超时不是 user 主动)(2026-06-13 audit + 用户拍板)
10. **PR 拆分** → ✅ **加 PR1.5 手动 smoke test**(2026-06-13 audit + 用户拍板)

---

## Requirements

### A2 后端(PR1 范围)

- [ ] **DB 迁移 1**:`add_session_column_if_missing(pool, "mode", "TEXT")` + backfill `UPDATE sessions SET mode = 'chat' WHERE mode IS NULL`(沿用 `model_id` 模板,`db/migrations.rs:285-294`)
- [ ] **DB 迁移 2**:`CREATE TABLE session_tool_permissions (session_id TEXT NOT NULL, tool_name TEXT NOT NULL, match_kind TEXT NOT NULL CHECK (match_kind IN ('tool','prefix','path')), match_value TEXT, granted_at TEXT NOT NULL DEFAULT (datetime('now')), PRIMARY KEY (session_id, tool_name, match_kind, match_value), FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE)`(MVP 只用 `match_kind='tool'`,`match_value=NULL`)
- [ ] **DB 迁移 3**:`CREATE TABLE session_audit_events (id INTEGER PRIMARY KEY AUTOINCREMENT, session_id TEXT NOT NULL, ts TEXT NOT NULL DEFAULT (datetime('now')), kind TEXT NOT NULL, payload_json TEXT, FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE)` + `CREATE INDEX idx_session_audit_events_session_ts ON session_audit_events(session_id, ts DESC)`
- [ ] **SQLite PRAGMA 确认**:`PRAGMA foreign_keys = ON` 在连接初始化时启用(PR1 实施时验证,见 audit §4.3)
- [ ] **`Mode` enum**(`Chat | Plan | Review | Yolo`)+ `parse_mode` / `to_string` / `serde`(**Background 留 enum 位置,但 UI/MVP 不暴露**)
- [ ] **`Risk` enum**(`Low | Medium | High | Critical`)+ `risk_for_tool(tool_name: &str) -> Risk`(per-tool 静态映射)+ `risk_label_cn(risk) -> &'static str`(中文 label)
- [ ] **`Decision` enum**:`Allow | Deny { reason: String, critical: bool } | Ask { reason: String, risk: Risk }`
- [ ] **⑧a Mode 检查(三重防御 — audit §2 反馈)**:
  - **a. Per-turn system prompt 注入 mode 前缀**:
    - Plan: "你处于 Plan 模式,只能分析并提出方案,不能执行写操作。"
    - Review: "你处于 Review 模式,只能做只读分析,不能修改文件。"
    - Yolo: "你处于 Yolo 模式,所有用户确认自动跳过,但硬拒绝规则仍生效。"
  - **b. Per-turn tool list 过滤**:
    - Plan/Review → 移除 `write_file` / `edit_file` / `shell`
    - Chat/Yolo → 全量 tool list
  - **c. ⑧a runtime 兜底**:Plan/Review 模式下 LLM 仍发 `tool_use`(虽然 tool list 过滤了,可能漏掉) → 返回 text 错误
- [ ] **⑨ 关 Permission 决策层**:新模块 `agent/permissions.rs`,实现顶部 SOT 5 道 check + Audit
  - 入口:`pub async fn check(tool_name: &str, tool_input: &Value, ctx: &PermissionContext) -> Decision`
  - `PermissionContext { session_id, mode, session_active_request }`(从 chat.rs 注入)
  - Tier 2 实现:`agent/permissions/dangerous.rs` 硬 kill list 模块
  - Tier 3 实现:`HashMap<rid, oneshot::Sender<Response>>` + `tokio::time::timeout(Duration::from_secs(120), receiver)`,超时 → `Response::Deny` + reason
  - Tier 6 实现:每个决策路径 emit `audit_event` 写 `session_audit_events`
- [ ] **硬 kill list**(`agent/permissions/dangerous.rs`):
  - 命令前缀 denylist:`rm -rf /` / `rm -rf /*` / `mkfs` / `dd if=` / `:(){:|:&};:`(fork bomb)/ `> /dev/sda` / `chmod -R 777 /` / `git push --force` / `git push -f`(主分支)/ `curl ... | bash` / `wget ... | bash`
  - 实现:正则 + 命令前缀匹配,纯函数 `is_kill_listed(tool_name, input) -> Option<String>` 返回触发原因
- [ ] **`permission:ask` IPC**:`emit("permission:ask", { rid, tool_name, tool_input, risk, reason? })` + 前端回 `invoke("permission_response", { rid, decision })`
- [ ] **`permission_response` Tauri command**(`commands/permissions.rs`):收到 `decision` → 查 `HashMap<rid, Sender>` → `send(decision).ok()`,remove rid
- [ ] **Yolo 4 件套**:
  - 硬 kill list(同 Tier 2 — **Yolo 也走,静默拒绝,audit 记 `tool_denied_yolo`**)— audit §1 确认
  - `tracing::warn!` + 写 `session_audit_events(kind='tool_allowed_yolo')` 双轨
  - root check:`unsafe { libc::geteuid() } == 0` + `#[cfg(target_family = "unix")]`,Windows 跳过(用 `winapi` 或 `windows-sys` 的 `IsUserAnAdmin`)— audit §3.3 建议不用 nix crate
  - per-session 绑定(已通过 `sessions.mode` 实现)
- [ ] **Yolo 二次确认**:后端 `set_session_mode('yolo')` 时检查 root → 拒则返回 `Err("Cannot enable Yolo as root")`;前端 `YoloConfirmModal` 弹"我已知风险 / 取消",通过后才发 IPC
- [ ] **`update_session_mode` DB fn**(沿用 `update_session_model_id` 模板,`db/sessions.rs:304-329`)
- [ ] **`grant_tool_permission` DB fn**:INSERT INTO session_tool_permissions,UPSERT 语义
- [ ] **`record_audit_event` DB fn**:INSERT INTO session_audit_events
- [ ] **Tauri command `set_session_mode`**(`commands/sessions.rs`)
- [ ] **`agent/chat.rs` 接入**:
  - L70-71 `execute_tool` 调用前 → 调 `permission::check(...)`
  - L71-78 Tier 3 Ask 路径:在循环里加 `tokio::select!` 处理 `permission_response` IPC 的 future(具体实现 `wait_for_response(rid)`)
  - L38 tool list 构造(在 `build_instructions_blocks` 之后)按 mode 过滤
  - L41 system_prompt 构造(在 `build_system_prompt` 之后)加 mode 前缀
- [ ] **审计 `kind` 枚举**(audit §3.4 补全):
  ```rust
  pub enum AuditKind {
    ToolDenied,          // ⑨ 关拒绝
    ToolAllowed,         // ⑨ 关放行
    ToolPermissionAsk,   // 弹窗询问
    PermissionGranted,   // 用户选"始终允许"
    ModeChanged,         // Mode 切换
    YoloEntered,         // 进入 Yolo
    YoloExited,          // 退出 Yolo
    ToolDeniedYolo,      // Yolo 模式下仍被 Deny 硬墙拦截
    PermissionTimeout,   // 120s 超时
    RequestCancelled,    // C1 cancel
  }
  ```

### A2 后端(PR1.5 范围 — 手动 smoke test)

- [ ] 手动跑 3 个 case,在 `docs/_reviews/REVIEW-a2-b7-permission-mode-plan-2026-06-13.md §5` 列:
  1. Mode 切换 → DB 持久化 → 重启后 mode 保留
  2. Plan 模式下 LLM 确实不能执行 write_file
  3. Chat 模式下 shell 执行正常
- 验证完 PR1 才接 PR2

### B7 前端(PR2 范围)

- [ ] **`ModeSelect.vue`**(新):popover 模式,跟 `ModelSelect.vue` 风格一致;**4 个** mode 选项(Chat/Plan/Review/Yolo,**不显示 Background**)
- [ ] **`ChatInput.vue`** 改 flex 布局:左侧 `<ModeSelect>`,右侧保留输入框 + 发送按钮(audit §4.2 提示其他元素可能挤,需布局 review)
- [ ] **`useKeyboard` 模块**(新):全局快捷键注册,`Shift+Tab` 触发当前 session mode 循环切换(Chat → Plan → Review → Yolo → Chat),捕获阶段 preventDefault(audit §4.4)
- [ ] **`YoloConfirmModal.vue`**(新):两键 modal(我已知风险 / 取消),仅在切换 Yolo 时弹
- [ ] **`SessionSummary` TS type 加 `mode: 'chat' | 'plan' | 'review' | 'yolo' | null`**
- [ ] **`:disabled="isStreaming"` 锁定**:所有 mode 切换 UI 在 streaming 期间不可点
- [ ] **`set_session_mode` IPC 调用**:前端 store action → 后端 DB 写

### B7 前端(PR3 范围)

- [ ] **`PermissionModal.vue`**(新):3-button 弹窗(始终允许 / 仅一次 / 拒绝)+ shield icon 容器 + 命令预览块(terminal icon + copy icon)+ "工具类别: <tool> · 风险等级: <risk>" 标签
- [ ] **`usePermissionsStore` Pinia store**:管理 `pendingPermission: PermissionAsk | null` + `response()` 方法
- [ ] **IPC 联通**:`permission:ask` event → store.setPending;`permission_response` invoke → 唤醒后端 future
- [ ] **risk 标签统一中文**(audit §6.2 反馈):Title 文本 + risk 标签都只用中文(low→低/medium→中/high→高/critical→极高)
- [ ] **critical risk 时 Enter 默认 focus 改"拒绝"**(audit §6.2 反馈)
- [ ] **toast z-index 10000**(在 modal 之上,audit §6.2)
- [ ] **`agent/permissions.rs` 单元测试**(cargo test):
  - Tier 2 硬 kill list 命中 → 静默 deny
  - Tier 2 Yolo 模式命中 → 静默 deny
  - Tier 3 有"始终允许"记录 → 跳过弹窗
  - Tier 3 120s 超时 → 自动 deny + reason "permission timed out"
  - Tier 4 Plan 模式 write_file → text error
  - Tier 4 Review 模式 read_file → 放行
  - Tier 4 Chat 模式 shell → 放行
  - Tier 5 白名单外 tool → deny
  - Audit event 写入
  - 根用户设 Yolo → 拒

### Spec 同步(PR3 范围)

- [ ] `.trellis/spec/backend/llm-contract.md`:加 Mode 行为 + ⑨ 关 5 道 check 描述
- [ ] `.trellis/spec/backend/tool-contract.md`:加 ⑨ 关决策合约(5 道顺序 + Deny 优先)
- [ ] `.trellis/spec/frontend/state-management.md`:加 `mode` 字段 + `usePermissionsStore` + PermissionAsk IPC
- [ ] `.trellis/spec/frontend/popover-pattern.md`:加 `ModeSelect.vue` 案例
- [ ] `docs/ARCHITECTURE.md §2.2 ⑧a + ⑨` + `§2.5.8 审计`:从"规划"改成"已实施"描述

---

## PermissionModal UX Spec(PR3 子 spec)

> 完整 design 沉淀,2026-06-12 拍板,2026-06-13 audit 反馈已合并。详细出处见 `research/permission-modal-ux.md`。

### 组件路径

`app/src/components/chat/PermissionModal.vue`(单文件,~180 行 TS + 100 行 CSS)

### 触发流程

1. 后端 `agent/permissions.rs` 决策返回 `Decision::Ask { tool_name, tool_input, risk }`
2. 后端 emit `permission:ask` event with `{ rid, tool_name, tool_input, risk }`
3. 前端 `usePermissionsStore.pendingPermission` 写入该 payload
4. `App.vue` 或 `ChatWindow.vue` 顶层 mount `<PermissionModal>`,当 `pendingPermission !== null` 时 `v-if` mount
5. 用户点 3 button 之一 → store action `respond(rid, "allow_once" | "allow_always" | "deny")` → invoke `permission_response` IPC
6. 后端收到 IPC → 唤醒 `wait_for_response(rid)` future → 按串行逻辑继续

### 视觉规范

> 视觉风格参考 `docs/spikes/ui-B-2.png`(原 003-ui-reference-prompts.md 的实际产出)。截图里 2-button 的"拒绝/允许"是早期 prompt 的设计,**最终 UX 以本 spec 3-button 为准**(用户已选);但**视觉细节**吸收进本 spec。

- **位置**:Center modal,`position: fixed; top: 50%; left: 50%; transform: translate(-50%, -50%)`
- **尺寸**:`width: min(560px, 90vw); max-height: 80vh`
- **遮罩**:`background: color-mix(in srgb, var(--color-bg-app) 70%, transparent); backdrop-filter: blur(4px); z-index: 9998`
- **内容卡片**:`background: var(--color-bg-surface); border: 1px solid var(--color-bg-border); border-radius: 8px; box-shadow: 0 8px 24px rgba(0,0,0,0.5); z-index: 9999`
- **Critical 变体**:`border-left: 3px solid var(--color-tool-error)`(仅 `risk === "critical"` 时)— audit §4.1 提示跟 design-tokens.md "Border width is always 1px" 冲突,本任务特殊例外
- **动画**:`[data-state="open"] { animation: modal-enter 150ms ease-out }`(沿用 popover-pattern.md "fade + scale 0.96 → 1")

### 头部(图 ui-B-2 风格)

- **Icon 容器**:`56x56px`,圆角 `12px`,bg 用 risk 对应 tint(参考 `bg = color-mix(in srgb, <risk-color> 12%, transparent)`)
  - Icon 用 lucide `shield`(low) / `shield-check`(medium/high) / `shield-x`(critical),size 28px,color 用 risk 满色
- **Title**:`16px semibold Sans`,`color: var(--color-text-primary)`,icon 右侧
  - 文本按 risk 变化(全中文,audit §6.2 反馈):low → "需要权限:只读操作",medium → "需要权限:写文件",high → "需要权限:执行 Shell",critical → "此命令匹配硬拒绝规则,默认拒绝"
- **关闭 X**:右上角,`color: var(--color-text-muted)`,hover → `var(--color-text-primary)`,click = "拒绝"

### 副标题 + 命令预览

- **副标题**:`14px Sans`,`color: var(--color-text-secondary)`,文本:`"Agent 想在项目 <project-name> 下执行以下操作:"`
- **命令预览块**(rounded 8px,**带 terminal icon + copy icon**):
  - 容器:`<div class="permission-modal__preview">`
  - 左 terminal icon(lucide `terminal`,16px,`var(--color-text-muted)`)
  - 中间 `<pre>` 块:mono 13px,`tab-size: 2`,`max-height: 240px; overflow: auto`,`white-space: pre-wrap; word-break: break-word`
  - 右 copy icon(lucide `copy`,16px,hover → check icon,click → `navigator.clipboard.writeText(JSON.stringify(toolInput, null, 2))`,2s toast "已复制",**toast z-index 10000**,audit §6.2 反馈)
  - 背景:`var(--color-bg-app)`(比 modal 卡片再深一档)
  - 边框:`1px solid var(--color-bg-border); border-radius: 8px`
  - 内边距:`12px 14px`
- **❌ 移除** "本次会话记住此选择" checkbox — ui-B-2 早期设计有此元素,但**我们已有"始终允许"按钮**(功能等价 + 更显式),不需要重复(用户 2026-06-12 反馈确认移除)

### 风险等级标签(图 ui-B-2 风格,全中文)

紧跟命令预览块下方,**左对齐 14px**,文本:

```
工具类别: <tool_name>  ·  风险等级: <risk-label-cn>
```

- **Risk 颜色点**:`●` 8px 圆,`color = <risk-color>`,文字前缀
- **Risk label**(统一中文,audit §6.2 反馈):
  - `low` → 灰色点 + "低"
  - `medium` → emerald 点 + "中"
  - `high` → amber 点 + "高"
  - `critical` → red 点 + "极高"
- Risk label 颜色 token:沿用 `design-tokens.md` `--color-tool-*` 家族(PR1 实施时验证完整性,audit §7 反馈)

### 按钮布局

- 3 按钮,等宽 33%,底栏水平排列,间距 8px
- 顺序(从左到右):**"拒绝" → "仅一次" → "始终允许"**
- 样式:
  - 拒绝:`background: transparent; border: 1px solid var(--color-bg-border); color: var(--color-text-primary)`
  - 仅一次:同拒绝
  - 始终允许:`background: var(--color-accent); color: #fff`(主操作,最强强调)
- Padding:`8px 16px`,radius 6px,Sans 13px
- **键盘**(audit §6.2 反馈 — critical Enter 改"拒绝"):
  - `Enter` → "仅一次"(默认 focus,risk=critical 时改 "拒绝" — 跟视觉暗示一致)
  - `Esc` → "拒绝"(reka-ui `DialogContent` 默认行为,路由到 `store.respond("deny")`)
  - `A` / `D` 快捷键 **MVP 不做**(power-user 后期可加)
- **Focus**:`v-if` mount 后 `setTimeout(0)` 把 focus 移到默认按钮(risk=critical 时 "拒绝",其他 "仅一次")

### "始终允许" 持久化(后端)

- DB 表:`session_tool_permissions(session_id, tool_name, match_kind, match_value, granted_at)`
- `match_kind` 枚举:`tool` / `prefix` / `path`(schema 留全,MVP 只用 `tool`)
- `match_value` 语义:
  - `tool`:NULL,匹配整 tool class(MVP 唯一支持的)
  - `prefix`:shell 命令前缀(例 `npm test`),匹配 `input.command.starts_with(value)`(后期用)
  - `path`:glob 表达式(例 `~/Documents/*`),匹配 `input.path` 用 sqlite GLOB(后期用)
- 写入时机:用户点 "始终允许" → store action → `invoke("grant_tool_permission", { sessionId, toolName, matchKind: "tool", matchValue: null })` → 后端 `INSERT INTO session_tool_permissions`
- 删除 session 时 cascade 清(`ON DELETE CASCADE`,SQLite `PRAGMA foreign_keys = ON` 必启用)

### "仅一次" 语义

- **不写任何存储**(不修改 `session_tool_permissions`)
- **session 重启后失效**(本身就是 in-memory 决策)
- **下次同 tool 重新 ask**
- **Nudge UX MVP 不做**(连续 N 次选仅一次 → 提示用户切始终允许;留 PR4+)

### Cancel / Esc / X / 遮罩点击

- **Esc / X / 遮罩点击 = Deny**(等同 Claude Desktop / Cursor / Continue.dev 事实标准)
- reka-ui `DialogContent` 内置 emit `EscapeKeyDown` / `PointerDownOutside` / `Close` 事件,接 `@update:open` handler 调 `respond(rid, "deny")`
- **不** 触发 CancellationToken(deny ≠ cancel 整轮)
- 用户在 modal 等待时按 **Stop 按钮**(C1 已有)→ CancellationToken 触发 → 整 turn 终止,跟 deny 在 audit log 区分

### IPC 异常路径(audit §3.2 补全)

| 异常场景 | 处理 |
|---|---|
| 用户从不响应(>120s) | `tokio::time::timeout` 触发 → 自动 deny + `is_error: true, content: "permission timed out after 120s, treat as denied"`(提醒 LLM 是超时不是 user 主动) |
| 重复 `permission_response` | 后端 `HashMap<rid, Sender>`:`send().ok()` 失败(rid 不存在)=> no-op,日志 warn |
| Session 在等待时被删除 | 给该 session 的所有 pending permission future 发 cancel(复用 C1 CancellationToken 模式,`SessionManager::on_delete` 时遍历清理) |
| `rid` 过期/无效 | 后端校验 rid 存在性,无效 → 日志 warn + no-op |

### Multi-tool_use 批处理

- **MVP 严格串行**:后端 `for tool_use in turn.tool_uses { ... }` 循环,每个 tool_use 独立 ⑨ 关 + 独立 emit `permission:ask`
- **前端单 modal**:`usePermissionsStore.pendingPermission` 一次只持一个,新 ask 替换旧的:`<PermissionModal :key="pendingPermission.rid" />` 触发重新 mount
- **Deny mid-batch**:不影响后续 tool_use,后续仍按 ⑨ 关决定(可能 auto-allow,可能再 ask)
- **Batch modal UI MVP 不做**(留 PR4+,需要时改成 `tool_uses: ToolUse[]` 数组 + 独立 button group)

### usePermissionsStore 状态机

```ts
// app/src/stores/permissions.ts
import { defineStore } from "pinia";
import { ref } from "vue";
import { invoke } from "@tauri-apps/api/core";

export type Risk = "low" | "medium" | "high" | "critical";

export interface PermissionAsk {
  rid: string;
  toolName: string;
  toolInput: Record<string, unknown>;
  risk: Risk;
  // optional:reason from server (e.g. "matches denylist: rm -rf /")
  reason?: string;
}

export const usePermissionsStore = defineStore("permissions", () => {
  const pendingPermission = ref<PermissionAsk | null>(null);

  function setPending(ask: PermissionAsk) {
    pendingPermission.value = ask;
  }

  function clearPending() {
    pendingPermission.value = null;
  }

  async function respond(
    rid: string,
    decision: "allow_once" | "allow_always" | "deny",
  ) {
    await invoke("permission_response", { rid, decision });
    // don't clear pending here — server will emit a new permission:ask
    // (or stream continues) that triggers setPending(null) upstream
  }

  return { pendingPermission, setPending, clearPending, respond };
});
```

### IPC 协议

- **Server → Client**:`emit("permission:ask", { rid, toolName, toolInput, risk, reason? })`
- **Client → Server**:`invoke("permission_response", { rid, decision: "allow_once" | "allow_always" | "deny" })`
- **Tauri command**:`permission_response(rid: String, decision: String) -> Result<(), String>`(注册在 `commands/permissions.rs`)

### PermissionModal Acceptance Criteria(14 条追加)

- [ ] Modal 在 `width: min(560px, 90vw)` 居中,带 4px backdrop-blur
- [ ] 头部 56x56 shield icon 容器,risk tint 背景,Title 16px semibold 按 risk 动态变化(全中文)
- [ ] Critical risk 时左 border 3px red,icon 用 `shield-x`,Title 显 "此命令匹配硬拒绝规则,默认拒绝"
- [ ] `toolInput` 用 `JSON.stringify(_, null, 2)` 渲染在 `<pre>` 块,`max-height: 240px; overflow: auto`
- [ ] 命令预览块带 terminal icon(左)+ copy icon(右,click 复制 + 2s toast,**toast z-index 10000**)
- [ ] 副标签 "工具类别: <tool> · 风险等级: <risk-label-cn>" 带 risk 颜色点(low 灰/medium emerald/high amber/critical red)+ 中文字(低/中/高/极高)
- [ ] 3 按钮等宽 33%,顺序 "拒绝 / 仅一次 / 始终允许",始终允许用 `--color-accent` 背景
- [ ] Mount 时 focus 自动到默认按钮(普通 risk→"仅一次",critical→"拒绝"),Enter 触发
- [ ] Esc / X / 遮罩点击 = "拒绝"(`is_error: true` 回 LLM,不触发 CancellationToken)
- [ ] 用户选 "始终允许" → `INSERT INTO session_tool_permissions (match_kind='tool', match_value=NULL)` 持久化,该 session 同 tool 后续不再弹
- [ ] 用户选 "仅一次" → 不写表,下次同 tool 重新 ask
- [ ] 用户选 "拒绝" → 后续 tool_use 继续按 ⑨ 关决定(串行,不被 deny 影响)
- [ ] Modal 等待时按 Stop(C1)→ CancellationToken 触发,整 turn 终止,跟 deny 在 audit log 区分
- [ ] **不渲染** "本次会话记住此选择" checkbox(参考 ui-B-2.png 早期设计,但功能跟"始终允许"按钮重复,已移除)
- [ ] 同一 turn 多个 tool_use → 串行弹,用户每个独立答
- [ ] reka-ui 2.9.9 `DialogContent` portal 子元素样式必须用 `:deep()`,遵循 reka-ui-usage.md gotcha
- [ ] **120s 超时** → 自动 deny,`is_error: true, content: "permission timed out after 120s, treat as denied"` 提醒 LLM 是超时

---

## Acceptance Criteria 总览(30 条)

### 后端 10 条

- [ ] DB 迁移跑过后,legacy sessions 自动 backfill `mode = 'chat'`
- [ ] Plan 模式 session,LLM 调 `write_file` → 后端返回 text "I cannot execute in plan mode" 给 LLM(不执行)— **且 write_file 不在 LLM 可见 tool list 里**
- [ ] Review 模式 session,LLM 调 `read_file` 允许,调 `write_file` 被拒(回 `is_error: true` 错误)
- [ ] 给某 session 选 Chat,LLM 调 `shell` 跑 `rm -rf /tmp/foo` → 命中硬 kill list,回 `is_error: true` 拒
- [ ] Yolo 模式 session,LLM 调 `shell` 跑 `rm -rf /tmp/foo` → **仍然被拒**(Deny 优先于 Yolo,静默拒绝,不弹窗)
- [ ] Yolo 模式 session,LLM 调 `shell` 跑 `echo hello` → 执行成功(无确认弹窗)
- [ ] 首次用某 tool(无 "始终允许" 记录),emit `permission:ask` 到前端
- [ ] 前端回 "始终允许" 后,该 session 后续该 tool 不再弹窗(从 `session_tool_permissions` 读)
- [ ] 前端回 "仅一次" → 本次放行,下次再弹
- [ ] 前端回 "拒绝" → tool 跳过,`is_error: true` 错误回传 LLM
- [ ] 检测到 root 启动 + 设 Yolo → 拒(报错"Cannot enable Yolo as root")
- [ ] 120s 未响应 → 自动 deny,LLM 收到 "permission timed out after 120s, treat as denied"

### 前端 4 条

- [ ] ChatInput 改 flex 布局,左侧显示当前 mode badge(4 档:Chat/Plan/Review/Yolo,**不显示 Background**)
- [ ] 点 mode badge 弹 popover,**4 个**选项可见
- [ ] 选 Yolo 弹 `YoloConfirmModal`,确认后 mode 切换
- [ ] Shift+Tab 切换 mode(streaming 时不响应,UI 灰显)

### 持久化 2 条

- [ ] session 重启后 Mode 持久化
- [ ] 删除 session 时 cascade 清 `session_tool_permissions` + `session_audit_events`

### PermissionModal 14 条(见上)

---

## Definition of Done

- 后端:⑨ 关 5 道 + ⑧a 三重防御 + Yolo 4 件套 + 3 个 DB 迁移 + 审计 10 类事件
- 前端:ModeSelect + ChatInput 改 flex + PermissionModal + YoloConfirmModal + useKeyboard 模块
- 测试:每条 AC 至少 1 个端到端 case(cargo test + 手动 smoke)
- 文档:4 个 spec 文件 + ARCHITECTURE.md §2.2/§2.5.8 同步
- IPC 异常路径完整:超时 120s / rid 去重 / session 删除清理 / 无效 rid
- PR1.5 手动 smoke test 3 case 通过

---

## Out of Scope(本任务不做)

- **C4 审计日志的完整 UI 查询**:事件表已写,UI 不做(C4 接走)
- **Skill / use_skill 集成**:⑨ 关可对 use_skill 走通用路径,Skill 系统本身不进 A2
- **use_memory / use_ui**:同上
- **跨 session Mode 同步 / 云端推送**:不在这
- **Mode 切换的可视化 DAG 编辑器**(B8 第四档,本任务不做)
- **Slash command `/mode`**:留接口,基础设施留给 B3(/command 面板)
- **Settings modal LLM Tab 入口**:B7 主入口 ChatInput 已够;Settings 入口留 Phase 2
- **Background Mode**:MVP 移除(enum 位置留,UI 不提供,后期可加)
- **PermissionModal "始终允许" 的 prefix/path 粒度**:schema 留 3 种 match_kind,MVP 只用 tool
- **PermissionModal batch UI / nudge UX**:留 PR4+

---

## Technical Notes

- `execute_tool` 在 `tools/mod.rs:95`,外面已有 `tokio::select!` 包 C1 cancel,在它之前插 ⑨ 关 dispatch 即可
- ⑨ 关 5 道全跑:先静态分析(白名单/schema/路径)再动态询问(deny 优先),跟 Claude Code 一致
- **root check** 用 `unsafe { libc::geteuid() } == 0` + `#[cfg(target_family = "unix")]`(audit §3.3 反馈 — 不用 nix crate,避免新增依赖)
- Windows root check:用 `windows-sys` 或 `winapi` 的 `IsUserAnAdmin` + `#[cfg(target_os = "windows")]`
- B7 UI 跟 `ModelSelect.vue` 同 popover 模板,只是事件 + IPC 不同
- **Shift+Tab** 默认浏览器行为是"反向 focus",需要在 capture phase preventDefault(audit §4.4)
- Permission modal 需要"命令预览块"用 `<pre>` 渲染 tool input 的 JSON.stringify
- 硬 kill list 用正则 + 命令前缀匹配(例:`^rm\s+(-[a-zA-Z]*f[a-zA-Z]*\s+)*/\s*$`)
- **SQLite `PRAGMA foreign_keys = ON`** 必须在连接初始化时启用(audit §4.3);检查 `db/mod.rs` 是否已启用,没有则 PR1 加
- ⑨ 关决策使用 `tokio::sync::oneshot::channel` + `tokio::time::timeout` 处理等待 + 超时
- 删除 session 时清理 pending permission futures(复用 C1 cancel 模式)

---

## Research References

- [`research/agent-permission-best-practice.md`](research/agent-permission-best-practice.md) — Claude Code `deny > ask > mode > allow` 顺序 + OpenHands 3-button 模式
- [`research/mode-state-machine.md`](research/mode-state-machine.md) — Per-session 绑定 + ⑧a runtime intercept + ChatInput quick switcher
- [`research/yolo-safety-design.md`](research/yolo-safety-design.md) — 4 件套全上 + 两键 modal UX
- [`research/permission-modal-ux.md`](research/permission-modal-ux.md) — PermissionModal 完整 spec(13 条 AC);reka-ui 2.9.9 portal gotcha 警告

## Audit Reference

- [`docs/_reviews/REVIEW-a2-b7-permission-mode-plan-2026-06-13.md`](../../docs/_reviews/REVIEW-a2-b7-permission-mode-plan-2026-06-13.md) — pre-implementation review,4/5 评分,P0/P1/P2/P3 全部已合并到本 PRD

---

## Decision(ADR-lite)

**Context**: ⑨ 关权限决策 + Mode 状态机是 V2 第二档 7 项中最核心的设计命题,有 10 个相互依赖的关键决策点(持久化粒度 / 5-check 顺序 / 用户确认 UX / Yolo 护栏 / 拒绝语义 / B7 UI 位置 / risk 粒度 / 始终允许粒度 / Yolo 下 deny 行为 / Background 去留 / IPC 超时)。每点都有 3-4 个候选方案,影响 DB schema、⑨ 关代码结构、前端组件拓扑、跟 C1 cancel / C4 audit 的联动。2026-06-13 audit review 进一步识别出 P0 顺序不一致 + P1 5 项遗漏,本决策记录已含 audit 反馈。

**Decision**:

1. **持久化粒度 = Per-session 绑定**(`sessions.mode TEXT` nullable,跟 `model_id` 同模板)
2. **⑨ 关 5 道全跑 + Deny 优先于 Mode**(Hooks → Deny → Ask → Mode → Allow → Audit,见顶部 SOT)
3. **3-button 确认 modal**(始终允许 / 仅一次 / 拒绝) + 14 条 AC
4. **Yolo 4 件套全上**(硬 kill list + 审计 + 拒 root + per-session) + **Deny 静默拒绝**(Yolo 下也走)
5. **拒绝 = 跳过该 tool_use**(回 `is_error: true` 给 LLM,LLM 自决;不触发 CancellationToken)
6. **B7 UI = ChatInput flex 布局 + 左侧 ModeSelect + 全局 Shift+Tab 快捷键**
7. **4 档风险等级**(low/medium/high/critical,per-tool 静态)
8. **"始终允许" 记忆粒度 = Schema 留 3 种 match_kind,MVP 只用 tool**
9. **Background Mode = MVP 移除**(enum 位置留,UI 不提供)
10. **IPC 超时 = 120s 自动 deny** + 提醒 LLM 是超时不是 user 主动
11. **PR 拆分 = 3 PR + PR1.5 手动 smoke test**

**Consequences**:

- 后端新增 `agent/permissions.rs` 模块(~250 行) + `agent/permissions/dangerous.rs` 硬 kill list(~80 行)
- 后端 3 个 DB 迁移:`sessions.mode` / `session_tool_permissions` / `session_audit_events`
- 前端新增 `ModeSelect.vue` + `PermissionModal.vue` + `YoloConfirmModal.vue` + `useKeyboard` 模块
- 前端 `ChatInput.vue` 改 flex 布局
- 4 个 spec 文件同步更新
- ARCHITECTURE.md §2.2 ⑧a + ⑨ + §2.5.8 审计 描述升级
- 为 C4 审计日志(第二档)留 hook 接口,本任务只写表不实现 UI
- PR1.5 手动 smoke test 验证 Mode 持久化 + Plan 拦截 + Chat 正常 3 case

---

## Implementation Plan(4 个 PR,含 PR1.5 smoke test)

**PR1: 后端基础设施 + Mode 持久化 + ⑨ 关 5 道 check + ⑧a 三重防御 + Yolo 4 件套**
- 3 DB 迁移(`sessions.mode` / `session_tool_permissions` / `session_audit_events`)+ 验证 `PRAGMA foreign_keys = ON`
- `Mode` enum(4 档:Chat/Plan/Review/Yolo,Background 留 enum 位置)+ parse
- `Risk` enum(4 档) + `risk_for_tool` 映射
- `Decision` enum(Allow/Deny/Ask)
- `permissions.rs` 模块(5 道 check)+ `permissions/dangerous.rs` 硬 kill list
- root check:`unsafe libc::geteuid() == 0`(`#[cfg(target_family = "unix")]`)
- `update_session_mode` + `grant_tool_permission` + `record_audit_event` DB fn
- Tauri command `set_session_mode` + `permission_response` + `grant_tool_permission`
- `agent/chat.rs` 接入 ⑨ 关 dispatch + ⑧a system prompt 前缀 + tool list 过滤
- 审计 10 类 `AuditKind` 实现
- 单元测试 8-10 个(cargo test)

**PR1.5: 手动 smoke test**(无 git commit,只验证)
- Case 1: Mode 切换 → DB 持久化 → 重启后 mode 保留
- Case 2: Plan 模式下 LLM 确实不能执行 write_file(8a 三重防御验证)
- Case 3: Chat 模式下 shell 执行正常(9 关放行)
- 验证完接 PR2

**PR2: 前端 ModeSelect + ChatInput 改 flex + useKeyboard 快捷键 + Yolo 二次确认 modal**
- `ModeSelect.vue` + popover pattern(**4 选项,不显示 Background**)
- `ChatInput.vue` 改 flex
- `useKeyboard` 模块 + Shift+Tab(capture phase preventDefault)
- `YoloConfirmModal.vue`
- `SessionSummary.mode` 类型 + IPC wire
- vitest 4-6 个

**PR3: 前端 PermissionModal + ⑨ ↔ permission:ask IPC 联通 + 端到端测试 + spec 同步**
- `PermissionModal.vue` + 3-button + shield icon 容器 + 命令预览块带 copy + 中文 risk label
- `usePermissionsStore`
- `permission:ask` / `permission_response` IPC 联通
- critical risk 时 Enter 默认 focus 改"拒绝"
- toast z-index 10000
- 120s 超时对接后端
- 端到端测试(vitest + 手动 smoke)
- 4 个 spec 文件同步
- ARCHITECTURE.md 升级

**PR4(可选,留 Phase 2)**:Settings modal LLM Tab Mode 入口 + Slash command `/mode` + C4 UI 查询 + Background Mode
