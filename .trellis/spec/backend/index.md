# Backend Development Guidelines

> `app/src-tauri/` 后端(Rust agent core + daemon)的编码契约索引。每份 spec 记录该项目**实际**约定(非理想),含代码示例、forbidden patterns、common mistakes。写后端代码前先读对应主题 spec;`.trellis/spec/guides/` 是跨层思考指南(代码复用 / 跨层数据流 / 文档维护 / 债务演进)。

---

## Spec Index

| Spec | 主题 | 现状 |
|------|------|------|
| [Directory Structure](./directory-structure.md) | 后端目录结构 + 模块组织 | 占位待补(仅 Large-File Splitting 对照表) |
| [Agent Loop Architecture](./agent-loop-architecture.md) | agent loop / 关卡 / turn 模式(含子目录 pattern-*) | ✅ 有实质内容 |
| [Daemon Server](./daemon-server.md) | daemon HTTP server 契约(axum 路由 / SSE / 进程模型) | ✅ 有实质内容 |
| [Scheduled Tasks](./scheduled-tasks.md) | F2 定时任务执行契约(30s tick / due 落账 / 三档 target_mode,08-31 per_run + 09-07 group_chat) | ✅ 有实质内容 |
| [Disk Governance](./disk-governance.md) | F3 磁盘治理(governor 每日节拍 / 孤儿回收 / 备份预算 / 日志进程内轮转) | ✅ 有实质内容 |
| [Sandbox Executor](./sandbox-executor.md) | 执行期沙盒(P3b Landlock+seccomp / P3c 三态 + Plan 只读面 / P3d 后台升级闭环) | ✅ 有实质内容 |
| [Permission Layer](./permission-layer.md) | per-session mode + ⑨ 关权限层(A2+B7;含 ask_no_timeout 全局开关) | ✅ 有实质内容 |
| [Background Shell Observability](./background-shell-observability.md) | 后台 shell UI 可观测性(list/kill IPC + `background_shell:update` 事件 + 双模式接线) | ✅ 有实质内容 |
| [Database Guidelines](./database-guidelines.md) | DB 模式(migrations / schema 纪律 / 审计落表 / 各表 CRUD) | ✅ 有实质内容(残留少量模板句可清) |
| [Memory Contract](./memory.md) | 指令内存(B5 静态 loader)+ 自主运行时记忆(V2 2 期;子目录含 decisions) | ✅ 有实质内容 |
| [Tool Contract](./tool-contract.md) | 工具定义 / ReadGuard / Bash spillover / 自主记忆写工具(子目录按工具族分篇) | ✅ 有实质内容 |
| [LLM Contract](./llm-contract.md) | LLM 核心类型 / 思考契约 / provider 差异 / A5+ 重试 / token 计量(子目录) | ✅ 有实质内容 |
| [Multi-Provider Contract](./multi-provider-contract.md) | Provider trait + catalog + Anthropic/OpenAI dispatch | ✅ 有实质内容 |
| [Workflow Plugin Builtin](./workflow-plugin-builtin.md) | builtin workflow plugin 内置化机制(dev 内容契约 / tasks 隔离) | ✅ 有实质内容 |
| [Worktree Contract](./worktree-contract.md) | worktree attach/detach/delete + cancel + system prompt(子目录) | ✅ 有实质内容 |
| [Subagent Runs Schema](./subagent-runs-schema.md) | `subagent_runs` 表 schema(B6 PR2,状态机 / 列 / 隔离) | ✅ 有实质内容 |
| [Token Usage Tracking](./token-usage-tracking.md) | A4 token 计量(turn_trace 各列 / cache 归因 / tools=0 判别) | ✅ 有实质内容 |
| [Latency Tracking](./latency-tracking.md) | F5 latency 三列 + ttfb/thinking 计量 | ✅ 有实质内容 |
| [Git Diff](./git-diff.md) | git diff workdir-vs-branch FileDiff 契约 | ✅ 有实质内容 |
| [Project CWD Boundary](./project-cwd-boundary.md) | 项目 cwd 边界与路径越界防护 | ✅ 有实质内容 |
| [Error Handling](./error-handling.md) | 错误类型与处理策略 | 有实质内容(残留少量模板句可清) |
| [Quality Guidelines](./quality-guidelines.md) | 代码质量标准 / forbidden patterns | 有实质内容(残留少量模板句可清) |
| [Logging Guidelines](./logging-guidelines.md) | 结构化日志 / log 级别 | 有实质内容(残留少量模板句可清) |
| [Test Model Contract](./test-model-contract.md) | per-model 连通性探测契约 | ✅ 有实质内容 |

> 子目录(pattern-* / tool-contract 分篇 / llm-contract 分篇等)由顶层 `.md` 入口的链接承接;加新 spec 时同步本表。跨层 thinking guide(代码复用 / 跨层数据流 / docs 维护 / debt 演进)见 [guides](../guides/index.md)。

---

## How Specs Are Written

- 记录项目**实际约定**(含 gotcha / 实证教训),附代码示例;列 forbidden patterns 与 common mistakes。
- 语言跟随正文既有 spec(中英混合,契约型新 spec 用中文为主);本索引表用中文描述。
- 沉淀入口:调试/实施/评审得出可复用契约后,经 `trellis-update-spec` 写入对应 spec(参考 `.trellis/spec/guides/docs-maintenance-guide.md` 的纪律)。
