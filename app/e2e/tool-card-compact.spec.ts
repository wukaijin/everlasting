// read 族工具卡片紧凑化(09-19-tool-card-compact-read)浏览器回归:
// glob / list_dir / read_file 三工具的卡片高度门 + 展开/收起交互。
//
// # 被测面(为什么在浏览器层)
// 「卡片占几行」由真实字体度量 + flex 换行决定,jsdom 无布局引擎
// (offsetHeight 恒 0),只能在这里断言。同时守护展开动作是真实指针
// 点击 + 原生 button 的键盘可达性。
//
// # 数据驱动方式
// - 历史消息:`sessions/load_session` mock 种子(LoadedSession wire,
//   streamRehydrate.ts 消费),与 question-card-scroll.spec.ts 同法。
// - 覆盖形态:glob 成功(带路径列表)/ glob 截断提示 / list_dir 目录
//   / read_file 全文 / read_file 带 offset+limit 的 range / read_file
//   报错。
import { expect, type Page } from "@playwright/test";
import { test, type MockPayload } from "./fixtures";

/** 单行卡片的纵向预算(design §3:桌面 1 行 = 卡片 padding 2+2 + 行高
 *  15.6 + 边框 2 ≈ 22px,实测 **26px**)。34px 的门给字体/缩放抖动留
 *  余量,同时挡回「header + input + output」三行旧形态 —— 那实测 85px。 */
const ONE_LINE_MAX = 34;

/** read 族卡片根:紧凑卡 `.rocard`;回退态是通用 `.tool-card`
 *  (本用例的种子全是 read 族,出现 .tool-card 即回归)。 */
const CARD = ".rocard";

function msg(
  seq: number,
  role: "user" | "assistant",
  content: Array<Record<string, unknown>>,
  text: string,
): Record<string, unknown> {
  return {
    id: seq,
    session_id: "e2e-session-1",
    role,
    content,
    text,
    has_tool_calls: role === "assistant",
    has_tool_results: false,
    created_at: "2026-01-01T00:00:00Z",
    seq,
    ttfb_ms: null,
    gen_ms: null,
    total_ms: null,
    thinking_ms: null,
  };
}

/** tool_result 的 wire 形态:后端包一层 `{result, cwd}` 信封。 */
function result(id: string, out: string, isError = false, durationMs = 12) {
  return {
    type: "tool_result",
    tool_use_id: id,
    content: JSON.stringify({ result: out, cwd: "/home/e2e/e2e-project" }),
    is_error: isError,
    duration_ms: durationMs,
  };
}

const GLOB_OUT = [
  "app/src/App.vue",
  "app/src/main.ts",
  "app/src/components/Icon.vue",
  "app/src/components/chat/ToolCallCard.vue",
  "app/src/components/chat/MessageItem.vue",
  "app/src/views/ChatView.vue",
].join("\n");

const LIST_OUT = [
  "chat/",
  "search/",
  "ActivityPanel.vue",
  "ChatPanel.vue",
  "MessageItem.vue",
  "ToolCallCard.vue",
  "ToolInputBody.vue",
  "ToolOutputBody.vue",
].join("\n");

/** read_file 的 cat -n 形态(制表符 + 行号 + 制表符 + 正文)。 */
const READ_OUT = [
  '<script setup lang="ts">',
  'import ChatWindow from "./components/ChatWindow.vue";',
  "const theme = useTheme();",
  "</script>",
]
  .map((line, i) => `\t${i + 1}\t${line}`)
  .join("\n");

const GLOB_TRUNCATED_OUT = `${GLOB_OUT}\n\n(...and 37 more matches; narrow your pattern to see them)`;

function seededSession(): MockPayload {
  const rows: Array<Record<string, unknown>> = [];
  let seq = 0;
  const push = (
    role: "user" | "assistant",
    content: Array<Record<string, unknown>>,
    text: string,
  ) => {
    seq += 1;
    rows.push(msg(seq, role, content, text));
  };
  push(
    "user",
    [{ type: "text", text: "看一下聊天面板的工具卡片实现。" }],
    "看一下聊天面板的工具卡片实现。",
  );
  push(
    "assistant",
    [
      { type: "text", text: "先定位相关文件。" },
      { type: "tool_use", id: "tu-glob", name: "glob", input: { pattern: "src/**/*.vue" } },
      result("tu-glob", GLOB_OUT, false, 128),
      { type: "tool_use", id: "tu-list", name: "list_dir", input: { path: "app/src/components/chat" } },
      result("tu-list", LIST_OUT, false, 34),
      { type: "tool_use", id: "tu-read", name: "read_file", input: { path: "app/src/App.vue" } },
      result("tu-read", READ_OUT, false, 21),
      {
        type: "tool_use",
        id: "tu-read-range",
        name: "read_file",
        input: { path: "app/src-tauri/src/agent/loop.rs", offset: 320, limit: 120 },
      },
      result("tu-read-range", READ_OUT, false, 12),
      { type: "tool_use", id: "tu-glob-cap", name: "glob", input: { pattern: "**/*.ts" } },
      result("tu-glob-cap", GLOB_TRUNCATED_OUT, false, 402),
      {
        type: "tool_use",
        id: "tu-read-err",
        name: "read_file",
        input: { path: "app/src/DoesNotExist.vue" },
      },
      result(
        "tu-read-err",
        "Failed to read file '/home/e2e/e2e-project/app/src/DoesNotExist.vue': No such file or directory (os error 2)",
        true,
        3,
      ),
    ],
    "先定位相关文件。",
  );
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
    messages: rows,
  };
}

const SESSION_ROW = {
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
};

async function cardBoxes(page: Page): Promise<Array<{ h: number; text: string }>> {
  return page.$$eval(CARD, (els) =>
    els.map((el) => ({
      h: Math.round((el as HTMLElement).getBoundingClientRect().height),
      text: ((el as HTMLElement).innerText || "").replace(/\s+/g, " ").trim(),
    })),
  );
}

test.describe("read 族工具卡片紧凑化", () => {
  test.beforeEach(async ({ mockCmd, boot, page }) => {
    mockCmd("sessions", "list_sessions", [SESSION_ROW]);
    mockCmd("sessions", "load_session", seededSession());
    await boot();
    await page.waitForSelector(CARD);
  });

  test("六个形态全部收成 1 行", async ({ page }) => {
    const boxes = await cardBoxes(page);
    expect(boxes.length).toBe(6);
    expect(boxes.map((b) => b.h).every((h) => h <= ONE_LINE_MAX)).toBe(true);
    // 通用卡片不该再出现(种子全是 read 族)。
    expect(await page.locator(".tool-card").count()).toBe(0);
  });

  test("默认收起:输出不在 DOM 里;点击行展开后才出现", async ({ page }) => {
    const first = page.locator(CARD).first();
    await expect(first.locator(".rocard__body")).toHaveCount(0);
    await first.locator(".rocard__row").click();
    await expect(first.locator(".rocard__body")).toHaveCount(1);
    await expect(first.locator(".tool-output-body__pre")).toContainText("app/src/App.vue");
  });

  test("点击第二次收起", async ({ page }) => {
    const first = page.locator(CARD).first();
    await first.locator(".rocard__row").click();
    await expect(first.locator(".rocard__body")).toHaveCount(1);
    await first.locator(".rocard__row").click();
    await expect(first.locator(".rocard__body")).toHaveCount(0);
  });

  test("报错卡片仍 1 行,展开后是错误文案", async ({ page }) => {
    const err = page.locator(CARD).nth(5);
    await expect(err.locator(".rocard__row")).toContainText("error");
    const h = await err.evaluate((el) => Math.round(el.getBoundingClientRect().height));
    expect(h).toBeLessThanOrEqual(ONE_LINE_MAX);
    await err.locator(".rocard__row").click();
    await expect(err.locator(".tool-output-body__pre")).toContainText("No such file or directory");
  });
});
