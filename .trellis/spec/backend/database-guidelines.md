# Database Guidelines

> Database patterns and conventions for this project.

---

## Overview

<!--
Document your project's database conventions here.

Questions to answer:
- What ORM/query library do you use?
- How are migrations managed?
- What are the naming conventions for tables/columns?
- How do you handle transactions?
-->

(To be filled by the team)

---

## Query Patterns

<!-- How should queries be written? Batch operations? -->

(To be filled by the team)

---

## Migrations

<!-- How to create and run migrations -->

(To be filled by the team)

---

## Naming Conventions

<!-- Table names, column names, index names -->

(To be filled by the team)

---

## Common Mistakes

<!-- Database-related mistakes your team has made -->

### Scenario: messages_fts — external-content FTS5 on a live table (D2, 2026-08-17)

**Scope/Trigger**: adding or changing an FTS5 external-content index on a table
that already has production rows (`messages_fts` over `messages.text`,
`db/migrations/schema.rs`; query layer `db/search.rs`). The traps below were
all verified empirically on 2026-08-17 — two of them silently fail (no error,
wrong result), which is worse than a crash.

**Trap 1 — staleness probes that lie**. After creating the vtable on an
existing DB you must backfill, but both "obvious" probes are wrong:

- ❌ `SELECT COUNT(*) FROM messages_fts` — **reads through to the content
  table**. Returns the base row count even when the index is completely empty
  (stale). Verified: 2 content rows + 0 indexed → `COUNT(*)` = 2.
- ❌ `INSERT INTO fts(fts) VALUES('integrity-check')` — passes on a
  never-indexed external-content table (it only checks index-internal
  consistency, not content coverage).
- ✅ `SELECT COUNT(*) FROM messages_fts_docsize` — the `%_docsize` shadow
  table holds exactly one row per **indexed** document (0 for a fresh index, N
  after backfill; empty-`text` rows count too — tool-result-only messages
  produce them and are common). Compare against `COUNT(*) FROM messages` and
  `rebuild` on divergence. This is the guard `run_migrations` uses — first
  boot after upgrade rebuilds once, subsequent boots skip.

**Trap 2 — unqualified `AFTER UPDATE` triggers**. The memories FTS
(`am_fts_update`) uses plain `AFTER UPDATE` — fine there because that table
has no hot non-text write path. `messages` does: `update_message_latency`,
`update_message_metadata`, tool-duration patches. An unqualified update
trigger does an FTS delete+insert pair on every one of those writes.
`messages_fts_update` is `AFTER UPDATE OF text` — keep that qualifier.

**Trap 3 — external-content delete idiom**. `AFTER DELETE` must insert
`VALUES('delete', old.id, old.text)` into the **vtable itself** (with the
table-name first column), not a normal row. Deleting sessions goes through
explicit `DELETE FROM messages` (plus FK cascade as backstop), so the trigger
covers it; AC test locks "deleted session leaves no hits".

**Query dispatch (db/search.rs contract)**: trigram tokenizer needs ≥3
unicode **chars** — shorter queries (2-char Chinese words like "权限") must
route to a `LIKE '%q%' ESCAPE '\'` fallback. `COUNT(*)` lies (trap 1), so
"does the fallback ever fire?" is only answerable from the dispatch code, not
from index counts.

**Tests**: `db/search_tests.rs` — backfill (drop vtable+triggers, re-migrate,
old rows searchable), delete propagation, `UPDATE OF text` red line
(metadata/latency updates don't churn docsize; text rewrite swaps the index),
LIKE-wildcard literalism, per-kind limit semantics.
