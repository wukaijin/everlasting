# Per-Agent Model UI + Per-Dispatch Override (B6+ B/C)

> **Source**: extracted from `frontend/chat.md` §"B6+ C — per-agent model UI + worker model 可观测性 (2026-07-03) / B6+ B — per-dispatch model override via @@agent --model= (2026-07-07)" (2026-08-10 doc-split task).

## B6+ C — per-agent model UI + worker model 可观测性 (2026-07-03)

`subagent_runs.model_display TEXT NULL` 列由后端 `dispatch.rs::run_subagent` 写 `resolve_worker_provider` 返回的 `Option<String>`(catalog hit = Some(display);parent 继承 / catalog miss = None → NULL,见 `subagent-runs-schema.md` "B6+ C additions" 段)。前端两个 chip 与一个 Settings tab 依赖该列。

### ToolCallCard dispatch 分支 — `workerModelText` chip

仿既有 `workerTokenText` 模式,在 card 折叠预览顶部加 model chip:
- computed `workerModelText = workerSummary.value?.modelDisplay ?? ""`
- 模板:token chip 旁 `<Icon name="cpu" :size="12"/> {{ workerModelText }}`,`v-if="workerModelText"`(空/legacy null 隐藏)
- `workerSummaryPreview` 的 fallback 分支(fallback 用 `props.result.content`)**必须 regex strip 掉 `[model: …]` 行**(并顺带 strip `[status: …]` 前缀,与 summary 路径对齐),避免与 chip 重复显示

### SubagentDrawerHeader — `run.modelDisplay` chip

main drawer 已透传 `run`,header 在 name 旁(或 meta 行)直接读 `run?.modelDisplay`,mono 小字,`v-if` 守护 null。**纯展示组件,无需新增 prop**。详见 `SubagentDrawerHeader.vue` 的 `.subagent-drawer__model` CSS。

### Settings → SubagentsTab (per-agent model config)

新建 `app/src/components/settings/SubagentsTab.vue`(与 `MemoryTab.vue` 同级)+ `app/src/stores/subagents.ts`。`SettingsModal.vue` 加第 5 个 `TabsTrigger value="subagents"`。

数据源:IPC `list_subagents_with_model(project_path)`(`commands::subagents::list_subagents_with_model`)→ `SubagentWithModelRow[]`,字段含 `resolvedModelId` / `resolvedModelDisplay` / `hasDbOverride` / `writable`(source!=builtin)。下拉数据复用 `useModelsStore.modelsGroupedByProvider`,label = `display_name`,value = `id`(UI 友好,人类不接触 UUID)。

per-row spinner:仿 `WorkerMergeControls` 的 reactive Map 模式(本任务用 `spinnerByName: reactive(Map<string, SpinnerState>)`),`finally` 清,防双击 + 二次 click 短路。

失效 model 兜底:DB override 指向已删 model(catalog miss)→ 下拉显示该 id + 红字"模型已删除,将降级";dispatch 走 `resolve_worker_provider` warn + parent fallback,不报错。

## B6+ B — per-dispatch model override via `@@agent --model=` (2026-07-07)

`chat.ts::send()` 的 `@@agent <task>` 前缀解析扩展为 `@@agent [--model=<X>] <task>`(`parseForcedDispatchPrefix` 纯函数,导出可单测)。flag 位置必须紧跟 agent 名之后、task 之前(git/cargo flag 语义);task 中间的 `--model=` **不**误解析(整段当 task)。`<X>` 支持两种值,经 `resolveModelInput(raw, models)` 纯函数反查 `useModelsStore().models`:① 精确 id 直返;② display_name 匹配取首(多同名 `console.warn`);③ 未命中 → 返 `undefined` + `console.warn`(dispatch 走 agent 默认,**不报错、不弹 toast**;raw `--model=` 文本留在输入框可改正重发)。

wire 形状:`ForcedDispatchPayload = { subagent, task, model_id? }`,字段 **snake_case**(`model_id` 非 `modelId`)—— 嵌套 IPC struct 字段经 serde verbatim,不像顶层 Tauri command arg 那样 auto-camel(项目惯例:顶层 arg camelCase 如 `forcedDispatch` / `resendSeq`;嵌套 struct 字段 snake_case 跟 Rust struct 一致)。`streamController.ts::StartRequestArgs.forcedDispatch` 类型同步加 `model_id?: string`。后端 `ForcedDispatch` 加 `#[serde(default)] model_id: Option<String>`,旧前端(无该字段)→ `None`(serde 容错)。

后端两条入口汇合:`run_subagent` 解析 `input.get("model")` —— LLM path 传 display_name(schema enum 值,见 `tool-contract.md` "B6+ B"),user `@@` path 传 id(前端已反查);统一经 `resolve_model_by_name_or_id(db, input)` 收敛成 id。优先级 `dispatch > DB > frontmatter > parent`,叠加逻辑见 `agent-loop-architecture.md` row 26。单测 `app/src/stores/chat.test.ts`(14 cases:flag 位置 / id+display_name 反查 / task 中间不误解析 / 未命中降级 / 多行 task 保留)。

### 数据流契约(per-row 改动)

```
User 改下拉
  → useSubagentsStore.setModel(name, source, modelId)
  → invoke('set_subagent_model', { name, source, projectPath, modelId })
    ├─ source=builtin  → db::set_subagent_model_override (UPSERT)
    │                   或 db::clear_subagent_model_override (None)
    └─ source=user|project → loader::write_frontmatter_model (atomic .tmp+rename)
                              → SubagentCache mtime-fenced 自动重读
  → 返回最新 SubagentWithModelRow → store 局部更新该行(spinner 隔离)
```

### Common Mistakes

- **不要从 `[model:]` tool_result 行解析 model** — A 任务加的 `[model: X]` 信号行是给 parent LLM 看的(格式属实现细节),前端解析脆弱。**唯一权威**是 `subagent_runs.model_display` 持久化列。
- **preview 重复显示** — 漏 strip `[model:]` 行会让 chip + preview 同时显示 model,user-visible 重复。
- **legacy row `modelDisplay=null` 未守护** — 旧 row 没该列(frontmatter 模型声明已有但持久化列是新加的),UI 任何 `.modelDisplay.toLowerCase()` / 等假设 non-null 的访问会崩。必须 `v-if` 或 `?? ""` 兜底。
- **`LoadedSubagent` 加 `file_path` 字段** — 设计决策明确**不**改 struct(避免污染 cache 抽象),文件路径由 `locate_agent_file(source, name, project_path)` helper 复用 loader 既有路径常量(`AGENTS_SUBDIR` / `PROJECT_NAMESPACE`)推导。

### Tests required

- `SubagentsTab.test.ts`(若易 mock IPC):渲染列表 + 下拉 + 改动调 IPC + inherit 选项 + 失效 model 标红
- `ToolCallCard.test.ts`: fixture `modelDisplay: null` 兜底 + chip 显示/隐藏 + preview strip `[model:]` 行
- `SubagentDrawer.test.ts` / `SubagentDrawerHeader.test.ts`: drawer header chip 显示/隐藏
- `WorkerMergeControls.test.ts`: fixture `modelDisplay: null`(同源 summary 类型)
- `subagentRuns.test.ts`: `SubagentRunSummary` / `SubagentRunRow` 含 `modelDisplay` 字段

### 设计决策完整版

见 `IMPLEMENTATION.md` §4 "2026-07-03 — B6+ C" D1-D6 决策日志 + `.trellis/spec/backend/subagent-runs-schema.md` "B6+ C additions" 段 + `.trellis/spec/backend/agent-loop-architecture.md` `run_chat_loop` 参数表 row 25(B6+ C 决策)。本节仅为前端 cross-ref 锚点。

