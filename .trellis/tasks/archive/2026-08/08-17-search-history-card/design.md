# design.md — SearchHistoryCard + modal 预填复用

## 1. 结构(3 个触碰点 + 1 新组件,后端零改动)

```
MessageItem.vue (timeline tool_use 分支)
  └─ name === "search_history" → <SearchHistoryCard :call :result />   (替换,同 end_discussion)
        └─ 自取 call.input {query, scope?, limit?}
        └─ transport.invoke("search_messages", {query, projectId, limit})   (① 的 IPC)
        └─ CTA → useSearchModal().open({query, projectId})
useSearchModal.ts
  └─ open(prefill?) 扩参 + module-level pendingPrefill ref
SearchModal.vue
  └─ open watcher 消费 prefill:预填 query/projectFilter + 立即 runSearch()
```

**为什么自取自查而非 streamController 路由**:tool_use.input 已随消息持久化,
timeline item 直接携带(`messageTimeline.ts:31`),卡片 onMounted 即可重查 ——
live 与 replay 天然同一条路径,不新增事件链。B12/C2 的 handleToolCall 路由是为
"store 需要赶在渲染前拿到数据"的场景;这里卡片自己就是数据归属,路由是多余间接层。

**为什么不缓存**:重查毫秒级(个人 SQLite + FTS);fresh 优先 —— 历史命中可能随
新消息增长,缓存反而展示陈旧。replay 多卡 = 多查,量级无害。

## 2. SearchHistoryCard 状态机

```
        ┌─ result 未到(streaming 窗口)──────────→ pending(加载态)
call ───┤
        └─ result 已到 → onMounted 重查 IPC ─┬─ 成功 → hits(≥1)| empty(0)
                                               └─ 失败 → 降级渲染 result.content 原文
result.is_error(空 query / DB 错)──────────────→ error(渲染 result.content)
```

- **重查失败降级**(R3):result.content 是给 LLM 的可读文本,作为最后兜底比白屏/
  spinner 永转好 —— 卡片 header 照常,正文一行提示 + 原文。
- **limit 用原值**:忠实还原模型当轮所见(≤50);modal 打开则按 modal 自己的
  RESULT_LIMIT 全量重搜("查看全部"语义 = 现在的完整结果,非历史快照)。
- **scope 映射**:`current_project → projectsStore.currentProjectId`(会话视图内
  恒等于该会话所属 project,session 列表按 project 组织,不存在错位窗口);
  `all / 缺省 → null`。
- title 命中行:`[title]` 徽标 + session 标题,无 seq/role/snippet(同 LLM 文本
  语义,视觉对齐 modal 的 title 行)。

## 3. useSearchModal 预填契约

```ts
const { searchModalOpen, open, close } = useSearchModal();
open()                                  // 现行为:空白开(零回归)
open({ query: "worktree", projectId })  // 预填:modal 开启即搜
```

- module-level `pendingPrefill: Ref<Prefill | null>`;SearchModal 的
  `watch(searchModalOpen)` 里:置 `query.value = prefill.query ?? ""`、
  `projectFilter.value = prefill.projectId ?? null`,prefill 存在时**同步调
  runSearch()**(跳 250ms 防抖 + 跳 IME 窗口 —— 程序打开无输入法),消费后清
  `pendingPrefill`。
- 预填不改动 modal 内任何后续交互(用户可改 query 重搜、切 chip、预览、跳转)。
- availableProjects 刷新逻辑不动(prefill 带 projectId 时首轮即有过滤态,chips
  由该轮结果刷新 —— 与用户手选 filter 后的行为一致,无新状态)。

## 4. 视觉

- 卡片容器:对齐 DiscussionSummaryCard(圆角卡 + header 图标行 + 正文区)。
- 命中行:**复用 ① 的层级语言**(08-17 `3655f3a`):meta 段 muted 小字
  (日期 · project)、标题段 primary medium、snippet 单行 ellipsis muted;
  左侧 2px accent bar。行高与触控区按 responsive-mobile(行 ≥ 44px 等效 hit)。
- CTA:整卡 footer 一行按钮态(非每行可点 —— 单入口,PRD Out of Scope 已定)。

## 5. 决策记录

- **D1 替换而非 inline 追加**(ask_user_question 模式):文本坨对用户零价值,
  双卡重复占屏;end_discussion 先例正是"结论卡替换通用卡"。
- **D2 重查而非解析文本 / 后端附结构化数据**:文本无 session_id(有意省的);
  后端附数据要动 wire + 双形态(给 LLM 文本 + 给前端 JSON)复杂化 tool_result。
  重查代价 = 一次毫秒级 IPC,换来零后端改动 + replay 免存储。
- **D3 modal 全量重搜而非回放快照**:"查看全部"的用户预期是"现在还有哪些",
  且 modal 的过滤/chips/预览逻辑全部免改。
- **D4 CTA 单入口**:逐行点击跳 session 留 modal 内(已有),卡内不重复入口。
