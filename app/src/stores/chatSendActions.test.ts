// RULE-FE-001 (2026-08-27) — staged 图片 objectURL 的三路 revoke 契约。
//
// B1 strip 生命周期此前零覆盖;本文件以 spy 注入锁定调用次数契约
// (泄漏本身用 DevTools memory 面板人工验证,单测只锁调用契约):
//   1. send 成功:本轮每个 staged URL revoke 恰好一次 + strip 清空(AC1);
//   2. save_attachment 失败:零 revoke、strip 保留、不 fire `chat`
//      IPC(P1-3 无部分发送),恢复后可重试成功(AC2);
//   3. removeStagedImage / discardStagedImages 各自 revoke 对应 URL(AC3);
//   4. 纯文本 send:零 revoke(防误伤)。
//
// 脚手架:jsdom 没有 URL.createObjectURL / revokeObjectURL 实现,用
// Object.defineProperty 在 URL 构造器上注入 revoke spy(只注入 revoke:
// StagedImage 手工构造、假 url 字符串,绕过 addStagedImages 的压缩 /
// 读尺寸链路,不需要 createObjectURL)。send 经 useChatStore 驱动,
// 与 production 同路(ChatInput emit strip 数组 → ChatWindow
// `store.send(text, staged)`);transport mock 仿 chatSend.test.ts /
// messageQueueStream.test.ts。

import { beforeEach, describe, expect, it, vi } from "vitest";
import { createPinia, setActivePinia } from "pinia";
import { nextTick } from "vue";

// jsdom 无原生实现 —— 注入 revoke spy(模块级一次,用例里 mockClear)。
const revokeSpy = vi.fn();
Object.defineProperty(URL, "revokeObjectURL", {
  configurable: true,
  writable: true,
  value: revokeSpy,
});

// Single shared invoke mock (chatSend.test.ts 同款):`list_sessions`
// returns [] so the project-change watcher doesn't throw;`load_session`
// returns an empty history;`save_attachment` succeeds by default and is
// overridden per-test to reject. Everything else returns null (Tauri
// unit).
// eslint-disable-next-line @typescript-eslint/no-explicit-any
const invokeMock: any = vi.fn();
async function defaultInvoke(cmd: string): Promise<unknown> {
  if (cmd === "list_sessions") return [];
  if (cmd === "load_session") return { messages: [] };
  if (cmd === "save_attachment") return { file: "att-x" };
  return null;
}
invokeMock.mockImplementation(defaultInvoke);
vi.mock("../transport", () => ({
  transport: {
    invoke: (...args: unknown[]) => invokeMock(...args),
    listen: async () => () => {},
  },
}));

import { useChatStore } from "./chat";
import { useProjectsStore } from "./projects";
import type { SessionSummary, StagedImage } from "./chat.types";

/** A minimal but well-typed SessionSummary seed (chatSend.test.ts
 *  同款). Only `session_type` matters here (`send` reads it for the
 *  group-chat preempt branch); the rest are inert defaults. */
function seedSession(id: string, sessionType: SessionSummary["session_type"]): SessionSummary {
  return {
    id,
    title: "t",
    updated_at: "",
    preview: "",
    project_id: "p1",
    current_cwd: "/tmp",
    worktree_path: null,
    worktree_state: "none",
    last_worktree_path: null,
    model_id: null,
    input_tokens_total: null,
    output_tokens_total: null,
    cache_creation_total: null,
    cache_read_total: null,
    last_context_input_tokens: null,
    last_input_tokens: null,
    last_output_tokens: null,
    last_cache_creation: null,
    last_cache_read: null,
    color_tag: null,
    mode: "edit",
    workflow_enabled: false,
    plugin_name: "dev",
    session_type: sessionType,
    metadata: null,
  };
}

/** Construct a StagedImage directly with a FAKE url string — bypasses
 *  `addStagedImages`' compress / dimension-read chain (which needs a
 *  real decoder); `file` only needs to be FileReader-readable for
 *  `uploadStagedImages`' base64 pass. */
function stagedImage(url: string): StagedImage {
  return {
    url,
    file: new File([new Uint8Array([1, 2, 3])], "paste.png", {
      type: "image/png",
    }),
    w: 10,
    h: 10,
    tokensEst: 1,
  };
}

/** URLs revoked so far, in call order. */
function revokedUrls(): string[] {
  return revokeSpy.mock.calls.map((c) => c[0]);
}

/** IPC command names invoked so far. */
function invokedCmds(): string[] {
  return (invokeMock.mock.calls as Array<[string, unknown]>).map((c) => c[0]);
}

describe("chatSendActions — staged objectURL revoke lifecycle (RULE-FE-001)", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    invokeMock.mockReset();
    invokeMock.mockImplementation(defaultInvoke);
    revokeSpy.mockClear();
  });

  /** chatSend.test.ts 同款 setup + one extra `nextTick`: the B1
   *  `watch(currentSessionId, discardStagedImages)` in chat.ts fires
   *  on our stamp — it MUST be flushed BEFORE the test seeds the
   *  strip, or the deferred watcher would fire during `send`'s first
   *  await, clear the seeded strip, and pollute the revoke
   *  assertions. */
  async function setupProjectAndSession(
    sessionId: string,
  ): Promise<ReturnType<typeof useChatStore>> {
    const projects = useProjectsStore();
    projects.currentProjectId = "p1";
    const store = useChatStore();
    // Let the async `onProjectChange` watcher settle (it sets
    // sessions=[] and currentSessionId=null).
    await Promise.resolve();
    await Promise.resolve();
    store.sessions = [seedSession(sessionId, "chat")];
    store.currentSessionId = sessionId;
    await nextTick();
    expect(store.stagedImages).toEqual([]);
    return store;
  }

  it("successful send revokes every staged localUrl exactly once and clears the strip", async () => {
    const store = await setupProjectAndSession("s1");
    store.stagedImages.push(stagedImage("blob:stage-1"), stagedImage("blob:stage-2"));

    // Production shape: ChatWindow passes the strip array itself.
    await store.send("看图", store.stagedImages);

    // Each URL revoked exactly once, in staged order; strip cleared.
    expect(revokedUrls()).toEqual(["blob:stage-1", "blob:stage-2"]);
    expect(store.stagedImages).toEqual([]);
  });

  it("save_attachment rejection: zero revokes, strip kept, retry succeeds after recovery", async () => {
    const store = await setupProjectAndSession("s1");
    store.stagedImages.push(stagedImage("blob:keep-1"), stagedImage("blob:keep-2"));

    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "save_attachment") throw new Error("disk full");
      return defaultInvoke(cmd);
    });
    await store.send("will fail", store.stagedImages);

    // P1-3: whole send aborted (no `chat` IPC), strip kept for retry,
    // and NO revoke — the images must keep rendering on the strip.
    expect(revokedUrls()).toEqual([]);
    expect(store.stagedImages.map((s) => s.url)).toEqual(["blob:keep-1", "blob:keep-2"]);
    expect(invokedCmds()).not.toContain("chat");

    // Recovery: same store state retries fine and now revokes.
    invokeMock.mockImplementation(defaultInvoke);
    await store.send("retry", store.stagedImages);
    expect(revokedUrls()).toEqual(["blob:keep-1", "blob:keep-2"]);
    expect(store.stagedImages).toEqual([]);
  });

  it("removeStagedImage and discardStagedImages revoke their own URLs", async () => {
    const store = await setupProjectAndSession("s1");
    store.stagedImages.push(
      stagedImage("blob:a"),
      stagedImage("blob:b"),
      stagedImage("blob:c"),
    );

    // ✕ button: only the removed entry's URL is revoked.
    store.removeStagedImage(1);
    expect(revokedUrls()).toEqual(["blob:b"]);
    expect(store.stagedImages.map((s) => s.url)).toEqual(["blob:a", "blob:c"]);

    // Session-switch discard: the remaining entries, strip emptied.
    store.discardStagedImages();
    expect(revokedUrls()).toEqual(["blob:b", "blob:a", "blob:c"]);
    expect(store.stagedImages).toEqual([]);
  });

  it("text-only send performs zero revokes", async () => {
    const store = await setupProjectAndSession("s1");
    await store.send("plain text");
    expect(revokedUrls()).toEqual([]);
  });
});
