// slashCommand.ts — builtin slash-command input matching (pure).
//
// 08-18-manual-compact-command: typing `/compact focus text` and hitting
// Enter (with the palette closed — the palette's own Enter selects an
// item instead) must dispatch the builtin command instead of sending the
// text to the LLM. Historically ONLY the palette path dispatched builtins
// (`/help` `/clear` `/new` typed directly were shipped to the model as
// plain user messages); this module gives the submit path the same
// dispatch, so the two entry points can never drift.

/**
 * The builtin command names recognized by the submit-path interception.
 * Mirrors `BUILTIN_COMMANDS` in `app/src-tauri/src/resource_loader.rs`
 * (single source of truth for the palette; this copy exists because the
 * submit path needs the names synchronously, before any panel load).
 * Keep in sync when adding a builtin.
 */
export const BUILTIN_COMMAND_NAMES: readonly string[] = [
  "help",
  "clear",
  "new",
  "compact",
  "handoff",
];

export interface BuiltinCommandMatch {
  /** The matched builtin command name (no leading slash). */
  name: string;
  /** Rest-of-line after the command token, trimmed — the optional
   *  focus text for `/compact`; empty string when absent. */
  rest: string;
}

/**
 * Match a chat-input line against the builtin command set.
 *
 * Rules:
 * - The first non-whitespace character must be `/`; the token after it
 *   (`[A-Za-z0-9_-]+`) must EXACTLY equal a builtin name — prefix
 *   matches don't count (`/comp` is not `/compact`), so unknown slash
 *   text (including escaped `//...`) still goes to the LLM verbatim;
 * - `rest` = everything after the token, trimmed of surrounding
 *   whitespace (`/compact  聚焦 API ` → `聚焦 API`);
 * - An empty / whitespace-only input never matches.
 */
export function matchBuiltinCommandInput(
  text: string,
  names: readonly string[] = BUILTIN_COMMAND_NAMES,
): BuiltinCommandMatch | null {
  const trimmed = text.trimStart();
  if (!trimmed.startsWith("/")) return null;
  const token = trimmed.slice(1).match(/^[A-Za-z0-9_-]+/);
  if (!token) return null;
  const name = token[0];
  if (!names.includes(name)) return null;
  const rest = trimmed.slice(1 + name.length).trim();
  return { name, rest };
}
