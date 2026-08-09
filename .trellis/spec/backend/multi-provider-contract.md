# Multi-Provider Contract — Provider Trait + Catalog + Anthropic/OpenAI Dispatch

> **基线**:2026-06-10 commit `0f9a167` (8-PR5拆分后)
> **来源**:从原 `llm-contract.md` (3149 行)拆出本文件
> **同源文档**:
> - [llm-contract.md](./llm-contract.md) —核心类型 + 反模式汇总(Extended Thinking 已拆至 [llm-contract/extended-thinking.md](./llm-contract/extended-thinking.md))
> - [tool-contract.md](./tool-contract.md) —工具定义 + ReadGuard + shell spillover
> - [worktree-contract.md](./worktree-contract.md) — attach/detach/delete + cancel + system prompt
> - [multi-provider-contract.md](./multi-provider-contract.md) (本文) — Provider trait + catalog + Anthropic/OpenAI 分发
> - [test-model-contract.md](./test-model-contract.md) — `test_model` IPC
>
> **何时读本文**:涉及 `Provider` trait / `WireMessage` 中间层 / `AnthropicProvider` / `OpenAIProvider` / `build_provider` factory / cross-protocol strip / catalog resolution 时。

---


---

## Part Index (08-07-large-file-splitting)

- [scenario-multi-provider-abstraction](./multi-provider-contract/scenario-multi-provider-abstraction.md)
- [scenario-provider-trait-anthropic](./multi-provider-contract/scenario-provider-trait-anthropic.md)
- [scenario-openai-wire](./multi-provider-contract/scenario-openai-wire.md)
