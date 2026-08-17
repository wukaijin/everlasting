# D2②+ search_history 前端专属卡片(SearchHistoryCard + 搜索 modal 复用)

## Goal

D2② 落地的 `search_history` tool 用户实测反馈:tool_result 是给 LLM 的紧凑文本,
在通用 ToolCallCard 里被截断、展开也难读。本任务给主聊天视图做**专属卡片**,
并复用 ① 的 SearchModal 做"点击还原完整结果"。

决策(2026-08-17 与用户对齐):**换专属卡片,不做 LLM 配套展示工具**(use_ui 路线)
——命中列表是确定性数据的投影,不该经 LLM 再生成一轮(额外 turn/token/不配合风险);
①的 modal 是现成资产,卡片点开 = 同一 query 的完整列表 + 预览 + 跳 session。

## Requirements

- **R1 替换渲染**:`MessageItem.vue` timeline `tool_use` 分支,`name ===
  "search_history"` 渲染 `<SearchHistoryCard>` **替换**(非 inline 追加)通用
  ToolCallCard —— 照 `end_discussion` → DiscussionSummaryCard 先例(文本坨没有
  保留价值);props 同款 `:call`(含 input)+ `:result`。
- **R2 数据源 = 重查 IPC**:卡片从 `tool_use.input`(已随消息持久化,replay 同路)
  取 `{query, scope, limit}`,映射后调 ① 的 `search_messages` IPC 拿结构化
  `MessageSearchHit[]`。**不解析 LLM 文本**(故意不含 session_id)、**后端零改动**、
  streamController 零改动(自取自查,不走 B12 路由)。scope 映射:
  `all → projectId: null`;`current_project → 当前 project id`(projectsStore)。
  limit 用调用原值(≤50,忠实还原模型所见)。
- **R3 卡片四态**:
  - `pending`(有 call 无 result,流式窗口):轻量加载态;
  - `error`(result.is_error 或重查失败):展示错误文本;**重查失败降级**为渲染
    result.content 原文(保底不白屏);
  - `empty`(重查 0 命中):「无命中」+ query 回显;
  - `hits`:紧凑列表前 3 条(日期 / project / session 标题 / #seq · role / 单行
    snippet)+ 「共 N 条命中 · 点击查看全部」CTA。title 命中行带 kind 标注。
- **R4 modal 预填复用**:`useSearchModal` 扩 `open(prefill?: { query: string;
  projectId?: string | null })`;SearchModal 开启时若有 prefill → 预填 query(+ 可
  选 projectFilter)并**立即搜索**(跳过防抖)。卡片 CTA → `open({query, projectId})`。
  modal 内部行为(列表/预览/跳转/关)零改动。
- **R5 视觉与移动端**:复用 ① SearchModal 的命中行视觉语言(三段文字色 + accent
  bar,08-17 `3655f3a` 刚定的层级)与卡片容器语言(对齐 DiscussionSummaryCard);
  移动端按 responsive-mobile hit-area 规范。
- **R6 边界**:SubagentDrawer 的 worker 调用**不做**卡片(drawer 保持
  DrawerToolCallCard 文本,MVP 收窄);给 LLM 的 tool_result 文本**零改动**;
  后端 / IPC / wire 零改动。

## Out of Scope

- 卡内逐条命中点击直接跳 session(CTA 单入口开 modal 足够;modal 内已有完整跳转)。
- 重查结果缓存(个人 DB 毫秒级,replay 时最多 N 卡 N 查,可接受;fresh 优先)。
- worker drawer 卡片、群聊、`use_ui` 路线。

## Acceptance Criteria

- [x] AC1:`search_history` 的 tool_use 在主视图渲染 SearchHistoryCard(替换
  ToolCallCard);其他工具不受影响;`end_discussion` 等既有分发零回归。
- [x] AC2:卡片以 input 参数重查 IPC,scope→projectId 映射正确
  (all → null;current_project → 当前 project)。
- [x] AC3:四态渲染正确(pending / error 含重查失败降级 / empty / hits 前 3 +
  CTA);CTA 打开 modal 且 prefill query 立即出结果。
- [x] AC4:replay(切换 session / 刷新)后卡片从存储的 input 自重建,同 live 路径。
- [x] AC5:worker drawer 内 search_history 仍走 DrawerToolCallCard 文本(不改)。
- [x] AC6:`pnpm test` 全绿(新增 SearchHistoryCard 状态机测试 + prefill 测试 +
  MessageItem 分发测试);`vue-tsc` + `pnpm build` 零错。
- [x] AC7:spec 更新 — `frontend/chat.md` 增 scenario(search-history-card:
  替换渲染先例引用 / 重查数据源决策 / 四态机 / modal prefill 契约)。

## Notes

- 前置:`08-17-agent-search-history-tool`(D2② 后端 tool,commit `a005b51`)。
- 关键既有资产:`useSearchModal` 单例 composable(`open()` 现无参)、
  `MessageSearchHit` TS 类型(`chat.types.ts:541`)、SearchModal 结果行样式、
  `end_discussion` 替换渲染先例(`MessageItem.vue:380`)。
