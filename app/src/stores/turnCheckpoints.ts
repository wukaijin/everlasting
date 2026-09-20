// turnCheckpoints — N2 轮间 diff 读面(2026-09-20, task
// `09-20-n2-checkpoint-revert` PR2)的 store 封装。
//
// 职责:`list_turn_checkpoints` / `get_turn_checkpoint_diff` 两条 IPC
// 的命令封装 + per-session 缓存,供 MessageItem → MessageActionsMenu
// 的「本轮 diff」入口消费(daemon + Tauri 双通道走 transport 抽象,
// CMD_TO_DOMAIN 已挂 checkpoint 域)。
//
// 入口四态(评审修正的判定语义,`hasTurnDiff` 是唯一判定面):
// - 有行:写轮的轮末 assistant 行,seq 命中一行且 prev_seq 非空 → true;
// - 无行:只读轮(写触发门零行)或旧 session → false;
// - 基线行:prev_seq === null(基线 = 会话开始前状态,无 diff 入口)→ false;
// - 破链:后端 `CheckpointBroken`(DB 行在、git 对象不在)→ 整 session
//   记 unavailable,全部入口隐藏(list 为空或 Unavailable 即不渲染)。
//
// 缓存刷新:session 切换(ChatPanel watcher)+ 轮终
// (streamEvents.finalizeRequest → refresh)双触发;unavailable 态
// 不重试(能力性缺失,重试只会空转)。

import { ref } from "vue";
import { defineStore } from "pinia";
import { transport } from "../transport";

/** Wire shape of `list_turn_checkpoints` rows (snake_case,与
 * DiffResult 同一条 serde 默认通道)。 */
export interface TurnCheckpoint {
  /** 轮末 assistant 行 messages.seq;基线行挂首轮 user 行 seq。 */
  seq: number;
  /** 前一个存在行的 seq(稀疏链语义,非字面 seq-1);基线为 null。 */
  prev_seq: number | null;
  /** Unix 毫秒。 */
  created_at: number;
  /** 与 prev 快照的变更文件数(徽标用;基线/净零轮为 0)。 */
  files_changed: number;
}

/** 与 diff_worktree 链同构的 diff 载荷(DiffView 直渲)。 */
export interface TurnDiffResult {
  files: Array<{
    path: string;
    status: string;
    added: number;
    removed: number;
    diff_text: string;
  }>;
}

/** per-session 缓存态:ready(行集,可为空数组)或 unavailable
 * (非 git / 群聊 / 无行 / 破链 —— 入口隐藏,不重试)。 */
type CheckpointCacheState =
  | { status: "ready"; rows: TurnCheckpoint[] }
  | { status: "unavailable" };

/** 从 invoke 错误中提取 AppCommandError 的 kind 字段。兼容两通道:
 * - Tauri:rejection 即序列化的 AppCommandError 对象({kind,...});
 * - HTTP:TransportError(.body 携带同一 JSON 形状)。
 * 未识别 → null(按未知错误处理)。 */
export function checkpointErrorKind(e: unknown): string | null {
  if (typeof e !== "object" || e === null) return null;
  const o = e as Record<string, unknown>;
  if (typeof o.kind === "string") return o.kind;
  const body = o.body;
  if (typeof body === "object" && body !== null) {
    const b = body as Record<string, unknown>;
    if (typeof b.kind === "string") return b.kind;
  }
  return null;
}

/** 能力性缺失的两种 kind:入口隐藏而非报错。 */
function isUnavailableKind(e: unknown): boolean {
  const kind = checkpointErrorKind(e);
  return kind === "CheckpointsUnavailable" || kind === "CheckpointBroken";
}

export const useTurnCheckpointsStore = defineStore("turnCheckpoints", () => {
  const bySession = ref(new Map<string, CheckpointCacheState>());
  // in-flight 去重:同一 session 的并发 fetch 共享同一 Promise。
  const inFlight = new Map<string, Promise<void>>();

  async function fetchForSession(sessionId: string): Promise<void> {
    try {
      const rows = await transport.invoke<TurnCheckpoint[]>(
        "list_turn_checkpoints",
        { sessionId },
      );
      // 防御:非数组响应(异常 transport 桩)按 unavailable 兜底,
      // 不让脏数据进入口判定。
      bySession.value.set(
        sessionId,
        Array.isArray(rows)
          ? { status: "ready", rows }
          : { status: "unavailable" },
      );
    } catch (e) {
      if (isUnavailableKind(e)) {
        bySession.value.set(sessionId, { status: "unavailable" });
      } else {
        // 未知错误:保留旧缓存(若有),无缓存则记 unavailable 防抖动。
        console.error("list_turn_checkpoints failed:", e);
        if (!bySession.value.has(sessionId)) {
          bySession.value.set(sessionId, { status: "unavailable" });
        }
      }
    } finally {
      // Map 原地改不触发响应性 —— 换新 Map(与 chat.ts diffCache 同款)。
      bySession.value = new Map(bySession.value);
    }
  }

  /** 拉取(缺缓存时)并缓存。session 切换调用;in-flight 去重。 */
  function ensureLoaded(sessionId: string): void {
    if (bySession.value.has(sessionId) || inFlight.has(sessionId)) return;
    const p = fetchForSession(sessionId).finally(() => {
      inFlight.delete(sessionId);
    });
    inFlight.set(sessionId, p);
  }

  /** 强制重拉(忽略缓存)。轮终(finalizeRequest)与 session 切换
   * 调用;错误静默(入口守卫读取的是缓存态,拉失败只是本轮不显示)。 */
  async function refresh(sessionId: string): Promise<void> {
    const existing = inFlight.get(sessionId);
    if (existing) return existing;
    const p = fetchForSession(sessionId).finally(() => {
      inFlight.delete(sessionId);
    });
    inFlight.set(sessionId, p);
    return p;
  }

  function invalidate(sessionId: string): void {
    if (bySession.value.has(sessionId)) {
      bySession.value.delete(sessionId);
      bySession.value = new Map(bySession.value);
    }
  }

  /** 「本轮 diff」入口判定(唯一判定面,四态语义见文件头):
   *  seq 命中一行且该行有 prev(基线行 prev_seq === null 不给入口)。 */
  function hasTurnDiff(sessionId: string, seq: number | undefined): boolean {
    if (seq === undefined) return false;
    const state = bySession.value.get(sessionId);
    if (!state || state.status !== "ready") return false;
    return state.rows.some((r) => r.seq === seq && r.prev_seq !== null);
  }

  /** 入口点击的 diff 载荷。错误向调用方传播(modal 内联渲染)。 */
  async function fetchTurnDiff(
    sessionId: string,
    seq: number,
  ): Promise<TurnDiffResult> {
    return transport.invoke<TurnDiffResult>("get_turn_checkpoint_diff", {
      sessionId,
      seq,
    });
  }

  return {
    bySession,
    ensureLoaded,
    refresh,
    invalidate,
    hasTurnDiff,
    fetchTurnDiff,
  };
});
