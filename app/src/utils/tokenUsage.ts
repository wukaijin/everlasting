// Token usage display helpers — extracted from `ChatInput.vue` so the
// color threshold logic can be unit-tested without spinning up a
// Vue renderer + Pinia store.
//
// A4 (Token Usage Tracking): the percentage color ladder is the
// small bit of business logic that decides whether the ChatInput
// chip renders green / yellow / red. The thresholds (50% / 75%) are
// spec'd in `.trellis/spec/backend/llm-contract.md` "Scenario:
// Token Usage Tracking" §3 "Color thresholds (UI)" and locked in
// the PRD §Q4 decision 6.
//
// Boundaries (locked by spec):
//   - 0-49%   → "ok"    (green)
//   - 50-74%  → "warn"  (amber)
//   - 75%+    → "alert" (red)
//
// The boundaries are inclusive on the lower edge of the next band
// (50% → "warn", not "ok"; 75% → "alert", not "warn"). The function
// is pure — no Vue / Pinia / DOM dependencies — so the test suite
// can exercise it directly.

/** Color-band returned for a given `input_tokens / context_window`
 *  ratio. `null` mirrors the ChatInput's "no usage yet" render
 *  mode ("—" placeholder). */
export type TokenUsageLevel = "ok" | "warn" | "alert";

/** Map a raw `input_tokens / context_window` ratio to the spec's
 *  color band. Boundaries: < 0.5 → "ok", < 0.75 → "warn", else
 *  → "alert". A negative `pct` (defensive — the caller divides a
 *  `u32` by a positive context window, so this can't happen in
 *  practice) is clamped to "ok" rather than producing a phantom
 *  "warn" or "alert" state. A `pct` above 1.0 (input tokens
 *  exceeding the model window — also defensive — the spec's
 *  `usageLevel` clamps the display to 100%, but the color band
 *  is computed from the raw ratio so a session that genuinely
 *  exceeded its window would still render as "alert"). */
export function tokenUsageLevel(pct: number): TokenUsageLevel {
  if (pct < 0) return "ok";
  if (pct >= 0.75) return "alert";
  if (pct >= 0.5) return "warn";
  return "ok";
}

/** Abbreviate a token count to the K/M form used in the
 *  ChatInput hint.
 *   - 0       → "0"
 *   - < 1000  → exact ("42", "999")
 *   - 1000-999999 → K with 1 decimal, trim trailing 0
 *     ("1K", "1.2K", "14.2K", "200K")
 *   - >= 1_000_000 → M with 1 decimal ("1M", "1.2M")
 *
 *  No locale handling — the project is zh-CN first; 1K reads
 *  as "1千" naturally. */
export function abbreviateTokens(n: number): string {
  if (n < 1000) return n.toString();
  if (n < 1_000_000) {
    const k = n / 1000;
    return k % 1 === 0 ? `${k}K` : `${k.toFixed(1).replace(/\.0$/, "")}K`;
  }
  const m = n / 1_000_000;
  return m % 1 === 0 ? `${m}M` : `${m.toFixed(1).replace(/\.0$/, "")}M`;
}

/** Parsed `TokenUsage` snapshot — the snake_case JSON stored in
 *  `subagent_runs.token_usage_json` (and `sessions.last_*_json`).
 *  Field names are snake_case because the Rust `TokenUsage` struct
 *  (`llm/types.rs:332`) has NO `#[serde(rename_all)]`. The
 *  `context_input_tokens` field carries `#[serde(default)]` on the
 *  Rust side, so legacy rows written before the 2026-06-26
 *  normalized-field fix may omit it — `parseTokenUsageJson`
 *  defaults it to 0. `context_input_tokens` is the
 *  cross-provider-normalized "total input for this request" the
 *  backend treats as the canonical numerator (see `types.rs:317`
 *  comment): Anthropic = input + cache_creation + cache_read;
 *  OpenAI = prompt_tokens. The subagent card shows this as the
 *  worker run's cumulative context footprint. */
export interface TokenUsageSnapshot {
  input_tokens: number;
  output_tokens: number;
  cache_creation_input_tokens: number;
  cache_read_input_tokens: number;
  context_input_tokens: number;
}

/** Parse a `token_usage_json` string into a `TokenUsageSnapshot`,
 *  tolerating the `context_input_tokens` `#[serde(default)]`
 *  history and corrupt rows. Returns `null` for null / empty /
 *  non-JSON / non-object input so every call site can render a
 *  placeholder chip without its own try/catch. Non-number fields
 *  coerce to 0 (defensive — the Rust side serializes u32, but a
 *  half-written row shouldn't crash the UI). */
export function parseTokenUsageJson(
  json: string | null | undefined,
): TokenUsageSnapshot | null {
  if (!json) return null;
  let parsed: unknown;
  try {
    parsed = JSON.parse(json);
  } catch {
    return null;
  }
  if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) return null;
  const o = parsed as Record<string, unknown>;
  const num = (k: string): number => {
    const v = o[k];
    return typeof v === "number" && Number.isFinite(v) ? v : 0;
  };
  return {
    input_tokens: num("input_tokens"),
    output_tokens: num("output_tokens"),
    cache_creation_input_tokens: num("cache_creation_input_tokens"),
    cache_read_input_tokens: num("cache_read_input_tokens"),
    context_input_tokens: num("context_input_tokens"),
  };
}
