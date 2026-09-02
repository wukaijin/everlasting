# Chat 任务面板:subagent + background shell + checklist 合并

## Goal

在 chat 界面提供一个统一的"运行状态"浮层面板(现进度清单 ChecklistCard 的进化版),一眼看到当前会话里:

1. 正在运行 / 已结束的 **subagent**(名称、状态、模型;点击打开现有 SubagentDrawer);
2. 正在运行 / 已结束的 **background shell**(命令、状态、exit code;点击展开输出预览);
3. 原有的 **进度清单**(checklist)作为面板的一个 section。

用户原话留了设计自由度("可以重新设计checklist 合并,也单独弄");**决策:合并为一个面板**(三个浮层会堆叠杂乱;三者都是"这一轮在跑什么"的同一心智)。checklist store 逻辑不动,只动渲染层。

## Background

- subagent 生命周期数据已全链路到前端(DB 表 + `subagent:event`/`subagent:finished` 事件 + `subagentRuns` store + SubagentDrawer),只缺汇总面板 UI。
- background shell 后端功能完整(`background_shell` registry + run_background_shell/shell_status/shell_kill 三工具),但**无 IPC 命令、无事件暴露给前端**,是本任务的主要新增面。

## Requirements

### R1 统一面板(合并 ChecklistCard)

- 新面板组件替换 ChatPanel 中的 ChecklistCard,保持现有浮层定位(右下、输入框上方)与两态(展开/最小化浮球)交互。
- 三个 section:**子代理**、**后台命令**、**清单**;空 section 不渲染(不占高度)。
- 可见性:任一 section 有数据才显示;只有 checklist 时外观等价于今天的卡片(加 section 标题)。
- 最小化浮球聚合展示:运行中数量徽标 + checklist `done/total`;有运行中内容时保留呼吸圈提示。
- 自动展开:面板首次从无到有时自动展开;用户手动最小化后尊重其选择(沿用现行为)。

### R2 子代理 section

- 每行:状态图标(running 旋转 / completed 勾 / error 叉 / cancelled·incomplete 中性)、`subagentName`、模型 chip(`modelDisplay`,null = 继承父级不显示 chip)、时长。
- 排序:running 优先,其余按开始时间倒序。
- 点击行 → 调 `subagentRuns.openDrawer(runId)` 打开现有 SubagentDrawer。
- 数据源:`subagentRuns.runSummaryBySession`,实时性由该 store 现有 listener 保证(面板不重复订阅)。

### R3 后台命令 section

- 每行:状态图标(running / completed / failed / killed / timed_out / spawn_failed)、命令文本(mono 截断)、running 行显示 elapsed、终态行显示 duration + exit code chip。
- 点击 running 行内联展开(无输出可看时仅提示);点击终态行内联展开 stdout/stderr 预览(preview 已有 ≤1KiB head+tail)。
- 数据源:新 store,经新 IPC `list_background_shells` + 新事件 `background_shell:update` 实时增量维护。
- running 行提供终止按钮(复用 registry kill,新 IPC `kill_background_shell`);失败静默 toast,不影响面板状态(终态以事件/重拉为准)。

### R4 后端:后台 shell 可观测性(新增)

- `list_background_shells(session_id)` IPC(daemon route + tauri command 双路径):返回该 session 全部未 prune 条目的摘要(命令、状态、时间、exit code、输出预览)。
- registry 新增 `list_for_session` trait 方法。
- 新事件 `background_shell:update`:在 started / exited(含 kill、超时、spawn 失败)/ pruned 时发射,payload 含完整摘要(pruned 只含 id);经 SseRegistry(daemon)与 AppHandle.emit(Tauri Full 模式)双路径下发,与 subagent 事件同模式。

### R5 行为与兼容约束

- checklist store(`stores/checklist.ts`)零逻辑改动;`update_checklist` 事件流、rehydrate、clear 语义全部不变。
- 时间显示只用 elapsed/duration(`started_at` 是进程单调毫秒,禁止当墙钟或与 `Date.now()` 混算)。
- session 切换:面板数据随 session 切换;session 删除时新 store 与 checklist 同点清理。
- z-index 层级不变(50,低于 modal 层)。
- Thin(daemon)与 Full(tauri transport)两模式均工作;事件 wire 形状两路径一致。

## Non-Goals

- 不做后台 shell 的实时 stdout 流式(仍靠完成后的 preview / spill 文件)。
- 不做 subagent 的任何新后端能力(纯 UI 消费)。
- 不做面板的拖拽/位置记忆/置顶开关。
- 不新增 Playwright e2e 用例(vitest 组件/ store 测试覆盖)。

## Acceptance Criteria

- [ ] AC1 仅 checklist 存在时,面板渲染与现 ChecklistCard 等价(展开浮层 + 浮球 + 全部交互),`pnpm test` 全绿。
- [ ] AC2 派发 subagent 后,面板出现 running 行(名称+模型+旋转);结束后翻终态;点击行打开 SubagentDrawer 且内容正确。
- [ ] AC3 `run_background_shell` 启动后,面板出现 running 行;命令退出后翻终态并显示 exit code;点击行展开 stdout/stderr 预览;1h prune 后条目消失(事件驱动)。
- [ ] AC4 daemon 模式(remote/HTTP transport)与 tauri 模式下事件均可达(代码路径双接线 + 单测覆盖 payload 形状)。
- [ ] AC5 running 行终止按钮可 kill 进程组,行翻 killed 终态。
- [ ] AC6 切换 session 面板数据切换;删除 session 无残留、无报错。
- [ ] AC7 后端 `cargo test -p everlasting --lib` 与前端 `cd app && pnpm test` 全绿;新增 Rust 单测覆盖 `list_for_session` + 三类事件发射。

## Notes

- 设计细节与数据流见 `design.md`;执行清单见 `implement.md`;现状架构调研(含 file:line)见 `research/existing-architecture.md`。
