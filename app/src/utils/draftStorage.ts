// draftStorage — CH5-4 (2026-08-29): composer draft persistence.
//
// The chat input's unsent text used to live only in the CodeMirror
// doc (memory) — a refresh or session switch silently dropped it.
// These helpers persist it per context in localStorage:
//
//   - active session  → `everlasting:draft:sess:<sessionId>`
//   - unsaved "new conversation" → `everlasting:draft:new:<projectId>`
//     (a fresh conversation belongs to a project, not a session row
//     yet — keying by project keeps drafts from cross-contaminating)
//   - neither         → null key, persistence disabled (nothing to
//     scope the draft to)
//
// The value is stored as RAW text (no JSON envelope): it is always a
// plain string, so there is nothing to parse and no corruption path —
// absent key = empty draft. All ops are try/catch-wrapped: localStorage
// throws in private mode / under quota pressure, and losing a draft is
// always better than breaking the input.
//
// Scope note: only TEXT drafts persist. Staged images (B1) keep their
// in-memory lifecycle — objectURLs don't survive a reload anyway.

const PREFIX = "everlasting:draft:";

/** Resolve the storage key for the current composer context. */
export function draftStorageKey(
  sessionId: string | null,
  projectId: string | null,
): string | null {
  if (sessionId) return `${PREFIX}sess:${sessionId}`;
  if (projectId) return `${PREFIX}new:${projectId}`;
  return null;
}

/** Load the persisted draft for this context (`""` when none / no
 *  key / storage unavailable). */
export function loadDraft(
  sessionId: string | null,
  projectId: string | null,
): string {
  const key = draftStorageKey(sessionId, projectId);
  if (!key) return "";
  try {
    return localStorage.getItem(key) ?? "";
  } catch {
    return "";
  }
}

/** Persist the draft for this context. Empty text REMOVES the key
 *  (a sent/cleared draft must not resurrect on the next visit). */
export function saveDraft(
  sessionId: string | null,
  projectId: string | null,
  text: string,
): void {
  const key = draftStorageKey(sessionId, projectId);
  if (!key) return;
  try {
    if (text) localStorage.setItem(key, text);
    else localStorage.removeItem(key);
  } catch {
    // private mode / quota → silent
  }
}
