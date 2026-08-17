# D2 跨 session 全文搜索(双驱动)— ① 用户驱动 MVP

## Goal

用户能通过全局搜索(Cmd/Ctrl+K)对所有 project 的全部 session 历史消息做全文检索,从结果钻入该 session 的**只读预览视图**(复用 chat 消息渲染、无输入区),并自动定位到命中消息。解决"我记得之前聊过/做过 X,但在哪个 session?"的找回问题。

ROADMAP D2 双驱动的 ① 用户驱动(本任务);② Agent 驱动 `search_history` tool 为 follow-up,共享本任务落地的 `search_messages` 后端查询。

## Background

- ROADMAP 第三档 active 项(`docs/ROADMAP.md:138`),2026-06-17 降档理由(session 积累尚浅)已不成立:06-17 起积累 2 个月会话。
- 既有 sidebar 搜索 = 纯标题子串过滤,仅当前 project、client 端(`app/src/utils/sessionGrouping.ts:134-141`),Cmd/Ctrl+K 聚焦该框(`SessionList.vue:421-428`)——与本任务的全局搜索是两个东西,保留不动。

## Requirements

### R1 后端 `search_messages` 共享查询层

- FTS5 `messages_fts`(external-content,索引 `messages.text` 列)+ trigger 同步 + 存量回填,复制 `autonomous_memories_fts` 模式(`db/migrations/schema.rs:843-905`)。
- 搜索半径 = `messages.text`(Text block 纯文本,天然排除 thinking,`llm/types/message.rs:146-163`);删除 session 物理删消息(`session_crud.rs:337-351`)→ 结果天然只含存活 session。
- 查询参数:query 文本、可选 project_id 过滤、limit。返回扁平命中列表(**两类命中统一返回**,带 `kind: title | content` 判别):session_id、session title/project_id/updated_at(两类均有);seq、role、speaker、snippet(前后文截断 + 匹配偏移)仅 content 命中有值。
- **中文 2 字词必须可搜**:trigram tokenizer 要求 ≥3 字符(`schema.rs:843-853` 实证教训),<3 字符查询走 LIKE `'%q%'` 兜底(个人应用消息量级全扫可接受)。

### R2 IPC + daemon 路由

- `search_messages` 命令走 6 处标准注册链(命令 + `_inner`、`lib.rs generate_handler!`、daemon handler、`.route()`、前端 `http.ts` CMD_TO_DOMAIN);路由段 = 命令名(B1 hotfix 惯例教训)。
- Tauri / 浏览器 / PWA-remote 三模式行为一致。

### R3 全局搜索 Modal(入口)

- 新建 SearchModal 挂 AppShell(与 TracePanel 同级全局层),**Cmd/Ctrl+K 从"聚焦 sidebar 标题框"改指向打开全局搜索**;sidebar 标题过滤保留不动(零回归)。
- 结果 = 标题命中 + 消息命中统一展示:标题命中排前;消息命中按 project 分组、组内按 session 聚合(session 标题 + 时间 + 命中片段)。提供 project 过滤。
- 输入防抖(约 250ms);空态 / 无结果态。

### R4 命中预览(钻入)

- 点击消息命中 → 同一 Modal 内切换为该 session 的**只读消息预览**:复用 MessageItem 渲染(markdown / 工具卡 / thinking 折叠 / 群聊 speaker,与主聊天视图一致),无输入区、无编辑/重发 hover 菜单(MessageItem 加 `readonly` gate)。
- 自动滚动到命中消息(视口中央)+ 临时高亮。
- 预览头提供"在主窗口打开"与返回结果列表。"在主窗口打开"跨 project 命中时必须正确切换 project(含 sidebar 刷新与 lastSession 归属),不允许把命中 session 记到当前 project 名下(设计定案:chat store 组合 action `openSessionInProject`,见 design §4.2)。

## Acceptance Criteria

- [ ] AC1:中文与英文关键词搜索均返回正确命中(含跨 session、跨 project)。
- [ ] AC2:2 字中文查询(如"权限")返回结果(LIKE 兜底生效)。
- [ ] AC3:点击消息命中后,Modal 内展示该 session 只读消息视图,命中消息滚动至视口中央并有可见高亮;预览内不可触发编辑/重发。
- [ ] AC4:"在主窗口打开"完成切换,主视图落在该 session;**跨 project 命中时 sidebar 列表切到目标 project,lastSession 归属目标 project**(回归测试锁定)。
- [ ] AC5:存量 DB(升级路径)启动后即可搜,无需手工 rebuild。
- [ ] AC6:浏览器模式(daemon HTTP)与 Tauri 模式搜索行为一致。
- [ ] AC7:删除 session 后其消息不再出现在搜索结果(回归测试锁定);/clear 清空消息后同理。
- [ ] AC8:Cmd/Ctrl+K 打开全局搜索;sidebar 标题过滤行为不变(回归锁定)。

## Out of Scope

- ② Agent 驱动 `search_history` tool(follow-up 任务,复用 R1 查询层)。
- thinking / tool_call / tool_result 内容、subagent_runs transcript、audit payload 的搜索。
- 预览内消息二次操作(编辑/删除/复制)、搜索历史持久化、结果分页(单 limit 截断 + 提示)。
- 正则 / 高级搜索语法。

## Technical Notes(关键决策,详证 design.md)

- **Q1 入口 = 全局 Modal 接管 Cmd/Ctrl+K**(2026-08-17 用户定案);sidebar 标题过滤保留。
- **Q2 范围 = 全部 project 默认**,结果按 project 分组 + 过滤(用户定案,按推荐)。
- **Q3 跳转 = Modal 内只读预览 + 定位**(用户定案:内容复用 chat 渲染、无 input、定位到目标;不复用 ChatPanel/MessageList 整壳——MessageList 直读 `store.messages` 非 prop 驱动,`MessageList.vue:25,43`)。
- **Q4 兜底 = <3 字符 LIKE**(按推荐定案)。
- FTS update trigger 限定 `AFTER UPDATE OF text`(messages 表有高频 metadata/latency UPDATE,避免 FTS 写放大;memories 模式未限定是因为该表无高频非文本更新)。
