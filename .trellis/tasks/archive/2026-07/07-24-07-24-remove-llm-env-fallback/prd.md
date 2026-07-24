# Remove LLM env fallback path

## Goal

项目已从「env 单模型」架构迁移到「DB catalog 多 provider」架构。`ANTHROPIC_API_KEY` / `LLM_MODEL` 等 env 变量只在 `LlmConfig::from_env()` 一处被读取，产物 `AppState.config` 仅被 `create_session` 当作新建 session 的 model 默认值（每次写死 `MiniMax-M2.7`）。chat 主路径和 `get_llm_config` IPC 早已走 catalog，env 路径是死代码。本次移除该死代码链路，并修复过程中暴露的两个问题。

## Background

- `LlmConfig::from_env()` 是早期「env 单模型」架构的遗留，multi-model catalog（2026-06-08/09）落地后降级为 cold-start fallback。
- PR2 之后 chat 命令、`get_llm_config` IPC 全部走 catalog，`from_env` 仅剩 `AppState::load` 调用，结果存进 `state.config` 字段。
- `state.config` 的唯一真实读者是 `sessions.rs:48` 的 `create_session` model fallback（前端从不传 model，每次命中）。

## Requirements

### R1: 移除 env 兜底链路（核心）

- 删除 `LlmConfig::from_env()` / `unconfigured()` / `is_unconfigured()`。
- 删除 `AppState.config` 字段、`load_inner` 里的 `from_env` 调用与 config 启动日志。
- `sessions.rs:48` 的 model fallback 改为 `unwrap_or_default()`（空串，不写死 MiniMax-M2.7）。
- 删除 `llm::LlmConfig` re-export（无外部消费者）。
- 清理 `config.rs` / `provider/mod.rs` / `openai.rs` 里引用 `from_env` 的过时注释。
- **保留** shell.rs 脱敏清单（`ANTHROPIC_API_KEY` 作为敏感变量示例）、error.rs Auth 文案、测试夹具 —— 这些是横截引用，非真实使用。

### R2: 同步文档

- `CLAUDE.md` / `STRUCTURE.md` / `README.md`：环境变量块改为「不读 LLM env，配置走 UI Settings → DB catalog」。
- `docs/HACKING-llm.md`：重写「env vs DB catalog 优先级」「daemon env 传递」两节，更新 checklist / thinking effort 来源 / IPC 坑位示例。
- `.trellis/spec/backend/llm-contract.md` / `multi-provider-contract.md`：Env-keys 小节、Error Matrix、request 示例、测试清单、Wrong/Correct 反模式同步更新。

### R3: 修复 DEFAULT_MAX_TOKENS dead_code warning

- R1 删除 `from_env` 后，`DEFAULT_MAX_TOKENS` 常量只剩测试引用，工厂用字面量 `16384`（重复）。
- 改 `DEFAULT_MAX_TOKENS` 为 `pub(crate)`，工厂 anthropic 分支复用 `anthropic::DEFAULT_MAX_TOKENS`，消除字面量重复。OpenAI 分支字面量保留（独立默认值，对称性约定）。

### R4: 修复 daemon sidecar 崩溃

- `pnpm tauri dev` 报「daemon 未在 15s 内就绪」。诊断：`binaries/everlasting-daemon-x86_64-unknown-linux-gnu` 是损坏的 ELF（`file` 报 `missing section headers at 388384720`，偏移 388MB 而文件仅 34MB），sidecar 一启动就 core dump。
- 根因：某次操作产生了损坏的 staged 文件，`build.rs` 的 mtime 增量检查看到 staged ≥ source 就跳过 copy，损坏文件一直留着。非本次改动引入。
- 修复：删除损坏的 staged 二进制，重新 `cargo build` 触发 `build.rs` 从 `target/debug/` 重新 stage 有效二进制。

## Acceptance Criteria

- [x] `LlmConfig::from_env` / `unconfigured` / `is_unconfigured` 删除，无残留引用。
- [x] `AppState.config` 字段删除，`load_inner` 不再读 env。
- [x] `sessions.rs:48` model fallback 改空串。
- [x] `cargo build` 0 warning（含 R3 的 dead_code 消除）。
- [x] `cargo test --lib` 全绿（1560 passed, 0 failed）。
- [x] staged daemon 二进制有效，手动运行启动完整（AppState loading → listening）。
- [x] 用户文档 + spec 契约文档同步更新，无「还在用 env」的过时描述（归档/历史快照除外）。

## Notes

- 不动 DB schema（`sessions.model` 列保留，存空串）、不跑 migration。
- 不动 OpenAI provider 路径（`OpenAIConfig` 独立）。
- daemon 化后 `pnpm tauri dev` 的 Rust 热更新行为有变化（daemon 代码不自动重编重启），已在对话中向用户说明，建议改 Rust 用 `pnpm dev:all`。本任务不解决该体验问题。
