# subagent 多模型 C:per-agent 默认模型 UI 配置 + builtin DB override + 写回 frontmatter

## Goal

让每个 subagent(researcher / general-purpose 等内置 + 用户自定义)都能在 **Settings UI** 里配一个默认模型,而不必手写 frontmatter 的 UUID——builtin agent(无文件)走 DB override,user/project agent 写回 frontmatter。补齐 B6+ A 留下的两块缺口:① 内置 agent 无法配 model;② frontmatter 写 `model: <UUID>` 对人类极不友好。

## 背景与动机

A 任务(07-03)落地了 frontmatter `model:` 声明 + catalog 穿透,但留了两个痛点(见上一任务 prd 的 Out-of-scope 与 ROADMAP §117 B6+ C):

1. **内置 agent 无法配 model**:`researcher` / `general-purpose` 硬编码在 `mod.rs::builtin_subagents()`,`model: None` 写死,没有 frontmatter 文件可改,dispatch 时也无法传 model(B 未做)。
2. **UUID 不友好**:frontmatter `model:` 认 `models.id`(UUID),人类手写易错、不可读。

本 task(C)用 **UI 下拉(display_name↔id 自动映射)+ DB override(给无文件的 builtin)+ 写回 frontmatter(给有文件的 user/project)** 一次性解决两者。

## 已敲定决策(brainstorm 结论)

- **优先级**:`DB override > frontmatter > parent`。UI 全局偏好优先,覆盖文件声明。
- **DB 作用域**:**全局**(`agent_name → model_id`),所有 project 共享。
- **范围**:**完整 C** —— builtin DB override + user/project 写回 frontmatter + UI 展示所有 subagent 的 model。
- **可观测性并入**(用户 2026-07-03 确认):card/drawer 显示 worker model(prd AC13-15 + design §10 + implement 阶段 1b / 4.6 / 4.7)**并入本 task,不拆独立 task** —— 虽独立于 DB override(实为补全 A 任务的可观测性),但共享 `resolve_final_model` 路径,拆 task 的合并冲突风险 > 管理成本。

> 技术设计见 `design.md`,执行清单见 `implement.md`。

## 范围

### In-scope

1. **DB**:新表 `subagent_model_overrides(agent_name PK, model_id, updated_at)` + `get/set/clear/list` CRUD(全局作用域)。
2. **优先级接入**:`run_subagent` 前置 `resolve_final_model`(DB override > frontmatter),收敛后仍喂 A 任务的 `resolve_worker_provider`(**后者不动**)。
3. **frontmatter 写回**:`loader` 加行级 `apply_model_line` + 原子写 `write_frontmatter_model`,保留 body / 注释 / 其余字段顺序;user/project agent 改 model 时写回文件,下一 turn 由 mtime-fenced cache 自动重读。
4. **IPC**:`list_subagents(project_path)`(cache.list + DB 叠加 + display_name 映射)+ `set_subagent_model(name, source, project_path, model_id)`(builtin→DB / user|project→文件)。
5. **UI**:Settings 新增 **Subagents tab**,列出 builtin + user + project 所有 agent,per-row model 下拉(`继承父级` + 按 provider 分组的 model,显示 display_name),改 builtin 标注 DB override,失效 model 标红。
6. **可观测性(card / drawer)**:`subagent_runs` 加 `model_display` 列,`run_subagent` resolve 后写入 worker 实际用的 model display;`SubagentRunSummary` 携带;chat 流 dispatch_subagent **card** 折叠预览加 model chip(复用 `workerTokenText` 模式);**SubagentDrawerHeader** 在 name 旁显示 model。**与 C 主体解耦**——其实补全 A 任务的可观测性(A 已 resolve 出 display 但没持久化/没展示),但共享 `resolve_final_model` 路径,故并入本 task。
7. **清理**:A 任务残留的过时注释(`loader.rs:119-121` "model warned-and-discarded",现已 STORED)。

### Out-of-scope(留 ROADMAP 或 follow-up)

- **B**:dispatch 时动态选模型(`dispatch_subagent` 加 `model` 参数 + `@@agent --model=`,优先级 dispatch > DB > frontmatter > parent)→ 仍是 `B6+` B,独立 task。
- **DB override 的 per-project 作用域**:本 task 只做全局;per-project 留 follow-up。
- **删 model 时级联清理 override**:靠 catalog miss + parent fallback 兜底,级联清理低优 follow-up。
- **UI 热跟 project 切换**:Settings 打开时锁定当前 project 快照,切 project 不热更(MVP)。
- **frontmatter display_name 直接写**:frontmatter 仍存 id(UI 代劳映射);支持写 display_name 留 follow-up。

## 约束

- **不改** `ProviderCatalog` 结构 / `run_chat_loop` 23 参签名 / `resolve_worker_provider` 签名与 6 个既有测试(A 任务零回归)。新优先级经前置 `resolve_final_model` 注入。
- **不引** YAML 序列化库:frontmatter 写回走行级编辑(保留原字节)。
- **不破坏** `dispatch_subagent` tool_use/tool_result 配对不变量(RULE-A-007)、worker 取消传播、worktree 隔离决策、`[model: <name>]` 信号行(A 任务 AC5)。
- 写回 frontmatter 必须**原子写**(`.tmp` + rename),防中途崩损坏 agent 文件。
- 全中文 user-facing 文案。

## Acceptance Criteria

- [ ] **AC1**:UI 给 builtin `researcher` 选一个非默认 model X → dispatch researcher,worker LLM 请求实际打到 X 的 provider(≠ parent),`tool_result` 的 `[model:]` 行显示 X。
- [ ] **AC2**(优先级 DB > frontmatter):同一 agent name 同时有 DB override=X 和 frontmatter `model: Y` 时,worker 用 X。
- [ ] **AC3**(frontmatter > parent):user/project agent 写 frontmatter `model: Y`、无 DB override 时,worker 用 Y。
- [ ] **AC4**(都无 → parent):agent 既无 DB override 也无 frontmatter model 时,行为与现状完全一致(继承 parent provider + ctx)——builtin 默认 / user agent 不写 model 零回归。
- [ ] **AC5**(全局作用域):在不同 project 下 dispatch 同一 builtin,DB override 一致生效。
- [ ] **AC6**(写回 frontmatter):UI 改 user/project agent 的 model → 对应 `.md` 文件的 `model:` 行被更新(或插入/删除),body 与其余 frontmatter 字段、注释、顺序原样保留;原子写(中途不产生半截文件)。
- [ ] **AC7**(cache 刷新):写回 frontmatter 后,下一 chat turn 的 `SubagentCache` mtime-fenced 扫描重读,新 model 生效(无需 reload 命令)。
- [ ] **AC8**(inherit 回退):UI 把 agent 设回"继承父级"→ builtin 清 DB override(DELETE)、user/project 删 frontmatter `model:` 行;之后 worker 继承 parent。
- [ ] **AC9**(失效 model 兜底):DB override 指向已删除的 model_id(catalog miss)→ dispatch 不失败,`resolve_worker_provider` warn + parent fallback;UI 该行标红提示"模型已删除,将降级"。
- [ ] **AC10**(context_window / thinking 跟随):worker 最终 model 命中时,`context_window` 按该 model 计算;thinking 块按该 provider caps(复用 A 任务机制,加测验证无泄漏)。
- [ ] **AC11**(id↔display 友好):UI 下拉显示 model display_name,值是 id;用户全程不接触 UUID。
- [ ] **AC12**(回归):`tests_subagent.rs` / `tests_agent_loop.rs` / `resolve_worker_provider` 既有用例全绿;新增用例覆盖 AC1-10。
- [ ] **AC13**(model_display 持久化):worker dispatch 后,`subagent_runs.model_display` 记录 worker 实际用的 model display —— **仅 catalog hit 时记 display,parent 继承 / catalog miss 时记 NULL**(语义与 tool_result `[model:]` 行一致;不改 `run_subagent` 签名);前端据此显示 model 或「继承父级」。
- [ ] **AC14**(card 显示):chat 流 dispatch_subagent card 折叠预览显示 model chip(从 `workerSummary.modelDisplay`,复用 `workerTokenText` 模式);preview 文本 strip 掉 `[model:]` 行避免与 chip 重复;run 进行中(row 未落地 / modelDisplay=null)时 chip 隐藏,不报错;legacy 旧 row(modelDisplay=null)不报错。
- [ ] **AC15**(drawer 显示):SubagentDrawerHeader 在 name 旁显示 model(从 `run.modelDisplay`);null 时不渲染。
- [ ] **AC16**(全绿):`cargo test --lib`(带 `PKG_CONFIG_PATH`)+ `vue-tsc --noEmit` + `vitest run` 全绿。

## Notes

- **前序任务**:本 task 是 `07-03-subagent-frontmatter-model`(A)的续作 —— `resolve_worker_provider` / catalog 穿透 / `[model:]` 行均来自 A。A 的 prd/design 在 `.trellis/tasks/archive/2026-07/07-03-subagent-frontmatter-model/`。**以实际代码为准**:A design §113/118 设想的 "parent display thread 到 run_subagent" **未落地**(实际 `resolve_worker_provider` parent 继承返回 `None`),故本 task `model_display` 在 parent 继承时记 `NULL`(见 design §10.1)。
- **必读 spec**:核心是 `.trellis/spec/backend/subagent-runs-schema.md`(阶段 1b 加 `model_display` 列 + 阶段 5 同步更新其 "XXX additions" 段,以 `worktree_path` 列为范例)+ `agent-loop-architecture.md` + `frontend/chat.md`;完整清单见 `implement.jsonl`(sub-agent 模式自动注入,inline 模式手动读)。
- 优先级链 / DB schema / 写回策略 / IPC 契约 / 前端结构 / 边界 / 回滚见 `design.md`。
- 分阶段执行(0 DB → 1 优先级 → **1b 可观测性** → 2 写回 → 3 IPC → 4 前端[Settings + card + drawer] → 5 收尾)+ 每阶段验证命令见 `implement.md`。
- B(dispatch 动态选模型)留独立 task;per-project 作用域 / model 删除级联 / display_name 直写 / parent display thread 留 follow-up。
