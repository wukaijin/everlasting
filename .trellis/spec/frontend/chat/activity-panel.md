# ActivityPanel — chat 运行状态面板

> 2026-09-02,task `09-02-chat-task-panel`。`ChecklistCard` 的合并进化版:一个浮层三 section(子代理 / 后台命令 / 清单)。

## 组件契约

- `components/chat/ActivityPanel.vue`,挂 `ChatPanel.vue` 右下浮层(原 ChecklistCard 位,absolute right:20px / bottom:156px / z-50,低于 modal 层)。Props:`items: ChecklistItem[] | null`(沿用 checklist store 语义:缺 key=隐藏 section,`[]`=空态仍渲染)、`sessionId: string | null`。
- 数据源三路:**子代理**只读 `subagentRuns.runSummaryBySession`(实时性由该 store 既有 `subagent:event`/`subagent:finished` eager-fetch 保证,面板不重复订阅,行点击 `openDrawer(runId)` 复用 SubagentDrawer);**后台命令**读 `stores/backgroundShells.ts`(fetch + `background_shell:update` 增量);**清单**来自 items prop(checklist store 零逻辑改动,只迁移渲染)。
- 可见性:任一 section 有数据才挂载;首次出现自动展开,用户手动最小化后尊重选择。浮球 = 运行中徽标(running subagents + running shells)+ 清单 `done/total` + 呼吸圈(running 总数 + in_progress 清单 > 0)。
- 挂载/session 切换时两个 store 各 `fetchForSession`(subagent 此前只有 ToolCallCard 懒加载路径,面板需显式拉历史);`backgroundShells.ensureStarted()` 幂等懒挂 listener,失败重置守卫下次重试。

## Gotcha:MonotonicMs 与墙钟两种时间源

后台 shell 的 `startedAtMs` 是**进程单调毫秒**(Rust `MonotonicMs`),subagent 的 `startedAt` 是 ISO 墙钟字符串 —— 同一面板两种源,严禁混算:

- shell running 行 elapsed 显示 = `elapsedMs + (now - receivedAt)`(墙钟偏移法,`startedAtMs` 永不与 `Date.now()` 相减);
- **running 摘要每次入店(事件 upsert 与 fetch 整表替换都算)无条件重置 `receivedAt`** —— 保留旧值会把 session 切走期间的时长双倍计入(check 实证 bug);
- 终态 duration 用后端算好的 `elapsedMs`(同源相减),前端不自己算。
- 排序 comparator(`compareShells`/`compareSubagentRuns`)是 store/组件导出的纯函数,vitest 直测;后端 list 已同序,前端排序只为事件 upsert 后保持。

后端契约(事件 shape / 发射点 / 双模式接线)见 [backend/background-shell-observability.md](../../backend/background-shell-observability.md)。
