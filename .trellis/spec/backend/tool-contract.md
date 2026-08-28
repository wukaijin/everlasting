# Tool Contract —工具定义 + ReadGuard + Bash Spillover + 自主记忆写工具

> **基线**:
> - 2026-06-13(PR1 + PR3 of `06-12-a2-b7-permission-and-mode`)
> - 2026-06-29 `06-29-am-p2-readwrite`(`remember` 工具加入 builtin_tools(),silent-allow 模式)
> **来源**:从原 `llm-contract.md` (3149 行)拆出本文件
> **同源文档**:
> - [llm-contract.md](./llm-contract.md) —核心类型 + 反模式汇总 + ⑨ 关 IPC 协议(Extended Thinking 已拆至 [llm-contract/extended-thinking.md](./llm-contract/extended-thinking.md))
> - [tool-contract.md](./tool-contract.md) (本文) —工具定义 + ReadGuard + shell spillover + `remember` silent-allow
> - [permission-layer.md](./permission-layer.md) —⑨ 关 Permission Layer 设计合约(A2 + B7 canonical,2026-06-13)
> - [worktree-contract.md](./worktree-contract.md) — attach/detach/delete + cancel + system prompt
> - [multi-provider-contract.md](./multi-provider-contract.md) — Provider trait + catalog + Anthropic/OpenAI 分发
> - [test-model-contract.md](./test-model-contract.md) — `test_model` IPC
> - [memory.md](./memory.md) §Scenario: Autonomous Memories —`remember` 完整契约(DB schema / 安全网 / 频率控制 / silent-allow 权限模型)
> - [memory.md](./memory.md) §Scenario: V2-2+ Observability & Management(2026-07-06)— `update_autonomous_memory` / `update_autonomous_memory_status` IPC + `ChatEvent::Recall` + `validate_memory_text` helper + `edited_by_user` provenance + 状态机矩阵(前端只读副本 / 后端硬墙)
>
> **何时读本文**:涉及 `builtin_tools()` / `edit_file` / `ReadGuard` / `shell` spillover / `grep` / `glob` / `list_dir` / `remember`(silent-allow 自主记忆写)/ `update_autonomous_memory{,_status}` IPC / `ChatEvent::Recall` / `load_tool_schemas` + `stubify`(tools Stub 注册,D)/ `search_history`(D2② agent 驱动跨 session 搜索)时。
>
> **⑨ 关 Permission Layer 设计合约**:[permission-layer.md](./permission-layer.md)(A2 + B7, 2026-06-13,2026-06-21 移入)。

---


---

## Scenario Index (08-07-large-file-splitting: 按 Scenario 拆分为 parts)

- [01-tool-set-extension](./tool-contract/01-tool-set-extension.md)
- [02-web-fetch](./tool-contract/02-web-fetch.md)
- [03-update-checklist](./tool-contract/03-update-checklist.md)
- [04-dispatch-subagent](./tool-contract/04-dispatch-subagent.md)
- [05-subagent-runs-persistence](./tool-contract/05-subagent-runs-persistence.md)
- [06-background-shell](./tool-contract/06-background-shell.md)
- [07-concurrent-dispatch-batch](./tool-contract/07-concurrent-dispatch-batch.md)
- [08-merge-discard-worker](./tool-contract/08-merge-discard-worker.md)
- [09-use-ui](./tool-contract/09-use-ui.md)
- [10-c2-loop-intervention](./tool-contract/10-c2-loop-intervention.md)
- [11-request-mode-change](./tool-contract/11-request-mode-change.md)
- [12-builtin-plugin-source-layer](./tool-contract/12-builtin-plugin-source-layer.md)
- [13-use-ui-button-apply-ui-diff](./tool-contract/13-use-ui-button-apply-ui-diff.md)
- [14-stub-registration](./tool-contract/14-stub-registration.md) — tools Stub 注册(渐进式披露 D,`load_tool_schemas` 契约 + 粘性 registry + 开关)
- [15-search-history](./tool-contract/15-search-history.md) — `search_history`(D2② agent 驱动跨 session 全文搜索,复用 db::search 共享层 + Tier 5 silent Allow + agent 侧 limit 50 vs modal 200)
- [16-web-search](./tool-contract/16-web-search.md) — `web_search`(F4 snippet-only 网页搜索,enum dispatch 双后端 Tavily/DDG + key 三态 AEAD 配置 + DDG 202 软封锁语义 + 全名单开闸含项目层 frontmatter)
- [17-schedule-task-family](./tool-contract/17-schedule-task-family.md) — `schedule_task`/`schedule_status`/`schedule_cancel`(LLM 调度家族,F2 detached dispatch;作者面分离 created_by='agent' + tool 侧 kill switch/上限双 gate + Tier 5 钉住 + pool 级核心范式)
