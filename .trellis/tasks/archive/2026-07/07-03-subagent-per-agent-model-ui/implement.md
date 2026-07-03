# Implement — subagent 多模型 C

> 执行计划。技术设计见 `design.md`。每阶段有独立验证命令 + review gate。不拆 parent/child(三块耦合:前端依赖后端 IPC,单任务分阶段更顺)。

## 全局约定

- Rust 命令统一带 `PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig"`(项目 HACKING-wsl)。
- 每阶段结束跑该阶段测试 + 上一阶段回归,绿才进下一阶段。
- 全中文 user-facing 文案(项目惯例)。

---

## 阶段 0 — DB 层(地基,零依赖)

- [ ] 0.1 `db/migrations.rs` 加 `subagent_model_overrides` 建表(走 `CREATE TABLE IF NOT EXISTS` 幂等模式,对齐 `models` 首建段)。
- [ ] 0.2 新模块 `db/subagent_overrides.rs`:`get_ / set_(UPSERT) / clear_ / list_` 四个 CRUD 函数 + `mod.rs` re-export。
- [ ] 0.3 `db/tests_subagent_overrides.rs`(或并入既有 `db/tests_*` 域):get/set/clear/list + set 两次(UPSERT 覆盖)+ clear 不存在行不报错 + list 顺序无关。

**验证**:
```bash
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib subagent_overrides
```

**Review gate 0**:CRUD 行为正确,UPSERT 幂等。

---

## 阶段 1 — 优先级接入(核心逻辑)

- [ ] 1.1 `agent/subagent/dispatch.rs` 加纯函数 `resolve_final_model(db, name, def_model) -> Option<String>`:`db_override(name).or(def_model)`。注释标注"DB > frontmatter > parent"决策来源。
- [ ] 1.2 改 `run_subagent`(catalog read lock 段 `dispatch.rs:549-562`):先 `resolve_final_model` 再喂 `resolve_worker_provider`。`resolve_worker_provider` 本身**不动**。
- [ ] 1.3 `tests_subagent.rs` 增优先级用例:
  - DB + frontmatter 都有 → DB 胜
  - 仅 frontmatter → frontmatter
  - 仅 DB → DB
  - 都无 → parent
  - DB 指向失效 model(catalog miss)→ `resolve_worker_provider` 走 frontmatter 兜底?**注意**:失效 DB override 在 `resolve_final_model` 仍返回 Some(它不查 catalog),miss 由 `resolve_worker_provider` 处理 → 直接 parent(不会回退 frontmatter)。这是当前决策的推论,测试锁定该行为 + 注释说明。

**验证**:
```bash
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib resolve_final_model
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib resolve_worker_provider   # 回归 A 任务,零改动应全绿
```

**Review gate 1**:优先级正确;`resolve_worker_provider` 既有 6 测试零回归。

---

## 阶段 1b — 可观测性:`subagent_runs.model_display` 持久化

> 与阶段 1 共享 `resolve_final_model` 输出,但独立可验证(prd AC13-15;不依赖 DB override)。

- [ ] 1b.1 `db/migrations.rs` 加 `model_display TEXT NULL` 列(`add_subagent_runs_column_if_missing`,同 `task` / `final_text` 模式)。
- [ ] 1b.2 `db/subagent_runs.rs`:`insert_run_with_id` 带 `model_display` 入参;`list_subagent_runs_by_session` / `get_subagent_run` SELECT 带上该列;`SubagentRunSummary` 后端 struct 加 `model_display`(camelCase 序列化)。
- [ ] 1b.3 `run_subagent`(dispatch.rs):把 `resolve_worker_provider` 返回的 display(**第三项 `Option<String>`**)写入 run(`insert_run_with_id` 调用处带上)。**直接用该 Option**:catalog hit → `Some(name)`;parent 继承 / catalog miss → `None` → 写 `NULL`。**不改 caller、不改 `run_subagent` 签名**(parent display 不 thread,见 design §10.1 / prd AC13)。
- [ ] 1b.4 前端 `stores/subagentRuns.types.ts`:`SubagentRunSummary` 加 `modelDisplay: string | null`(容忍 legacy null)。
- [ ] 1b.5 测试:catalog hit → run.model_display = 该 model display;parent 继承 → `NULL`;catalog miss 降级 → `NULL`(语义同 tool_result `[model:]` 行省略);前端 type 对齐(`vue-tsc`)。

**验证**:
```bash
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib subagent_runs
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo check
cd app && pnpm vue-tsc --noEmit
```

**Review gate 1b**:run 行带 model_display;前后端类型对齐;既有 subagent_runs 测试零回归。

---

## 阶段 2 — frontmatter 写回

- [ ] 2.1 `agent/subagent/loader.rs` 加纯函数 `apply_model_line(content: &str, model_id: Option<&str>) -> String`(行级编辑,便于单测,不碰 IO)。
- [ ] 2.2 加 IO 包装 `write_frontmatter_model(path, model_id: Option<&str>) -> io::Result<()>`:读 → `apply_model_line` → 原子写(`.tmp` + rename,复用 `files.rs` 惯例)。
- [ ] 2.3 loader 测覆盖:纯函数 `apply_model_line`(已声明替换 / 未声明插首行 / None 删除 / 保留 body + 注释 + 其余字段顺序)+ `write_frontmatter_model` IO(无 fence 返错 / 原子写 `.tmp`+rename)。
- [ ] 2.4 loader 暴露"由 name + source + project_path 定位文件路径"的能力(若 `SubagentCache` 已有路径信息则复用,否则加最小定位 helper,供阶段 3 IPC 用)。

**验证**:
```bash
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib apply_model_line
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib loader
```

**Review gate 2**:写回保留原格式;无 fence 边界正确。

---

## 阶段 3 — IPC

- [ ] 3.1 `commands/` 下 `list_subagents(project_path) -> Vec<SubagentListModel>`:cache.list + DB override 叠加 + display_name 映射(一次 `list_subagent_model_overrides` + 按需 `get_model`)。
- [ ] 3.2 `set_subagent_model(name, source, project_path, model_id: Option<String>) -> SubagentListModel`:source 分发 DB / 文件;返回最新行。
- [ ] 3.3 `lib.rs` 注册两个 command。
- [ ] 3.4 command 层测试(builtin→DB / user→文件,仿既有 command 测试 harness)。

**验证**:
```bash
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo check
```

**Review gate 3**:IPC 端到端;`cargo check` 通过(command 注册完整)。

---

## 阶段 4 — 前端 SubagentsTab + card/drawer model 显示

- [ ] 4.1 新建 `stores/subagents.ts`(per-agent model 配置,职责与 `subagentRuns.ts` 的 run 状态不同,故独立):`listSubagents` / `setSubagentModel` actions + 列表状态。
- [ ] 4.2 `components/settings/SubagentsTab.vue`:列表 + per-row model 下拉(inherit + provider group)+ source chip + DB override 标注 + 失效 model 标红。
- [ ] 4.3 `SettingsModal.vue`:加 `subagents` TabsTrigger + TabsContent + import。
- [ ] 4.4 `utils/`:若需 display_name 映射 helper 放此(否则复用 `useModelsStore`)。
- [ ] 4.5 vitest(若易 mock IPC):渲染 + 下拉 + 改动调 setSubagentModel + inherit。
- [ ] 4.6 `components/chat/ToolCallCard.vue` dispatch 分支:加 `workerModelText` computed(`workerSummary.value?.modelDisplay ?? ""`)+ 模板 model chip(仿 `workerTokenText`,`<Icon name="cpu" :size="12"/> {{ workerModelText }}`,`v-if="workerModelText"` 空隐藏)。
- [ ] 4.7 `components/chat/SubagentDrawerHeader.vue`:title-row name 旁加 model 显示(读 `run?.modelDisplay`,mono 小字,`v-if` 守护 null;无需新 prop,main drawer 已透传 `run`)。

**验证**:
```bash
cd app && pnpm vue-tsc --noEmit
cd app && pnpm vitest run
```

**Review gate 4**:`vue-tsc` 0 err;vitest 全绿;UI 手测(builtin 改 DB / user 改文件 → 下一 turn 生效)。

---

## 阶段 5 — 收尾

- [ ] 5.1 全量回归:`cargo test --lib` + `vue-tsc --noEmit` + `vitest run`。
- [ ] 5.2 清理 A 任务留下的过时注释(`loader.rs:119-121` "model parsed but warned-and-discarded" — 现已 STORED,文档漂移)。
- [ ] 5.3 spec 更新(`tool-contract.md` / `agent-loop-architecture.md` 优先级链 + 新表;`frontend/chat.md` 或 settings spec 加 SubagentsTab)。
- [ ] 5.4 ROADMAP §117 B6+ C 标记完成 + §1.2 加条目。
- [ ] 5.5 IMPLEMENTATION §4 决策日志。
- [ ] 5.6 commit + archive。

## Rollback points

| 阶段 | 回滚动作 |
|---|---|
| 0 | DROP TABLE `subagent_model_overrides`(无数据依赖) |
| 1 | 删 `resolve_final_model` + caller 还原 `def.model.as_deref()`(一行) |
| 2 | 删 `apply_model_line` / `write_frontmatter_model`(纯新增) |
| 3 | 注销 2 command + 删 store action(编译即暴露) |
| 4 | 删 `SubagentsTab.vue` + Settings 一处 import |

各阶段独立可回滚,无破坏性变更。
