# 讨论库检索 — 技术设计(GCE M4b)

## 0. 定位

历史群聊审议(含定时审议)的**场级检索 + GUI 讨论库**。命中粒度 = 一场(结论文档),非逐消息(逐消息已被 `messages_fts` 覆盖)。消费入口 = GUI 独立面板(用户已决),查询层与入口分离以便 agent / M1 / MCP 后续复用。**转录 .md 正文不入索引**(仅定时场存在、是 DB 的渲染投影)。

## 1. 架构与边界

```
┌─ GUI: DiscussionLibraryModal.vue(独立面板,新增)
│   浏览全部群聊场 / 关键词搜索 / 跳回会话
│       │  listGroupChatSessions / searchGroupChatDiscussions
└─ transport(http/tauri 双) → daemon route + Tauri cmd
        │  (共享同一 _inner)
        ▼
   db/search.rs 新增「场级检索」模块 search_group_chat.rs
        │
        ▼
   DB:sessions(session_type='group_chat')  +  可选 FTS(discussion_search_fts)
        ▲ 写入:db 层 session 生命周期(建/终态/删)内联维护 + 存量 boot 回填
```

**不动**:群聊编排器(`group_chat_loop.rs`)、转录导出、scheduled_tasks、checkpoints、现有 messages_fts / SearchModal / search_history。经典聊天路径零行为变化。

## 2. 索引源与文档形状(核心决策)

**文档 = sessions 中 `session_type='group_chat'` 的一场**,每场一行。

| 文档字段 | 来源 | 用途 |
|---|---|---|
| id | session_id | 主键/打开会话 |
| title | sessions.title | 展示 + 索引 |
| task_name | sessions.metadata.scheduled_task_name | 展示 + 索引 |
| participants | sessions.metadata.participants[].name | 展示 + 索引(join,存短逗号串) |
| stop_reason | sessions.stop_reason | 展示/筛选 |
| discussion_summary | sessions.discussion_summary | **主检索面**(结论) |
| created_at / updated_at | sessions | 日期分组/排序 |
| transcript 路径 | `{app_data_dir}/discussions/{date}-{task}-{sid8}.md`(定时场) | 打开 .md(可选项) |
| project_id | sessions.project_id | 打开会话 |

> summary 是终态后才稳定落库的一等字段(GC7);场终态前 summary 为空 → 该场仍以 title/task/participants 可搜,不因 summary 缺失丢失。MVP 不把 messages 长文拼进文档(逐句已在 messages_fts)。

### 2a. 检索中文档是否含「转录正文」?
**否(MVP)**。理由:(1) 转录 .md 仅定时场存在,GUI/MCP/script 场没有;(2) 它是 messages 的渲染投影,不含 DB 外新信息;(3) 逐句命中已由 messages_fts 覆盖。场级文档 = summary + 短字段即可支撑「这议题以前审过吗、结论、哪场」。

## 3. 索引形态:视图表 + FTS(采纳 roadmap 备选「独立 discussion 视图表」)

**「要不要 FTS」的回答**:要,但**不引入新 FTS5 external-content 表 + 触发器的实时同步**。改为:

- **非 FTS 全量扫描 + 程序内评分**(首选):数据量 = 群聊场数十~数百(远小于全 messages),`WHERE session_type='group_chat'` 全表行读 + `LIKE` 匹配 + Rust 内排序分页,毫秒级。理由:
  - messages_fts 引入 external-content + 三触发器 + `%_docsize` 回填是**为「全量消息、每 insert/update 同步、行数 10^4+」**的必要复杂度;场级表行数低 2-3 个数量级,实时触发器的复杂度与风险不值。
  - 免去:新 FTS 表 schema + 三触发器 + `AFTER UPDATE OF` 红线 + boot 回填 staleness probe + FTS 与 summary 晚到(终态才写)的同步时序——summary 在终态一次写入,程序化重建天然覆盖。
  - 检索命中 = 行匹配,天然场级;无需 bm25 也能按 (summary 命中优先,updated_at DESC) 排。
  - **代价**:LIKE 无 trigram 优势(<3 字符中文本来就 LIKE;≥3 的中文词组 LIKE 仍可用),无 bm25 相关度。MVP 数据量下可接受。
- **兜底兼容**:若未来讨论场数上 10^3 或需语义排序,再按 database-guidelines.md:51-97 模板加 FTS5 external-content(内容列指向视图表),程序回填一次,不破坏查询层契约。

**查询层**新增 `db/search_group_chat.rs`(或并入 `db/search.rs` 同模块):

```sql
-- 浏览:全量群聊场,updated_at DESC
SELECT session_id, project_id, title, stop_reason, discussion_summary,
       created_at, updated_at
  FROM sessions
 WHERE session_type='group_chat'
 ORDER BY updated_at DESC;
```

- 筛选:任务名(metadata)、参与人(metadata)、日期段、状态(stop_reason)、项目 —— WHERE 子句,JS/Rust 侧解析。
- 关键词 `q`(≥1 字符):`title LIKE` + `summary LIKE` + task_name/participants LIKE → 合并去重;空 `q` = 全量浏览(面板打开即见全部场次)。
- snippet 由 Rust 对 summary/title 切 ~±100 字符窗口(复用 `cut_snippet` 思路),不做跨语言算术。

> 结论:采纳 roadmap 备选「要不要独立视图表」的 **yes 形态**,但用「session 行直接作文档 + 程序 LIKE」而非「独立表 + FTS5 实时触发器」。这既解决「命中文档粒度场级 + 含 summary」,又避开 FTS5 触发器同步复杂度。

## 4. 写入/回填策略(Q4)

- **主数据源 = sessions 行本身**(不新增冗余表)→ 无需触发器,行即文档。
- 群聊场生命周期写入:创建(insert session)→ 终态(summary 落库,`group_chat_loop.rs` 现有落库点)→ 删除(现有 delete_session 链路,连 messages 一起删)。
- 若走「独立列/视图」再讨论;MVP 主源直读,无回填。若未来加 FTS 表:boot 幂等重建(参考 messages_fts 的 staleness 判定,spec 红线)。

## 5. 契约(daemon API + Tauri cmd)

新增两个端点/命令(命名沿用 snake_case 字段,wire camelCase 仅 Tauri arg):

| 端点 | 语义 |
|---|---|
| `POST /api/v1/sessions/list_group_chat_sessions` | 分页浏览全部群聊场(可筛选) |
| `POST /api/v1/sessions/search_group_chat_discussions` | 关键词 + 筛选检索,场级命中 |

返回 `GroupChatSessionHit[]`:session_id / project_id / title / task_name / participants[] / stop_reason / discussion_summary / created_at / updated_at / transcript_path?(定时场)。

- daemon route 挂到现有 router(`daemon/routes/sessions.rs:307` 同区);Tauri cmd 同名注册(`commands/sessions.rs` 同区)。
- 前端 `transport` http.ts / tauri.ts 各加映射。
- 打开命中 = 复用 `chatStore.openSessionInProject`(SearchModal 同款,project-aware)。

## 6. GUI 讨论库(独立浏览面板)

- 组件 `app/src/components/discussions/DiscussionLibraryModal.vue`(新)。
  - 全量场次分组列表(今天/昨天/本周/更早,仿 SessionList)或 updated_at DESC 单列;顶部搜索框(debounce 同 SearchModal 250ms)+ 筛选 chips(项目 / 状态 / 任务档)。
  - 命中行:任务名徽章(定时场)/ 标题 / 参与人 / 日期 / stop_reason 徽章 / summary 预览 2-3 行。
  - 点行:「打开会话」→ `openSessionInProject`;定时场有转录路径时附「打开转录 .md」(见 Q3,可选做)。
- 入口:Sidebar 或顶部按钮(AppHeader/Sidebar 群聊区)挂到 AppShell;开态单例 composable(仿 `useSearchModal`)。
- 键盘:Esc 关;无需 roving tabindex(简单列表)。
- 空态:「还没有历史审议——右上角发起一场 / 或去设置建定时审议」。

## 7. 兼容与迁移

- 无 DB migration(主源 sessions 已有列;不建新表 MVP)。选做转录路径时,路径 = 现导出命名规则,不落库、运行时拼。
- 经典聊天路径零行为变化(新查询只 `WHERE session_type='group_chat'`)。
- daemon API 纯增量,不影响既有调用方。

## 8. 权衡与风险

| 项 | 判断 |
|---|---|
| 不建独立冗余表/不建 FTS 触发器 | 数据量低;未来加 FTS 有完整模板兜底。风险 = 场数暴增(>10^3)时 LIKE 全扫变慢——届时按模板加 FTS |
| summary 晚到(终态才稳定) | 场终态前 title/task/participants 仍可搜;MVP 接受「进行中场不被 summary 搜到」 |
| 打开 = 跳回会话,不做转录 .md 查看器 | 定时场结论在 summary card 完整;.md 是渲染投影,价值低 |
| 转录路径运行时拼 | 与现导出命名规则耦合(改名规则会失联);MVP 可接受 |

## 9. 不做(MVP 边界)

- 转录 .md 正文全文检索/入库/文件打开器。
- discussion FTS5 实时表(先例与兜底见 §3)。
- 跨场语义检索/聚类、导出。
- agent 工具 / M1 / MCP 复用(查询层已分离,后续薄包装)。
