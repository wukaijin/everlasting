# D2 跨 session 全文搜索 — 实施计划

> 前置:`design.md` 定稿;3 PR 顺序执行,每 PR 独立可测、可交付。

## PR1 后端:FTS 数据层 + 查询层

- [x] `db/migrations/schema.rs`:`messages_fts` 虚拟表(trigram,external-content 挂 `messages.id`/`text`)+ 三 trigger(insert / delete / **`AFTER UPDATE OF text`**)+ 幂等 `rebuild` 回填(design §1.1-1.2)
- [x] 新文件 `db/search.rs`:`MessageSearchHit` + `search_messages(pool, query, project_id, limit)`(≥3 字符 FTS/bm25,<3 LIKE 兜底;snippet + match 偏移 Rust 统一生成;标题命中并入返回 `kind: title|content`,design §2 / §4.2)
- [x] 单测(Colored in-memory pool,对齐 `db/memories_tests/fts5_migration.rs` 模式):
  - [x] CJK ≥3 字命中 / 英文命中 / 2 字中文 LIKE 兜底 / 1 字符查询
  - [x] 存量回填:预插消息 → 跑 migration → 可搜(AC5)
  - [x] 删 session → 零命中 + FTS 行数 0(AC7);`/clear` 路径同理
  - [x] UPDATE metadata/latency → FTS 行数不变;UPDATE text(D3 编辑路径)→ 新文本可搜旧文本不可搜
  - [x] project 过滤 / limit / 空 query 返回空
- [x] `cargo test -p everlasting --lib` 全绿(PKG_CONFIG_PATH 见 AGENTS.md)

## PR2 IPC + daemon 路由

- [x] `commands/sessions.rs`:`search_messages_inner` + `#[tauri::command]`
- [x] `lib.rs` `generate_handler!` 注册
- [x] `daemon/routes/sessions.rs`:Request struct + handler + `.route("/search_messages", post(...))`(路由段 = 命令名;**必须 post**——`httpTransport.invoke` 对 CMD_TO_DOMAIN 命令硬编码 POST,评审 P1)
- [x] `commands/mod.rs all_command_names()` 补名
- [x] 前端 `transport/http.ts` CMD_TO_DOMAIN:`search_messages → sessions`;`transport` 层 `searchMessages()` 封装 + `MessageSearchHit` TS 类型(snake_case 直传)
- [x] 测试:transport 命令路由测试(既有模式)+ daemon route 单测(对齐 sessions 域邻居)
- [x] 手工冒烟:`scripts/daemon.sh` 起本地 daemon → `curl -X POST :7456/api/v1/sessions/search_messages -d '{"query":"...","limit":20}'`(wire 形态 snake_case:前端 `transformArgsTopLevel` 做 camel→snake;注意 B1 教训:curl 只能验证路由本身,前端链路要走 transport 真实 URL 形态)

## PR3 前端:SearchModal + 只读预览

- [x] `utils/messageFormat.ts`:提取 `buildRunGroups(messages)` 纯函数(自 `MessageList.vue:79-91`);MessageList 改调用,行为等价
- [x] `MessageItem.vue`:加 `readonly?: boolean` prop → gate `<MessageActionsMenu>`(结构禁止,非 CSS 隐藏)
- [x] 新组件 `components/search/SearchModal.vue`(reka-ui Dialog,AppShell 全局层):
  - [x] 态 A:输入(autofocus + 250ms 防抖 + IME gate)+ 标题命中区 + 消息命中区(project→session 两级分组,单 session 多命中折叠)+ project chips + 空态/截断提示/错误态(旧 daemon)
  - [x] 态 B:预览头(标题/project/[在主窗口打开]/[← 返回])+ `SearchPreviewBody`
- [x] 新组件 `components/search/SearchPreviewBody.vue`:`load_session` → `rehydrateMessages` → `buildRunGroups` → `MessageItem(readonly)`;`<li :data-seq>` 锚点 → `nextTick` 后 `scrollIntoView({block:"center"})` + 2s 高亮
- [x] `AppShell.vue` 挂 SearchModal;`registerKeybinding` Cmd/Ctrl+K → open;**摘除** `SessionList.vue:421-428` 旧 Cmd+K 聚焦绑定(sidebar 标题过滤逻辑保留)
- [x] 移动端:Modal 全屏降级(`max-width:767px`)+ AppHeader 搜索 icon 入口(移动端唯一可见入口,桌面隐藏)
- [x] "在主窗口打开" → chat store 新增组合 action `openSessionInProject(projectId, sessionId)`(评审 P2 方案甲,细节见 design §4.2):跨 project 时 `switchProject` → `await loadSessions` → `switchSession`;同 project 直接 `switchSession`。**禁止**裸 `switchSession` 跨 project 调用(会把 B session 记成 A 的 last active + `sessions.value.find` miss 导致 `currentCwd` 置空)
- [x] vitest 补:跨 project 打开回归(lastSession 写对 project / currentCwd 正确 / sidebar 列表为目标 project)
- [x] vitest:SearchModal(防抖/分组/过滤/两态切换/readonly 无菜单)、`buildRunGroups` 等价、Cmd+K 绑定替换(AC8:sidebar 标题过滤回归)
- [x] `pnpm test` + `pnpm vue-tsc --noEmit`(或 build)全绿

## 收口(每 PR 后 + 任务级)

- [x] `trellis-check`:spec 合规 + 跨层数据流(6 处注册两两对齐)
- [x] spec 沉淀:`.trellis/spec/backend/database-guidelines.md` 加 messages_fts Scenario(UPDATE OF text 红线);`frontend/chat.md` 或新文件记 SearchModal/readonly 契约
- [x] ROADMAP(D2 → §1.2)+ BACKLOG 同步;② Agent 驱动 tool 立 follow-up 项
- [x] live 验证:真实 DB 搜中文 2 字词 + 钻入预览定位(WSL 本地 GUI 或浏览器模式)

## 验证命令速查

```bash
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test -p everlasting --lib
cd app && pnpm test && pnpm build   # build 含 vue-tsc
```

## 风险点 / 回滚

- **`buildRunGroups` 提取**(PR3 唯一动主聊天热路径的点):行为等价重构,MessageList 既有测试 + 手工一轮流式对话兜底;回滚 = 还原函数体内联。
- **MessageItem readonly gate**:漏 gate 编辑入口 = 预览里改错 session;以"readonly 下 MessageActionsMenu 不渲染"测试锁定。
- **Cmd+K 语义变更**:摘旧绑定时注意 `SessionList.vue` 的 `searchActive` gate 联动(别把标题过滤本身摘了);AC8 测试锁定。
- **FTS migration**:全部幂等(IF NOT EXISTS + rebuild),旧库升级失败不影响消息数据(FTS 表独立);核选项 = 手动 `rebuild`。
- PR1/PR2 纯后端新增,无前端耦合,可独立回滚(路由/命令无人调用即惰性)。
