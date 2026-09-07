# 讨论库检索 — 实施计划(GCE M4b)

## 0. 变更总览

后端(db 查询层 + daemon route + Tauri cmd + transport)+ 前端(独立讨论库面板)+ 文档(GROUP-CHAT-API-ROADMAP / DAEMON-API)+ spec。经典聊天路径零行为变化。

## 1. 后端查询层(db)

- [ ] `app/src-tauri/src/db/search.rs`(或新 `search_group_chat.rs`)新增:
  - struct `GroupChatSessionHit { session_id, project_id, title, task_name, participants, stop_reason, discussion_summary, created_at, updated_at }`(snake_case,serde)
  - `list_group_chat_sessions(pool, filters) -> Vec<Hit>`:全量群聊场,`updated_at DESC`,支持 project/status/date 过滤
  - `search_group_chat_discussions(pool, query, filters) -> Vec<Hit>`:关键词 LIKE 匹配 title/summary/task_name/participants + 同过滤
  - participants 解析:`metadata` JSON 读 `participants[].name` join 成串;task_name 读 `scheduled_task_name`
- [ ] 单测:`db/search_group_chat_tests.rs`(浏览/过滤/关键词命中 summary vs title/空结果/多场排序)

## 2. daemon API + Tauri cmd

- [ ] daemon route:`daemon/routes/sessions.rs` 增 `POST /api/v1/sessions/list_group_chat_sessions` + `search_group_chat_discussions`(薄包装,同 `search_messages` 先例 :307)
- [ ] Tauri cmd:`commands/sessions.rs` 增同名 cmd(share `_inner`)
- [ ] lib.rs 注册;transport http.ts / tauri.ts 映射
- [ ] `docs/DAEMON-API.md` 增两个端点契约(§sessions 区)

## 3. GUI 讨论库面板

- [ ] `app/src/components/discussions/DiscussionLibraryModal.vue`(新):全量场次列表 + 搜索框(debounce)+ 筛选 chips + 命中行(summary 预览/徽章)+ 「打开会话」
- [ ] 入口:Sidebar/顶部群聊区按钮 → AppShell mount;开态 composable `useDiscussionLibrary`(仿 useSearchModal)
- [ ] 打开命中:`chatStore.openSessionInProject`(SearchModal 同款)
- [ ] vue-tsc + 前端测试(组件 store/setup 断言;Playwright 冒烟选做)

## 4. 文档 + spec

- [ ] `docs/GROUP-CHAT-API-ROADMAP.md` §5 M4「讨论库检索」子项 → 交付段(待定项定案:yes 视图形态 + GUI 独立面板)
- [ ] `.trellis/spec/backend/database-guidelines.md`(或新 spec)增「场级 discussion 检索」场景:session 行直作文档 + 程序 LIKE(不引 FTS 触发器)的理由与数据量边界、未来升 FTS 的判定条件
- [ ] 视需:DAEMON-API / AGENTS.md 速查行

## 5. 验证命令

```bash
cd app/src-tauri && PKG_CONFIG_PATH=... cargo test --lib search_group_chat  # 新单测
cd app && pnpm test          # 前端(讨论库相关)
cd app && pnpm vue-tsc
cd app && pnpm test:e2e      # 若加 Playwright 冒烟
node scripts/... (无)
```

## 6. 风险 / 回滚点

- 查询层是新增,不动既有 search_messages;rollback = 不挂 GUI,db 函数无副作用。
- summary 字段晚到:场终态前仅 title/task/participants 可搜,AC 不要求进行中场被搜到。
- daemon route/cmd 命名与既有 `_inner` 风格一致(照抄 search_messages 三段式)。
- 本任务不建 DB 表、不改群聊编排器、不动转录导出——如实施中发现必须改这些,回 Phase 1 复议。
