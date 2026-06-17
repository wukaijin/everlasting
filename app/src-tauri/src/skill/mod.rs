//! B4 Skill system — `use_skill` virtual tool + progressive disclosure.
//!
//! PR1 (this module): the **loader layer** — scans user/project skill
//! dirs, mtime-fenced cache, precedence merge, L0 listing block. No
//! agent-loop wiring yet; PR2 registers the `use_skill` tool + injects
//! the listing into the agent loop. See
//! `.trellis/tasks/06-18-skill-system/prd.md`.
//!
//! Design decisions (B4 brainstorm):
//! - **Independent `SkillCache`**: a deliberate copy of the B3
//!   `resource_loader` pattern (B3 untouched, ~200 lines structural
//!   duplication acceptable under YAGNI until a later refactor extracts
//!   `ResourceLoader<Kind>`).
//! - **Skill = directory**: `<name>/SKILL.md` (+ optional reference
//!   files), whereas a command is a single `*.md` file — so the scan
//!   walks subdirs, not `*.md` files. This is the one structural delta
//!   from B3.
//! - **MVP frontmatter**: minimal set `name` + `description` only
//!   (agentskills.io standard min set). No `allowed-tools` / switches.

pub mod loader;
