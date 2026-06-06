/**
 * LRU — a tiny Least-Recently-Used cache.
 *
 * Bounded by `maxSize`; on `put` past capacity, evicts the entry
 * whose `get` / `put` was least recent. "Pin" prevents eviction
 * even when capacity is exceeded — used by `streamController` so
 * a session that has an in-flight request can never be thrown out
 * of the in-memory message cache (which would lose the streaming
 * message). When the request ends, the caller `unpin`s and the
 * normal LRU rules apply.
 *
 * Reactivity: this is a plain class, NOT a Vue reactive. The
 * store that owns the LRU (e.g. `streamController`) wraps it in
 * refs / computeds where components need to observe. Lookups
 * (`get`) are O(1) and mutation (`put`, `pin`, `unpin`) are O(1)
 * amortized.
 *
 * Why not a Map? Map preserves insertion order; we'd have to
 * delete + re-insert on every access to model LRU recency. The
 * two-map approach below does that bookkeeping with one extra
 * map and no allocation per access.
 */
export class LRU<K, V> {
  private cache = new Map<K, V>();
  private pinned = new Set<K>();

  constructor(public readonly maxSize: number) {
    if (maxSize < 1) {
      throw new Error(`LRU: maxSize must be >= 1, got ${maxSize}`);
    }
  }

  /** Get a value, marking it as most-recently-used. */
  get(key: K): V | undefined {
    const v = this.cache.get(key);
    if (v === undefined) return undefined;
    // Re-insert to move to MRU end. Map's insertion order is
    // its iteration order; delete + set achieves the move.
    this.cache.delete(key);
    this.cache.set(key, v);
    return v;
  }

  /** Touch an existing key (no value change) to mark it MRU. */
  touch(key: K): boolean {
    return this.get(key) !== undefined;
  }

  /** Put a value, evicting the LRU non-pinned entry if at capacity.
   *  Pass `pinned: true` to also pin the new key (common pattern:
   *  `put(key, value, true)` when starting a streaming session). */
  put(key: K, value: V, pinned = false): V {
    if (this.cache.has(key)) {
      this.cache.delete(key);
    }
    this.cache.set(key, value);
    if (pinned) {
      this.pinned.add(key);
    }
    this.evictIfNeeded();
    return value;
  }

  /** Check existence without touching recency. */
  has(key: K): boolean {
    return this.cache.has(key);
  }

  /** Pin a key against eviction. Pinned keys are not evicted by
   *  `evictIfNeeded` even when at capacity. Pinning a non-existent
   *  key is a no-op (you must `put` first). Returns whether the
   *  key is now pinned. */
  pin(key: K): boolean {
    if (!this.cache.has(key)) return false;
    this.pinned.add(key);
    return true;
  }

  /** Unpin a key. Returns whether the key was previously pinned. */
  unpin(key: K): boolean {
    return this.pinned.delete(key);
  }

  /** Number of entries (including pinned). */
  get size(): number {
    return this.cache.size;
  }

  /** Iterate keys from LRU to MRU. */
  *keys(): IterableIterator<K> {
    yield* this.cache.keys();
  }

  /** Remove a key explicitly (e.g. on session delete). */
  delete(key: K): boolean {
    this.pinned.delete(key);
    return this.cache.delete(key);
  }

  /** Clear all entries and pins. */
  clear(): void {
    this.cache.clear();
    this.pinned.clear();
  }

  private evictIfNeeded(): void {
    if (this.cache.size <= this.maxSize) return;
    // Iterate from the LRU end and skip pinned entries.
    for (const key of this.cache.keys()) {
      if (this.pinned.has(key)) continue;
      this.cache.delete(key);
      // Don't keep evicting — we only overran by one. (If somehow
      // we overran by more, the next `put` will catch it.)
      return;
    }
    // All entries are pinned. We don't evict anything; the cache
    // temporarily exceeds `maxSize` until something unpins.
  }
}
