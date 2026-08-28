// F1 消息队列 (2026-08-25): 排队视图 store。
//
// 后端 `AppState.message_queues` 是唯一事实源(SoT);本 store 只持有
// **视图副本**(`list_queued_messages` 水合),用于:
// 1. 切 session / 页面刷新后恢复排队徽标(消除"后端持有但看不见");
// 2. Stop / 错误终止后的保留计数 toast;
// 3. R8 单条撤销/退回的视图同步(寻址一律按后端 uuid,见 revoke /
//    recallToComposer —— 位置随增删漂移,不可用于寻址,评审 Round 2 P1)。
//
// 入队/撤销不广播事件(design §9):他端与本地都靠水合看到最终态,
// 非实时是已接受的 MVP trade-off。归 chat store 家族(state-management
// spec:排队视图不是流状态,不进 streamController)。
import { defineStore } from "pinia";
import { ref } from "vue";
import { transport } from "../transport";
import { extractErrorMessage } from "../utils/useErrorBus";
import { useProjectsStore } from "./projects";
import type { QueuedTaskOrigin } from "./chat.types";

export interface QueuedEntry {
  /** 后端 uuid(R8 remove/recall 按 id 寻址,不用会漂移的位置)。 */
  id: string;
  text: string;
  position: number;
  /** F2 定时任务(2026-08-28):来源标记(`QueuedMessage.origin`)。
   *  调度器 fire 的条目携带 → 排队占位气泡显「定时」徽标;用户手发
   *  条目恒 undefined(serde skip_serializing_if None 不上 wire)。 */
  origin?: QueuedTaskOrigin;
}

export const useMessageQueueStore = defineStore("messageQueue", () => {
  // sessionId -> 有序排队项(位置升序)。
  const queuedBySession = ref(new Map<string, QueuedEntry[]>());
  // R8「修改」= 退回输入框:跨组件传递回填文本(ChatInput watch 草稿
  // 本身 —— 当前 session 内 recall 也要立即回填,评审 Round 2 P1)。
  const recallDraft = ref<{ sessionId: string; text: string } | null>(null);

  function entriesFor(sessionId: string | null | undefined): QueuedEntry[] {
    return (sessionId && queuedBySession.value.get(sessionId)) || [];
  }

  /** 从后端 SoT 重建该 session 的排队视图(幂等)。返回重建后的
   *  entries(空数组 = 队列已空),供调用方物化占位气泡(R4)。 */
  async function hydrate(sessionId: string): Promise<QueuedEntry[]> {
    let entries: QueuedEntry[] = [];
    try {
      const rows = await transport.invoke<
        Array<{
          id: string;
          message: { content: unknown };
          position?: number;
          origin?: QueuedTaskOrigin;
        }>
      >("list_queued_messages", { sessionId });
      entries = rows.map((r, i) => ({
        id: r.id,
        text: flattenText(r.message?.content),
        position: r.position ?? i + 1,
        origin: r.origin,
      }));
      queuedBySession.value.set(sessionId, entries);
    } catch (e) {
      // 水合失败不阻塞 UI —— 视图缺页只影响徽标精度。
      useProjectsStore().showToast(
        `排队视图刷新失败：${extractErrorMessage(e)}`,
        "warn",
      );
    }
    return entries;
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

  /** 视图内按 id 移除一条并重排剩余位次。 */
  function dropEntryFromView(sessionId: string, id: string): void {
    const list = queuedBySession.value.get(sessionId) ?? [];
    const idx = list.findIndex((x) => x.id === id);
    if (idx >= 0) list.splice(idx, 1);
    list.forEach((x, i) => (x.position = i + 1));
  }

  /** R8 撤销:按后端 uuid 删除单条并同步视图。失败(not-found =
   *  已开始注入)toast「已开始处理」。占位气泡的移除由调用方
   *  (MessageItem)经 streamController.dropQueuedPlaceholder 完成。 */
  async function revoke(sessionId: string, id: string): Promise<boolean> {
    try {
      await transport.invoke("remove_queued_message", { sessionId, id });
    } catch (e) {
      useProjectsStore().showToast(
        `无法撤销：${extractErrorMessage(e)}`,
        "warn",
      );
      void hydrate(sessionId);
      return false;
    }
    dropEntryFromView(sessionId, id);
    return true;
  }

  /** R8 修改 = 单条退回输入框:recall IPC 按 id 取原文 → 写回填草稿
   *  (ChatInput watch recallDraft 消费)。 */
  async function recallToComposer(sessionId: string, id: string): Promise<boolean> {
    let text = "";
    try {
      const row = await transport.invoke<{
        message: { content: unknown };
      }>("recall_queued_message", { sessionId, id });
      text = flattenText(row.message?.content);
    } catch (e) {
      useProjectsStore().showToast(
        `无法退回：${extractErrorMessage(e)}`,
        "warn",
      );
      void hydrate(sessionId);
      return false;
    }
    dropEntryFromView(sessionId, id);
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
