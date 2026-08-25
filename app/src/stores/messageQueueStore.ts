// F1 消息队列 (2026-08-25): 排队视图 store。
//
// 后端 `AppState.message_queues` 是唯一事实源(SoT);本 store 只持有
// **视图副本**(`list_queued_messages` 水合),用于:
// 1. 切 session / 页面刷新后恢复排队徽标(消除"后端持有但看不见");
// 2. Stop / 错误终止后的保留计数 toast;
// 3. R8 单条撤销/退回的 id 寻址(占位气泡按 position 对齐)。
//
// 入队/撤销不广播事件(design §9):他端与本地都靠水合看到最终态,
// 非实时是已接受的 MVP trade-off。归 chat store 家族(state-management
// spec:排队视图不是流状态,不进 streamController)。
import { defineStore } from "pinia";
import { ref } from "vue";
import { transport } from "../transport";
import { extractErrorMessage } from "../utils/useErrorBus";
import { useProjectsStore } from "./projects";

export interface QueuedEntry {
  /** 后端 uuid(R8 remove/recall 按 id 寻址,不用会漂移的位置)。 */
  id: string;
  text: string;
  position: number;
}

export const useMessageQueueStore = defineStore("messageQueue", () => {
  // sessionId -> 有序排队项(位置升序)。
  const queuedBySession = ref(new Map<string, QueuedEntry[]>());
  // R8「修改」= 退回输入框:跨组件传递回填文本(ChatInput watch)。
  const recallDraft = ref<{ sessionId: string; text: string } | null>(null);

  function entriesFor(sessionId: string | null | undefined): QueuedEntry[] {
    return (sessionId && queuedBySession.value.get(sessionId)) || [];
  }

  /** 从后端 SoT 重建该 session 的排队视图(幂等)。 */
  async function hydrate(sessionId: string): Promise<void> {
    try {
      const rows = await transport.invoke<
        Array<{ id: string; message: { content: unknown }; position?: number }>
      >("list_queued_messages", { sessionId });
      const entries: QueuedEntry[] = rows.map((r, i) => ({
        id: r.id,
        text: flattenText(r.message?.content),
        position: r.position ?? i + 1,
      }));
      queuedBySession.value.set(sessionId, entries);
    } catch (e) {
      // 水合失败不阻塞 UI —— 视图缺页只影响徽标精度。
      useProjectsStore().showToast(
        `排队视图刷新失败：${extractErrorMessage(e)}`,
        "warn",
      );
    }
  }

  /** 续轮注入边界:同步弹走前 count 条(随后 hydrate 对账)。 */
  function shiftFront(sessionId: string, count: number): void {
    const list = queuedBySession.value.get(sessionId);
    if (!list) return;
    list.splice(0, Math.min(count, list.length));
  }

  function clearSession(sessionId: string | null | undefined): void {
    if (sessionId) queuedBySession.value.delete(sessionId);
  }

  /** R8 撤销:删除单条并从视图移除。失败(not-found = 已开始注入)
   *  toast「已开始处理」。成功后重排剩余位次。*/
  async function revoke(sessionId: string, entry: QueuedEntry): Promise<boolean> {
    try {
      await transport.invoke("remove_queued_message", {
        sessionId,
        id: entry.id,
      });
    } catch (e) {
      useProjectsStore().showToast(
        `无法撤销：${extractErrorMessage(e)}`,
        "warn",
      );
      void hydrate(sessionId);
      return false;
    }
    const list = queuedBySession.value.get(sessionId) ?? [];
    const idx = list.findIndex((x) => x.id === entry.id);
    if (idx >= 0) list.splice(idx, 1);
    list.forEach((x, i) => (x.position = i + 1));
    return true;
  }

  /** R8 修改 = 单条退回输入框:recall IPC 取原文 → 回填 composer。
   *  ChatInput 通过 takeRecallDraft 消费。*/
  async function recallToComposer(sessionId: string, entry: QueuedEntry): Promise<boolean> {
    let text = entry.text;
    try {
      const row = await transport.invoke<{
        message: { content: unknown };
      }>("recall_queued_message", { sessionId, id: entry.id });
      text = flattenText(row.message?.content);
    } catch (e) {
      useProjectsStore().showToast(
        `无法退回：${extractErrorMessage(e)}`,
        "warn",
      );
      void hydrate(sessionId);
      return false;
    }
    const list = queuedBySession.value.get(sessionId) ?? [];
    const idx = list.findIndex((x) => x.id === entry.id);
    if (idx >= 0) list.splice(idx, 1);
    list.forEach((x, i) => (x.position = i + 1));
    recallDraft.value = { sessionId, text };
    return true;
  }

  /** ChatInput 消费一次回填草稿(仅匹配当前 session)。 */
  function takeRecallDraft(sessionId: string | null | undefined): string | null {
    const d = recallDraft.value;
    if (d && sessionId && d.sessionId === sessionId) {
      recallDraft.value = null;
      return d.text;
    }
    return null;
  }

  return {
    queuedBySession,
    recallDraft,
    entriesFor,
    hydrate,
    shiftFront,
    clearSession,
    revoke,
    recallToComposer,
    takeRecallDraft,
  };
});

/** MessageContent wire 形态(text 或 blocks 数组)拍平成纯文本预览。 */
function flattenText(content: unknown): string {
  if (typeof content === "string") return content;
  if (Array.isArray(content)) {
    return content
      .map((b) => (typeof b?.text === "string" ? b.text : ""))
      .filter(Boolean)
      .join("\n");
  }
  return "";
}
