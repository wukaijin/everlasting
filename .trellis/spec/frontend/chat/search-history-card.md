# search-history-card

> `search_history` tool 专属卡片(D2②+,2026-08-17,task `08-17-search-history-card`)。
> 后端 tool 契约见 [backend tool-contract 15-search-history](../../backend/tool-contract/15-search-history.md);
> 本文只管前端呈现层。

## Scenario: SearchHistoryCard 替换渲染 + modal 预填复用

### 1. 替换渲染(非 inline 追加)

`MessageItem.vue` 的 tool_use 分发(timeline + 非 timeline 回退**两处**)对
`name === "search_history"` 渲染 `<SearchHistoryCard>` **替换**通用 ToolCallCard —
照 `end_discussion` → DiscussionSummaryCard 先例。判断必须 `===` 全名匹配。
inline 追加模式(ask_user_question,通用卡保留 + 专属卡在下方)不适用:tool_result
文本对用户零价值,双卡重复占屏。

### 2. 数据源 = 卡片自取自查(不走 streamController)

卡片从 `tool_use.input`(随消息持久化,timeline item 直接携带)自取
`{query, scope, limit}`,onMounted 调 ① 的 `search_messages` IPC 重查拿结构化
`MessageSearchHit[]`。**live 与 replay 同一条路径**,无事件链。

与 [streamcontroller-routing](./streamcontroller-routing.md) 的边界:该路由模式
适用于"feature store 需要赶在渲染前拿到 tool 副作用数据"(B12 checklist /
review-state);当卡片自己就是数据归属、且数据可由确定性 input 重导出时,
自取自查更简单 —— 不要为纯展示加事件链。

**为什么不解析 tool_result 文本**:后端给 LLM 的文本故意不含 session_id(模型
无法行动,省 token);结构化数据重查一次毫秒级 IPC 即得。

### 3. 四态机

```
pending(call 无 result,流式窗口)→ 加载态
error(result.is_error)          → 渲染后端错误文案
onMounted 重查 ─┬─ 成功 0 条    → empty(query 回显,非 error)
                ├─ 成功 ≥1 条   → hits:前 3 条 + 「共 N 条 · 查看全部」CTA
                └─ 失败        → degrade:渲染 result.content 原文(保底不白屏)
```

- limit 用调用原值(≤50,忠实还原模型所见);CTA 打开的 modal 用它自己的
  RESULT_LIMIT 全量重搜("查看全部" = 现在的完整结果,非历史快照)。
- scope 映射:`current_project → projectsStore.currentProjectId`;`all/缺省 → null`。
- 当前 session 命中加「本会话」标记(chatStore.currentSessionId 比对)。

### 4. useSearchModal prefill 契约

```ts
open()                                   // 空白开(原行为,零回归)
open({ query, projectId?: string | null }) // 预填:开即搜(跳防抖/IME)
```

- `pendingPrefill` module-level 一次性消费(open watcher `consumePrefill()`,
  消费即清 — 防 stale prefill 泄漏到下次空白 open)。
- **bootingPrefill guard**(双触发防线):open watcher 给 query/projectFilter
  赋值会连带触发这两个 watcher 的防抖搜索 — guard 挡住本次 flush,nextTick
  后清除(watcher 没触发也清除,不会吞掉用户后续输入;一次性计数 flag 在
  "同 query 重复 open"场景会泄漏吃掉下次按键 — 勿回退成 counter)。
- **直绑陷阱**:`open` 有可选对象参后,`@click="openSearch"` 裸绑会把
  PointerEvent 当 prefill 传入 — 模板必须 `@click="openSearch()"`(vue-tsc
  能抓到;2026-08-17 AppHeader 实修一例)。

### 5. 复用与边界

- 命中行视觉:复用 ① 的层级语言(accent bar + title primary medium + meta
  muted + snippet 单行,`<mark>` 高亮);helper 抽 `utils/searchHits.ts`
  (`hitTimeLabel` / `splitSnippetAt`)modal 与卡片共享 — 新增第三处消费前先查它。
- SubagentDrawer 的 worker 调用**不渲染**本卡片(drawer 保持
  DrawerToolCallCard 文本)。
- 移动端:CTA ≥44px(responsive-mobile §6)。

### 6. Tests

`SearchHistoryCard.test.ts`(11):四态 / scope 映射 / top-3 + CTA 计数 / 本会话
标记 / title 徽标 / degrade 原文。`SearchModal.test.ts` +3:prefill 即搜 /
projectId 不双发 / stale prefill 不复用(测试要点:组件先 mount 再 open —
open watcher 只观察挂载后的 false→true 转移,生产 modal 常驻无此约束)。
`MessageItem.test.ts` +2:替换分发 + 其他工具不受影响。测试断言 filter 到
`search_messages` 调用(store 机制性 invoke 不计入)。
