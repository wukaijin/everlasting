// 试点① 键盘类回归(BUGLIST CH5-1 原型):ChatInput 的 CodeMirror
// contenteditable 在真实 Chromium 里的 Enter 语义。
//
// # 为什么这一层测得了而 jsdom 测不了
// `chatInputCodeMirror.ts` 的 keymap 只绑 `Enter`(submit);Shift+Enter
// **没有**任何 CM keymap 接管,走 contenteditable 的浏览器默认行为
// (插入换行,DOMObserver 回读 doc)。jsdom 的合成 keydown 不触发
// CM 的 key 处理链(contenteditable 默认编辑行为缺失),只有真实
// trusted input(Playwright CDP)能驱动 —— 这正是 RULE-TEST-001
// 登记的盲区。
//
// # 断言策略(implement.md 风险注意)
// - Shift+Enter:看**编辑器 DOM**(真实行 = `.cm-line` 数量;软换行不
//   拆行),不看 store —— CM doc 是唯一事实源。
// - Enter 发送:看**网络观察**(route dispatcher 记录的 POST),不依赖
//   合成事件语义。发送链 = 懒建 session(POST sessions/create_session)
//   → agent/chat;请求体顶层键为 snake_case(transformArgsTopLevel,
//   见 fixtures.ts 文件头)。
import { expect } from "@playwright/test";
import { EDITOR, EDITOR_LINE, test } from "./fixtures";

/** 发送链会打出的 cmd(断言「未发送」用;其余 boot 命令不在此列)。 */
function sendFlowRequests(
  reqs: Array<{ method: string; cmd: string | null }>,
): Array<{ method: string; cmd: string | null }> {
  return reqs.filter(
    (r) =>
      r.method === "POST" &&
      (r.cmd === "chat" ||
        r.cmd === "create_session" ||
        r.cmd === "load_session"),
  );
}

test.describe("ChatInput 真实键盘(CH5-1)", () => {
  test("Shift+Enter 插入换行且不发送", async ({ page, boot, reqs }) => {
    await boot();
    await page.click(EDITOR);
    await page.keyboard.type("hello");
    await page.keyboard.press("Shift+Enter");
    await page.keyboard.type("world");

    // 编辑器 DOM:两个真实行(enter 分行),文本各就各位。
    const lines = page.locator(EDITOR_LINE);
    await expect(lines).toHaveCount(2);
    await expect(lines.nth(0)).toHaveText("hello");
    await expect(lines.nth(1)).toHaveText("world");

    // 留出「假如误发送」的窗口(发送链首个 POST 是同步微任务路径,
    // 300ms 足够它落网),再断言网络面零发送。
    await page.waitForTimeout(300);
    expect(sendFlowRequests(reqs)).toEqual([]);
  });

  test("Enter 发送:懒建 session + chat POST,编辑器清空", async ({
    page,
    boot,
    waitForCmd,
  }) => {
    await boot();
    await page.click(EDITOR);
    await page.keyboard.type("hello e2e");
    await page.keyboard.press("Enter");

    // 懒建 session(currentSessionId=null → createNewSession 先行;
    // 请求体顶层键经 transformArgsTopLevel 转 snake_case)。
    const create = await waitForCmd("sessions", "create_session");
    expect(create.body).toMatchObject({
      project_id: "e2e-project",
      initial_cwd: "/home/e2e/e2e-project",
    });

    // chat 受理请求:history 首条 = 刚输入的 user 消息,request_id /
    // session_id 已填(mock 的 create_session 应答 id)。
    const chat = await waitForCmd("agent", "chat");
    const chatBody = chat.body as {
      request_id?: unknown;
      session_id?: unknown;
      messages?: Array<{ role?: string; content?: unknown }>;
    };
    expect(chatBody.messages?.[0]).toMatchObject({
      role: "user",
      content: "hello e2e",
    });
    expect(typeof chatBody.request_id).toBe("string");
    expect(chatBody.session_id).toBe("e2e-session-1");

    // 编辑器清空:doc 清空 → 占位符回归(占位文本是 .cm-placeholder
    // widget 的 textContent,不能拿 .cm-line 文本判空)。
    const lines = page.locator(EDITOR_LINE);
    await expect(lines).toHaveCount(1);
    await expect(
      page.locator(".chat-input__field .cm-placeholder"),
    ).toBeVisible();
  });
});
