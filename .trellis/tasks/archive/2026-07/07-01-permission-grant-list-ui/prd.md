# Permission grant list management UI

## Goal

为「主对话始终允许」提供一个放行清单管理 UI：用户能看到当前对哪些 tool / path glob / shell prefix 做了「始终允许」放行，并能撤销其中任意一条。

**痛点**：用户点了「始终允许」后没有任何前端入口查看或反悔，目前只能直连 SQLite 或删 session（ON DELETE CASCADE）才能清除放行。

## Background / 现状（代码已确认）

- 放行持久化在 SQLite `session_tool_permissions`（`db/migrations.rs:433`），PK = `(session_id, tool_name, match_kind, match_value)`，`ON DELETE CASCADE` 跟 session。**session 级别**，非项目级。
- 实际写入的 match_kind 分 tool 类型（`agent/permissions/check.rs:690` `match_value_for_allow_always`）：
  - Path 类（read/write/edit_file…）→ `path`，value = glob 如 `parent/*`
  - Shell → `prefix`，value = 首 token 如 `git`
  - WebFetch / GitMutation / Other → `tool`，value = NULL
  - ⇒ DB 表实际**混存 tool/prefix/path 三种维度**，UI 必须带出 match_kind + match_value，否则同名 tool 多条放行无法区分。
- 后端 IPC：`grant_tool_permission` 已暴露（`commands/permissions.rs:261`）；`revoke_tool_permission`（`db/permissions.rs:158`）已写但标 `#[allow(dead_code)]` 未暴露，且当前签名按 `(session_id, tool_name)` 整 tool 删除（不分 match_kind/value，粒度粗）。**无 list IPC**。
- 前端：`stores/permissions.ts` 只管 pending ask 路由 + respond，无放行 list 数据；`settings/` 下无 PermissionsTab。
- 现成 UI 范本：`components/audit/AuditLogModal.vue` + `AuditLogItem.vue` + `stores/audit.ts`，在 `ChatPanel.vue:586` 实例化、绑当前 session、`loadForSession` on open + 手动 `refresh()`（`audit.ts:8-15` 明说 MVP 无 live push）。
- subagent worker 的「本次运行始终允许」走 per-run 内存 cache（`run_grant.rs`），**不进这张表**，不在本任务范围。

## Requirements

- **R0 范围与入口（已定）**：列表范围 = **当前 session**；入口 = ChatPanel 内独立 modal（仿 `AuditLogModal`，`ChatPanel.vue:586` 模式），触发按钮与 audit 按钮并列。
- **R1 列表展示**：展示当前 session 的已放行记录，至少含 `tool_name` + `match_kind` + `match_value`（path glob / prefix token 必须可见）+ `granted_at`。
- **R2 撤销（已定：单条）**：每行按 PK `(session_id, tool_name, match_kind, match_value)` 精确撤销，不影响同 tool 其他维度放行。后端把 `revoke_tool_permission` 从 `(session_id, tool_name)` 扩展为支持 match_kind/match_value（含 NULL match_value 的 tool kind，见 design D2）。
- **R3 后端**：新增 list IPC（`WHERE session_id = ?`）；扩展 revoke IPC 支持 PK 维度。
- **R4 刷新**：沿用 audit 模式（open 时 load + 手动 refresh，无 live push）。
- **R5 生效语义**：check 路径每次查 DB（`check.rs:503/630/641`，无 main-session 缓存），撤销 DELETE 后下次 tool_use 即重新弹审批，无需刷后端缓存。

## Acceptance Criteria

- [ ] 打开放行管理 modal，展示当前 session 所有 `session_tool_permissions` 记录（含 tool / prefix / path 三种 match_kind）。
- [ ] 每行可见 `tool_name` + match_kind 标识 + `match_value`（path glob / prefix token；tool kind 的 NULL 显示"—"）+ `granted_at`。
- [ ] 点单行撤销 → 按 PK 精确 DELETE，该行消失，且不影响同 tool 其他维度放行（如 read_file 多条 path glob 只删命中那条）。
- [ ] 撤销立即生效：下次该 tool 触发 check 时重新弹审批 modal（check 每次查 DB）。
- [ ] 空列表显示空状态文案；提供手动 refresh 按钮。
- [ ] `cargo test`（list + revoke，含 NULL match_value 边界）+ `vitest`（store revoke + 组件渲染/空状态）通过。

## Out of Scope

- 手动新增放行（用户已确认不做）
- 实时推送（live push）刷新
- subagent worker 的 per-run grant cache 管理
- prefix/path 维度的**新增写入**逻辑变更（沿用现有 grant 路径，本任务只读 + 撤销）
