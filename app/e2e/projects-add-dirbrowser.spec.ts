// 「添加项目」DirBrowserModal 全流程回归(09-03-dirbrowser-desktop-unify):
// 无项目空态 → 添加项目 → 模态框选目录 → 注册成功。
//
// # 被测面(为什么在浏览器层)
// 模态框是 reka-ui DialogPortal 传送门 + roving tabindex 键盘导航,
// Enter 的原生 button 激活、焦点复位、Tab 序都是 jsdom 测不到的
// 真实浏览器语义(jsdom 无 activation behavior,RULE-TEST-001 同类)。
//
// # 入口与 wire 形状(实读源码确认)
// - 入口:EmptyProjectState「添加项目」(`.empty-state__add`,store
//   `openDirBrowser()`)。默认 boot 表 list_projects 返回 1 个项目会
//   直接进 ChatPanel —— 本 spec 覆盖为 `[]` 触发空态,故不能复用
//   fixtures 的 `boot`(它以 CM 编辑器挂载为 mount 观察点),goto +
//   等空态按钮即等价。
// - 冷启动:模态框 open → `config/get_home_dir` → `projects/browse_dir`
//   (body `{path, show_hidden}`,顶层键 snake_case,fixtures 头注)。
// - 注册:「选择此目录」→ `projects/create_project`(body `{path}`)
//   → loadProjects 重拉 → Tab 出现。list_projects 的注册表应答在点
//   「选择此目录」**之前**换成已注册行 —— dispatcher 逐请求查表,
//   预先换好即无竞态(不该靠 waitForCmd 后补,loadProjects 紧随
//   create_project 响应)。
import { expect, type Page } from "@playwright/test";
import { test, type MockPayload } from "./fixtures";

const HOME = "/home/e2e";
const PROJ = "/home/e2e/proj";

/** home 目录列表:一个子目录 proj。 */
const HOME_BROWSE: MockPayload = {
  path: HOME,
  parent: "/home",
  entries: [{ name: "proj", path: PROJ }],
};

/** 子目录列表:空(渲染「空目录」提示)。 */
const PROJ_BROWSE: MockPayload = {
  path: PROJ,
  parent: HOME,
  entries: [],
};

/** create_project 应答(ProjectRow,snake_case wire)。 */
const CREATED_PROJECT: MockPayload = {
  id: "proj-new",
  name: "proj",
  path: PROJ,
  is_git_repo: false,
  git_branch: null,
  is_legacy: false,
  created_at: "2026-09-03T00:00:00Z",
  updated_at: "2026-09-03T00:00:00Z",
  hidden: false,
  metadata: null,
};

/** 无项目空态 boot:list_projects 覆盖为 [] → EmptyProjectState。 */
async function bootEmpty(
  page: Page,
  mockCmd: (domain: string, cmd: string, payload: MockPayload) => void,
): Promise<void> {
  mockCmd("projects", "list_projects", []);
  await page.goto("/", { waitUntil: "domcontentloaded" });
  await page.waitForSelector(".empty-state__add", {
    state: "visible",
    timeout: 30_000,
  });
}

/** 等 `projects/browse_dir` 第 N 次(path 区分)请求出现。 */
async function waitForBrowsePath(
  reqs: { cmd: string | null; body: unknown }[],
  path: string,
): Promise<void> {
  await expect
    .poll(
      () =>
        reqs.filter(
          (r) =>
            r.cmd === "browse_dir" &&
            (r.body as { path?: string } | null)?.path === path,
        ).length,
    )
    .toBe(1);
}

test.describe("添加项目 DirBrowserModal(09-03 全模式统一入口)", () => {
  test("空态 → 添加项目 → 点选目录 → create_project 请求体正确 + Tab 出现", async ({
    page,
    mockCmd,
    waitForCmd,
    reqs,
  }) => {
    mockCmd("projects", "browse_dir", HOME_BROWSE);
    mockCmd("projects", "create_project", CREATED_PROJECT);
    await bootEmpty(page, mockCmd);

    // 空态「添加项目」→ 打开模态框(不再有 native pick 链)
    await page.click(".empty-state__add");
    await page.waitForSelector(".dir-browser", { state: "visible" });
    const browse = await waitForCmd("projects", "browse_dir");
    expect(browse.body).toEqual({ path: HOME, show_hidden: false });
    await expect(page.locator(".dir-browser__row", { hasText: "proj" })).toBeVisible();

    // 点 "proj" 目录行 → 进入子目录(注册表先换成子目录应答)
    mockCmd("projects", "browse_dir", PROJ_BROWSE);
    await page.locator(".dir-browser__row", { hasText: "proj" }).click();
    await waitForBrowsePath(reqs, PROJ);
    await expect(page.locator(".dir-browser__empty")).toBeVisible();

    // 「选择此目录」:先把 list_projects 换成已注册行(无竞态),
    // 再点 → create_project body 只带 path,之后 loadProjects 拉到 Tab。
    mockCmd("projects", "list_projects", [CREATED_PROJECT]);
    await page.click(".dir-browser__choose");
    const create = await waitForCmd("projects", "create_project");
    expect(create.body).toEqual({ path: PROJ });

    await expect(
      page.locator(".tab", { hasText: "proj" }).first(),
    ).toBeVisible();
    await expect(page.locator(".dir-browser")).toHaveCount(0);
  });

  test("键盘导航:ArrowDown 移动焦点 + Enter 原生进入 + 焦点复位新列表首行", async ({
    page,
    mockCmd,
    reqs,
  }) => {
    mockCmd("projects", "browse_dir", HOME_BROWSE);
    await bootEmpty(page, mockCmd);

    await page.click(".empty-state__add");
    await page.waitForSelector(".dir-browser", { state: "visible" });
    const projRow = page.locator(".dir-browser__row", { hasText: "proj" });
    await expect(projRow).toBeVisible();

    // roving tabindex:首行("..")tabindex=0,其余 -1
    await expect(page.locator(".dir-browser__row--up")).toHaveAttribute(
      "tabindex",
      "0",
    );
    await expect(projRow).toHaveAttribute("tabindex", "-1");

    // ArrowDown:焦点从 ".." 行移到 proj 行(不环绕),锚随行移动
    await page.focus(".dir-browser__row--up");
    await page.keyboard.press("ArrowDown");
    await expect(projRow).toBeFocused();
    await expect(page.locator(".dir-browser__row--up")).toHaveAttribute(
      "tabindex",
      "-1",
    );
    await expect(projRow).toHaveAttribute("tabindex", "0");

    // 输入框聚焦时方向键不劫持:焦点留在输入框
    await page.focus(".dir-browser__path");
    await page.keyboard.press("ArrowDown");
    await expect(page.locator(".dir-browser__path")).toBeFocused();

    // Enter 在焦点行原生激活(真实浏览器 activation)→ 进入子目录;
    // 列表发起的导航完成后焦点复位新列表首行(子目录列表只有 ".." 行)
    mockCmd("projects", "browse_dir", PROJ_BROWSE);
    await page.focus(".dir-browser__row:not(.dir-browser__row--up)");
    await page.keyboard.press("Enter");
    await waitForBrowsePath(reqs, PROJ);
    await expect(page.locator(".dir-browser__row--up")).toBeFocused();
    await expect(page.locator(".dir-browser__empty")).toBeVisible();
  });
});
