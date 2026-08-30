// 试点③ 指针+弹窗层叠回归(BUGLIST CH7-4 原型):放行管理的撤销
// 确认弹窗在真实 Chromium 里的指针语义。
//
// # 被测面(为什么在浏览器层)
// ConfirmDialog 渲染在 reka-ui DialogContent **内部**(层叠上下文先例,
// PermissionGrantsModal.vue 文件头)—— 弹窗叠弹窗的 backdrop 命中、
// pointerdown-outside / click.self 取消,都是 jsdom 无法模拟的指针
// 语义(MarkdownDetailModal.test.ts 的 pointerdown-outside 仅占位守护,
// RULE-TEST-001 登记原因)。这里用真实鼠标点击逐条验证。
//
// # 入口与 wire 形状(实读源码确认)
// - 入口:ChatPanel 头部 `.chat-panel__grants-btn`(v-if
//   currentSessionId → 用 list_sessions 种子一个会话,试点②同款)。
// - 列表:`list_session_tool_permissions`(permissions 域,body
//   `{session_id}`);**应答行是 camelCase**(Rust PermissionGrantRow
//   `rename_all="camelCase"`,permissionGrants.ts)—— 与请求体的
//   snake_case 转换(transformArgsTopLevel)方向相反,别混。
// - 撤销:`revoke_tool_permission`,body `{session_id, tool_name,
//   match_kind, match_value}`(snake_case;matchValue=null 必须以
//   JSON null 上 wire,store 注释)。
// - 确认弹窗已有登记过的 data-testid="grant-revoke-confirm"
//   (CH7-4 引入,见 e2e/README.md 登记表);取消/确认按钮走
//   ConfirmDialog 的稳定 class。
import { expect, type Page } from "@playwright/test";
import { test, type MockPayload } from "./fixtures";

/** 放行列表 mock(camelCase wire)。两条:prefix(git)与整工具。 */
const GRANT_ROWS: MockPayload = [
  {
    sessionId: "e2e-session-1",
    toolName: "bash",
    matchKind: "prefix",
    matchValue: "git",
    grantedAt: "2026-01-01T00:00:00Z",
  },
  {
    sessionId: "e2e-session-1",
    toolName: "read_file",
    matchKind: "tool",
    matchValue: null,
    grantedAt: "2026-01-01T00:00:00Z",
  },
];

/** 种子一个当前会话(入口按钮 v-if currentSessionId;消息走默认
 *  load_session=null 空历史即可)。 */
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
  mockCmd("permissions", "list_session_tool_permissions", GRANT_ROWS);
}

/** 打开放行管理弹窗并等列表渲染。 */
async function openGrantsModal(page: Page): Promise<void> {
  await page.click(".chat-panel__grants-btn");
  await page.waitForSelector(".grant-modal", { state: "visible" });
  await expect(page.locator(".grant-item")).toHaveCount(2);
}

test.describe("放行撤销确认(CH7-4)", () => {
  test("点撤销 → 确认弹窗;取消 → 零 revoke 请求,行保留", async ({
    page,
    boot,
    mockCmd,
    reqs,
  }) => {
    seedSessionMocks(mockCmd);
    await boot();
    await openGrantsModal(page);

    // 点第一行(bash prefix git)的撤销 → 确认弹窗出现,正文复述该行。
    await page.locator(".grant-item__revoke").first().click();
    const dialog = page.locator('[data-testid="grant-revoke-confirm"]');
    await expect(dialog).toContainText("撤销此放行?");
    await expect(dialog).toContainText("bash git");

    // 取消(真实点击 Cancel 按钮)→ 弹窗关、行保留、零 revoke 请求。
    await page.locator(".confirm-modal__btn--cancel").click();
    await expect(dialog).toHaveCount(0);
    await expect(page.locator(".grant-item")).toHaveCount(2);
    expect(
      reqs.filter((r) => r.cmd === "revoke_tool_permission"),
    ).toEqual([]);
  });

  test("点撤销 → 确认 → revoke POST(snake_case)+ 行消失", async ({
    page,
    boot,
    mockCmd,
    waitForCmd,
  }) => {
    seedSessionMocks(mockCmd);
    mockCmd("permissions", "revoke_tool_permission", null);
    await boot();
    await openGrantsModal(page);

    await page.locator(".grant-item__revoke").first().click();
    await page.locator(".confirm-modal__btn--danger").click();

    // 请求体按 PK 四元组上 wire(顶层键 snake_case;match_value=null)。
    const revoke = await waitForCmd("permissions", "revoke_tool_permission");
    expect(revoke.body).toMatchObject({
      session_id: "e2e-session-1",
      tool_name: "bash",
      match_kind: "prefix",
      match_value: "git",
    });
    // 行本地移除(store 无全量重拉);弹窗关闭。
    await expect(page.locator(".grant-item")).toHaveCount(1);
    await expect(page.locator(".grant-item__tool")).toHaveText("read_file");
    await expect(
      page.locator('[data-testid="grant-revoke-confirm"]'),
    ).toHaveCount(0);
  });

  test("确认弹窗点遮罩(backdrop)→ 取消,零 revoke 请求", async ({
    page,
    boot,
    mockCmd,
    reqs,
  }) => {
    seedSessionMocks(mockCmd);
    await boot();
    await openGrantsModal(page);

    await page.locator(".grant-item__revoke").first().click();
    const dialog = page.locator('[data-testid="grant-revoke-confirm"]');
    await expect(dialog).toBeVisible();

    // 真实指针点在确认弹窗自己的 backdrop(.confirm-backdrop 的空隙,
    // click.self 命中)—— 这是 jsdom 测不出的 pointer 语义;弹窗在
    // reka-ui DialogContent 内部,该点击不应连带关掉外层放行弹窗。
    await page.locator(".confirm-backdrop").click({ position: { x: 8, y: 8 } });
    await expect(dialog).toHaveCount(0);
    // 外层放行弹窗仍在(未被连带关闭),行保留、零请求。
    await expect(page.locator(".grant-modal")).toBeVisible();
    await expect(page.locator(".grant-item")).toHaveCount(2);
    expect(
      reqs.filter((r) => r.cmd === "revoke_tool_permission"),
    ).toEqual([]);
  });
});
