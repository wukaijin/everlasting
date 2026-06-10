# fix: provider config hot-reload + model_id 必填 + session 绑定 model_id

## Goal

修复三个相互关联的问题：(1) 修改 provider 配置后内存 catalog 不刷新，发消息仍用旧 base_url；(2) display_name 改为选填，空时默认等于 model_name；(3) session 只绑定 model_id（UUID），chat 优先使用 session 绑定的 model。

## Requirements

### R1: Provider catalog 热更新

**现状**：`AppState.catalog: Arc<ProviderCatalog>` 在启动时构建一次，所有 provider/model CRUD 命令不触发重建。

**方案**：
- `catalog` 类型从 `Arc<ProviderCatalog>` 改为 `Arc<tokio::sync::RwLock<ProviderCatalog>>`
- `state.rs` 新增 `pub async fn rebuild_catalog(&self)` 方法，内部调用已有的 `build_provider_catalog`
- `commands/providers.rs` 中 6 个 CRUD 命令（`add_provider` / `update_provider` / `delete_provider` / `add_model` / `update_model` / `delete_model`）执行 DB 操作后调用 `state.rebuild_catalog()`
- `lookup_provider_for_default` 读 catalog 时改为 `state.catalog.read().await`

### R2: display_name 选填，空时默认 model_name

**现状**：前端 `canSave` 要求 `modelName` 和 `displayName` 都非空；DB `display_name TEXT NOT NULL`。

**方案**：
- **前端**：`canSave` 去掉 `form.displayName.trim() !== ""` 条件；`save()` 提交时如果 `displayName` 为空则赋值 `modelName`
- **Rust**：`add_model` / `update_model` 命令里做同样的 fallback：`let display_name = if display_name.is_empty() { model_name.clone() } else { display_name }`
- **DB**：不改 schema（`display_name NOT NULL` 仍然成立，因为 Rust 侧保证非空）

### R3: Session 只绑定 model_id，chat 使用 session 的 model

**现状**：`create_session` 不写 model_id；`chat` 命令只看全局 `default_model_id`。

**方案**：
- **创建 session**：`db/sessions.rs` 的 `create_session` 在 INSERT 时读 `app_config.default_model_id` 并写入 `sessions.model_id`
- **Chat 解析**：`lookup_provider_for_default` 改为 `lookup_provider_for_session`，接受 `session_id` 参数：
  1. 从 DB 读 `sessions.model_id`
  2. 如果有值 → 用它查找 catalog（替代全局 default）
  3. 如果 NULL（老 session）→ fallback 到全局 `default_model_id`
- **`sessions.model` legacy 列**：不再写入 model name，新 session 设为空字符串 `""`（保持 NOT NULL 约束不破）

## Acceptance Criteria

- [ ] 修改 provider base_url/api_key 后，已有 session 发消息立即使用新配置
- [ ] 新增/删除 model 后，catalog 立即反映变化
- [ ] display_name 留空提交后自动填入 model_name 值
- [ ] display_name 手动填写时使用用户填写的值
- [ ] 新建 session 的 `model_id` 不为 NULL，等于当前全局 default
- [ ] chat 优先使用 session 绑定的 model_id 对应的 provider
- [ ] session.model_id 为 NULL 时 fallback 到全局 default_model_id
- [ ] session.model_id 指向已删除 model 时 fallback 到全局 default
- [ ] cargo check 通过
- [ ] 前端 vue-tsc --noEmit 通过

## Definition of Done

- cargo check 通过
- 前端 vue-tsc --noEmit 通过
- 手动验证热更新流程
- PRD 关联文件更新

## Technical Approach

### 文件变更清单

| 文件 | 变更 |
|------|------|
| `src-tauri/src/state.rs` | `catalog` 改 `Arc<RwLock<ProviderCatalog>>`；新增 `rebuild_catalog()`；`build_provider_catalog` 提升为 pub |
| `src-tauri/src/commands/providers.rs` | 6 个 CRUD 命令末尾加 `state.rebuild_catalog()` |
| `src-tauri/src/agent/chat.rs` | `lookup_provider_for_default` → `lookup_provider_for_session`，接受 `session_id`，优先读 session model_id |
| `src-tauri/src/db/sessions.rs` | `create_session` 写入 `model_id`（从 app_config 读 default） |
| `src-tauri/src/db/models.rs` | `create_model` / `update_model` 加 display_name fallback 逻辑 |
| `src/components/settings/ModelsTab.vue` | `canSave` 去掉 displayName 必填；`save()` 加 fallback |
| `src/components/settings/ModelForm.vue` | displayName label 改为"显示名称（选填）" |

### 不变的文件

- `migrations.rs` — 不改 schema（display_name NOT NULL 由 Rust 侧保证非空）
- `db/types.rs` — ModelRow struct 不变

## Out of Scope

- session 列表 UI 显示 model 名称
- model 连通性测试改进
- daemon 架构改造
- `sessions.model` legacy 列删除（SQLite 不支持 DROP COLUMN，留着不影响）

## Technical Notes

### Catalog RwLock 线程安全

- `catalog: Arc<tokio::sync::RwLock<ProviderCatalog>>` — 多个 chat 并发 read() 不阻塞
- rebuild_catalog 用 write() 独占，但频率极低（只在用户点保存配置时）
- agent loop spawn 时 clone 的是 `Arc<RwLock<...>>`，后续 `.read().await` 拿到的是最新 catalog

### Session model_id fallback 链

```
chat(request_id, session_id, ...)
  → db::get_session(session_id).model_id
  → if Some(mid) → catalog[mid]
  → if None / catalog miss → global default_model_id → catalog[default]
  → if still miss → DB slow path (resolve_chat_provider)
```
