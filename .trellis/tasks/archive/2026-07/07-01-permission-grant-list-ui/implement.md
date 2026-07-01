# Implement — Permission grant list management UI

## 实现顺序（先 backend，再 frontend，逐层可验）

### 1. db 层（`app/src-tauri/src/db/permissions.rs`）
- [ ] 1.1 新增 `PermissionGrantRow` 结构（`#[derive(sqlx::FromRow)]` + `#[serde(rename_all="camelCase")]`）：`session_id, tool_name, match_kind, match_value: Option<String>, granted_at`。
- [ ] 1.2 新增 `list_tool_permissions(pool, session_id) -> Vec<PermissionGrantRow>`：`SELECT session_id, tool_name, match_kind, match_value, granted_at FROM session_tool_permissions WHERE session_id = ? ORDER BY granted_at DESC, rowid DESC`。
- [ ] 1.3 改签名 `revoke_tool_permission(pool, sid, tool, kind, value: Option<&str>)`，DELETE 按 PK：`WHERE session_id=? AND tool_name=? AND match_kind=?` + value 分支（`IS NULL` / `= ?`，见 design D2）。去掉 `#[allow(dead_code)]`。
- [ ] 1.4 db 单测（`db/permissions_tests.rs` 或内联 `#[cfg(test)]`）：list 空/多条、revoke 命中 NULL value 行、revoke 不误删同 tool 异维度行、revoke 不影响别 session。

### 2. IPC 层（`app/src-tauri/src/commands/permissions.rs` + `lib.rs`）
- [ ] 2.1 新增 `#[tauri::command] list_session_tool_permissions(state, session_id) -> Result<Vec<PermissionGrantRow>, String>`，委派 db。
- [ ] 2.2 扩展现有 `revoke_tool_permission` 命令壳入参为 `(state, session_id, tool_name, match_kind, match_value: Option<String>)`，校验 `match_kind ∈ {tool,prefix,path}`（仿 grant 命令的校验），委派 db。去掉 `#[allow(dead_code)]`。
- [ ] 2.3 `lib.rs` 的 `invoke_handler!` 注册两个命令（list 新增；revoke 若未注册则补注册）。

### 3. 前端 store（`app/src/stores/permissionGrants.ts`，新增）
- [ ] 3.1 仿 `audit.ts`：`grants: Ref<PermissionGrantRow[]>` + `loading/error/lastSessionId`。
- [ ] 3.2 `loadForSession(sessionId)` → `invoke<PermissionGrantRow[]>("list_session_tool_permissions", { sessionId })`，前端稳定排序兜底（`grantedAt DESC`）。
- [ ] 3.3 `revoke(row)` → `invoke("revoke_tool_permission", { sessionId, toolName, matchKind, matchValue })`，成功后从 `grants` 本地移除该行（按四元组匹配）+ 不全量重拉。
- [ ] 3.4 `refresh()` 重拉 lastSessionId。

### 4. 前端组件（`app/src/components/permissions/`，新增）
- [ ] 4.1 `PermissionGrantItem.vue`：badge(matchKind) + toolName + `<code>`matchValue（null 显示 "—"）+ grantedAt + 撤销按钮；emit `revoke`。
- [ ] 4.2 `PermissionGrantsModal.vue`：reka-ui Dialog，header(标题 + refresh + close)，body 列表 / loading / error / 空状态文案。`v-model:open` 控制；open 时 `watch` → `loadForSession(currentSessionId)`。

### 5. 入口接线（`app/src/components/chat/ChatPanel.vue`）
- [ ] 5.1 在 `ChatPanel.vue:475` audit 按钮之后、`<WorkerAskBanner>` 之前，插入「放行管理」按钮，复用 audit 的 `v-if="chatStore.currentSessionId"` gate（同为 session 级）；图标用 `key`（KeyIcon 已在 Icon 注册表 `Icon.vue:104`，未被占用 —— audit 占了 `shield-check`）。`@click="grantsModalOpen = true"`。同步加 session-switch watcher（仿 `ChatPanel.vue:380` audit 的关闭逻辑）。
- [ ] 5.2 模板加 `<PermissionGrantsModal v-model:open="grantsModalOpen" />`（仿 `ChatPanel.vue:586`）。

### 6. 前端测试（vitest）
- [ ] 6.1 `permissionGrants` store：loadForSession 填充、revoke 本地移除命中四元组（含 null matchValue 用例）、revoke 不误删异维度行。
- [ ] 6.2 `PermissionGrantsModal` / `PermissionGrantItem`：渲染三 kind、空状态、点撤销 emit 正确 payload。

## 验证命令

```bash
# Rust（WSL 必须带 PKG_CONFIG_PATH，见 CLAUDE.md）
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test

# 前端类型 + 构建（含 vue-tsc --noEmit）
cd app && pnpm build

# 前端单测
cd app && pnpm vitest run            # 或具体文件 permissionGrants / PermissionGrantsModal
```

## 风险点 / 回滚锚

- **R-D2（最高）**：revoke DELETE 的 NULL match_value 分支——写错会"静默删 0 行"（撤销按钮看似成功，记录仍在）。必须 db 单测覆盖 `match_kind='tool'`（value NULL）撤销命中。
- **R-revoke-sig**：改 `revoke_tool_permission` 签名前再 grep 确认零调用方（当前仅 dead_code 定义处）。若 cargo 报未解析引用，回滚锚 = 还原签名为 `(pool, sid, tool)` + 新增 `revoke_tool_permission_exact`。
- **R-wire**：`matchValue: null` 经 JSON → Rust `Option<String>` → sqlx `Option<String>` 全链路需对齐；前端 TS 用 `string | null`（非 `string | undefined`），否则 `invoke` 传 undefined 会被 serde 当 missing。

## 完成前自检
- [ ] 三种 match_kind 在 UI 都能展示 + 撤销（手动跑一次：对 read_file 点始终允许→出现 path 行→撤销→消失→再触发 read_file 重新弹审批）。
- [ ] 撤销后无需手动 refresh 即生效（D1）。
- [ ] cargo test + pnpm build + vitest 全绿。
