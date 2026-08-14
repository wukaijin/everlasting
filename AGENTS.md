<!-- TRELLIS:START -->
# Trellis Instructions

These instructions are for AI assistants working in this project.

This project is managed by Trellis. The working knowledge you need lives under `.trellis/`:

- `.trellis/workflow.md` — development phases, when to create tasks, skill routing
- `.trellis/spec/` — package- and layer-scoped coding guidelines (read before writing code in a given layer)
- `.trellis/workspace/` — per-developer journals and session traces
- `.trellis/tasks/` — active and archived tasks (PRDs, research, jsonl context)

If a Trellis command is available on your platform (e.g. `/trellis:finish-work`, `/trellis:continue`), prefer it over manual steps. Not every platform exposes every command.

If you're using Codex or another agent-capable tool, additional project-scoped helpers may live in:
- `.agents/skills/` — reusable Trellis skills
- `.codex/agents/` — optional custom subagents

Managed by Trellis. Edits outside this block are preserved; edits inside may be overwritten by a future `trellis update`.

<!-- TRELLIS:END -->

## Running Tests

**Frontend** (Vitest, in `app/`):

```bash
cd app && pnpm test            # run all *.test.ts under app/src
cd app && pnpm test -- --ui    # interactive watch mode
```

**Backend** (Rust `cargo test`). 2026-08-11 workspace 翻转后根目录有 `Cargo.toml`(members = app/src-tauri + crates/everlasting-remote(-protocol);default-members 只含 remote 两 crate)——**根目录裸 `cargo test` 只跑 default-members(remote 两 crate,不会跑 app)**;app 的测试需显式 `-p everlasting`,或 cd app/src-tauri 后裸命令。On WSL you must export `PKG_CONFIG_PATH` or system libs (gdk-pixbuf / webkit2gtk) won't be found — see [docs/HACKING-wsl.md](./docs/HACKING-wsl.md) 坑 1:

```bash
cd app/src-tauri && \
  PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" \
  cargo test --lib             # ~1689 unit tests (2026-08-13 实测);default is multi-threaded (= nproc)
# 根 workspace 等价写法(推荐从根跑):
cargo test -p everlasting --lib               # 结果同 cd app/src-tauri && cargo test --lib(PKG_CONFIG_PATH 仍需)
cargo test -p everlasting-remote              # remote crate:零系统库依赖,无需 PKG_CONFIG_PATH,远快于 everlasting
```

Notes:
- `cargo test` is multi-threaded by default (one thread per core). **Don't add `--test-threads=1`** for routine runs — single-threaded is ~3× slower here (72s vs 26s for `--lib`).
- Scope a smoke run with a filter inside one `cargo test` call, e.g. `cargo test --lib "agent::tests_agent_loop::"`. Avoid looping `cargo test <module>` per module — each invocation pays ~11s relink + spawn tax and skews timing.
- To profile slow tests, prefer [`cargo-nextest`](https://nexte.st) (`cargo nextest run --lib`, per-test timings); otherwise see the timestamp fallback in [HACKING-wsl.md §测试性能](./docs/HACKING-wsl.md#测试性能wsl-后端-cargo-test).
- Cold compile `--no-run` ≈ 1m37s; incremental ≈ 11s.
- Remote 链路 E2E 冒烟:`node scripts/remote-e2e-smoke.mjs`(需本地 remote 服务端在跑,见 [docs/REMOTE-ACCESS-E2E.md](./docs/REMOTE-ACCESS-E2E.md))。

## DB / 单轮烟测速查

- **SQLite DB(daemon + GUI 共用)**:`~/.local/share/dev.everlasting.app/everlasting.db`(WSL/Linux;macOS `~/Library/Application Support/...`,见 [docs/DEBUG_DB.md](./docs/DEBUG_DB.md) §1,schema 索引 + 常用查询都在那)。**WAL writer 是 daemon 进程** — `sqlite3 -readonly` 查询随时安全;直连写要先 `./scripts/daemon.sh stop`(GUI Thin 模式不开 pool,不影响)。
- **单轮烟测**:`scripts/turn-smoke.sh` — 经 daemon HTTP API(`:7456`)建临时 session 实跑一轮 LLM,轮询 `turn_trace` 报 per-turn token(tools_token / context_input / 占比),跑完自动删 session。改了 agent loop / trace / tools 链路后用它做 live 验证,别手翻 DB。
