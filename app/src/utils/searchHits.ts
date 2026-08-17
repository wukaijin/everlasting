// searchHits — D2 hit-rendering helpers shared by the user-facing
// SearchModal (①) and the agent-tool SearchHistoryCard (D2②+).
// Extracted 08-17-search-history-card from SearchModal.vue where
// they first appeared (pure functions; the modal/card pass their
// own query string in).

/** Compact zh-CN date label; same-year dates omit the year. Empty
 *  string for unparseable input (renders as nothing, not "Invalid"). */
export function hitTimeLabel(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "";
  const sameYear = d.getFullYear() === new Date().getFullYear();
  return d.toLocaleDateString("zh-CN", {
    year: sameYear ? undefined : "numeric",
    month: "2-digit",
    day: "2-digit",
  });
}

/** Split a snippet around the first case-insensitive occurrence of
 *  `query` so the caller can render [before, <mark>match</mark>,
 *  after]. Match is null when the query is empty or absent (FTS may
 *  match with different casing — callers render the plain snippet). */
export function splitSnippetAt(
  snippet: string,
  query: string,
): [string, string | null, string] {
  const q = query.trim().toLowerCase();
  if (!q) return [snippet, null, ""];
  const idx = snippet.toLowerCase().indexOf(q);
  if (idx === -1) return [snippet, null, ""];
  return [
    snippet.slice(0, idx),
    snippet.slice(idx, idx + q.length),
    snippet.slice(idx + q.length),
  ];
}
