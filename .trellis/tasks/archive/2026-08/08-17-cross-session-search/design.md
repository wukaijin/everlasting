# D2 跨 session 全文搜索(① 用户驱动 MVP)— 技术设计

> 对应 PRD:`prd.md`(Q1-Q4 已定案)。取证锚点为 2026-08-17 现状(main `f1926e5`),行号随漂移以符号名为准。

## 0. 总体架构

```
[前端]                              [daemon / Tauri IPC]           [SQLite]
SearchModal (AppShell 全局层)
  Cmd/Ctrl+K → open
  input 防抖 250ms
        │ search_messages(query, project_id?, limit)
        ▼
  http.ts CMD_TO_DOMAIN ──────────→ commands/sessions.rs
                                    search_messages_inner
                                          │
                                          ▼
                                    db/search.rs ← 新增
                                    ├─ ≥3 字符: messages_fts MATCH (bm25)
                                    └─ <3 字符: LIKE 兜底
                                          │
                                          ▼
                                    JOIN sessions/projects → 扁平命中
  结果分组(标题/消息 × project)
  点击命中 → 预览态(同 Modal 内切换)
        │ load_session(target) → rehydrateMessages   ← 只读,不动当前 session 状态
        ▼
  SearchPreviewBody: run 分组 + MessageItem(readonly) 渲染
  data-seq 锚点 → scrollIntoView(center) + 高亮
```

三层解耦点:① 查询层(`db/search.rs`)与 Agent 驱动 ②(follow-up `search_history` tool)共享;② 预览走 `load_session` 只读路径,不经 streamController LRU / `switchSession`,主视图状态零污染;③ 搜索 Modal 与 sidebar 标题过滤完全独立(零回归面)。

## 1. 数据层:`messages_fts`(R1)

### 1.1 虚拟表 + trigger(复制 memories 模式,一处关键差异)

```sql
-- db/migrations/schema.rs 新增(migration 幂等:CREATE ... IF NOT EXISTS)
CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(
  text,
  content='messages',
  content_rowid='id',
  tokenize='trigram'
);

CREATE TRIGGER IF NOT EXISTS messages_fts_insert AFTER INSERT ON messages BEGIN
  INSERT INTO messages_fts(rowid, text) VALUES (new.id, new.text);
END;
CREATE TRIGGER IF NOT EXISTS messages_fts_delete AFTER DELETE ON messages BEGIN
  INSERT INTO messages_fts(messages_fts, rowid, text) VALUES('delete', old.id, old.text);
END;
-- 关键差异:限定 OF text。memories 的 am_fts_update 未限定列(schema.rs:874-905),
-- 但 messages 有高频非文本 UPDATE(update_message_latency / metadata 路径),
-- 不限定 = 每次 latency 落库都做一次 FTS delete+insert 写放大。
CREATE TRIGGER IF NOT EXISTS messages_fts_update AFTER UPDATE OF text ON messages BEGIN
  INSERT INTO messages_fts(messages_fts, rowid, text) VALUES('delete', old.id, old.text);
  INSERT INTO messages_fts(rowid, text) VALUES (new.id, new.text);
END;
```

- `messages.id INTEGER PRIMARY KEY AUTOINCREMENT` = rowid 别名,external-content 挂接成立(schema.rs:156)。
- tokenizer = **trigram**,与 memories 一致;选型实证:unicode61 对 CJK 不分词(schema.rs:843-853 注释),trigram 代价 = 查询 ≥3 字符 → LIKE 兜底(§2)。
- D3 编辑(`messages.rs:459-504` 重写 content+text)、B2 metadata 写入(`update_message_metadata`,不触 text 列→零 FTS churn)、`/clear` 物理删(trigger 覆盖)均被 trigger 覆盖。

### 1.2 存量回填(AC5)

migration 末尾:`INSERT INTO messages_fts(messages_fts) VALUES('rebuild')`(external-content 标准重建,全表扫 `messages.text` 重灌)。新库 = 空表 rebuild no-op。个人量级(月级千条消息)毫秒级,启动路径可接受。

### 1.3 回归红线

- **AC7 锁定**:`delete_session` 物理删(`session_crud.rs:337-351`)→ trigger 同步删 FTS;测试:建 session + 消息 + 删 session → `search_messages` 零命中 + FTS 表行数为 0。
- `AFTER UPDATE OF text` 红线:metadata/latency UPDATE 不产生 FTS 写(测试:连续 UPDATE metadata 后 messages_fts 行数不变)。

## 2. 查询层:`db/search.rs`(R1)

```rust
pub struct MessageSearchHit {
    pub kind: SearchHitKind,    // Title | Content —— 标题命中与消息命中统一返回(§4.2)
    pub session_id: String,
    // 以下字段仅 Content 命中有值;Title 命中为 None/默认(前端按 kind 判别)
    pub seq: Option<i64>,
    pub role: Option<String>,
    pub speaker: Option<String>,
    pub snippet: Option<String>,   // Rust 侧统一截断(见下)
    pub match_start: Option<usize>, // snippet 内匹配起点(前端高亮)
    pub match_end: Option<usize>,
    // 以下字段两类命中均有值
    pub session_title: String,
    pub project_id: String,
    pub project_name: Option<String>,
    pub updated_at: String,
}

pub fn search_messages(
    pool: &SqlitePool,
    query: &str,
    project_id: Option<&str>,
    limit: u32,                  // 默认 50,上限 200
) -> Result<Vec<MessageSearchHit>>
```

- **分派**:`query.chars().count() >= 3` → FTS;否则 → LIKE。阈值按字符数(trigram 按 unicode 字符计),不按字节——3 个中文 = 3 字符。
- **FTS 路径**:复用 `escape_fts5` 短语包裹(`db/memories/search.rs:20-22` 风格,`"..."` 内双引号翻倍)→ `WHERE messages_fts MATCH ? ... ORDER BY bm25(messages_fts) LIMIT ?`,JOIN messages 取 seq/role/speaker + JOIN sessions/projects 取标题与 project 维度。限定 `sessions.id` 存在(FK 已保证,防御性 JOIN 顺手完成)。
- **LIKE 路径**:`WHERE m.text LIKE '%' || ? || '%' ESCAPE '\'`(Rust 侧转义 `%_\`),`ORDER BY m.id DESC`(新→旧,近因优先)。无索引全扫——个人量级实测接受;若未来慢,升级路径 = 2 字词辅助 LIKE 由 FTS trigram 无法覆盖的既有结论担保,不阻塞 MVP。
- **snippet 统一在 Rust 生成**(两条路径一个契约):`locate` = 对 `text` 做大小写不敏感 `find`(LIKE 路径直接用 query;FTS 路径用 query 首个 term)→ 截取 `[pos-40, pos+60]`(char 边界对齐)→ `match_start/end` 相对 snippet。不用 FTS5 `snippet()`:两条路径 marker 形态不一,前端要二次解析,统一 Rust 侧生成消掉该分叉。
- **排序语义**:bm25(相关度)优先于 LIKE 路径的近因——两类路径不混排(前端按 project/session 分组后组内顺序即返回序,见 §4)。
- 单测覆盖:CJK ≥3 命中 / 2 字兜底命中 / 空 query 拒绝(返回空)/ project 过滤 / limit / 删 session 零命中 / UPDATE OF text 触发同步 / metadata UPDATE 不动 FTS。

## 3. IPC + daemon 路由(R2)

六处注册(以 `group_chat_cache_rates` 为模板,取证见 PRD):

1. `commands/sessions.rs`:`search_messages_inner(pool, req) -> Vec<MessageSearchHit>` + `#[tauri::command] search_messages`。
2. `lib.rs` `generate_handler!` 注册。
3. `daemon/routes/sessions.rs`:handler 调 `_inner`;Request struct `{ query: String, project_id: Option<String>, limit: Option<u32> }`。
4. 同文件路由表 `.route("/search_messages", post(handler))`——**路由段 = 命令名**(B1 hotfix `0191947` 惯例)。**必须 POST**:`httpTransport.invoke` 对所有 CMD_TO_DOMAIN 命令硬编码 `POST /api/v1/{domain}/{cmd}`(评审 P1,`http.ts:338-341`);sessions 域唯一 GET `/:id/snapshot` 走 transport 特判 URL、不经映射,不是先例。args 走 JSON body,线上形态 snake_case(`transformArgsTopLevel` 做 camelCase→snake_case 顶层键转换,`http.ts:230-239`——前端封装传 `projectId`,wire 上是 `project_id`)。
5. 前端 `transport/http.ts` CMD_TO_DOMAIN 加 `search_messages → sessions`(与 Rust 侧两处手工对齐,`http.ts:46-50` 注释惯例)。
6. `commands/mod.rs all_command_names()` 名单补一行(文档性质,顺手)。

- 返回结构 snake_case 直传(BACKLOG §5.2 决策:Rust struct → TS 零 rename)。
- TS 侧 `transport` 加 `searchMessages()` 封装,三模式(Tauri invoke / daemon HTTP / pwa-remote proxy)由既有 transport 层免费覆盖。

## 4. 前端(R3/R4)

### 4.1 入口与状态

- `AppShell.vue` 全局层加 `<SearchModal v-model:open>`(与 TracePanel 同级,`AppShell.vue:89` 毗邻);`registerKeybinding({ combo: { ctrlOrMeta: true, key: "k" } })` 打开(`utils/useKeyboard.ts:37-107` 既有体系)。
- **摘除旧绑定**:`SessionList.vue:421-428` 的 Cmd/Ctrl+K 聚焦逻辑删除;sidebar 标题过滤框与其逻辑(`sessionGrouping.ts:134-141`)**保留不动**(AC8 回归锁定)。
- 移动端:Modal 全屏形态(`max-width:767px` 降级);入口 = AppHeader 加搜索 icon 按钮(桌面隐藏,与汉堡按钮同区,`AppHeader.vue:64-74` 模式)——抽屉内的旧 Cmd+K 入口摘除后移动端需要一个可见入口。

### 4.2 SearchModal 结构(单 Modal 两态)

```
SearchModal.vue (reka-ui Dialog,AppShell 挂载)
├─ 态 A: 结果列表
│   ├─ 输入框(autofocus,防抖 250ms,IME composition gate——TriggerMenu 同款)
│   ├─ 标题命中区(排前,点击 → switchSession + 关 Modal)
│   └─ 消息命中区:project 分组 → session 聚合(标题+时间+N 命中)
│        └─ 命中行:snippet 高亮段 → 点击 → 态 B
│   └─ project 过滤 chips(全部 / 各 project)
└─ 态 B: 只读预览(返回按钮 → 态 A)
    ├─ 头:session 标题 + project + [在主窗口打开] + [← 返回]
    └─ SearchPreviewBody(§4.3)
```

- 结果分组/聚合在前端做:后端返回扁平 hits,按 project_id→session_id 两级 group;单 session 多命中折叠展示(首条 snippet + "还有 N 条")。
- 标题命中:复用 `list_sessions` 数据(store 已有 sessions?——注意那是**当前 project** 的列表;全局标题搜索走后端:对 `sessions.title` 的 LIKE 查询并入 `search_messages` 返回(加 `kind: "title" | "content"` 判别字段)——**修正**:统一一个 IPC 一次往返返回两类命中,避免前端拉全量 sessions。
- **跨 project"在主窗口打开"语义(评审 P2 定案,方案甲)**:chat store 新增组合 action `openSessionInProject(projectId, sessionId)`:`projectId !== currentProjectId` 时先 `projectsStore.switchProject(projectId)`(`projects.ts:308-310`,本身只设 id)→ `await loadSessions(projectId)`(显式等待,勿依赖 chat.ts watcher 的异步 `onProjectChange`——`switchSession` 内 `sessions.value.find` 只含当前 project 列表,不等待会 miss → `currentCwd` 置空 + `writeLastSession` 写错 project)→ `switchSession(sessionId)`。三步同在 chat store 模块作用域内,无竞态。同 project 命中退化为直接 `switchSession`。**禁止**裸 `switchSession` 跨 project 调用:它不切 `currentProjectId`(`chatSessionActions.ts:149-181`),会把 B 的 session 错记为 A 的 last active,sidebar 与聊天视图也会分裂。

### 4.3 SearchPreviewBody(只读消息视图,Q3 决议)

- **不复用 ChatPanel/MessageList 整壳**:`MessageList` 直读 `store.messages`(`MessageList.vue:25,43`),字面复用会渲染当前 session;且其滚动逻辑与 streaming/forceFollow 耦合。
- **复用粒度 = MessageItem + run 分组纯函数**:
  - `MessageItem` 已是 prop 驱动(`message: ChatMessage`,`MessageItem.vue:65-67`),markdown/工具卡/thinking/speaker 渲染免费复用。
  - **新增 `readonly?: boolean` prop**:gate 掉 `<MessageActionsMenu>`(Edit/Resend/Copy 走当前 session store action,在预览里触发 = 打错 session,必须结构禁止而非 CSS 隐藏)。流式相关 computed 对落库消息天然 false,无感。
  - run 分组逻辑从 `MessageList.vue:79-91` 提取纯函数 `buildRunGroups(messages)` 进 `utils/messageFormat.ts`(`isRealUserTurnStart` 已在该文件),MessageList 改调用(行为等价重构,既有测试兜底),预览复用同函数——交错思考分组观感与主视图一致。
- **数据获取**:`transport.invoke<LoadedSession>("load_session", { sessionId })` → `rehydrateMessages(loaded.messages)`(`streamRehydrate.ts:127`,`streamController.ts:806` 同款路径)。**不经 streamController LRU / 不动 currentSessionId / 不写 lastSession 持久化**——纯只读快照;目标 session 正在流式中的边界 = DB 已落库的部分可见,可接受(快照语义)。
- **定位与高亮**:预览内每条消息包 `<li :data-seq="m.seq">`(预览层自己包,不改 MessageItem 根节点);`onMounted → nextTick → querySelector([data-seq="${target}"]) → scrollIntoView({ block: "center" })` + `--search-hit` 高亮 class(2s 后移除)。MessageList 无虚拟化 = 全量渲染后 scrollIntoView 直接可用,无需虚拟定位基建。
- 长会话性能 = 与主视图切 session 同量级(全量 TransitionGroup 渲染),既有行为对齐,不新增虚拟化。

### 4.4 前端测试

- SearchModal:防抖触发 / 空态 / 分组结构 / project 过滤 / 态 A↔B 切换 / readonly 下 MessageActionsMenu 不渲染。
- `buildRunGroups` 提取:MessageList 行为等价(既有 streamController/MessageList 相关测试零回归)。
- scrollIntoView jsdom 缺失 → vi.mock(`Element.prototype.scrollIntoView`)。

## 5. 兼容 / 迁移 / 回滚

- **迁移**:全部 `IF NOT EXISTS` + 幂等 rebuild,对齐 `add_models_column_if_missing` 既有幂等惯例;新库/旧库同一代码路径。
- **daemon 兼容**:GET 路由纯新增;旧前端 + 新 daemon = 多一个无人调用的路由,无害。新前端 + 旧 daemon = 搜索报 transport 错误,Modal 内错误态提示(daemon 升级由 `scripts/daemon.sh` 管控,个人部署场景接受)。
- **回滚**:FTS 表/trigger 独立于业务表,回滚 = 停用搜索入口(前端)即可,数据无损;FTS 表损坏的核选项 = `rebuild` 重灌。

## 6. 权衡记录(为什么不是别的)

- **不做 `snippet()` SQL 函数**:FTS/LIKE 两路径 marker 不一致,前端二次解析成本 > Rust 统一截断(§2)。
- **不做前端全量 sessions 标题过滤复用**:全局标题命中必须跨 project,client 端没有全量数据,并入后端一次往返(§4.2 修正)。
- **不做 MessageList prop 化**:给核心滚动组件加数据源注入口,回归面 = 主聊天视图热路径;提取 20 行纯函数的回归面 = 可枚举。选后者。
- **不做结果分页**:limit 截断 + "仅显示前 N 条" 提示;分页 UX(游标/加载更多)留 follow-up,不影响查询层契约(`limit` 参数已预留)。
- **② Agent 驱动 search_history tool 不并入**:工具的返回形态(给 LLM 的精简摘要 vs 给用户的 snippet/高亮偏移)不同,共用 `db/search.rs` 的 SQL 层即可,IPC 层各自薄封装——follow-up 任务再定工具契约。
