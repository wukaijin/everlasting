# 讨论库检索——历史审议可检索复用(GCE M4b)

## Goal

让历史群聊审议(含定时审议)成为可检索的「讨论库」:GUI 用户在召集新审议或复盘前,能先浏览全部历史场次、按议题/结论/任务名/参与人检索,命中直接跳回该场完整会话看结论——避免重复审议烧 token、结论散落不可寻。

## Background(关键事实)

- 群聊消息本体在 `messages`(带 `speaker`),**逐消息已被现有 messages_fts 命中**(D2,`db/search.rs` 同时服务 GUI SearchModal 与 agent `search_history`)——不在本期范围。
- 场级语义数据全在 DB:`sessions`(session_type='group_chat' / stop_reason / discussion_summary(GC7 一等字段)/ metadata 存 participants + scheduled_task_name)+ `scheduled_tasks`(定时档)+ `group_chat_checkpoints`(起止锚点)。
- 转录 `.md` **仅定时场**由 daemon 自动导出到 `{app_data_dir}/discussions/`,是 DB 内容的渲染投影;纯磁盘孤儿文件,无 DB 引用/无 GUI 读口。
- 可抄的接线模板:daemon route + Tauri cmd + transport(`commands/sessions.rs:1099` / `daemon/routes/sessions.rs:307` / SearchModal)+ FTS5 external-content 模板(schema.rs:1195 / spec database-guidelines.md:51-97)。

## Requirements

- R1 **场级检索/浏览**:历史全部群聊场,每场一条命中,展示 task_name(定时场)/ 标题 / 参与人 / 日期 / stop_reason / discussion_summary 预览;经典 chat 场永不出现。
- R2 **GUI 独立「讨论库」面板**(用户已决 Q1+Q1b):空关键词也能浏览全量场次;关键词命中 title / summary / task_name / participants 任一即中;支持项目与 stop_reason 筛选。
- R3 **检索源 = sessions 行直读 + 程序 LIKE**(设计决策 Q4):不建独立冗余表、不引入新 FTS5 表与实时触发器(数据量低;场数上 10^3 或需语义排序时再按 database-guidelines 模板升 FTS5,查询层契约不变)。
- R4 **打开命中 = 跳回该场完整会话**(`chatStore.openSessionInProject`,含 DiscussionSummaryCard 结论);不做转录 .md 文件打开/查看器。
- R5 **查询层与入口分离**:daemon API/Tauri cmd 薄封装暴露(list + search 两端点),agent/M1/MCP 后续可复用同一查询层,本期不做。
- R6 经典聊天路径零行为变化;不新增 DB migration;转录导出零改动。

## Acceptance Criteria

- [ ] AC1 后端:list/search 返回全部历史群聊场(每场一行:session_id/project_id/title/task_name/participants/stop_reason/discussion_summary/created_at/updated_at);`session_type='chat'` 场永不返回;多场按 updated_at DESC。
- [ ] AC2 检索:关键词命中 title 或 summary 或 task_name 或 participants 任一即返回该场;空关键词返回全量(浏览模式);无命中返回空数组不报错。
- [ ] AC3 GUI:Sidebar/顶部群聊入口打开独立面板,默认列出全部群聊场(按日期分组);搜索框 debounce 输入即搜;项目/stop_reason 筛选可用;每行展示任务名徽章(定时场)/ 标题 / 参与人 / 日期 / summary 预览(≤2-3 行);点「打开会话」跳回该场完整会话并定位到结论区。
- [ ] AC4 零回归:经典会话消息搜索(SearchModal / search_history / messages_fts)行为不变;`cargo test --lib` 与前端相关测试通过;无 DB migration。
- [ ] AC5 边界:进行中场(终态前)按 title/task/participants 可搜,不要求被 summary 关键词命中;转录 .md 文件与导出行为零改动。

## Out of Scope

- 转录 .md 正文全文检索 / 入库 / 文件打开查看器(仅定时场产物;逐句已由 messages_fts 覆盖)。
- 独立冗余 discussion 表 + FTS5 实时触发器(见 R3 判定条件)。
- 跨场语义聚类/导出、成本治理、远程鉴权、飞书推送等其余 GCE M4 子项。
- agent 工具 / M1 / MCP 复用(查询层已分离,R5)。

## 决策记录

- Q1 消费场景 = **GUI 讨论库**(2026-09-07 用户选择;内核带 daemon API 查询层供后续复用)。
- Q1b GUI 形态 = **独立浏览面板**(2026-09-07 用户选择,非塞进 SearchModal)。
- Q2 文档内容 = 场级短字段(summary + title/task_name/participants/日期),不拼 messages 长文进文档。
- Q3 打开命中 = 跳回完整会话(见 R4)。
- Q4 索引维护 = 主源 sessions 行直读,无触发器无回填(见 R3)。
