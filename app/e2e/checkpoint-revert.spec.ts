// N2 PR3(2026-09-20, task `09-20-n2-checkpoint-revert`)浏览器回归:
// 「回到此轮后」revert 闭环的交互面 —— 入口渲染门、确认弹窗(评审
// 重排清单)、dangerous confirm 流与能力隐藏。RULE-TEST-001 route-mock
// 确定性档:无 daemon / 无 LLM / 无网络,CI blocking。
//
// # 被测面(为什么在浏览器层)
// hover ⋯ 菜单(真实 pointer)→ 菜单项 → 确认弹窗(modal 覆盖层,
// Esc/backdrop 消解)→ confirm 按钮 → toast。跨组件覆盖层联动是
// jsdom 的已知失效面(spec browser-regression.md §1 第 3 类)。
//
// # 数据驱动
// - 历史:`sessions/load_session` 种子(1 user + 1 assistant,seq
//   1/2)—— 权威历史读入,同 question-card-scroll.spec 的形状。
// - 快照行:`checkpoint/list_turn_checkpoints` mock(snake_case,
//   TurnCheckpoint wire;基线 seq=1 挂 user 行、写轮 seq=2 挂
//   assistant 行)。
// - preview / execute:`checkpoint/revert_to_checkpoint_*` mock,
//   payload = Json<T> 本体。
// - 非 git session:注册一条晚于 world dispatcher 的 route handler,
//   直接 fulfill 400 + `{kind:"CheckpointsUnavailable"}`(mockCmd 只
//   能应 200;store 的 unavailable 判定走 TransportError.body.kind)。
import { expect, type Page, type Route } from "@playwright/test";
import {
  test,
  waitForCmd,
  waitForListReady,
  type MockPayload,
} from "./fixtures";

const SID = "e2e-session-1";

/** 快照行(snake_case,`TurnCheckpoint` wire):基线 seq=1(首轮
 *  user 行 seq)+ 写轮 seq=2(assistant 行 seq,files_changed=1)。 */
const CHECKPOINT_ROWS: MockPayload = [
  { seq: 1, prev_seq: null, created_at: 1_700_000_000_000, files_changed: 0 },
  { seq: 2, prev_seq: 1, created_at: 1_700_000_000_100, files_changed: 1 },
];

/** preview 应答(无 foreign):3 文件、三种归属全占。 */
function previewPayload(foreign: MockPayload | null = null): MockPayload {
  return {
    files: [
      { path: "src/main.rs", action: "checkout", attribution: "tool_written" },
      { path: "out.log", action: "delete", attribution: "shell_write" },
      { path: "notes.md", action: "checkout", attribution: "unknown" },
    ],
    foreign_delta: foreign,
    target_seq: 2,
    target_created_at: 1_700_000_000_100,
    preview_token: "aaaa:bbbb",
  };
}

const EXECUTE_PAYLOAD: MockPayload = { restored: 2, deleted: 1 };

/** 1 user + 1 assistant 的最小历史(形状同 question-card-scroll.spec)。 */
function seededSession(): MockPayload {
  const messages = [
    {
      id: 1,
      session_id: SID,
      role: "user",
      content: [{ type: "text", text: "帮我改一下 main.rs" }],
      text: "帮我改一下 main.rs",
      has_tool_calls: false,
      has_tool_results: false,
      created_at: "2026-01-01T00:00:00Z",
      seq: 1,
      ttfb_ms: null,
      gen_ms: null,
      total_ms: null,
      thinking_ms: null,
    },
    {
      id: 2,
      session_id: SID,
      role: "assistant",
      content: [{ type: "text", text: "已修改 main.rs 并清理 out.log。" }],
      text: "已修改 main.rs 并清理 out.log。",
      has_tool_calls: false,
      has_tool_results: false,
      created_at: "2026-01-01T00:00:00Z",
      seq: 2,
      ttfb_ms: null,
      gen_ms: null,
      total_ms: null,
      thinking_ms: null,
    },
  ];
  return {
    session: {
      id: SID,
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

/** boot 前 mock:会话历史 + 快照行 + preview/execute 面。 */
function seedCore(
  mockCmd: (domain: string, cmd: string, payload: MockPayload) => void,
): void {
  mockCmd("sessions", "list_sessions", [
    {
      id: SID,
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
  mockCmd("checkpoint", "list_turn_checkpoints", CHECKPOINT_ROWS);
  mockCmd("checkpoint", "revert_to_checkpoint_preview", previewPayload());
  mockCmd("checkpoint", "revert_to_checkpoint_execute", EXECUTE_PAYLOAD);
}

/** 打开某条消息行的 ⋯ 菜单(真实 pointer 事件)。data-seq 是
 *  MessageList 盖在行上的稳定钩子。 */
async function openRowMenu(page: Page, seq: number): Promise<void> {
  const row = page.locator(`[data-seq='${seq}']`);
  await row.locator("[data-testid='msg-actions-trigger']").click();
  const item = page.locator("[data-testid='msg-actions-copy']");
  await expect(item).toBeVisible();
}

test.describe("checkpoint revert 闭环(N2 PR3)", () => {
  test("入口仅轮末 assistant 卡渲染:user 卡 seq 命中基线也不给入口", async ({
    page,
    boot,
    mockCmd,
  }) => {
    seedCore(mockCmd);
    await boot();
    await waitForListReady(page);

    // assistant 卡(seq=2,命中写轮):两项入口都在(「本轮 diff」
    // 与「回到此轮后」同入口区)。
    await openRowMenu(page, 2);
    await expect(
      page.locator("[data-testid='msg-actions-turn-diff']"),
    ).toBeVisible();
    const revertItem = page.locator("[data-testid='msg-actions-revert']");
    await expect(revertItem).toBeVisible();
    await expect(revertItem).toContainText("回到此轮后");
    await page.keyboard.press("Escape");

    // user 卡(seq=1,seq 命中基线行):role 闸 —— revert 入口
    // 不渲染(基线是 revert 的合法 target,但只挂在 assistant 卡)。
    await openRowMenu(page, 1);
    await expect(
      page.locator("[data-testid='msg-actions-revert']"),
    ).toHaveCount(0);
    await expect(
      page.locator("[data-testid='msg-actions-turn-diff']"),
    ).toHaveCount(0);
    await page.keyboard.press("Escape");
  });

  test("确认弹窗:文件列表 + 归属 badge + 按钮带数 + gitignore 常驻脚注;foreign 仅非空渲染", async ({
    page,
    boot,
    mockCmd,
    waitForCmd,
  }) => {
    seedCore(mockCmd);
    await boot();
    await waitForListReady(page);

    // 打开 revert 确认弹窗(preview 命中:body 顶层 snake_case)。
    await openRowMenu(page, 2);
    await page.locator("[data-testid='msg-actions-revert']").click();
    const previewReq = await waitForCmd(
      "checkpoint",
      "revert_to_checkpoint_preview",
    );
    expect(previewReq.body).toMatchObject({
      session_id: SID,
      target_seq: 2,
    });
    const modal = page.locator("[data-testid='revert-confirm-modal']");
    await expect(modal).toBeVisible();

    // 还原集清单:3 行,action + path + 三种归属 badge 全渲染。
    const rows = page.locator("[data-testid='revert-file-row']");
    await expect(rows).toHaveCount(3);
    await expect(rows.nth(0)).toContainText("src/main.rs");
    await expect(
      rows.nth(0).locator("[data-testid='revert-badge-tool_written']"),
    ).toBeVisible();
    await expect(rows.nth(1)).toContainText("删除");
    await expect(
      rows.nth(1).locator("[data-testid='revert-badge-shell_write']"),
    ).toBeVisible();
    await expect(
      rows.nth(2).locator("[data-testid='revert-badge-unknown']"),
    ).toBeVisible();

    // 确认按钮带还原文件数(评审清单 2)。
    await expect(page.locator("[data-testid='revert-confirm-btn']")).toHaveText(
      "还原 3 个文件",
    );
    // gitignore 双重不可见常驻脚注(评审清单 5)。
    await expect(
      page.locator("[data-testid='revert-gitignore-note']"),
    ).toContainText(".gitignore");
    // foreign 为 null:警告区完全不渲染(评审清单 1)。
    await expect(
      page.locator("[data-testid='revert-foreign-warning']"),
    ).toHaveCount(0);

    // 无逐文件勾选(评审清单 6)。
    expect(
      await page
        .locator("[data-testid='revert-confirm-modal'] input[type='checkbox']")
        .count(),
    ).toBe(0);

    // 取消 → 换带 foreign 的 preview → 警告区出现(仅非空渲染的另一面)。
    await page.locator("[data-testid='revert-cancel-btn']").click();
    mockCmd(
      "checkpoint",
      "revert_to_checkpoint_preview",
      previewPayload([
        {
          path: "hand.txt",
          status: "modified",
          added: 1,
          removed: 1,
          diff_text: "",
        },
      ]),
    );
    await openRowMenu(page, 2);
    await page.locator("[data-testid='msg-actions-revert']").click();
    const foreign = page.locator("[data-testid='revert-foreign-warning']");
    await expect(foreign).toBeVisible();
    await expect(foreign).toContainText("非本会话快照内变更");
    await expect(foreign).toContainText("hand.txt");
  });

  test("confirm → execute(带 preview_token)→ toast 计数 + 弹窗关闭", async ({
    page,
    boot,
    mockCmd,
    waitForCmd,
  }) => {
    seedCore(mockCmd);
    await boot();
    await waitForListReady(page);

    await openRowMenu(page, 2);
    await page.locator("[data-testid='msg-actions-revert']").click();
    await expect(
      page.locator("[data-testid='revert-confirm-modal']"),
    ).toBeVisible();
    await page.locator("[data-testid='revert-confirm-btn']").click();

    // execute 收到 preview 原样带回的 token(确认语义 = token 授权)。
    const execReq = await waitForCmd(
      "checkpoint",
      "revert_to_checkpoint_execute",
    );
    expect(execReq.body).toMatchObject({
      session_id: SID,
      target_seq: 2,
      preview_token: "aaaa:bbbb",
    });
    // toast 计数(restored/deleted)+ 弹窗关闭。toast 是
    // projectsStore.showToast 的底部单 slot(class hook `.toast`,
    // 非 useToast 错误总线的 toast-viewport)。
    await expect(page.locator(".toast")).toContainText("还原 2 个文件");
    await expect(page.locator(".toast")).toContainText("删除 1 个");
    await expect(
      page.locator("[data-testid='revert-confirm-modal']"),
    ).toHaveCount(0);
  });

  test("非 git session(CheckpointsUnavailable):入口隐藏不报错", async ({
    page,
    boot,
    mockCmd,
  }) => {
    seedCore(mockCmd);

    // 晚于 world dispatcher 注册的 route handler 优先(Playwright
    // 后注册先跑):400 + kind,store 落 unavailable,入口隐藏
    // (store catch 静默,无 error toast)。boot 前注册 → 首次 load
    // 的 ensureLoaded 即命中。
    await page.route(
      "**/api/v1/checkpoint/list_turn_checkpoints",
      async (route: Route) => {
        await route.fulfill({
          status: 400,
          headers: {
            "Access-Control-Allow-Origin": "*",
            "Content-Type": "application/json",
          },
          body: JSON.stringify({
            kind: "CheckpointsUnavailable",
            message: "checkpoint: project is not a git repository",
          }),
        });
      },
    );

    await boot();
    await waitForListReady(page);

    await openRowMenu(page, 2);
    await expect(
      page.locator("[data-testid='msg-actions-revert']"),
    ).toHaveCount(0);
    await expect(
      page.locator("[data-testid='msg-actions-turn-diff']"),
    ).toHaveCount(0);
    // 能力隐藏不报错:无 error toast。
    await expect(
      page.locator("[data-testid='toast-root-error']"),
    ).toHaveCount(0);
  });
});
