# C4 审计日志查询 UI

## Goal

为已落地的审计日志写入(⑯ `session_audit_events` 表)补齐**前端查询 UI**,让用户能事后回看单个 session 内"agent 做了哪些权限决策 / 哪步最慢 / Yolo 下被静默 hard-kill 了什么 / mode 怎么变的"。本任务 = 后端补 ⑩ tool 执行落表 + 一个薄查询 command + 纯前端 Modal。

## What I already know

### 后端现状(已落地,2026-06-13 A2+B7 PR1)
- 写入:`db::record_audit_event(pool, session_id, kind, payload_json)` — best-effort,失败只 `tracing::warn!`
- 查询:`db::list_audit_events(pool, session_id)` → `Vec<AuditEventRow>`,按 `ts DESC`,**已存在但标 `#[allow(dead_code)]`,无 Tauri command 调用**
- `AuditEventRow { id, session_id, ts, kind, payload_json: Option<String> }`,payload 是 raw JSON(不同 kind 不同 schema)→ 前端按 `kind` 分发渲染
- 表 schema 有索引 `idx_session_audit_events_session_ts(session_id, ts DESC)`,删 session cascade 清表
- AuditKind enum 在 `agent/permissions/mod.rs:152-187`,`as_str()` 映射 snake_case;加 `ToolExecuted` 变体无需 migration(kind 列是 TEXT)

### ⚠️ 实际落表范围核实(PRD 阶段 grep 确认)
文档 §2.5.8 说"每次记录 ⑨⑩⑬⑮",但代码只落了 ⑨ + Mode:

| 类别 | 现状 | 本任务 |
|---|---|---|
| ⑨ 权限决策(8 类) | ✅ 已落表 | 不动 |
| Mode 切换(3 类) | ✅ 已落表 | 不动 |
| **⑩ tool 执行(duration/exit_code)** | ❌ 未落表 | **补**(新 `tool_executed`) |
| ⑬ 循环检测 / ⑮ 路由 | ❌ 未落表 | 不做(收益低) |

### payload 形态(前端按 kind 分发)
- `tool_denied` / `tool_denied_yolo`:tool_name, tool_input, reason, mode, critical
- `tool_allowed` / `permission_granted`:tool_name, tool_input, mode
- `tool_permission_ask` / `permission_timeout`:tool_name, tool_input, mode
- `tool_executed`(新):tool_name, tool_input, duration_ms, exit_code
- `mode_changed` / `yolo_entered` / `yolo_exited`:mode(from/to)
- `request_cancelled`:(实现时确认)

### 前端入口锚点
`ChatPanel.vue:370-379` memory 按钮(`chat-panel__memory-btn`,条件 `projectsStore.currentProjectId`,Icon `brain`)→ 审计按钮对称加其后(line 379 后),条件 `chatStore.currentSessionId`,Icon shield 类。ref `auditModalOpen`(仿 line 304 `memoryModalOpen`),Modal 挂载仿 line 449 `<MemoryModal v-model:open>`。

## Requirements

### UI
- **独立 Modal**(`v-model:open` + ref 模式),标题"审计日志 — <session title>"
- **入口**:chat panel header Memory 按钮旁(绑**当前** session)
- 顶部:**kind 下拉筛选**(全部 / 按 10 类)+ **"仅 critical" 复选** + 事件计数
- 列表:**按时间倒序**,一项一事件:
  - 时间 `HH:MM:SS` + 带色 icon(🔴denied/critical · 🟢allowed/executed-success · 🟡mode · ⏱timeout)+ kind
  - tool 事件:tool_name + tool_input(截断)
  - 额外:reason(denied)/ duration + exit_code(executed)/ mode from→to(mode 变更)
- **critical 事件**(payload.critical === true)红左条(复用 PermissionModal 3px 红左 border)
- 非 0 exit code 有区分(橙/红)

### 后端
- 新增 `AuditKind::ToolExecuted`(as_str = `tool_executed`)+ agent loop tool 执行完成处补 `record_audit_event`(payload: tool_name/tool_input/duration_ms/exit_code)
- 新增 Tauri command `list_session_audit_events(session_id)` 包装 `list_audit_events` → `AuditEventRow[]`(serde 序列化)

## Acceptance Criteria
- [ ] 点击 header 审计按钮,Modal 打开,显示当前 session 全部 audit 事件,按时间倒序
- [ ] ⑩ `tool_executed` 事件显示 duration + exit_code(非 0 exit code 视觉区分)
- [ ] critical 事件红左条;denied 显 reason;mode 变更显 from→to
- [ ] kind 下拉筛选 + "仅 critical" 复选生效,计数实时更新
- [ ] 空 session / 无事件显示"暂无审计事件"占位
- [ ] payload 为 null/malformed 时不崩(容错)
- [ ] vue-tsc --noEmit + cargo check/test 绿(含 `tool_executed` db test)

## Definition of Done
- vue-tsc --noEmit 绿;cargo check / 相关 cargo test 绿
- 文档:ARCHITECTURE §2.5.8 "UI 查询(C4 任务)" 改已实施 + 补 ⑩ 落表 + 修 ⑩⑬⑮ gap 描述;ROADMAP §1.2 归档

## Edge Cases(MVP 处理)
| Edge | 处理 |
|---|---|
| 空 session / 无事件 | 占位"暂无审计事件" |
| payload null / malformed JSON | 容错,不崩(显示 raw 或省略额外字段) |
| 切 session 时 Modal 开着 | 关闭 Modal(换上下文,watch currentSessionId 重置 open) |
| 事件量大 | MVP 全量拉取(`fetch_all`),不做虚拟滚动/分页;若实测 >500 条卡顿再优化(标注 TODO) |
| Modal 开着期间 agent 又写新事件 | 手动刷新按钮(轻量);MVP 不做实时推送 |

## Decision (ADR-lite)

### D1 — UI 形态:独立 Modal
**Context**: 文档留白,候选 Modal / 侧边抽屉 / 消息流内联。
**Decision**: 独立 Modal。
**Consequences**: (+) 最小改动、隔离干净、无消息事件有处可挂、不触及废弃的 3b-2 三栏;(−) 模态遮罩不能边对话边看——可接受(用途是事后回看)。

### D2 — 入口:chat panel header Memory 旁
**Context**: 候选 sidebar footer / 右键菜单 / item hover / header memory 旁。
**Decision**: header Memory 按钮旁(用户指定)。
**Consequences**: (+) header 是"当前 session 操作"区,语义一致;发现性好;绑当前 session 符合最高频"回看刚才";(−) 查别的 session 要先切过去。

### D3 — 本任务补 ⑩ tool 执行落表
**Context**: ⑩ 只走 tracing 未落表,"哪步最慢"用途依赖它。
**Decision**: 本任务补(新 `AuditKind::ToolExecuted` + agent loop 落表)。
**Consequences**: (+) "哪步最慢"可用,审计价值完整;(−) scope 扩到写入侧 + 1 个 db test;⑬⑮ 仍不做。

## Out of Scope (explicit)
- 审计数据编辑 / 删除 / 导出
- 跨 session / 全局审计聚合视图
- 实时推送(WebSocket)— MVP 手动刷新
- ⑬ 循环检测 / ⑮ 路由落表(收益低)
- 虚拟滚动 / 分页(全量够用,>500 条再优化)
- 列表项分组(按 turn)/ 按 duration 排序(后续 enhancement)

## Implementation Plan (small PRs)
- **PR1(后端)**:⑩ 落表 + 查询 command
  - `agent/permissions/mod.rs` AuditKind 加 `ToolExecuted` 变体 + as_str
  - agent loop tool 执行完成处补 `record_audit_event`(调研调用点 + duration 来源:F5 "tool duration 字段"是否可复用)
  - `commands/permissions.rs` 加 `list_session_audit_events` command + 注册
  - db test:`tool_executed` 写入 + 查询往返
- **PR2(前端)**:AuditLogModal + 入口 + 渲染 + 筛选
  - `stores/audit.ts`(拉取 + 筛选状态)
  - `components/audit/AuditLogModal.vue`(+ 按 kind 分发的列表项子组件)
  - `ChatPanel.vue` 加入口按钮 + Modal 挂载 + 切 session 关 Modal watch
  - 文档收尾

## Technical Notes
- 后端:`db/permissions.rs:212`(`list_audit_events`)、`commands/permissions.rs`(写 command)、`agent/permissions/mod.rs:152-187`(AuditKind enum)
- ⑩ 调用点待调研:`agent/mod.rs` / `agent/chat.rs` 的 tool 执行完成处;duration 来源 F5 提"tool duration 字段携带"
- payload 不同 kind 不同 schema:`db/permissions.rs:240-244`
- ARCHITECTURE §2.5.8(行 678-735)权威描述(⑩⑬⑮ gap 本任务修 ⑩)
- critical 视觉约定:PermissionModal 3px 红左 border + shield-x
- 入口锚点:`ChatPanel.vue:370-379` memory 按钮、line 304 `memoryModalOpen`、line 449 MemoryModal 挂载
