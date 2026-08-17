# D2 跨 session 全文搜索 — 评审记录

> 评审对象:task `cross-session-search`(planning 阶段)的 `prd.md` / `design.md` / `implement.md`。
> 评审方式:design/implement 引用的取证锚点逐一与真实代码(2026-08-17 main `f1926e5`)核对。
> 结论:**方案总体成立,可进入实施;1 处必须修的契约矛盾(P1)+ 1 处需提前定案的语义决策(P2)+ 1 处文档同步(P3)**。

## 结论概要

三件套质量高,取证锚点绝大多数精确无误(FTS5 模式、Text block 排除 thinking、物理删链路、latency/metadata 不碰 text、六处注册链、rehydrate 路径、MessageItem prop 驱动等均核实成立)。planning 状态合理(implement.jsonl / check.jsonl 仍是模板占位,未开始写码),P1 现在修正零成本。

---

## P1(必须修)design §3 第 4 条 "GET + query string" 与前端统一 POST 契约矛盾

- `httpTransport.invoke` 对**所有** CMD_TO_DOMAIN 命令发 `POST /api/v1/{domain}/{cmd}`,args 走 JSON body(`app/src/transport/http.ts:330`);daemon `sessions` 域路由全部 `post(...)`,唯一 GET 是 `/:id/snapshot`,而它不在 CMD_TO_DOMAIN 里(transport 特判 URL)。
- 若按 design 注册 `get(search_messages)`,前端 POST 会 405,AC6 直接挂;`tauriTransport` 走同一 invoke,同受影响。
- **修正**:改为 `post(search_messages)`,与其他 sessions 域命令零特例;query/project_id/limit 走 body。相应 implement.md PR2 的 curl 冒烟改 `-X POST -d '{"query":"..."}'`。

## P2(需提前定案)跨 project "在主窗口打开"的语义

`switchSession`(`app/src/stores/chatSessionActions.ts:149`)**不切换 project**——只 ensureLoaded → 设 currentSessionId → `writeLastSession(currentProjectId, sessionId)`。后果:

1. 从 project A 打开 project B 的命中 → B 的 session 被错记为 A 的 last active(A 下次打开落错 session);
2. sidebar 列表仍显示 A 的 sessions,与聊天视图指向 B 不一致。

implement.md PR3 写"实施时取证",但结论现在就能下,建议 design 提前定案,二选一:

- **方案甲(推荐)**:打开前先走 projects store 的切 project 入口(设 currentProjectId)→ 再 `switchSession`;行为与 sidebar 点击跨 project 的既有链路对齐,lastSession 语义正确。
- 方案乙:明确接受"不切 project、lastSession 只写同 project 命中"的约束,命中预览头标注所属 project 防误导。

留到实施现场定容易变成隐式行为,trellis-check 也难判定对错。

## P3(文档同步)返回契约三处描述不一致

- PRD R1 的 `MessageSearchHit` 无标题命中字段;
- design §4.2 修正后"标题命中并入 `search_messages` 返回,加 `kind: title|content`"未回写 PRD。

实施以 design 为准,但建议顺手补 PRD R1 字段,避免 trellis-check 阶段 contract 漂移。

---

## 取证核对(确认无误的关键点)

| 锚点 | 核对结果 |
|---|---|
| FTS5 模式样板 `schema.rs:843-905`(trigram + external-content + 3 trigger) | ✅ 属实;注释明确记录 "2 字中文 '权限' 不 MATCH" 实证教训,LIKE 兜底决策有据 |
| `messages.id` rowid 别名(external-content 挂接成立) | ✅ `schema.rs:156-157` `id INTEGER PRIMARY KEY AUTOINCREMENT` |
| 搜索半径 = `messages.text`(排除 thinking) | ✅ `llm/types/message.rs` `to_text()` 只取 Text block,注释明说 thinking 排除 |
| 删 session 物理删消息 → 结果天然只含存活 session | ✅ `db/sessions/session_crud.rs:341` `delete_session` 先 `DELETE FROM messages WHERE session_id=?`;`/clear` 走 `delete_messages_by_session` 同型 |
| 高频非文本 UPDATE 不触 text 列 → `AFTER UPDATE OF text` 红线成立 | ✅ `update_message_latency` 只 SET ttfb/gen/total/thinking_ms;`update_message_metadata` 只 SET metadata;tool_result duration patch 只 SET content(`db/sessions/messages.rs:152/209/312`) |
| D3 编辑路径 `SET content/text/metadata` → FTS 同步 | ✅ `db/sessions/messages.rs`(update 末尾 SET content+text+metadata);no-op fast path 不写,零 FTS churn |
| `escape_fts5` 短语包裹可复用 | ✅ `db/memories/search.rs:20` |
| 六处注册链模板 `group_chat_cache_rates` | ✅ `commands/sessions.rs` `_inner` + daemon handler + `.route("/命令名")` 模式真实存在;`lib.rs:278` generate_handler! 注册;`commands/mod.rs:74` `all_command_names()` 存在 |
| `all_command_names()` 补名 | ✅ `commands/mod.rs:74` |
| CMD_TO_DOMAIN 手工对齐惯例 | ✅ `transport/http.ts:51` 起,B1 hotfix(缺行报 `unknown cmd`)+ S2 tunnel 缺 3 行两条注释实证惯例 |
| Q3 不复用 MessageList 整壳的根据 | ✅ `MessageList.vue:25,43` 直读 `store.messages`(computed);`renderGroups`(`MessageList.vue:79-91`)可提取,`isRealUserTurnStart` 已在 `utils/messageFormat.ts` |
| MessageItem prop 驱动,可加 `readonly` gate | ✅ `MessageItem.vue:65-67` `defineProps<{ message }>`;`<MessageActionsMenu>` 在 MessageItem 内(结构 gate 可行) |
| 预览数据路径 `load_session` → `rehydrateMessages` | ✅ `commands/sessions.rs:157` `load_session_inner`;`streamController.ts:806` / `streamRehydrate.ts:127` 同款路径 |
| 只读快照不污染主视图 | ✅ `rehydrateMessages` 产全新数组;不经 LRU/不写 lastSession 的约束实施时遵守即可 |
| Cmd/Ctrl+K 摘旧绑定风险(AC8) | ✅ `SessionList.vue:421-428` 旧绑定 `enabled: () => props.searchActive && ...`;implement.md 风险点已捕获"别把标题过滤本身摘了" |
| `registerKeybinding` 支持 `{ ctrlOrMeta, key }` | ✅ `utils/useKeyboard.ts:72` KeyBinding 接口 `ctrlOrMeta?` |
| AppShell 全局层挂载点 | ✅ `AppShell.vue:89` `<TracePanel />` 同级可挂 SearchModal |
| AppHeader 移动端 icon 入口 | ✅ `AppHeader.vue:64-74` 汉堡按钮同区模式 |
| reka-ui Dialog | ✅ `app/package.json:32` `reka-ui ^2.9.9` |
| 迁移幂等 / daemon 版本兼容 / FTS 回滚处置 | ✅ 均合理,无需改动 |

---

## 建议动作

1. design §3 第 4 条改 `post(search_messages)`,implement.md PR2 curl 冒烟同步改(P1);
2. design §4.2 补"跨 project 打开"定案,选方案甲或乙(P2);
3. PRD R1 返回字段补 `kind: title|content`(P3);
4. 其余按现有三件套执行,无需其他改动。
