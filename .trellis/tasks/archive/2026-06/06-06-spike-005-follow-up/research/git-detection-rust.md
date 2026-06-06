# Research: git detection + current branch in Rust Tauri 2 backend

- **Query**: Best way to detect "is this directory a git repo" + read the current branch name in a Rust Tauri 2 backend, in the context of adding `is_git_repo: bool` and `git_branch: Option<String>` columns to the `projects` table.
- **Scope**: mixed (internal codebase inspection + external crate metadata)
- **Date**: 2026-06-06
- **Repo**: `/usr/local/code/github/everlasting`

## Findings

### Existing in-repo pattern (the elephant in the room)

The project **already implements Approach A** for `is_git_repo`. The implementation lives at `app/src-tauri/src/projects/detector.rs` and was written explicitly to **avoid pulling in `git2`** for the first-time git probe.

Key quote from `detector.rs:1-11` module-level doc:

> // We deliberately do **not** depend on the `git2` crate here — this is
> // the first time the project would touch git, and step 4 will own the
> // worktree logic. A short `git -C <path> rev-parse --show-toplevel`
> // shell-out is the lowest-cost "is this a git repo" check that works
> // across the platforms we target (Linux / WSL / macOS / Windows).

The current `is_git_repo` implementation:

- Sync variant (`is_git_repo_sync`, line 21): `std::process::Command::new("git").arg("-C").arg(path).args(["rev-parse", "--show-toplevel"]).output()`; returns `true` iff exit-0 AND stdout is non-empty.
- Async variant (`is_git_repo`, line 47): wraps the sync version in `tokio::task::spawn_blocking` with a `tokio::time::timeout(1s)`. Returns `false` on any timeout/error (defensive: a missing/slow `git` binary must not stall a Tauri command).
- `store::create_project` and `store::update_project_path` both call `is_git_repo_sync` and persist the result. Tests use `tempfile::tempdir` + `git init --quiet` to set up fixtures.

This is the **natural place to add `current_branch`** — same module, same pattern. A second small helper (`current_branch_sync(path: &Path) -> Option<String>`) shelling out to `git -C <path> rev-parse --abbrev-ref HEAD` fits the existing style exactly.

### Approach A — `std::process::Command` / `tokio::process::Command` + `git` binary

- **Crate size / compile time impact**: zero (no new dependency; `tokio` already has `process` feature enabled in `Cargo.toml:24`).
- **API ergonomics for our two operations**:
  - `is_inside_work_tree`: `git -C <path> rev-parse --is-inside-work-tree` (existing code uses `--show-toplevel`; either works and is already validated).
  - current branch: `git -C <path> rev-parse --abbrev-ref HEAD` — returns e.g. `main`, `feature/foo`, or `HEAD` (detached). 5-line wrapper function.
- **Error handling**: parse `stdout` / `stderr` from `Output`; `Err(_)` from `Command::output()` → `None`; non-zero exit → `None`; `HEAD` sentinel (detached) → caller decides whether to treat as `None` or store the literal `"HEAD"`. Matches the existing `is_git_repo_sync` "any error → conservative default" style.
- **Maintenance / release**: not a crate — relies on the system `git`. WSL dev env has it; macOS ships with `git`; Windows requires Git for Windows. The CLAUDE.md / HANDOFF.md already assume `git` is on PATH for the `shell` tool.
- **WSL / cross-platform**: works on Linux, macOS, Windows (with Git for Windows on PATH). The `-C <path>` form is portable.
- **License**: n/a (no crate).

### Approach B — `git2` crate (libgit2 bindings)

- **Current version**: `0.21.0` (from `cargo search git2`, `cargo info git2`).
- **Crate size / compile time impact**: **significant**. Pulls in `libgit2-sys 0.18.5+1.9.4` (a C library). `libgit2-sys` features include `vendored` (build libgit2 from C source — ~minutes of extra compile time on cold cache, hundreds of KB of generated code), `openssl-sys`, `libssh2-sys`, `zlib-ng-compat`. Even with `vendored` off, on most CI/dev machines libgit2 must be present as a system lib or you accept the vendored hit. Well-known community complaint: first `cargo build` after adding `git2` is materially slower than any other LLM/Tauri dependency we use.
- **API ergonomics for our two operations**:
  - `is_inside_work_tree`: `git2::Repository::discover(path)` succeeds iff path is inside a work tree. Equivalent: `repo.workdir().is_some()`.
  - current branch: `let head = repo.head()?; head.shorthand()` returns the branch short name (handles detached HEAD by returning `"HEAD"`).
  - Pattern: `Repository::discover(path).ok().filter(|r| r.workdir().is_some())` plus `and_then(|r| r.head().ok().map(|h| h.shorthid().unwrap_or("HEAD").to_string()))`.
- **Error handling**: `git2::Error` is its own error type with `code()` returning `git2::ErrorCode` (e.g. `NotFound`, `Exists`, `ClassBranch`); converts cleanly to `anyhow` / `thiserror` via `From`. No subprocess = no `stderr` parsing.
- **Maintenance / last release**: actively maintained under the `rust-lang/git2-rs` repo. Last release line is the 0.20/0.21 series used widely (used by cargo, deno, etc.).
- **WSL / cross-platform**: works everywhere libgit2 builds. Linux is fine; Windows historically needed `vcpkg` or prebuilt libgit2 (the `vendored` feature exists for this reason). No PATH dependency on the user's `git` binary.
- **License**: MIT OR Apache-2.0.

### Approach C — `gix` (gitoxide) crate

- **Current version**: `0.84.0` (`cargo search gix`, `cargo info gix`).
- **Crate size / compile time impact**: pure-Rust, no C dependency. Significantly lighter than `git2` (no libgit2 C build). However the crate is **modular and large** by default — `cargo info gix` reports "31 activated features / 29 deactivated features" in the default build. The `gix` umbrella crate pulls in a lot; the project would more likely use a sub-crate (e.g. `gix-discover`, `gix-revwalk`, `gix-ref`) to keep the footprint minimal. Choosing sub-crates is the documented gix best-practice but adds design overhead.
- **API ergonomics for our two operations**:
  - `is_inside_work_tree`: `gix::discover(path)` returns `Result<Repository, _>`; success implies inside a work tree (discover walks parents up to find `.git`).
  - current branch: `let head = repo.head().ok()?; head.referent_name().map(|n| n.shorten().to_string())` or `repo.head_branch().ok().flatten().map(|b| b.name.shorten().to_string())`. API is more verbose and the semantics around detached HEAD are slightly different.
  - Net: doable, but the API surface is **less ergonomic for a quick "branch name" read** than libgit2's `head().shorthand()`. The library is optimized for full-blown git operations (plumbing), not a single-field read.
- **Error handling**: `gix::discover::Error`, `gix::reference::Category`, etc. Richer error model than git2 in some respects; converts to anyhow fine. No subprocess.
- **Maintenance / last release**: very active, `GitoxideLabs/gitoxide`. Frequent releases (0.84.0 is the current line as of June 2026). Some 1.0 milestones still open.
- **WSL / cross-platform**: pure-Rust = builds anywhere Rust builds. No system C deps.
- **License**: MIT OR Apache-2.0.
- **MSRV caveat**: `cargo info gix` reports `rust-version: 1.85`. The project's `rust-version` is currently `unknown` (we don't pin it in `Cargo.toml`), but if the project adopts 1.85 MSRV later, the choice should be re-evaluated. This is a minor concern today.

### Constraints from our repo (verified)

- **Cargo.toml verification** (`app/src-tauri/Cargo.toml`): no `git2` and no `gix` in `[dependencies]` or `[dev-dependencies]`. Only the dependencies listed are `tauri`, `tauri-plugin-dialog`, `serde`, `serde_json`, `reqwest`, `futures-util`, `tokio`, `tokio-util`, `async-stream`, `anyhow`, `thiserror`, `tracing`, `tracing-subscriber`, `sqlx`, `uuid`, `chrono`, `tauri-plugin-os`; dev-dep `tempfile`. No heavy git crate is present.
- **Minimal-deps posture**: project CLAUDE.md "Tech Stack (Locked)" table does not list any git crate today. Adding a new heavy dep (git2) for a single `bool` + `Option<String>` field is in tension with the locked stack.
- **Existing shell-out pattern**: `app/src-tauri/src/tools/shell.rs:99-107` already uses `tokio::process::Command` with `tokio::time::timeout` and a validated `current_dir`. `app/src-tauri/src/projects/detector.rs:47-59` already uses the same `spawn_blocking + timeout` pattern for the `is_git_repo` probe. The proposed `current_branch` helper would slot into the same module using the same idiom.
- **`docs/DESIGN.md:166`** already flags: `Git2-rs worktree API 不全 | 中 | 必要时 spawn `git worktree` 命令` — i.e. the design notes already plan to mix-and-match: even when `git2` arrives for step 4 worktree work, the project accepts that some operations will still shell out. Adding more shell-out calls **before** step 4 is therefore not in tension with the longer-term plan.
- **`docs/TECH.md:20`** lists `git2-rs` as the locked choice for "Git 操作" but explicitly scoped to `worktree / diff / commit` (i.e. step 4). The current task is a pre-step-4 read-only probe, which the locked-tech table does not cover.
- **WSL reality**: dev runs in WSL 2 Ubuntu 22.04 (CLAUDE.md, `docs/HACKING-wsl.md`). `git` is on PATH; `git rev-parse` is sub-millisecond locally. The 1s `timeout` already present in `detector.rs` is a defensive guard, not a hot-path concern.

### Code patterns observed

| Pattern | File:line | Reuse opportunity |
|---|---|---|
| `tokio::process::Command` + `tokio::time::timeout` | `tools/shell.rs:99-107` | same idiom for `git rev-parse` |
| `tokio::task::spawn_blocking` wrapping sync `Command::output` | `projects/detector.rs:49-58` | same idiom for the new `current_branch` probe |
| "any error → conservative default" (`is_git_repo` returns `false` on any failure) | `projects/detector.rs:32-41` | apply the same defensive `Option<String> → None` mapping for branch |
| `is_git_repo` re-probed on path change | `projects/store.rs:57` | `current_branch` should be re-probed on the same path-change trigger |

### Feasible approaches here (ranked)

1. **Approach A (extend the existing shell-out in `detector.rs`)** — add `pub fn current_branch_sync(path: &Path) -> Option<String>` and `pub async fn current_branch(path: &Path) -> Option<String>`, mirroring the existing pair. Zero new deps, matches the module's stated design ("lowest-cost probe"), reuses the already-validated `tokio` timeouts, and stores already in use. **Recommended.**
2. **Approach B (add `git2` now)** — possible but expensive: triggers libgit2 build cost on every clean `cargo build` in exchange for two trivially short shell calls. Only becomes attractive when step 4 brings worktree/diff/commit operations that justify the C dependency. Adds a `libgit2` install / vendor decision to the dev-onboarding story.
3. **Approach C (add `gix` now)** — pure-Rust, lighter than git2, but the API is over-spec'd for a single-field read and would either pull the full `gix` umbrella crate (heavy) or require picking sub-crates (design overhead). Same downside as B: we'd be paying dependency cost for one `bool` + one `Option<String>`.

**Recommended: Approach A.** Smallest viable change, zero new compile-time cost, reuses the exact same `tokio` timeout + spawn_blocking pattern already in the file, and is consistent with the explicit "step 4 will own the worktree logic" comment in `detector.rs`. When step 4 lands and `git2`/`gix` is added for worktree operations, the read-only probe here can either stay as a fast path or be replaced wholesale — that is a step-4 refactor decision, not a blocker for adding the column now.

## External references

- [git2 on crates.io](https://crates.io/crates/git2/0.21.0) — bindings to libgit2; v0.21.0, MIT/Apache-2.0.
- [git2-rs on GitHub](https://github.com/rust-lang/git2-rs) — `Repository::discover` + `head().shorthand()` is the canonical "branch name" idiom.
- [libgit2-sys on crates.io](https://crates.io/crates/libgit2-sys/0.18.5+1.9.4) — the C library backing `git2`; `vendored` feature = source build cost.
- [gix on crates.io](https://crates.io/crates/gix/0.84.0) — pure-Rust gitoxide umbrella; v0.84.0, MIT/Apache-2.0, MSRV 1.85.
- [GitoxideLabs/gitoxide on GitHub](https://github.com/GitoxideLabs/gitoxide) — modular pure-Rust git implementation.
- [`git-rev-parse` docs](https://git-scm.com/docs/git-rev-parse) — `--is-inside-work-tree`, `--abbrev-ref HEAD`, `--show-toplevel` flags are stable, documented, and shipped in every git 2.x.

## Related Specs / Docs (in-repo)

- `app/src-tauri/src/projects/detector.rs` — current `is_git_repo` shell-out; the natural home for the new `current_branch` helper.
- `app/src-tauri/src/projects/store.rs:35, 57` — `create_project` and `update_project_path` already re-probe on path change; add a re-probe call for branch at the same sites.
- `app/src-tauri/src/projects/types.rs:14-24` — `ProjectRow` struct; will gain a `pub git_branch: Option<String>` field.
- `app/src-tauri/src/db.rs:129-140` — `projects` table DDL; will gain a `git_branch TEXT` column + idempotent ALTER via the `pragma_table_info` probe pattern already used at `db.rs:287-301`.
- `app/src-tauri/src/tools/shell.rs:99-107` — `tokio::process::Command` + timeout reference pattern.
- `docs/TECH.md:20` — `git2-rs` is the **locked** choice for step 4 worktree/diff/commit work; not for the current read-only probe.
- `docs/DESIGN.md:166` — note that "git2-rs worktree API 不全" + fallback to spawn `git worktree`; even with git2 later, mixed strategy is OK.
- `docs/HANDOFF.md:43` — "步骤 4 Git 集成（worktree + auto commit）" is the trigger for the heavier git crate, not step 3b-1.

## Caveats / Not Found

- The exact "compile-time delta of adding `git2` to this project" was **not** measured (would require an actual `cargo clean && cargo build`). The qualitative claim "well-known to be slow" is sourced from general community consensus and the size of libgit2's C source; treat as order-of-magnitude, not a precise number. If step 4 ever wants empirical numbers, the standard recipe is `cargo build --timings` before/after.
- The current branch read for a **detached HEAD** returns the literal string `"HEAD"` from `git rev-parse --abbrev-ref HEAD`. The detector should decide: store `"HEAD"` (transparent to the LLM/UI) or collapse to `None`. The recommendation in the rank section above is to store what git says (so the UI can distinguish "detached" from "no branch known") — but the final call belongs to the spec/implementation step, not the research step.
- The `gix` MSRV of 1.85 is not currently a constraint (we don't pin `rust-version`), but it would become one if the project later adopts MSRV 1.85. Flag for re-evaluation then.
- No sub-crate matrix comparison for `gix` was performed (e.g. `gix-discover` + `gix-revparse` alone). The above is a top-level umbrella assessment.
