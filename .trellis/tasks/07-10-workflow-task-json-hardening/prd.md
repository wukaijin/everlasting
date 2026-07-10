# Workflow task.json 对 LLM 手写的健壮性加固

## Goal

让 workflow 的 `task.json` 对 LLM 的直接写入（`write_file`/`edit_file`）健壮，并补齐 LLM 合规的 task 写入路径，消除「LLM 一手写 task.json 就崩」的系统性缺陷，使 workflow 全流程（planning→implement→check→done）在单个 agent loop 内可自主跑通。

## 背景:两次崩的共同根因

在 precaution-frontend 项目用 workflow 测试 `improve-readme` task,agent 两次把 task.json 写崩,且都卡在 `read_task` 反序列化:

**第一次(planning 阶段)**:agent 用 `write_file` 手写整个 task.json(因为 `create_task` 只是 Tauri IPC,LLM 在 loop 内调不到),漏了 required 字段 `created_at`/`updated_at`,且 item 用了 `title`(应为 `content`)、`status:"pending"`(不在 `TaskStatus` 枚举)。→ `read_task` 失败 → `resolve_current_task` 静默跳过(per-file error swallow)→ `current_task` 永远 `None` → `request_task_state_transition` 报 `no active workflow task`。

**第二次(implement 阶段)**:planning→implement transition 成功后,agent 用 `write_file`/`edit_file` 手改 task.json 的 `items[].status` 为 `in_progress`(绕过 `update_checklist` 的安全映射 `InProgress→Implement`)。→ `read_task` 报 `unknown variant 'in_progress'` → `resolve_task_state_transition` IPC 失败。

**共同根因**:`task.json` 是 LLM 可写的普通文件,但无 schema 保护。`create_task`/`update_checklist` 做得再正确,LLM 只要一次 `write_file` 就能写崩。第一次手写建档漏字段、第二次手改进度写错枚举 —— 只要它还能 `write_file` task.json,就一定会再崩。

**附加缺陷(ctx 冻结)**:transition 在 IPC handler 改了 `task.json.status`,但当前 loop 的 `workflow_ctx` 在 IPC 入口(`chat.rs:119`)建一次就冻结,breadcrumb 仍显示旧状态,agent 以为没切成功,反复试 → 撞上手写崩坏的 task.json。

## 已确认事实(代码证据)

| 事实 | 来源 |
|---|---|
| `TaskJson.created_at`/`updated_at` 是 `String`,**无 `#[serde(default)]`**,缺字段必崩 | `task.rs:173-174` |
| `TaskItem.status: TaskStatus`,derive Deserialize + `rename_all=lowercase`,**严格** —— `pending`/`in_progress` 不在枚举必崩 | `task.rs:147-157` + `111-119` |
| `TaskStatus` 有 `from_str_opt`(lenient,非法→Planning),但 `read_task` 用 derive 不用它 | `task.rs:122-130` |
| `resolve_current_task` 对解析失败的 task **静默跳过** | `inject.rs:303-311` |
| `create_task` 是 Tauri IPC,**不是 LLM tool**,LLM 调不到 | `commands/task.rs:146` + `tools/mod.rs`(无此 tool) |
| `update_checklist` workflow 分支有正确 status 映射 `InProgress→Implement` | `update_checklist.rs:458-465` |
| `workflow_ctx` 在 IPC 入口建一次,整个 200-turn loop 冻结 | `chat.rs:119` + `chat_loop.rs:1208 for turn` |
| transition 拦截分支用冻结 ctx 的 `current_slug` | `chat_loop.rs:3426-3432` |
| bootstrap hint 让 agent "call create_task IPC",但 agent 够不到 | `inject.rs:620-625` |

## Requirements

### 功能需求

1. **read_task lenient 解析(止血 + 防御)**:缺 `created_at`/`updated_at` → 默认值;`status`/`items[].status` 非法枚举值 → fallback `Planning`;item 缺 `content` → 默认空串。任意 LLM 手写不再致命,task 仍能被解析为合法 `TaskJson`。
2. **`create_task` 升级为 LLM tool**:新增 `tools/create_task.rs`,复用 `create_task_init` 保证 schema 正确建档;注册进 `builtin_tools()` 并新增 `filter_tools_for_workflow`,**只在 `workflow_enabled` session 对 LLM 可见**(顺带把 `request_task_state_transition` 收编进同一过滤,修掉它非 workflow session 也可见 schema 的缺陷);走正常 `execute_tool` 分发(非 blocking,无需 QuestionStore)。
3. **transition 拦截分支即时 resolve**:`chat_loop.rs:3426` 不再读冻结 ctx,改为即时读盘取 `current_state`/`current_slug`。
4. **breadcrumb 注入即时 resolve**:让 breadcrumb 也读盘上最新 task 状态(而非冻结 ctx),transition 成功后同 loop 后续 turn 的 breadcrumb 反映新状态。
5. **bootstrap hint 改文案(软推荐,不禁止 write_file)**:`inject.rs:622` 引导 LLM **优先**用 `create_task` tool 建档(省事、字段全),但**不禁止 write_file** —— 韧性由 R1 lenient 在读取侧兜底,不在写入侧设 path guard 卡(避免过度严格与扩展性损失:未来 task.json 加字段 LLM 仍可直接写)。

### 非功能需求

- 严格 serde 路径(内部 `write_task` 产出)继续 100% 合法,lenient 只兜底外部手写。
- `resolve_current_task` 的「跳过坏 task」语义保留(防真的不可恢复),但 lenient 让绝大多数手写可恢复。
- 现有测试零回归;`cargo test --lib` + `pnpm test` 全过。
- 改动可独立验证、独立提交。

## Acceptance Criteria

- [ ] `read_task` 对「缺 created_at/updated_at」「items[].status=in_progress/pending」「items[] 缺 content」的 task.json 不再返回 Err(返回带 default 的合法 TaskJson)
- [ ] LLM 可调用 `create_task` tool 建档,产出的 task.json 能被 `read_task`/`resolve_current_task` 正常解析
- [ ] workflow session 内 `request_task_state_transition` 允许后,同 loop 后续 turn 的 breadcrumb 显示新状态
- [ ] transition 拦截分支即时读盘:即使 IPC 入口 ctx 是 `current_task=None`,只要盘上有非终态 task,transition 能拿到 slug
- [ ] improve-readme 场景全程重跑:planning→implement→check→done 无 `no active task` / `unknown variant` 崩溃
- [ ] `cd app/src-tauri && PKG_CONFIG_PATH=... cargo test --lib` 零回归 + 新增 lenient/create_task/即时-resolve case 全过
- [ ] `cd app && pnpm test` 零回归
- [ ] bootstrap hint 文案指向 `create_task` tool,不再出现 "call create_task IPC"(LLM 够不到的措辞)

## Notes

- 这是 `07-08-workflow-integration` 的收尾加固,不是新功能。优先级跟随 workflow 稳定性。
- 与 `07-09-workflow-transition-card`(前端卡片)正交,不碰前端。
- 风险点在 R3/R4(读侧即时 resolve 的借用与性能) —— 见 design.md。
