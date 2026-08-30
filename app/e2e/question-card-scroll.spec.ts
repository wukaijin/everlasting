// 试点② 滚动+store 联动回归(BUGLIST CH8-2 / CH8-2b 原型):
// 提问卡(tool:question)与排队发送在真实滚动容器里的行为。
//
// # 被测面(为什么在浏览器层)
// MessageList 的强制回底是「pending 状态 null→some 的 watch → scrollTop
// 赋值」的跨组件联动;jsdom 里 scrollTo/scrollIntoView 是 mock 的、
// scrollHeight 恒 0,只有真实 Chromium 能给出行为值。断言全部用
// scrollTop/scrollHeight 行为值(implement.md 风险注意),不用截图。
//
// # 数据驱动方式
// - 历史消息:`sessions/load_session` mock 种子(LoadedSession wire,
//   streamRehydrate.ts 消费)—— 权威历史读入走 load_session,不存在
//   list_messages;覆盖默认注册表的 list_sessions 让 boot 后即有当前会话。
// - 提问卡:`stream.emit("tool:question", …)` 推 ToolQuestionPayload
//   (snake_case,questionCards.types.ts 镜像 Rust 结构)。卡片渲染的
//   前提:消息流里有 name=ask_user_question 的 tool_use 块且 id 与
//   payload.tool_use_id 精确匹配(askCard.ts resolveAskCardState)。
// - 流式态:裸 HTTP 发起的轮次事件带 `session_id` 时会被跨客户端认领
//   (streamEvents.adoptForeignRequest,2026-08-27)—— emit 一条
//   chat-event start 即让 isCurrentSessionStreaming 翻 true,无需伪造
//   前端发送路径。
// - 排队提示:send() 里 `queueingClassic && getPending(sid)` 分支
//   (chatSendActions CH8-2b)。⚠️ send 先过 ensureLoaded 的权威拉平
//   (get_pending_interaction,spec test-environment.md §8):mock 必须
//   与 seed 一致地返回同一条 pending,否则拉平会把卡片清掉、toast 不来。
import { expect, type Page } from "@playwright/test";
import { EDITOR, test, type MockPayload } from "./fixtures";

/** 提问卡事件 payload(tool:question 通道,snake_case)。 */
const QUESTION = {
  session_id: "e2e-session-1",
  tool_use_id: "toolu-e2e-q1",
  questions: [
    {
      question: "要继续部署到生产环境吗?",
      header: "确认",
      options: [{ label: "继续部署" }, { label: "先停下" }],
      multi_select: false,
    },
  ],
  ts: 1_700_000_000_000,
};

/** 权威拉平(get_pending_interaction)与 seed 一致的应答。 */
const PENDING_ENTRY: MockPayload = { kind: "question", payload: QUESTION };

/** 视口可滚动的种子历史:8 轮长文本 + 尾部 assistant 挂 ask_user_question
 *  tool_use(与 QUESTION.tool_use_id 配对,卡片渲染的锚点)。 */
function seededSession(): MockPayload {
  const long = (tag: string) =>
    `${tag}:${"这是一段用于撑高消息列表的种子文本。".repeat(12)}`;
  const messages: Array<Record<string, unknown>> = [];
  let seq = 0;
  const row = (
    role: "user" | "assistant",
    text: string,
    extra: Record<string, unknown> = {},
  ) => {
    seq += 1;
    messages.push({
      id: seq,
      session_id: "e2e-session-1",
      role,
      content: [{ type: "text", text }],
      text,
      has_tool_calls: false,
      has_tool_results: false,
      created_at: "2026-01-01T00:00:00Z",
      seq,
      ttfb_ms: null,
      gen_ms: null,
      total_ms: null,
      thinking_ms: null,
      ...extra,
    });
  };
  for (let i = 0; i < 8; i += 1) {
    row("user", long(`历史问题 ${i}`));
    row("assistant", long(`历史回答 ${i}`));
  }
  // 尾部:assistant 带 ask_user_question tool_use 块(无 tool_result
  // → resolveAskCardState 走 live pending 分支,卡片渲染 pending 态)。
  seq += 1;
  messages.push({
    id: seq,
    session_id: "e2e-session-1",
    role: "assistant",
    content: [
      { type: "text", text: "部署前需要你确认。" },
      {
        type: "tool_use",
        id: QUESTION.tool_use_id,
        name: "ask_user_question",
        input: { questions: QUESTION.questions },
      },
    ],
    text: "部署前需要你确认。",
    has_tool_calls: true,
    has_tool_results: false,
    created_at: "2026-01-01T00:00:00Z",
    seq,
    ttfb_ms: null,
    gen_ms: null,
    total_ms: null,
    thinking_ms: null,
  });
  return {
    session: {
      id: "e2e-session-1",
      title: "e2e 种子会话",
      created_at: "2026-01-01T00:00:00Z",
      updated_at: "2026-01-01T00:00:00Z",
      model: "",
      project_id: "e2e-project",
      current_cwd: "/home/e2e/e2e-project",
      worktree_state: "none",
      worktree_path: null,
      last_worktree_path: null,
      model_id: null,
      input_tokens_total: null,
      output_tokens_total: null,
      cache_creation_total: null,
      cache_read_total: null,
      session_type: "chat",
      metadata: null,
    },
    messages,
  };
}

/** 让 boot 后即有一个带历史的当前会话(覆盖默认空注册表)。 */
function seedSessionMocks(
  mockCmd: (domain: string, cmd: string, payload: MockPayload) => void,
): void {
  mockCmd("sessions", "list_sessions", [
    {
      id: "e2e-session-1",
      title: "e2e 种子会话",
      updated_at: "2026-01-01T00:00:00Z",
      preview: "…",
      project_id: "e2e-project",
      current_cwd: "/home/e2e/e2e-project",
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
      plugin_name: "",
      session_type: "chat",
      metadata: null,
    },
  ]);
  mockCmd("sessions", "load_session", seededSession());
}

/** 滚动容器 `ul.messages` 的「距底距离」(与 isNearBottom 同式,
 *  MessageList 的阈值 80px)。容器未挂载时返回 +∞。 */
async function distanceFromBottom(page: Page): Promise<number> {
  return page.evaluate(() => {
    const el = document.querySelector("ul.messages");
    if (!el) return Number.POSITIVE_INFINITY;
    return el.scrollHeight - el.scrollTop - el.clientHeight;
  });
}

test.describe("提问卡 × 滚动联动(CH8-2)", () => {
  test("上滚读历史时收到 tool:question → 强制回底", async ({
    page,
    boot,
    mockCmd,
    stream,
  }) => {
    seedSessionMocks(mockCmd);
    await boot();

    // 挂载 stickToBottomUntilStable 完成的行为信号:已钉在底部
    // (距底 < 80px = MessageList 的 near-bottom 阈值)。
    await expect
      .poll(() => distanceFromBottom(page), { timeout: 10_000 })
      .toBeLessThan(80);
    // 钉底状态下回底按钮不出现。
    await expect(page.locator(".scroll-to-bottom")).toHaveCount(0);

    // 用户上滚到顶(直接写 scrollTop,scroll 事件照常触发 onScroll)。
    await page.evaluate(() => {
      const el = document.querySelector("ul.messages")!;
      el.scrollTop = 0;
    });
    await expect(page.locator(".scroll-to-bottom")).toBeVisible();

    // 推提问卡 → pending null→some watch → 强制回底。
    await stream.emit("tool:question", QUESTION);
    await expect
      .poll(() => distanceFromBottom(page), { timeout: 10_000 })
      .toBeLessThan(80);
    // 回底按钮随 isAtBottom 复位消失;提问卡以 pending 态渲染在
    // 匹配的 tool_use 之下(联动的事实源,不只是滚动值变了)。
    await expect(page.locator(".scroll-to-bottom")).toHaveCount(0);
    await expect(page.locator(".ask-card")).toContainText("等待你回答");
    await expect(page.locator(".ask-card")).toContainText(
      "要继续部署到生产环境吗?",
    );
  });

  test("pending 未答 + 流式中发送 → 排队并存提示(CH8-2b)", async ({
    page,
    boot,
    mockCmd,
    stream,
    waitForCmd,
  }) => {
    seedSessionMocks(mockCmd);
    // 权威拉平与 seed 一致(send → ensureLoaded 会重拉;若回 null 会
    // 把 pending 清掉,toast 分支静默不触发 —— test-environment §8)。
    mockCmd("question", "get_pending_interaction", PENDING_ENTRY);
    // 经典 session 流式中发送走后端「闲也入队」受理(F1)。
    mockCmd("agent", "chat", { status: "queued", id: "q-e2e-1", position: 1 });
    await boot();

    // 铺垫并存前提:提问卡 pending + 流式态(跨客户端认领外来轮次)。
    await stream.emit("tool:question", QUESTION);
    await stream.emit("chat-event", {
      request_id: "rid-e2e-foreign",
      session_id: "e2e-session-1",
      kind: "start",
    });
    // 流式态的行为信号:输入行进入 streaming 视觉态。
    await expect(page.locator(".chat-input__row--streaming")).toBeVisible();

    await page.click(EDITOR);
    await page.keyboard.type("排队我也要发");
    await page.keyboard.press("Enter");

    // 发送确实走出了 chat POST(排队受理),且 toast 说明并存语义;
    // 排队气泡带位次徽标(assistant 占位被回收 = 不开新轮)。
    // history 含种子历史 + 外来流占位,只断言尾部 user 行(本条发送)。
    const chat = await waitForCmd("agent", "chat");
    expect(chat.body).toMatchObject({ session_id: "e2e-session-1" });
    const history = (chat.body as { messages: Array<{ role: string; content: unknown }> })
      .messages;
    expect(history.at(-1)).toMatchObject({
      role: "user",
      content: "排队我也要发",
    });
    await expect(page.locator(".toast.toast--warn")).toContainText(
      "已排队",
    );
    await expect(page.locator(".toast.toast--warn")).toContainText(
      "提问卡",
    );
    await expect(page.locator(".f1-queued-chip")).toContainText("第 1 位");
  });
});
