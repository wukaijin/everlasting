// Unit tests for `LRU<K, V>` (streamController cache).
//
// Properties protected:
//   1. Capacity bound: putting past `maxSize` evicts the LRU entry.
//   2. Recency updates on get (touch marks MRU).
//   3. Pinning protects an entry from eviction even at capacity.
//   4. Unpinning restores normal LRU rules.
//   5. delete() removes from cache AND from pinned set.
//   6. Pinning a non-existent key is a no-op.
//   7. clear() empties both maps.
//   8. All-pinned overflow is tolerated (size temporarily > maxSize)
//      so streaming sessions are never lost mid-request.

import { describe, it, expect } from "vitest";
import { LRU } from "./lru";

describe("LRU", () => {
  describe("capacity", () => {
    it("evicts the least-recently-used entry when over capacity", () => {
      const c = new LRU<string, number>(2);
      c.put("a", 1);
      c.put("b", 2);
      c.put("c", 3); // should evict "a"
      expect(c.has("a")).toBe(false);
      expect(c.has("b")).toBe(true);
      expect(c.has("c")).toBe(true);
      expect(c.size).toBe(2);
    });

    it("re-putting an existing key does not grow the cache", () => {
      const c = new LRU<string, number>(2);
      c.put("a", 1);
      c.put("b", 2);
      c.put("a", 10); // update value, not a new entry
      expect(c.size).toBe(2);
      expect(c.get("a")).toBe(10);
    });
  });

  describe("recency", () => {
    it("get() marks the entry as most-recently-used", () => {
      const c = new LRU<string, number>(2);
      c.put("a", 1);
      c.put("b", 2);
      c.get("a"); // touch "a" — should now be MRU
      c.put("c", 3); // should evict "b" (now the LRU)
      expect(c.has("a")).toBe(true);
      expect(c.has("b")).toBe(false);
      expect(c.has("c")).toBe(true);
    });

    it("touch() updates recency without changing value", () => {
      const c = new LRU<string, number>(2);
      c.put("a", 1);
      c.put("b", 2);
      expect(c.touch("a")).toBe(true);
      c.put("c", 3);
      expect(c.has("a")).toBe(true);
    });

    it("touch() returns false for non-existent key", () => {
      const c = new LRU<string, number>(2);
      expect(c.touch("missing")).toBe(false);
    });
  });

  describe("pinning", () => {
    it("a pinned entry is not evicted at capacity", () => {
      const c = new LRU<string, number>(2);
      c.put("a", 1);
      c.put("b", 2);
      expect(c.pin("a")).toBe(true);
      c.put("c", 3); // would normally evict "a" (LRU), but "a" is pinned
      expect(c.has("a")).toBe(true);
      expect(c.has("b")).toBe(false); // "b" is now the LRU non-pinned
      expect(c.has("c")).toBe(true);
    });

    it("unpinning restores normal LRU rules", () => {
      const c = new LRU<string, number>(2);
      c.put("a", 1);
      c.put("b", 2);
      c.pin("a");
      c.put("c", 3);
      expect(c.has("a")).toBe(true);
      c.unpin("a");
      c.put("d", 4); // should evict "a" (now the LRU non-pinned)
      expect(c.has("a")).toBe(false);
      expect(c.has("c")).toBe(true);
      expect(c.has("d")).toBe(true);
    });

    it("pinning a non-existent key returns false and does not add to pinned set", () => {
      const c = new LRU<string, number>(2);
      expect(c.pin("ghost")).toBe(false);
      c.put("a", 1);
      c.put("b", 2);
      c.put("c", 3);
      // "ghost" should not have been pinned (and isn't in cache anyway)
      expect(c.has("a")).toBe(false);
    });

    it("tolerates all-pinned overflow (streaming sessions are sacred)", () => {
      const c = new LRU<string, number>(2);
      // Pin all entries at put time. Eviction can't drop any of
      // them, so the cache temporarily exceeds maxSize rather than
      // throwing or silently losing a pinned entry.
      c.put("a", 1, true);
      c.put("b", 2, true);
      c.put("c", 3, true);
      expect(c.size).toBe(3);
      expect(c.has("a")).toBe(true);
      expect(c.has("b")).toBe(true);
      expect(c.has("c")).toBe(true);

      // Once a key is unpinned, the next put evicts it (it's now
      // the only non-pinned entry that's at LRU position).
      c.unpin("a");
      c.put("d", 4);
      expect(c.has("a")).toBe(false);
      expect(c.has("b")).toBe(true);
      expect(c.has("c")).toBe(true);
      expect(c.has("d")).toBe(true);
    });
  });

  describe("delete + clear", () => {
    it("delete() removes from cache and unpins", () => {
      const c = new LRU<string, number>(2);
      c.put("a", 1, true); // pinned at put time
      c.put("b", 2);
      // Sanity: "a" pinned, "b" not.
      expect(c.delete("a")).toBe(true);
      expect(c.has("a")).toBe(false);
      // "a" is gone from both cache and pinned set. Now re-add it
      // unpinned and verify it can be evicted normally.
      c.put("c", 3);
      c.put("a", 4);
      // Cache order LRU→MRU: c, a (size 2, no eviction needed).
      // Add one more → c is LRU, not pinned → evicted.
      c.put("b", 5);
      expect(c.has("a")).toBe(true);
      expect(c.has("b")).toBe(true);
      expect(c.has("c")).toBe(false);
    });

    it("clear() empties cache and pins", () => {
      const c = new LRU<string, number>(2);
      c.put("a", 1, true);
      c.clear();
      expect(c.size).toBe(0);
      c.put("a", 2);
      c.put("b", 3);
      c.put("c", 4);
      // "a" is no longer pinned — it should be evictable like normal.
      expect(c.has("a")).toBe(false);
    });
  });

  describe("constructor", () => {
    it("rejects maxSize < 1", () => {
      expect(() => new LRU<string, number>(0)).toThrow(/maxSize/);
      expect(() => new LRU<string, number>(-1)).toThrow(/maxSize/);
    });
  });
});
