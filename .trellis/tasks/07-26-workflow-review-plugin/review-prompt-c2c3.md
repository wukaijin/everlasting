# 评审提示词：review epic — C2↔C3 契约 + C2 前端可行性（定向）

> 这是定向评审，只聚焦三个最高价值维度，不送全套 design。
> 评审 LLM 需能读取 everlasting 仓库代码 + 下列 4 个文档片段。

--- 提示词正文 ---

我在为一个 AI 编程助手（everlasting，Tauri + Rust 后端 + Vue 前端）设计一个「多模型评审」功能，已拆成 3 个子任务（C1 引擎/C3 资源包/C2 可视化）。这次请你做**定向评审**，只看 3 个重点维度，不要泛泛而谈。

## 你需要读的文档（4 个片段）

1. **C3 schema 定义**：`.trellis/tasks/07-26-review-plugin-pack/prd.md` 的 **R7 段**（review-state.json schema）+ design.md 的 **§4**（写入流程）
2. **C2 TS 类型**：`.trellis/tasks/07-26-review-viz/design.md` 的 **§7**（ReviewState TS 类型）
3. **C2 前端机制**：`.trellis/tasks/07-26-review-viz/design.md` 的 **§2**（get_review_state IPC + review-state-updated 事件）+ **§5**（组件结构/数据流）
4. **C2 implement 待确认点**：`.trellis/tasks/07-26-review-viz/implement.md` 的 **Phase 0**（task_slug 获取 + 事件发送点）

## 背景（最小必要上下文）

- review-state.json 是 C3（主 LLM 在 revising 写）和 C2（前端读）之间的**跨任务数据契约**。C3 用 Rust serde 结构写 JSON，C2 用 TS 类型读 JSON。
- everlasting 的前端事件订阅用 Tauri `transport.on`（listen 模式），IPC 走 Tauri command + daemon route 双路径。参考现有 `list_workflow_plugins`（IPC）和 `subagent:finished`（事件）。
- task 体系：`.everlasting/tasks/<slug>/` 下有 task.json/prd.md，后端 `resolve_current_task`（`inject.rs:229`）按 project 扫描选当前 task。

## 请你重点评这 3 个维度

### 维度 1：C3 schema 与 C2 TS 类型是否真的一一对应？

这是我最担心的盲点——我一个人写两边，容易字段名/枚举/嵌套结构漂移。请逐字段比对：
- C3 R7 schema 的每个字段（schema_version/task_id/current_round/rounds[]/models{}/findings[]/triage/change_log/convergence_note/models_present/status/source_run_id 等）在 C2 §7 TS 类型里是否都有对应？
- 枚举值是否一致（verdict: pass/pass_with_minor/revise/reject；severity: critical/high/medium/low/info；status: completed/failed/timed_out/...）？
- 嵌套结构是否对齐（rounds[].models 是 map 还是 array？findings 在 models 内还是顶层）？
- models map 的 key：C3 说用 model_id（稳定 id），C2 TS 是否也用 model_id？还是用了 display_name？

如果有任何不一致，明确指出「C3 写 X，C2 读 Y」。

### 维度 2：C2 的前端机制在 everlasting 架构里能否跑通？

C2 design §2 设计了 `get_review_state` IPC + `review-state-updated` 事件。请你判断：
- 参考现有 `list_workflow_plugins`（`commands/sessions.rs` + `daemon/routes/sessions.rs:245`）和 `subagent:finished`（`stores/subagentRuns.ts`）的模式，C2 的 IPC + 事件设计是否符合 everlasting 现有惯例？有没有画蛇添足或遗漏的环节？
- 事件推送方案（主 LLM 写完 review-state.json → 后端发事件 → 前端 transport.on → 重新 invoke 刷新）这个链路有没有时序问题（如事件早于文件落盘到达、重复触发、丢失）？
- `useReviewStateStore`（§6）的生命周期管理（start/stop + task_slug 变化时重新订阅）有没有内存泄漏或孤儿监听风险？

### 维度 3：C2 implement Phase 0 的两个待确认点，你怎么看？

C2 implement.md Phase 0 列了两个我自己没拍板的点，请你给方案：
- **task_slug 获取**：前端 ChatPanel 要拿当前 task 的 slug 才能调 get_review_state。后端有 `resolve_current_task`（inject.rs:229）但前端可能没暴露。是该新加 `get_current_task_slug` IPC，还是从现有 workflow state IPC 暴露？有没有更轻量的办法？
- **review-state-updated 事件发送点**：在 workflow transition 钩子发（review 专用，零污染）还是在 write_file 工具后识别路径发（通用但要校验）？everlasting 有 transition 钩子机制吗（看 `agent/workflow/state.rs`）？你倾向哪个？

## 评审要求

- 直接、批判性。发现不一致就明确说「C3 写 X，C2 读 Y，会断裂」。
- 如果某个机制在 everlasting 现有代码里已有先例可复用，请指出具体 file:line。
- 不要泛泛而谈「设计很好」「建议加强测试」——要具体的、可执行的反馈。
- 如果你判断某个维度没问题（如 schema 完全对齐），也明确说「维度 N：已对齐，无问题」，不要省略。

--- 提示词正文 结束 ---
