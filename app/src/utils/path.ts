/**
 * Path utilities for display purposes.
 *
 * `simplifyPath` shortens an absolute path by replacing the user's
 * home-directory prefix with `~` (e.g. `/home/carlos/code/foo` →
 * `~/code/foo`). It is purely a display concern: the underlying cwd
 * stays in its full form on the wire and in the DB. PR1 will consume
 * the chat store's `simplifiedCwd` computed and render it in the
 * chat panel header; PR3 (this) only prepares the data and helper.
 *
 * The boundary check (`path === homeDir` and `path.startsWith(homeDir + "/")`)
 * avoids a false prefix match where `/home/carlosOther` would be
 * wrongly treated as under `/home/carlos` by a naive
 * `startsWith(homeDir)` check.
 *
 * Windows paths are out of scope for v1 (see parent PRD). If
 * `homeDir` looks like `C:\Users\foo`, behavior is best-effort and
 * may not produce the desired `~\...` form — acceptable per the
 * "WSL-first" stance in `docs/TECH.md`.
 */
export function simplifyPath(path: string, homeDir: string | null): string {
  if (!homeDir) return path;
  if (path === homeDir) return "~";
  // Boundary check: require the next char to be `/` (or end of
  // string handled above). Without this, `/home/carlosOther` would
  // be wrongly simplified to `~Other`.
  if (path.startsWith(homeDir + "/")) {
    return `~${path.slice(homeDir.length)}`;
  }
  return path;
}

/**
 * Lexical-normalize a POSIX-ish path by collapsing `.` and resolving
 * `..` segments WITHOUT touching the filesystem (no canonicalize, no
 * symlink resolution). Used by `isPathInRoot` so a path like
 * `/repo/../sibling/file` reduces to `/sibling/file` and is correctly
 * rejected as outside `/repo`. Empty segments and trailing slashes are
 * dropped.
 *
 * Mirrors the lexical_normalize helper in the Rust
 * `projects/boundary.rs::is_within_root` (re-grill, 2026-06-13) —
 * kept here as a defensive copy because the PermissionModal needs
 * to render the in-repo/out-of-repo badge on the frontend without
 * an extra IPC round-trip just to ask the backend "is this path in
 * the project?".
 */
function lexicalNormalize(input: string): string {
  // Normalize separators (handle Windows-style on the dev box too,
  // even though WSL-first is the documented target). Strip leading
  // `~` (rare on the wire, but cheap to defend against).
  const replaced = input.replace(/\\/g, "/");
  const segments = replaced.split("/").filter((s) => s.length > 0);
  const out: string[] = [];
  for (const seg of segments) {
    if (seg === ".") continue;
    if (seg === "..") {
      out.pop();
      continue;
    }
    out.push(seg);
  }
  // Preserve leading slash so `/repo/foo` stays distinguishable
  // from `repo/foo` (a relative path inside the project — that's
  // not what callers want, but we don't want to silently flip
  // either). Empty result means the path was all `..` — caller
  // treats that as "not inside".
  const joined = out.join("/");
  if (replaced.startsWith("/") && joined.length > 0) return "/" + joined;
  return joined;
}

/**
 * Component-wise "is `target` inside `root`?" predicate. Mirrors
 * the Rust `projects/boundary::is_within_root` (re-grill 2026-06-13):
 *   - Lexical normalize both root and target (so `/repo/../sibling/file`
 *     → `/sibling/file` is rejected as outside `/repo`).
 *   - Component-wise prefix match (so `/repo/foobar` is NOT inside
 *     `/repo/foo` — this is the classic prefix trap that a naive
 *     `startsWith("/repo/foo")` falls into).
 *   - `target === root` counts as inside.
 *   - Empty / relative inputs return `false` defensively.
 *
 * Used by the frontend `PermissionModal` (re-grill 2026-06-13 PR2)
 * to render the in-repo/out-of-repo badge on the path range row,
 * avoiding an extra IPC round-trip to the backend's
 * `is_within_root` helper. The two implementations MUST agree on
 * the 7 edge cases documented in
 * `.trellis/spec/backend/project-cwd-boundary.md §6`.
 */
export function isPathInRoot(target: string, root: string): boolean {
  if (!target || !root) return false;
  if (!root.startsWith("/") || !target.startsWith("/")) return false;
  const t = lexicalNormalize(target);
  const r = lexicalNormalize(root);
  if (t === r) return true;
  // Component-wise prefix: split on `/` and compare segments.
  // Equivalent to Rust `Path::starts_with` semantics.
  const tSegs = t.split("/").filter((s) => s.length > 0);
  const rSegs = r.split("/").filter((s) => s.length > 0);
  if (tSegs.length < rSegs.length) return false;
  for (let i = 0; i < rSegs.length; i++) {
    if (tSegs[i] !== rSegs[i]) return false;
  }
  return true;
}
