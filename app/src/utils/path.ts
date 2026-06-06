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
