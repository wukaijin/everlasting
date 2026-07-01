# Design — Permission grant list management UI

## 范围回顾

- 列表范围 = **当前 session**；入口 = ChatPanel 内独立 modal（仿 `AuditLogModal`）。
- 撤销粒度 = **单条**，按 PK `(session_id, tool_name, match_kind, match_value)`。
- 不做手动新增、不做 live push、不碰 subagent per-run cache。

## 架构与边界

完全沿用 audit list 的三层结构（IPC → Pinia store → reka-ui Dialog），新增独立模块，不动 grant 写入路径与 check 路径。

```
ChatPanel.vue
  ├─ <PermissionGrantsModal v-model:open="grantsModalOpen" />   ← 仿 ChatPanel.vue:586 audit 实例化
  └─ 触发按钮（与 audit 按钮并列）
        │ open
        ▼
stores/permissionGrants.ts  (loadForSession / revoke / refresh)
        │ invoke
        ▼
commands::list_session_tool_permissions(state, sessionId) -> Vec<GrantRow>
commands::revoke_tool_permission(state, sessionId, toolName, matchKind, matchValue) -> ()
        │
        ▼
db::permissions
  ├─ list_tool_permissions(pool, session_id)        ← 新增
  └─ revoke_tool_permission(pool, sid, tool, kind, value)  ← 扩签名（原 dead_code，零调用方）
        │
        ▼
session_tool_permissions  (SELECT / DELETE WHERE session_id = ?)
```

## 数据契约

### GrantRow（IPC wire shape，camelCase）

后端 `serde(rename_all = "camelCase")`，前端对应 TS interface：

```ts
interface PermissionGrantRow {
  sessionId: string;
  toolName: string;
  matchKind: "tool" | "prefix" | "path";
  matchValue: string | null;   // tool kind = null；path = glob 如 "src/*"；prefix = token 如 "git"
  grantedAt: string;           // SQLite datetime，second 精度
}
```

排序（与 audit 一致，second 精度 tie 用稳定排序）：`granted_at DESC`，secondary `rowid DESC`。

### 新增/改造 IPC

| 命令 | 入参 | 返回 | 后端函数 |
|---|---|---|---|
| `list_session_tool_permissions` | `{ sessionId }` | `Vec<GrantRow>` | `db::list_tool_permissions`（新增） |
| `revoke_tool_permission` | `{ sessionId, toolName, matchKind, matchValue }` | `()` | `db::revoke_tool_permission`（扩签名） |

`revoke_tool_permission` 在 `commands/permissions.rs` 已有同名命令壳（当前 dead_code），直接扩展入参即可；`db::revoke_tool_permission`（`db/permissions.rs:158`）从 `(pool, sid, tool)` 扩为 `(pool, sid, tool, kind, value)`。

## 关键设计决策

### D1 撤销立即生效，无需刷缓存

check 路径的 main-session grant 命中**每次都查 DB**：
- path glob：`check.rs:503` `SELECT match_value FROM session_tool_permissions WHERE ...`
- tool：`check.rs:630` → `db::has_tool_permission`（`db/permissions.rs:133` `fetch_optional`）
- prefix：`check.rs:641` `SELECT 1 FROM session_tool_permissions WHERE ...`

（`ctx.run_grants` 那个 in-memory cache 是 subagent worker 的 per-run cache，main session 不走它。）

⇒ 前端撤销写 DELETE 后，下一个 tool_use 的 check 即时读到"已无放行"，重新弹审批 modal。**无需任何 invalidate/notify 机制**。

### D2 NULL match_value 的 SQL（实现坑，必须在 db 层统一处理）

`match_kind='tool'` 的 `match_value` 是 **NULL**。SQLite 里 `match_value = NULL` 永假，必须用 `IS NULL`：

- list：直接 `SELECT match_value`（拿到 Option<String>，没问题）。
- **revoke DELETE 必须分支**：
  - `value.is_none()` → `WHERE ... AND match_value IS NULL`
  - `value.is_some()` → `WHERE ... AND match_value = ?`
- 不可写成 `WHERE match_value = ?` 然后 bind NULL——会静默删 0 行（撤销看似成功但记录仍在）。

这是本任务最高优先级的正确性坑，`implement.md` 标为风险点 + 必测。

### D3 revoke 改签名零风险

现有 `db::revoke_tool_permission` 标 `#[allow(dead_code)]`，全仓无调用方（grep 确认仅定义处 + 注释提及）。直接改签名，不留兼容壳。

### D4 match_kind 展示

badge 三色区分（tool=中性灰 / path=蓝 / prefix=紫），`matchValue` 用 `<code>` 字体显示 glob 或 prefix token；tool kind 的 null 显示"—"。

## 前端组件

- `components/permissions/PermissionGrantsModal.vue` — reka-ui `DialogRoot/DialogOverlay/DialogContent/DialogTitle`，header（标题 + close + refresh），body 为列表。结构照搬 `AuditLogModal.vue`。
- `components/permissions/PermissionGrantItem.vue` — 单行：badge(kind) + toolName + `<code>`matchValue + grantedAt + 撤销按钮。照搬 `AuditLogItem.vue`。
- `stores/permissionGrants.ts` — `grants/loading/error/lastSessionId` + `loadForSession/revoke/refresh`。照搬 `audit.ts`。

## 兼容性 / 回滚

- 纯**新增** IPC 命令 + 新增前端文件 + ChatPanel 加 2 行（按钮 + modal）。唯一**改签名**的是 dead_code 的 revoke（零调用方）。
- 回滚：删新增文件 + 撤 `lib.rs` 命令注册 + 还原 revoke 签名。无 schema 变更（表早已存在）。
- 不动 grant 写入、不动 check、不动 migrations。

## 不在范围

- 手动新增放行、live push、subagent per-run cache 管理、按 tool 批量撤销/"全部清除"。
