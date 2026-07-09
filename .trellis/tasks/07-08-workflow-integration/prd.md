# Workflow 集成:工作流引擎 + plugin(dev/review)

## Goal

给 Everlasting 加一个 **session 级 opt-in 的"工作流引擎"**。打开后,agent 不再随机发挥,而是按一个**可切换的 workflow plugin**(状态机 + skill + sub-agent + 沉淀闭环)驱动的标准流程干活;长跑下来自然沉淀出项目 spec 规范。

**架构核心:engine 与 content 分离。** Rust 后端只提供 engine(注入 seam、门控、state 转移、UI 切换);workflow 的"内容"——state 枚举、breadcrumb 文本、角色映射、协调模型——是 plugin,落在 `.everlasting/` 文件态,项目可改可换。一个 Rust engine 承载多个结构不同的 workflow plugin。

**MVP plugin = `dev`**(planning→implement→check→done,调研/实施/验收角色分工)。**愿景 plugin = `review`**(多 sub-agent 评审,延迟讨论)。

设计文档:`docs/WORKFLOW-INTEGRATION.md`(两轮评审已过,9 个 Q 对齐 + S1-S3/M1-M7 + S-A/S-B/M-A-M-E 全部修正)。

## 背景

- 现状:工作流的 ~80% 零件已落地(B6 subagent / B12 checklist / B4 skill / C4 audit / memory_recall 注入 seam),但被 agent **随机使用**——缺一个机制把它们串成标准流程
- 参考:[Trellis](https://github.com/mindfold-ai/Trellis) 跑在本项目管开发,其 task 元数据 + state machine + skill + sub-agent + spec 沉淀架构是借鉴对象(借鉴机制 + 可定制性,不是 UI)
- 用户诉求:轻量 TDD、多 agent 协作、协作文档类似 Trellis、过程可视可观测、自动化机制注入项目执行规范让 AI 每次按标准写代码

## 已确认事实(设计文档对齐结论)

| 事实 | 来源 |
|---|---|
| 主角是"机制"不是 task;task 是 agent 自动产出的文件态记账副产物,用户不感知不操作 | 设计文档 §3.2 + §10.2 |
| session 不绑 task,task 不入 DB;task 纯文件态(`.everlasting/tasks/<slug>/`) | §6.2 + §10.3 |
| workflow 是 session 级开关(`sessions.workflow_enabled` 列),正交于 Mode(edit/plan/yolo) | §5 + §10.4 |
| 元流程 state(planning/implement/check/done)固定枚举 + 实施阶段 LLM 在 planning 拆进 task.json.items | §6.1 + §6.2 + §10.5 |
| state 转移用户确认门,agent 不能自翻;走专用 IPC `resolve_task_state_transition`(对标 `resolve_mode_change` 双 IPC pattern) | §8 + §10.6 + M-A |
| 注入一律 append 到 per-turn messages[0],`cache_control: None`,绝不新开 user message | §10.7 + S-B |
| UI workflow 切换 ≠ task picker;选 plugin 不选 task | §5.5 + §10.8 |
| plugin 配置用 JSON(`workflow.json`),非 markdown | Q1 |
| Phase 0 就预留 plugin 接口(WorkflowDef struct + 4 访问函数),数据源先硬编码常量,Phase 2 换读 workflow.json | Q2 + §5.4 |
| 门控违反统一走协商(engine 主动 ask_user_question),不分角色/state,无硬拒档 | Q3 + S3 |
| 不自动 inject skill body(保 B4 三层渐进披露),靠 breadcrumb 提示 + agent use_skill | Q4 |
| workflow plugin 自带 `agents/` + `skills/` 子目录;plugin agents/skills 在 workflow session 优先于全局;builtin 不动 | Q5 + §6.5 |
| delegation 模板:task meta(`{title}`/`{summary}`/`{state}`/`{relevant_specs}`,engine 填)+ plugin 模板指导主 LLM 写委托语;不用 `{current_item}` | Q6 + M-B |
| spec 沉淀进新目录 `.everlasting/spec/`(职责独立于 `.trellis/spec/`) | Q7 |
| 评审流(review plugin)延迟讨论;立项 trigger = dev 跑通 ≥1 完整 task + 沉淀 spec 起见过收益 | Q8 |
| hook 选 Rust 固定逻辑(嵌 `set_task_state` 写入路径);沉淀闭环不靠 agent 自觉;不做脚本 runner | Q9 |
| 门控下沉到 `run_subagent` 内部(三处调用点串行~3286/并发~2937/测试~1000 都过),签名追加 `current_state` | S-A + §6.6.2 |
| delegation 模板注入走 `inject_recall_into_turn` 同款 seam,但依赖 messages[0] 必有 user instruction(B5 默认满足);fallback prepend 破坏 cache 禁触发 | S-B + §6.6.1 |
| coordination 字段用 enum `{Pipeline, SynthesisRound}` + gather_strategy map(原 round-robin 语义错位) | M-C + §5.4 |
| task CLI 脚本(借鉴 Trellis task.py create/archive)种模板 + 归档,不走裸 write_file | §6.8 |

## Requirements

### 功能需求

1. **engine 骨架**:session workflow 开关 + WorkflowDef struct + 4 访问函数 + state breadcrumb 注入(复用 inject_recall_into_turn seam)
2. **第一个 plugin `dev`**:planning/implement/check/done 四态 + researcher/implementer/checker 三角色 + 沉淀闭环
3. **plugin 内容文件态**:`.everlasting/workflow/<name>/workflow.json` + `agents/` + `skills/`;项目可改,覆盖 builtin 默认
4. **task 文件态记账**:agent 自动产 `.everlasting/tasks/<slug>/`;无 task DB 表、无 task UI、用户不操作 task
5. **wf-* skill 包**:wf-overview / wf-brainstorm / wf-before-dev / wf-check / wf-update-spec(plugin 自带)
6. **沉淀闭环**:task done 时把决策/教训升级进 `.everlasting/spec/`
7. **任务交接**:task 跨 session 靠文件位置天然达成;新 session agent 读 progress.md + task.json.items 续上
8. **UI workflow 切换**:session 顶栏 workflow toggle + plugin 切换
9. **门控 + delegation 模板**:门控下沉 run_subagent 内部,统一协商档;delegation 模板 engine 注入(task meta 占位符 + plugin 模板)
10. **hook**:state 转移 Rust 固定 hook(done 触发沉淀);专用 IPC resolve_task_state_transition 触发
11. **task CLI**:create_task / archive_task / update_task command(非裸 write_file)

### 非功能需求

- 复用 `run_chat_loop` 26 参签名,不新开 loop 函数
- 复用现有注入 seam(memory_recall / checklist append),不破坏 prompt cache breakpoint
- opt-in:不开开关 = 现有行为零改动;workflow session 内的硬约束不污染普通 session
- 英文命名约束:plugin 目录/builtin 一律英文(`dev`/`review`)

## Acceptance Criteria

### Phase 0(engine 骨架)
- [ ] sessions 表加 `workflow_enabled` 列 + migration;前端读得到字段
- [ ] 顶栏 workflow toggle UI;点 toggle DB 列翻转,关 session 重开状态保持
- [ ] WorkflowDef struct + 4 访问函数 + `default_workflow()` 硬编码常量(dev 四态);cargo test 通过
- [ ] task.json 读写 + `.everlasting/tasks/<slug>/` 目录约定 + create_task command
- [ ] state breadcrumb 注入(append messages[0]);workflow session 发开发意图 → task.json 生成 → breadcrumb 可见
- [ ] 首次起 task 触发点:agent list_dir 找未完成 task 续上 / 无则判断开发意图起 task

### Phase 1(skill 规范包 + plugin skill loader)
- [ ] plugin skill loader(skill/loader.rs 加 workflow plugin skills 解析层)
- [ ] wf-overview skill body 完整(进 workflow 时 agent 加载,建立全局意识)
- [ ] wf-brainstorm / wf-before-dev / wf-check / wf-update-spec 4 个 skill body
- [ ] artifact 查阅:task.json 元数据+summary append messages[0];prd/design/progress 给 path

### Phase 2(plugin 外置 + sub-agent 角色 + 门控 + 注入)
- [ ] workflow.json 外置:default_workflow() → load_workflow() + validate + fallback
- [ ] UI plugin 切换(此时只有 dev 但机制就位)
- [ ] plugin agents/ 落地(researcher/implementer/checker frontmatter + body);loader 加 plugin agents 解析层
- [ ] 门控下沉 run_subagent 内部(三处调用点都过);签名追加 current_state;统一协商档
- [ ] delegation 模板注入(engine 填占位符 → append messages[0]);worker 读到角色规范 + relevant_specs
- [ ] checklist 同步(S2):update_checklist workflow session 内改写 task.json.items;跨 session 持久

### Phase 3(hook + 沉淀闭环)
- [ ] state 转移 Rust 固定 hook(set_task_state 加 match 分支);resolve_task_state_transition IPC 触发
- [ ] `.everlasting/spec/` 目录 + wf-update-spec 落地
- [ ] progress.md 交接叙述 + archive_task command
- [ ] task done → spec 写进 .everlasting/spec/;下次 implement 读得到;跨 session 续 task 完整闭环

## Out of Scope

- ❌ 任何 task UI(picker/list/board/切换器)—— task 是 agent 记账副产物
- ❌ task DB 表 / session↔task 关联 —— task 纯文件态
- ❌ B8 DAG 拓扑 / 依赖图 —— 留 B8(第四档)
- ❌ 强制全局 workflow / 强制全局 TDD —— opt-in / per-item
- ❌ workflow 绑定 Mode —— 正交,state 不替用户切 Mode
- ❌ 新建 session 强制选 task —— session 创建不被 task 打断
- ❌ 重写 agent core —— 复用 run_chat_loop 26 参
- ❌ 第二个 plugin `review` + 新通讯架构 —— 延迟讨论(Q8),dev 跑通 ≥1 task 再立项
- ❌ peer / 黑板协作模型 —— 选 orchestrator+pipeline

## Notes

- 设计文档:`docs/WORKFLOW-INTEGRATION.md`(单一引用源,两轮评审已过)
- 评审记录:`docs/WORKFLOW-INTEGRATION-REVIEW.md` + `WORKFLOW-INTEGRATION-REVIEW-2.md`
- 分步实施指导见设计文档 §9.5(Phase 0-3 拆 18 个可独立验证小步 + 依赖图)
- dev plugin 完整示例见设计文档 §A(workflow.json + 三角色 + wf-* skill body + 示例 task 目录)
- 风险最高步:Phase 2 步 2.4(门控下沉 run_subagent,改签名+三处调用点)、2.6(checklist 同步,改 B12 写路径)
