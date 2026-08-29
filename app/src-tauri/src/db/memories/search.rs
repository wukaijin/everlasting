use sqlx::SqlitePool;

use super::types::{MemoryInsertError, MemoryRow, MemoryScope};

// escape_fts5 (phrase-match helper) — used by `db::search` (messages
// FTS); the memories recall path OR-expands per-token instead.
/// Escape a user-supplied query string for safe FTS5 MATCH.
///
/// Wraps the query in double quotes (FTS5 phrase-match syntax) and
/// doubles any embedded double quotes per the FTS5 string-literal
/// rule. This neutralizes FTS5 operators (`AND` / `OR` / `NOT` /
/// `NEAR` / `*` / `^`) — a query like `cargo AND test` is treated
/// as a single phrase, not a boolean expression.
///
/// **Tradeoff (H3)**: phrase match requires the tokens to appear
/// contiguously AND in the given order. `"WSL cargo"` won't match
/// content reading "cargo ... WSL" (different order). v1 accepts
/// this (precision-first); v2 can switch to per-token escaping +
/// OR-join for recall-first semantics. See prd §4 H3 tradeoff
/// note.
pub(crate) fn escape_fts5(q: &str) -> String {
    format!("\"{}\"", q.replace('"', "\"\""))
}

/// Search memories via FTS5 `MATCH` + `bm25` ranking. The query is
/// escaped via [`escape_fts5`] (phrase match; H3 tradeoff accepted
/// for v1).
///
/// **scope/project_id interaction (H2)**:
/// - `scope = Some(User)` → `WHERE scope='user'` (project_id
///   ignored — a user-scope memory is global to the user).
/// - `scope = Some(Project)` + `project_id = None` → **Err**
///   (a project query without a project id is meaningless).
/// - `scope = Some(Project)` + `project_id = Some(id)` →
///   `WHERE scope='project' AND project_id=?`.
/// - `scope = None` → search both layers:
///   `WHERE scope='user' OR (scope='project' AND project_id=?)`.
///   In this case `project_id` MUST be `Some` (the project branch
///   of the OR needs it) — returns Err otherwise.
///
/// `status_filter` controls which status values are surfaced:
/// - [`RecallStatusFilter::ActiveVerifiedOnly`] (default, P1
///   semantics) — `active` + `verified` only.
/// - [`RecallStatusFilter::IncludeCandidate`] (P2 session-start
///   recall) — adds `candidate`.
///
/// `limit` caps the result count (P2's session-start recall uses
/// a small top-k; the caller decides).
#[allow(dead_code)] // test-locked: production recall goes through
                    // `search_memories_fts_recall`; this phrase variant survives as the
                    // H2-scope-semantics + phrase-escape test vehicle (memories_tests/
                    // list_delete_search.rs). Remove together with those tests if the
                    // phrase search is ever retired for real.
pub async fn search_memories_fts(
    pool: &SqlitePool,
    project_id: Option<&str>,
    scope: Option<MemoryScope>,
    query: &str,
    limit: i64,
    status_filter: RecallStatusFilter,
) -> Result<Vec<MemoryRow>, MemoryInsertError> {
    // Empty / whitespace query → empty result (FTS5 MATCH on an
    // empty phrase is a syntax error; short-circuit instead).
    if query.trim().is_empty() {
        return Ok(Vec::new());
    }

    let escaped = escape_fts5(query);
    let status_in = status_filter.status_in_clause();

    // Build the scope filter per H2. Three branches:
    // (a) User scope — ignore project_id.
    // (b) Project scope — require project_id.
    // (c) None — search both; project_id required for the project
    //     branch of the OR.
    let (sql, bind_project_id) = match scope {
        Some(MemoryScope::User) => (
            // (a)
            format!(
                r#"
            SELECT m.id, m.memory_id, m.scope, m.project_id, m.kind, m.status,
                   m.title, m.content, m.tags, m.tool_name, m.command_pattern,
                   m.path_globs, m.source_session_id, m.source_ref, m.confidence,
                   m.hit_count, m.last_used_at, m.created_at, m.updated_at,
                   m.demoted_reason, m.edited_by_user
            FROM autonomous_memories_fts f
            JOIN autonomous_memories m ON m.id = f.rowid
            WHERE autonomous_memories_fts MATCH ?
              AND m.scope = 'user'
              AND m.status IN ({status_in})
            ORDER BY bm25(autonomous_memories_fts)
            LIMIT ?
            "#
            ),
            false,
        ),
        Some(MemoryScope::Project) => {
            if project_id.is_none() {
                return Err(MemoryInsertError::ProjectScopeMissingId);
            }
            (
                // (b)
                format!(
                    r#"
                SELECT m.id, m.memory_id, m.scope, m.project_id, m.kind, m.status,
                       m.title, m.content, m.tags, m.tool_name, m.command_pattern,
                       m.path_globs, m.source_session_id, m.source_ref, m.confidence,
                       m.hit_count, m.last_used_at, m.created_at, m.updated_at,
                       m.demoted_reason, m.edited_by_user
                FROM autonomous_memories_fts f
                JOIN autonomous_memories m ON m.id = f.rowid
                WHERE autonomous_memories_fts MATCH ?
                  AND m.scope = 'project'
                  AND m.project_id = ?
                  AND m.status IN ({status_in})
                ORDER BY bm25(autonomous_memories_fts)
                LIMIT ?
                "#
                ),
                true,
            )
        }
        None => {
            // (c) — search both layers; project_id required.
            if project_id.is_none() {
                return Err(MemoryInsertError::ProjectScopeMissingId);
            }
            (
                format!(
                    r#"
                SELECT m.id, m.memory_id, m.scope, m.project_id, m.kind, m.status,
                       m.title, m.content, m.tags, m.tool_name, m.command_pattern,
                       m.path_globs, m.source_session_id, m.source_ref, m.confidence,
                       m.hit_count, m.last_used_at, m.created_at, m.updated_at,
                       m.demoted_reason, m.edited_by_user
                FROM autonomous_memories_fts f
                JOIN autonomous_memories m ON m.id = f.rowid
                WHERE autonomous_memories_fts MATCH ?
                  AND (m.scope = 'user'
                       OR (m.scope = 'project' AND m.project_id = ?))
                  AND m.status IN ({status_in})
                ORDER BY bm25(autonomous_memories_fts)
                LIMIT ?
                "#
                ),
                true,
            )
        }
    };

    let mut q = sqlx::query_as::<_, MemoryRow>(&sql).bind(&escaped);
    if bind_project_id {
        q = q.bind(project_id);
    }
    q = q.bind(limit);
    let rows = q.fetch_all(pool).await?;
    Ok(rows)
}

// ---------------------------------------------------------------------------
// FTS5 bm25 search: the P1 phrase-match variant (test-locked, see its
// per-item allow) + the P2 loose-recall variant that production uses
// ---------------------------------------------------------------------------

/// Status-filter policy for the FTS recall search.
///
/// - `ActiveVerifiedOnly` — original P1 semantics (P3 pre-tool
///   pitfall recall, P5 status-machine path). `candidate` rows are
///   NOT surfaced — they haven't earned recall surface yet.
/// - `IncludeCandidate` — P2 session-start recall semantics. P5's
///   state machine landed and DELIBERATELY kept this (see
///   `memory_recall.rs`: candidate is promoted BY being recalled —
///   hit_count accrues on recall — so excluding it would sever the
///   candidate→active graduation path; noise is controlled by the
///   promotion threshold + hygiene job, not the recall filter).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecallStatusFilter {
    /// Policy knob, currently unconstructed in production (the
    /// recall path deliberately always passes `IncludeCandidate`).
    /// Retained as the documented dial for a future tightening.
    #[allow(dead_code)]
    ActiveVerifiedOnly,
    IncludeCandidate,
}

impl RecallStatusFilter {
    /// The SQL fragment used in the `AND m.status IN (...)` clause.
    fn status_in_clause(&self) -> &'static str {
        match self {
            Self::ActiveVerifiedOnly => "'active','verified'",
            Self::IncludeCandidate => "'candidate','active','verified'",
        }
    }
}

/// Build an OR-joined FTS5 query from a natural-language phrase
/// (the user's latest message). Splits on whitespace, drops
/// stopwords + tokens shorter than 3 chars (trigram tokenizer
/// needs ≥3 chars to match), then OR-joins the per-token
/// phrase-escaped fragments. Used by P2's session-start recall —
/// the phrase-match [`escape_fts5`] is too strict for natural-
/// language recall (it requires contiguous in-order tokens, which
/// a free-form user message almost never satisfies against a
/// concise memory body).
///
/// Returns an empty `String` when no usable tokens survive the
/// filter — the caller short-circuits to "no recall" (avoids
/// passing an empty MATCH expression to FTS5, which is a syntax
/// error).
///
/// **Token cap**: only the first 8 surviving tokens are OR-joined
/// — beyond that, bm25 ranking degrades and the MATCH expression
/// grows (FTS5 has a default 64-phrase OR limit, but the practical
/// precision/recall tradeoff caps out well before that).
pub(crate) fn build_recall_fts_query(text: &str) -> String {
    // Minimal English + Chinese stopword set. Kept tiny — the
    // goal is to drop high-frequency function words that would
    // match too many rows, not to be a complete NLP stoplist.
    const STOPWORDS: &[&str] = &[
        "the", "a", "an", "and", "or", "but", "of", "to", "in", "on", "at", "for", "is", "are",
        "was", "were", "be", "been", "being", "this", "that", "these", "those", "it", "its",
        "with", "as", "by", "how", "what", "when", "why", "i", "you", "we", "they", "he", "she",
        "my", "your", "our", "的", "了", "是", "在", "和", "与", "或", "我", "你", "他", "她",
        "这", "那",
    ];
    const MAX_TOKENS: usize = 8;

    let mut phrases: Vec<String> = Vec::new();
    for raw in text.split_whitespace() {
        // Trim punctuation around the token.
        let token = raw.trim_matches(|c: char| !c.is_alphanumeric());
        let lower = token.to_lowercase();
        // trigram tokenizer needs ≥3 chars; stopwords are noise.
        if lower.chars().count() < 3 {
            continue;
        }
        if STOPWORDS.contains(&lower.as_str()) {
            continue;
        }
        // Escape each token as its own phrase (handles embedded
        // quotes / operators per-token).
        phrases.push(format!("\"{}\"", lower.replace('"', "\"\"")));
        if phrases.len() >= MAX_TOKENS {
            break;
        }
    }
    phrases.join(" OR ")
}

/// Loose-recall FTS search for P2's session-start recall. Same scope/project_id interaction (H2)
/// and same `status_filter` semantics, but the query is OR-joined
/// per-token via [`build_recall_fts_query`] (natural-language
/// friendly) instead of phrase-matched (which is too strict for a
/// free-form user message).
///
/// Returns an empty Vec when the query yields no usable tokens
/// (all stopwords / too short) — the caller treats this as "no
/// recall".
pub async fn search_memories_fts_recall(
    pool: &SqlitePool,
    project_id: Option<&str>,
    scope: Option<MemoryScope>,
    query: &str,
    limit: i64,
    status_filter: RecallStatusFilter,
) -> Result<Vec<MemoryRow>, MemoryInsertError> {
    let or_query = build_recall_fts_query(query);
    if or_query.is_empty() {
        return Ok(Vec::new());
    }
    let status_in = status_filter.status_in_clause();

    let (sql, bind_project_id) = match scope {
        Some(MemoryScope::User) => (
            format!(
                r#"
            SELECT m.id, m.memory_id, m.scope, m.project_id, m.kind, m.status,
                   m.title, m.content, m.tags, m.tool_name, m.command_pattern,
                   m.path_globs, m.source_session_id, m.source_ref, m.confidence,
                   m.hit_count, m.last_used_at, m.created_at, m.updated_at,
                   m.demoted_reason, m.edited_by_user
            FROM autonomous_memories_fts f
            JOIN autonomous_memories m ON m.id = f.rowid
            WHERE autonomous_memories_fts MATCH ?
              AND m.scope = 'user'
              AND m.status IN ({status_in})
            ORDER BY bm25(autonomous_memories_fts)
            LIMIT ?
            "#
            ),
            false,
        ),
        Some(MemoryScope::Project) => {
            if project_id.is_none() {
                return Err(MemoryInsertError::ProjectScopeMissingId);
            }
            (
                format!(
                    r#"
                SELECT m.id, m.memory_id, m.scope, m.project_id, m.kind, m.status,
                       m.title, m.content, m.tags, m.tool_name, m.command_pattern,
                       m.path_globs, m.source_session_id, m.source_ref, m.confidence,
                       m.hit_count, m.last_used_at, m.created_at, m.updated_at,
                       m.demoted_reason, m.edited_by_user
                FROM autonomous_memories_fts f
                JOIN autonomous_memories m ON m.id = f.rowid
                WHERE autonomous_memories_fts MATCH ?
                  AND m.scope = 'project'
                  AND m.project_id = ?
                  AND m.status IN ({status_in})
                ORDER BY bm25(autonomous_memories_fts)
                LIMIT ?
                "#
                ),
                true,
            )
        }
        None => {
            if project_id.is_none() {
                return Err(MemoryInsertError::ProjectScopeMissingId);
            }
            (
                format!(
                    r#"
                SELECT m.id, m.memory_id, m.scope, m.project_id, m.kind, m.status,
                       m.title, m.content, m.tags, m.tool_name, m.command_pattern,
                       m.path_globs, m.source_session_id, m.source_ref, m.confidence,
                       m.hit_count, m.last_used_at, m.created_at, m.updated_at,
                       m.demoted_reason, m.edited_by_user
                FROM autonomous_memories_fts f
                JOIN autonomous_memories m ON m.id = f.rowid
                WHERE autonomous_memories_fts MATCH ?
                  AND (m.scope = 'user'
                       OR (m.scope = 'project' AND m.project_id = ?))
                  AND m.status IN ({status_in})
                ORDER BY bm25(autonomous_memories_fts)
                LIMIT ?
                "#
                ),
                true,
            )
        }
    };

    let mut q = sqlx::query_as::<_, MemoryRow>(&sql).bind(&or_query);
    if bind_project_id {
        q = q.bind(project_id);
    }
    q = q.bind(limit);
    let rows = q.fetch_all(pool).await?;
    Ok(rows)
}

// ---------------------------------------------------------------------------
// find_pitfalls_by_trigger — pre-tool recall (P3 consumer)
// ---------------------------------------------------------------------------

/// Find pitfall memories matching the current tool invocation. Used
/// by P3's pre-tool recall hook (the `permissions/check.rs` Tier 1
/// Hooks site). The probe is `tool_name` exact-match (indexed by
/// `idx_am_pitfall`); `command_pattern` is an optional secondary
/// substring filter.
///
/// **command_pattern semantics (2026-08-18, task
/// 08-18-debug-session-5df29977 问题1)**:
/// - If a pitfall's `command_pattern` is `NULL` → the pitfall is
///   command-agnostic (fires for ANY command / no command).
/// - If `command_pattern` is `Some(cp)` AND the probe command is
///   `Some(cmd)` → the pitfall fires only if `cmd.contains(cp)`.
/// - If `command_pattern` is `Some(cp)` AND the probe command is
///   `None` (Path-kind tools never extract one — see
///   `extract_probe_args`) → the pitfall does NOT fire (the
///   constraint can't be confirmed; precision-first). Pre-fix this
///   arm fell through and recalled on EVERY call of the tool_name
///   (the 5df29977 incident: two edit_file pitfalls carrying
///   error-text patterns footnoted 60/15 healthy calls).
///
/// **path_globs semantics (M2)**:
/// - If a pitfall's `path_globs` is `NULL` → the pitfall is
///   path-agnostic (fires for ANY path; e.g. "always pass
///   `--offline` to cargo").
/// - If `path_globs` is `Some(globs)` AND `path` is `Some(p)` →
///   the pitfall fires only if `p` matches at least one glob in
///   the JSON array.
/// - If `path_globs` is `Some(globs)` AND `path` is `None` → the
///   pitfall does NOT fire (the caller didn't supply a path, so
///   we can't confirm the glob match; precision-first).
///
/// Only `status IN ('active','verified')` rows are returned (a
/// `candidate` pitfall hasn't earned recall surface yet).
pub async fn find_pitfalls_by_trigger(
    pool: &SqlitePool,
    tool_name: &str,
    command_pattern: Option<&str>,
    path: Option<&str>,
) -> Result<Vec<MemoryRow>, sqlx::Error> {
    // First: the indexed tool_name equality probe (idx_am_pitfall).
    let candidates: Vec<MemoryRow> = sqlx::query_as::<_, MemoryRow>(
        r#"
        SELECT id, memory_id, scope, project_id, kind, status, title, content,
               tags, tool_name, command_pattern, path_globs, source_session_id,
               source_ref, confidence, hit_count, last_used_at, created_at,
               updated_at, demoted_reason, edited_by_user
        FROM autonomous_memories
        WHERE tool_name = ?
          AND kind = 'pitfall'
          AND status IN ('active','verified')
        "#,
    )
    .bind(tool_name)
    .fetch_all(pool)
    .await?;

    // Second: in-memory filtering for command_pattern + path_globs.
    // The candidate set is small (one tool_name's worth — typically
    // single digits), so the post-filter is cheaper than a complex
    // SQL expression and avoids SQLite glob's lack of JSON-array
    // iteration support.
    let mut out = Vec::with_capacity(candidates.len());
    for mem in candidates {
        // command_pattern substring match — precision-first on a
        // missing probe command (see the doc comment above for the
        // full 4-arm semantics). A row that constrains on command
        // text but the caller supplied none (Path-kind tools) can't
        // be confirmed → skip, mirroring the path_globs arm below.
        if let Some(mem_cp) = &mem.command_pattern {
            match command_pattern {
                Some(cp) if cp.contains(mem_cp.as_str()) => {}
                _ => continue,
            }
        }
        // path_globs match (M2).
        if let Some(globs_json) = &mem.path_globs {
            match path {
                Some(p) => {
                    // Parse the JSON array; if it fails or is empty,
                    // treat as "no match" (precision-first).
                    let globs: Vec<String> = serde_json::from_str(globs_json).unwrap_or_default();
                    let matched = globs.iter().any(|g| glob_matches_path(g, p));
                    if !matched {
                        continue;
                    }
                }
                None => {
                    // path_globs is set but caller supplied no path —
                    // can't confirm; skip (precision-first).
                    continue;
                }
            }
        }
        // NULL path_globs → path-agnostic → always fires (no filter).
        out.push(mem);
    }
    Ok(out)
}

/// P5 (2026-06-29): same probe as [`find_pitfalls_by_trigger`] but
/// returns rows in **any** non-`demoted` status (`candidate` +
/// `active` + `verified`). Used by [`crate::agent::permissions::recall_pitfall`]
/// so the new `PitfallRecall` tiering can:
/// - surface `candidate` pitfalls as footnotes + bump them (the
///   promotion entry point — design §3; without this, candidate
///   pitfalls could never be recalled and would never promote),
/// - surface `active` pitfalls as footnotes (unchanged from P3),
/// - surface `verified` pitfalls as `SoftBlock` (when fully matched
///   + not yet soft-blocked this session — design §4).
///
/// `demoted` rows stay excluded (they've been aged out / superseded;
/// the hygiene job can re-promote them via `update_status`).
pub async fn find_pitfalls_by_trigger_all_status(
    pool: &SqlitePool,
    tool_name: &str,
    command_pattern: Option<&str>,
    path: Option<&str>,
) -> Result<Vec<MemoryRow>, sqlx::Error> {
    // Same indexed tool_name probe as find_pitfalls_by_trigger, but
    // the status filter is widened to all non-demoted statuses.
    let candidates: Vec<MemoryRow> = sqlx::query_as::<_, MemoryRow>(
        r#"
        SELECT id, memory_id, scope, project_id, kind, status, title, content,
               tags, tool_name, command_pattern, path_globs, source_session_id,
               source_ref, confidence, hit_count, last_used_at, created_at,
               updated_at, demoted_reason, edited_by_user
        FROM autonomous_memories
        WHERE tool_name = ?
          AND kind = 'pitfall'
          AND status IN ('candidate','active','verified')
        "#,
    )
    .bind(tool_name)
    .fetch_all(pool)
    .await?;

    // Same in-memory command_pattern + path_globs filter as the
    // original. Kept in sync deliberately (the two functions share
    // the matching semantics; only the status filter differs).
    let mut out = Vec::with_capacity(candidates.len());
    for mem in candidates {
        if let Some(mem_cp) = &mem.command_pattern {
            match command_pattern {
                Some(cp) if cp.contains(mem_cp.as_str()) => {}
                _ => continue,
            }
        }
        if let Some(globs_json) = &mem.path_globs {
            match path {
                Some(p) => {
                    let globs: Vec<String> = serde_json::from_str(globs_json).unwrap_or_default();
                    let matched = globs.iter().any(|g| glob_matches_path(g, p));
                    if !matched {
                        continue;
                    }
                }
                None => {
                    continue;
                }
            }
        }
        out.push(mem);
    }
    Ok(out)
}

/// Simple glob matcher for `path_globs`. Supports `*` (any sequence
/// not crossing `/`) and `?` (one char). The glob set is supplied by
/// the writer (P2 remember tool / P4 reflection); this function is
/// the read-side matcher.
///
/// **Dialect note**: this is the `session_tool_permissions`-style
/// glob, NOT native SQLite GLOB. Verified empirically against SQLite
/// 3.53.0 at check time: native `'a/b' GLOB 'a*'` returns 1 (SQLite
/// GLOB's `*` DOES cross `/`). This matcher instead treats `*` as
/// segment-scoped (matches `app/src-tauri/Cargo.toml` but NOT
/// `app/src-tauri/src/lib.rs`), matching the
/// `session_tool_permissions.path` glob contract that
/// spike-007's re-grill explicitly standardized on (no `**`
/// recursion). The doc comment previously claimed "SQLite GLOB
/// semantics" — that was inaccurate and is corrected here.
///
/// **Char-level vs byte-level caveat**: `?` matches a single **byte**
/// here, not a single char. SQLite GLOB uses `sqlite3Utf8Read` and
/// is char-level (a CJK char is one match unit). For ASCII paths
/// (the dominant case) the two are equivalent; a CJK glob with `?`
/// (e.g. `中?` to match `中文`) would NOT match here. `*` is
/// unaffected (matching UTF-8 bytes within a segment == matching
/// chars within a segment). Accepted as low-priority for P1 (CJK
/// path globs with `?` are vanishingly rare); revisit if P3/P4
/// surface the case.
fn glob_matches_path(glob: &str, path: &str) -> bool {
    // Convert the glob to a regex-free byte-by-byte match. `*` →
    // any non-`/` run; `?` → any single byte (see char-level caveat
    // in the doc comment above).
    let glob_b: &[u8] = glob.as_bytes();
    let path_b: &[u8] = path.as_bytes();
    glob_match_inner(glob_b, path_b)
}

/// Recursive glob matcher (`session_tool_permissions`-style glob, NOT
/// native SQLite GLOB — see [`glob_matches_path`] doc). `*` matches
/// zero or more chars that are NOT `/`; `?` matches any single byte
/// (including `/`). All other chars are literal.
fn glob_match_inner(glob: &[u8], path: &[u8]) -> bool {
    let (mut gi, mut pi) = (0, 0);
    let mut star_gi: Option<usize> = None;
    let mut star_pi = 0;
    while pi < path.len() {
        if gi < glob.len() {
            match glob[gi] {
                b'?' => {
                    gi += 1;
                    pi += 1;
                    continue;
                }
                b'*' => {
                    // `*` doesn't cross `/` — remember position, try
                    // to consume zero chars first; if the next path
                    // char is `/`, the star stops matching.
                    if path[pi] == b'/' {
                        // Star can't cross `/`; advance past the star.
                        gi += 1;
                        continue;
                    }
                    star_gi = Some(gi);
                    star_pi = pi;
                    gi += 1;
                    continue;
                }
                c if c == path[pi] => {
                    gi += 1;
                    pi += 1;
                    continue;
                }
                _ => {}
            }
        }
        // Mismatch — backtrack to the last `*` and consume one more
        // char from the path (if possible).
        if let Some(sg) = star_gi {
            gi = sg + 1;
            star_pi += 1;
            if path[star_pi - 1] == b'/' {
                // Star can't cross `/`; no more backtracking.
                return false;
            }
            pi = star_pi;
        } else {
            return false;
        }
    }
    // Consume trailing `*`s in the glob.
    while gi < glob.len() && glob[gi] == b'*' {
        gi += 1;
    }
    gi == glob.len()
}
