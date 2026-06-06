1. 前端 sessions 的状态圆点放到最左侧，size 改成 8px
2. chat panel header 改版个修复
 - 高度改成28px
 - session title 字体变小
 - git branch 没有正确展示， 要修复
 - header 右远端显示 pwd
3. write tool 有问题，有一次llm调用一直失败。 可能跟参数有关， 目前是偶发无法复现。
4. 与 llm 交流过程要允许打断， 之前 tool call 死循环了， 只能强退app.
5. llm text message 加 margin-top, 支持markdown渲染，另外首行有空行。
   - [RESOLVED 2026-06-06 in PR7] 首行空行根因: Anthropic SSE 流式首字符常为 `\n` (role marker 后的格式),`white-space: pre-wrap` 保留为可见空行
   - 修法: `MessageItem.vue` 显示层 `displayContent` computed, `replace(/^\s+/, "")` strip leading whitespace, 不污染 DB / wire format, 流式增量 idempotent
   - 剩余: "加 margin-top" + "markdown 渲染" 仍未做,见父 task 06-06-spike-005-follow-up PR6
