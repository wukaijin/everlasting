// N7 DiffView 增强(09-26-diffview-enhance)浏览器回归:side-by-side
// 双栏 + 行内高亮 + 档位切换持久化 + 窄屏降级。RULE-TEST-001
// route-mock 确定性档:无 daemon / 无 LLM / 无网络,CI blocking。
//
// # 被测面(为什么在浏览器层)
// - 真实视口语义:<768px 降级是 matchMedia 选树,jsdom 无 matchMedia
//   (组件守卫恒 false)—— 只能在真 Chromium 里 setViewportSize 验。
// - localStorage 跨挂载记忆:关闭弹窗(DiffView unmount)再开
//   (remount)后档位仍生效,是跨实例生命周期行为。
// - CSS 整行隐藏(mobile-hide-toolbar):vitest css:false 看不见。
//
// # 数据驱动
// 「本轮 diff」入口驱动(checkpoint-revert.spec.ts 同款 seeding):
// sessions 种子 + `checkpoint/list_turn_checkpoints`(seq=2 命中
// files_changed>0)+ `checkpoint/get_turn_checkpoint_diff` 载荷。
// DiffModal(Teleport body)内渲染 DiffView。
//
// # fixture 为什么不能为空(diff_text: "")
// 照抄 checkpoint-revert.spec.ts 的 `diff_text: ""` foreign_delta 行会
// 踩恒真陷阱:空文本让 DiffView 走 `parsed=false` 的 raw 分支,所有
// `.diff-srow` / `.diff-mark` 断言都对着「根本没渲染的双栏结构」数 0
// —— 结构性写错也恒绿。本 spec 落地前做过负控:空 fixture + 断言
// `.diff-srow` 存在 → 红(证明断言有判废力);换正控 fixture 后绿。
// 故 fixture 必含 ctx 行 + 不等长 del/add run(2 del vs 1 add)+ 行内
// 可变片段("1"→"42")。
import { expect, type Page } from "@playwright/test";
import { test, waitForListReady, type MockPayload } from "./fixtures";

const SID = "e2e-session-1";
const MODE_KEY = "everlasting:diffview.mode";

/** 快照行:基线 seq=1 + 写轮 seq=2(files_changed>0 → 「本轮 diff」
 *  入口可用,判定链与 checkpoint-revert.spec.ts 相同)。 */
const CHECKPOINT_ROWS: MockPayload = [
  { seq: 1, prev_seq: null, created_at: 1_700_000_000_000, files_changed: 0 },
  { seq: 2, prev_seq: 1, created_at: 1_700_000_001_000, files_changed: 1 },
];

/** 正控 fixture:标准 unified diff。run 形态 =
 *   ctx(fn main)→ del-run×2 / add-run×1(不等长,短侧补占位)
 *   → ctx(println)→ 纯 add-run×1(左占位)→ ctx(})。
 * 行内可变片段:total 行 "1"→"42"(行内 mark 断言锚)。 */
const SPLIT_DIFF_TEXT = [
  "--- a/src/counter.rs",
  "+++ b/src/counter.rs",
  "@@ -1,5 +1,5 @@",
  " fn main() {",
  "-    let total = 1;",
  '-    let label = "old";',
  "+    let total = 42;",
  '     println!("total");',
  '+    println!("extra");',
  " }",
].join("\n");

const TURN_DIFF_PAYLOAD: MockPayload = {
  files: [
    {
      path: "src/counter.rs",
      status: "modified",
      added: 2,
      removed: 2,
      diff_text: SPLIT_DIFF_TEXT,
    },
  ],
};

function seededSession(): MockPayload {
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
    messages: [
      {
        id: 1,
        session_id: SID,
        role: "user",
        content: [{ type: "text", text: "改一下 counter" }],
        text: "改一下 counter",
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
        content: [{ type: "text", text: "已修改 counter.rs。" }],
        text: "已修改 counter.rs。",
        has_tool_calls: false,
        has_tool_results: false,
        created_at: "2026-01-01T00:00:00Z",
        seq: 2,
        ttfb_ms: null,
        gen_ms: null,
        total_ms: null,
        thinking_ms: null,
      },
    ],
  };
}

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
  mockCmd("checkpoint", "get_turn_checkpoint_diff", TURN_DIFF_PAYLOAD);
}

/** 打开 seq=2 行菜单并点「本轮 diff」(真实 pointer 事件,同
 *  checkpoint-revert.spec.ts 的 openRowMenu 惯例)。 */
async function openTurnDiff(page: Page): Promise<void> {
  const row = page.locator("[data-seq='2']");
  await row.locator("[data-testid='msg-actions-trigger']").click();
  const item = page.locator("[data-testid='msg-actions-turn-diff']");
  await expect(item).toBeVisible();
  await item.click();
  await expect(page.locator(".diff-modal")).toBeVisible();
}

test.describe("DiffView split + 行内高亮(N7 PR2)", () => {
  test("双栏结构 + 行内 mark + 不等长 run 占位 + 切换持久化 + 窄屏降级", async ({
    page,
    boot,
    mockCmd,
  }) => {
    seedCore(mockCmd);
    await boot();
    await waitForListReady(page);

    await openTurnDiff(page);
    const body = page.locator(".diff-modal__body");

    // modified 文件默认折叠(现状行为不回归):展开文件体再断言行级。
    await page.locator(".diff-file__header").click();

    // 桌面默认:unified(与现状零现差)+ 工具行在(aria-pressed 表态)。
    await expect(body.locator(".diff-view__toolbar")).toBeVisible();
    const unifiedBtn = body.locator(".diff-view__toolbar button", {
      hasText: "单栏",
    });
    const splitBtn = body.locator(".diff-view__toolbar button", {
      hasText: "双栏",
    });
    await expect(unifiedBtn).toHaveAttribute("aria-pressed", "true");
    await expect(body.locator(".diff-line").first()).toBeVisible();

    // 切双栏:网格行登场(unified 行退场)。
    // 行结构 = ctx(fn) + pair×2 + ctx(println) + pair(extra, 纯新增) + ctx(}) = 6。
    await splitBtn.click();
    await expect(body.locator(".diff-srow")).toHaveCount(6);
    await expect(body.locator(".diff-line")).toHaveCount(0);
    // hunk 头通栏 1 行。
    await expect(body.locator(".diff-sfull")).toHaveCount(1);
    await expect(splitBtn).toHaveAttribute("aria-pressed", "true");

    // 结构:不等长 run(2 del / 1 add)zip 后短侧补占位;行内 mark 配对。
    const rows = body.locator(".diff-srow");
    await expect(rows.nth(1)).toContainText("let total = 1;");
    await expect(rows.nth(1)).toContainText("let total = 42;");
    // 第 3 行 = pair 短侧:左 del 内容、右占位空格。
    await expect(rows.nth(2)).toContainText('let label = "old";');
    // 行内 mark:total 行 1→42 配对(右 1 个 add mark;左 del 跨两行共 2)。
    await expect(body.locator(".diff-mark--del").first()).toBeVisible();
    await expect(body.locator(".diff-mark--add")).toHaveCount(1);
    await expect(body.locator(".diff-mark--del")).toHaveCount(2);
    // 纯新增行(第 5 行)未配对:无 mark,左占位。
    await expect(rows.nth(4)).toContainText('println!("extra");');
    await expect(rows.nth(4).locator(".diff-mark")).toHaveCount(0);

    // 切换写 localStorage(仅用户点击写)。
    const stored = await page.evaluate((k) => localStorage.getItem(k), MODE_KEY);
    expect(stored).toBe("split");

    // 持久化 = remount 生效:关弹窗(DiffView unmount)再开(remount),
    // 不再点任何「单栏/双栏」按钮,双栏应直接恢复(折叠态是组件内
    // 状态,remount 后回默认折叠,照旧点开文件体)。
    await page.locator(".diff-modal__close").click();
    await expect(page.locator(".diff-modal")).toHaveCount(0);
    await openTurnDiff(page);
    await page.locator(".diff-file__header").click();
    await expect(page.locator(".diff-modal__body .diff-srow")).toHaveCount(6);

    // 真窄屏降级(matchMedia change 选树):unified 恒渲染,工具行 CSS
    // 藏整行;翻回宽视口恢复 split(组件实例未重挂,监听活着)。
    await page.setViewportSize({ width: 640, height: 900 });
    await expect(page.locator(".diff-modal__body .diff-srow")).toHaveCount(0);
    await expect(
      page.locator(".diff-modal__body .diff-line").first(),
    ).toBeVisible();
    await expect(page.locator(".diff-view__toolbar")).not.toBeVisible();
    await page.setViewportSize({ width: 1280, height: 720 });
    await expect(page.locator(".diff-modal__body .diff-srow")).toHaveCount(6);
    await expect(page.locator(".diff-view__toolbar")).toBeVisible();
  });

  test("raw-only 文件不给工具行(缺席条件①);空载荷走 modal 空态", async ({
    page,
    boot,
    mockCmd,
  }) => {
    seedCore(mockCmd);
    // raw-only:LLM 风格无头 +/- 片段(parsePatch 空 hunks → raw 分支,
    // generative-ui spec RULE-FrontDiff-001 形态二)。DiffView 挂载、
    // raw 行在,但无任何 parsed 文件 → 工具行缺席(vitest 同名用例的
    // 浏览器层复认)。
    mockCmd("checkpoint", "get_turn_checkpoint_diff", {
      files: [
        {
          path: "scratch.txt",
          status: "modified",
          added: 1,
          removed: 1,
          diff_text: " foo\n-x\n+y\n bar",
        },
      ],
    });
    await boot();
    await waitForListReady(page);

    await openTurnDiff(page);
    await page.locator(".diff-file__header").click();
    await expect(
      page.locator(".diff-file__raw .diff-raw-line").first(),
    ).toBeVisible();
    await expect(page.locator(".diff-view__toolbar")).toHaveCount(0);
    await page.locator(".diff-modal__close").click();
    await expect(page.locator(".diff-modal")).toHaveCount(0);

    // 空载荷对照:0 文件被 modal 层拦截(DiffModal 空态),DiffView 都
    // 不挂 → 同样无工具行。onTurnDiff 每次打开重置 turnDiffResult 并重
    // 取(MessageItem.vue),mid-test 覆盖 mock 生效。
    mockCmd("checkpoint", "get_turn_checkpoint_diff", { files: [] });
    await openTurnDiff(page);
    await expect(page.locator(".diff-modal__body")).toContainText(
      "No file changes.",
    );
    await expect(page.locator(".diff-view__toolbar")).toHaveCount(0);
  });
});
