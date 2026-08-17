# implement.md — SearchHistoryCard

前端单 PR 体量:1 新组件 + 1 新测试 + 3 文件小改。后端零改动。

## 步骤

1. **`composables/useSearchModal.ts`**:扩 `open(prefill?)` + module-level
   `pendingPrefill` ref + `consumePrefill()`(SearchModal 用,消费后清空)。
   现有 `open()` 无参调用零回归(全仓 grep 确认调用点:AppShell / AppHeader)。
2. **`components/search/SearchModal.vue`**:open watcher 消费 prefill(预填
   query / projectFilter + 同步 runSearch);无 prefill 分支维持现状。
3. **`components/chat/SearchHistoryCard.vue`(新)**:
   - props `{ call: { id, name, input }, result?: ToolResult | null }`;
   - 状态机见 design §2(pending / error / empty / hits / 降级);
   - onMounted + watchEffect(input 变化仅 replay 重建时会触发)重查;
   - CTA → `open({ query, projectId })`;
   - 样式:容器对齐 DiscussionSummaryCard,命中行复用 ① 层级语言(accent bar +
     三段文字色),移动端 hit-area。
4. **`components/chat/MessageItem.vue`**:timeline `tool_use` 分支加
   `search_history` 判断(照 `end_discussion` 形状,替换 ToolCallCard)。
   注意两处:timeline 分支(`item.kind === 'tool_use'`)+ 若存在非 timeline
   回退路径的 tool 渲染(`msg__tools`),同步加(检查 buildTimeline 回退路径是否
   也渲染 tool_use —— 回退路径 tool 区在 useTimeline=false 时仍走旧结构)。
5. **测试**:
   - `SearchHistoryCard.test.ts`:四态(mock transport.invoke)+ scope→projectId
     映射 + CTA 触发 prefill open + 重查失败降级渲染 result.content;
   - `SearchModal.test.ts` 增 prefill 用例(或新文件,看现有测试组织);
   - `MessageItem.test.ts` 增 search_history 分发断言(渲染卡片不渲染 ToolCallCard)。
6. **验证**:`pnpm test` 全量 + `pnpm build`(含 vue-tsc)。
7. **spec**:`frontend/chat.md` scenario index + `chat/search-history-card.md`
   (替换渲染先例 / 自取自查决策 vs streamcontroller-routing 的适用边界 / 四态 /
   prefill 契约)。

## 风险 / 回滚

- MessageItem 是核心渲染组件,分发判断必须窄(`===` 全名匹配);回退路径若遗漏
  会造成 timeline/非 timeline 两种视图不一致 —— 步骤 4 显式检查。
- useSearchModal 扩参为向后兼容(可选参),AppShell/AppHeader 现调用零改动。
- 回滚 = revert 单 commit(纯前端)。
