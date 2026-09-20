// Tests for `stores/turnCheckpoints.ts` — N2 轮间 diff 读面的 store
// 封装(2026-09-20, task `09-20-n2-checkpoint-revert` PR2)。
//
// 契约(implement PR2「vitest:store 层命令封装 + 入口条件渲染单测」):
//   1. 入口四态 —— `hasTurnDiff` 是唯一判定面:
//      有行(写轮 seq 命中且 prev_seq 非空)→ true;
//      无行(只读轮/旧 session,rows 空)→ false;
//      基线行(prev_seq === null)→ false;
//      破链(CheckpointBroken)与能力不可用(CheckpointsUnavailable)
//      → 整 session unavailable → false。
//   2. wire 形状:snake_case 透传(seq / prev_seq / created_at /
//      files_changed),rows 非数组的脏响应按 unavailable 兜底。
//   3. ensureLoaded 缓存语义(有缓存不重拉)+ refresh 强制重拉;
//      in-flight 去重(并发共享同一 Promise)。
//   4. fetchTurnDiff 直传 get_turn_checkpoint_diff,错误向调用方传播。
//   5. checkpointErrorKind 双通道兼容:Tauri 对象形状 / HTTP
//      TransportError 的 .body 形状 / 未知错误 → null。

import { describe, it, expect, beforeEach, vi } from "vitest";
import { setActivePinia, createPinia } from "pinia";

const invokeMock = vi.fn();

vi.mock("../transport", () => ({
  transport: {
    invoke: (...args: unknown[]) => invokeMock(...args),
    listen: async () => () => {},
  },
}));

import {
  useTurnCheckpointsStore,
  checkpointErrorKind,
  type TurnCheckpoint,
} from "./turnCheckpoints";

function row(overrides: Partial<TurnCheckpoint> = {}): TurnCheckpoint {
  return {
    seq: 1,
    prev_seq: 0,
    created_at: 1_700_000_000_000,
    files_changed: 2,
    ...overrides,
  };
}

/** AppCommandError 形状的 rejection(Tauri 通道)。 */
function ipcError(kind: string): Record<string, unknown> {
  return {
    category: "InvalidRequest",
    kind,
    message: `degraded: ${kind}`,
    retryable: false,
    requestId: null,
  };
}

beforeEach(() => {
  setActivePinia(createPinia());
  invokeMock.mockReset();
  invokeMock.mockResolvedValue([]);
});

describe("turnCheckpoints store — hasTurnDiff 入口四态", () => {
  it("有行:写轮 seq 命中且 prev_seq 非空 → true(仅 assistant 卡消费该判定)", async () => {
    const store = useTurnCheckpointsStore();
    invokeMock.mockResolvedValue([
      row({ seq: 0, prev_seq: null, files_changed: 0 }),
      row({ seq: 1, prev_seq: 0, files_changed: 2 }),
    ]);
    await store.refresh("s1");
    expect(store.hasTurnDiff("s1", 1)).toBe(true);
    // 未命中行 seq(只读轮,后端无行)→ false。
    expect(store.hasTurnDiff("s1", 2)).toBe(false);
  });

  it("无行:list 为空数组(旧 session / 全只读)→ false", async () => {
    const store = useTurnCheckpointsStore();
    invokeMock.mockResolvedValue([]);
    await store.refresh("s-empty");
    expect(store.hasTurnDiff("s-empty", 1)).toBe(false);
  });

  it("基线行:prev_seq === null 不给入口(基线无 diff,vs 空树会列整仓)", async () => {
    const store = useTurnCheckpointsStore();
    invokeMock.mockResolvedValue([
      row({ seq: 0, prev_seq: null, files_changed: 0 }),
    ]);
    await store.refresh("s-baseline");
    expect(store.hasTurnDiff("s-baseline", 0)).toBe(false);
  });

  it("破链/Unavailable:kind 命中即整 session unavailable → false", async () => {
    const store = useTurnCheckpointsStore();
    for (const kind of ["CheckpointBroken", "CheckpointsUnavailable"]) {
      invokeMock.mockRejectedValue(ipcError(kind));
      await store.refresh(`s-${kind}`);
      expect(store.hasTurnDiff(`s-${kind}`, 1)).toBe(false);
      expect(store.bySession.get(`s-${kind}`)?.status).toBe("unavailable");
    }
  });

  it("seq undefined(占位行)恒 false", async () => {
    const store = useTurnCheckpointsStore();
    invokeMock.mockResolvedValue([row()]);
    await store.refresh("s1");
    expect(store.hasTurnDiff("s1", undefined)).toBe(false);
  });
});

describe("turnCheckpoints store — 命令封装语义", () => {
  it("list 走 list_turn_checkpoints 且 snake_case 直传;invoke 异常但非 unavailable kind 时保留旧缓存", async () => {
    const store = useTurnCheckpointsStore();
    invokeMock.mockResolvedValue([row({ seq: 3, prev_seq: 1 })]);
    await store.refresh("s1");
    expect(invokeMock).toHaveBeenCalledWith("list_turn_checkpoints", {
      sessionId: "s1",
    });
    expect(store.hasTurnDiff("s1", 3)).toBe(true);

    invokeMock.mockRejectedValue(new Error("network blip"));
    await store.refresh("s1");
    // 未知错误不清旧缓存(入口不闪断),只 console.error。
    expect(store.hasTurnDiff("s1", 3)).toBe(true);
  });

  it("脏响应(非数组)按 unavailable 兜底,不进入口判定", async () => {
    const store = useTurnCheckpointsStore();
    invokeMock.mockResolvedValue(null);
    await store.refresh("s-null");
    expect(store.bySession.get("s-null")?.status).toBe("unavailable");
  });

  it("ensureLoaded 有缓存不重拉;invalidate 后可重拉", async () => {
    const store = useTurnCheckpointsStore();
    invokeMock.mockResolvedValue([row()]);
    await store.refresh("s1");
    expect(invokeMock).toHaveBeenCalledTimes(1);
    store.ensureLoaded("s1");
    expect(invokeMock).toHaveBeenCalledTimes(1); // 有缓存不重拉
    store.invalidate("s1");
    store.ensureLoaded("s1"); // invoke 同步发起(首个 await 前执行)
    expect(invokeMock).toHaveBeenCalledTimes(2);
  });

  it("in-flight 去重:并发 refresh 共享同一 Promise(一次 invoke)", async () => {
    const store = useTurnCheckpointsStore();
    let release!: (v: unknown) => void;
    invokeMock.mockReturnValue(
      new Promise((resolve) => {
        release = resolve;
      }),
    );
    const a = store.refresh("s1");
    const b = store.refresh("s1");
    release([]);
    await Promise.all([a, b]);
    expect(invokeMock).toHaveBeenCalledTimes(1);
  });

  it("fetchTurnDiff 直传 get_turn_checkpoint_diff;错误向调用方传播(modal 内联渲染)", async () => {
    const store = useTurnCheckpointsStore();
    invokeMock.mockResolvedValue({
      files: [{ path: "a.txt", status: "modified", added: 1, removed: 1, diff_text: "…" }],
    });
    const result = await store.fetchTurnDiff("s1", 1);
    expect(invokeMock).toHaveBeenCalledWith("get_turn_checkpoint_diff", {
      sessionId: "s1",
      seq: 1,
    });
    expect(result.files).toHaveLength(1);

    invokeMock.mockRejectedValue(ipcError("CheckpointBroken"));
    await expect(store.fetchTurnDiff("s1", 2)).rejects.toBeTruthy();
  });
});

describe("checkpointErrorKind — 双通道错误 kind 提取", () => {
  it("Tauri 形状(顶层 kind)/ HTTP 形状(body.kind)/ 未知 → null", () => {
    expect(checkpointErrorKind(ipcError("CheckpointBroken"))).toBe(
      "CheckpointBroken",
    );
    expect(
      checkpointErrorKind({
        status: 400,
        body: { kind: "CheckpointsUnavailable", message: "x" },
      }),
    ).toBe("CheckpointsUnavailable");
    expect(checkpointErrorKind(new Error("boom"))).toBeNull();
    expect(checkpointErrorKind("plain string")).toBeNull();
    expect(checkpointErrorKind(null)).toBeNull();
  });
});
