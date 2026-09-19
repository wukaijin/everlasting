// N4 PR1 虚拟化专项回归(任务 09-19-n4-render-virtualization,design §7)。
//
// # 被测面(为什么在浏览器层)
// 虚拟化的全部价值都是**结果断言**:DOM 数量与 session 长度解耦、
// 滚动到底/顶/中段的窗口迁移、展开/上方项变高的滚动位置校正、
// data-seq 定位命中。jsdom 无布局全测不了;这里真 Chromium + 10k 种子
// (复用 bench/fixtures 的 LoadedSession wire + e2e/fixtures world,
// 零 daemon/LLM/网络)。
//
// # 与旧实现的判据关系
// 本 spec 全部断言**实现中立**(scrollHeight / scrollTop / 元素几何 /
// data-seq 存在性),不读任何虚拟化内部状态 —— 回滚 PR1 后同套应再绿
// (评审定的回滚验收面;除 data-seq 用例外,旧实现 querySelector 路径
// 同样满足「命中行进入视口」)。
//
// # 滚动写入方式
// 测试侧 `el.scrollTop = x` 直写等效真实用户滚动(scroll 事件照常触发,
// 库经 observeElementOffset 同步内部 offset;spike 约束 3 管的是产品
// 代码的程序化滚动,必须走库 API —— 回底走组件按钮路径即是)。
import { expect, type Page } from "@playwright/test";
import { MESSAGES, test, waitForListReady } from "./fixtures";

// bench fixture 单一出处(N9 种子;node:fs 走 io.mjs 薄层,不进类型面)。
const { readFixture } = await import("../bench/io.mjs");

const SESSION_ID = "e2e-session-1";
/** 10k 种子会话 + 当前会话注册(形态同 question-card-scroll.spec 的
 *  seedSessionMocks;行形状出处 bench/render.bench.ts SESSION_ROW)。 */
async function seed10k(
  mockCmd: (domain: string, cmd: string, payload: unknown) => void,
  withCompactionRow = false,
): Promise<void> {
  const loaded = (readFixture as (n: number) => {
    messages: Array<Record<string, unknown>>;
  })(10000);
  if (withCompactionRow) {
    // 压缩摘要行(metadata.kind = compaction_summary,MessageItem 的
    // 可展开系统行)—— 高度探针的确定性变高源:点击展开 → 行高真实
    // 增长,位置完全可控。头部注入,seq 用段外值防撞。
    loaded.messages.splice(3, 0, {
      id: 10001,
      session_id: SESSION_ID,
      role: "user",
      content: "## 历史摘要\n\n- 决策一:采用虚拟化\n- 决策二:锚定交库",
      text: "## 历史摘要\n\n- 决策一:采用虚拟化\n- 决策二:锚定交库",
      has_tool_calls: false,
      has_tool_results: false,
      created_at: "2026-01-01T00:00:00Z",
      seq: 10001,
      ttfb_ms: null,
      gen_ms: null,
      total_ms: null,
      thinking_ms: null,
      metadata: { kind: "compaction_summary" },
    });
  }
  mockCmd("sessions", "list_sessions", [
    {
      id: SESSION_ID,
      title: "虚拟化种子会话",
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
      color_tag: null,
      mode: "edit",
      workflow_enabled: false,
      plugin_name: "",
      session_type: "chat",
      metadata: null,
    },
  ]);
  mockCmd("sessions", "load_session", loaded);
}

interface ListMetrics {
  scrollTop: number;
  scrollHeight: number;
  clientHeight: number;
  distBottom: number;
  renderedRows: number;
}

async function metrics(page: Page): Promise<ListMetrics> {
  return page.evaluate((sel) => {
    const el = document.querySelector<HTMLElement>(sel)!;
    return {
      scrollTop: el.scrollTop,
      scrollHeight: el.scrollHeight,
      clientHeight: el.clientHeight,
      distBottom: el.scrollHeight - el.scrollTop - el.clientHeight,
      // 虚拟化证据:渲染行数(任意 class 形态的行根)与 session 长度解耦。
      renderedRows: el.querySelectorAll(".msg").length,
    };
  }, MESSAGES);
}

/** 等一个数值读数连续 `samples` 次采样不变(采样间隔 gapMs)。虚拟化下
 *  滚动落点后 estimate→actual 测量校正还会微调若干帧布局 —— 探针的
 *  before 基线必须取自静默后的布局,否则量到的是校正尾流,不是被测
 *  行为(首轮实测踩过)。超时 fail-loud。 */
async function waitForReadingQuiet(
  read: () => Promise<number>,
  opts: { samples?: number; gapMs?: number; timeoutMs?: number } = {},
): Promise<number> {
  const { samples = 3, gapMs = 150, timeoutMs = 15_000 } = opts;
  const startedAt = Date.now();
  let last = await read();
  let stable = 1;
  while (stable < samples) {
    await new Promise((r) => setTimeout(r, gapMs));
    if (Date.now() - startedAt > timeoutMs) {
      throw new Error(`waitForReadingQuiet: reading never went quiet (last=${last})`);
    }
    const now = await read();
    if (now === last) stable += 1;
    else {
      stable = 1;
      last = now;
    }
  }
  return last;
}

/** 距底距离(MessageList isNearBottom 同式,阈值 80px)。容器瞬时不在
 *  DOM(ChatPanel 切会话的 spinner v-if 重挂)时返回 +∞,让 poll 继续
 *  等而不是抛错。 */
async function distBottom(page: Page): Promise<number> {
  return page.evaluate((sel) => {
    const el = document.querySelector<HTMLElement>(sel);
    if (!el) return Number.POSITIVE_INFINITY;
    return el.scrollHeight - el.scrollTop - el.clientHeight;
  }, MESSAGES);
}

/** 慢速程序化滚动(rAF 节流,每帧 6000px,分块 evaluate 防超时)。
 *  慢滚 = 让 estimate→actual 测量校正与滚动真实交错(瞬跳会整窗跳过,
 *  测不到校正补偿的累积效应)。返回时已到目标端。 */
async function slowScroll(page: Page, dir: "top" | "bottom"): Promise<void> {
  for (let i = 0; i < 200; i += 1) {
    const done = await page.evaluate(({ sel, dir }) => {
      const el = document.querySelector<HTMLElement>(sel)!;
      const step = 6000;
      return new Promise<boolean>((resolve) => {
        let frames = 0;
        const tick = () => {
          const atTarget =
            dir === "bottom"
              ? el.scrollTop + el.clientHeight >= el.scrollHeight - 2
              : el.scrollTop <= 0;
          if (atTarget || frames >= 12) {
            resolve(atTarget);
            return;
          }
          el.scrollTop += dir === "bottom" ? step : -step;
          frames += 1;
          requestAnimationFrame(tick);
        };
        requestAnimationFrame(tick);
      });
    }, { sel: MESSAGES, dir });
    if (done) return;
  }
  throw new Error(`slowScroll(${dir}): never reached target`);
}

test.describe("N4 虚拟化消息列表(10k 种子)", () => {
  test("10k 会话:mount 落底 + DOM 数量与长度解耦 + 滚动底/顶/中段窗口迁移", async ({
    page,
    boot,
    mockCmd,
  }) => {
    await seed10k(mockCmd);
    await boot();
    await waitForListReady(page);

    // 落底(mount/session 打开钉底语义,虚拟化下单次定位)。
    await expect
      .poll(() => metrics(page), { timeout: 10_000 })
      .toMatchObject({ distBottom: expect.any(Number) });
    await expect
      .poll(async () => (await metrics(page)).distBottom, { timeout: 10_000 })
      .toBeLessThan(80);

    // DOM 解耦:渲染行常数级(<100),内容高度百万级(10k 行)。
    const m0 = await metrics(page);
    expect(m0.renderedRows).toBeLessThan(100);
    expect(m0.scrollHeight).toBeGreaterThan(1_000_000);

    // 滚到顶:窗口迁移,首行渲染,行数仍常数。
    await page.evaluate((sel) => {
      const el = document.querySelector<HTMLElement>(sel)!;
      el.scrollTop = 0;
    }, MESSAGES);
    await expect
      .poll(async () => (await metrics(page)).scrollTop, { timeout: 10_000 })
      .toBe(0);
    // 首条消息进入渲染窗口(fixture seq 0-based,data-seq=0 是第一条)。
    await expect(page.locator(`${MESSAGES} [data-seq="0"]`)).toBeVisible();
    expect((await metrics(page)).renderedRows).toBeLessThan(100);

    // 滚到中段:窗口迁移,行数仍常数。
    await page.evaluate((sel) => {
      const el = document.querySelector<HTMLElement>(sel)!;
      el.scrollTop = el.scrollHeight / 2;
    }, MESSAGES);
    await expect
      .poll(async () => (await metrics(page)).renderedRows, {
        timeout: 10_000,
      })
      .toBeLessThan(100);
    const mid = await metrics(page);
    expect(mid.scrollTop).toBeGreaterThan(0);

    // 回底走组件行为路径(回底按钮,jumpToBottom —— idle 平滑滚动)。
    await page.evaluate((sel) => {
      const el = document.querySelector<HTMLElement>(sel)!;
      el.scrollTop = 0;
    }, MESSAGES);
    const button = page.locator(".scroll-to-bottom");
    await expect(button).toBeVisible();
    await button.click();
    await expect
      .poll(async () => (await metrics(page)).distBottom, { timeout: 15_000 })
      .toBeLessThan(80);
  });

  test("data-seq 滚到命中:搜索「在主窗口打开」把命中消息带进视口(AC4 前半)", async ({
    page,
    boot,
    mockCmd,
  }) => {
    await seed10k(mockCmd);
    // 命中固定指向中段消息(虚拟化下它默认不在 DOM —— 断言它的出现
    // 本身就是 scrollToIndex 命中的证据)。
    const hitSeq = 5001;
    // search_messages 的 wire domain 是 sessions(http.ts CMD_TO_DOMAIN)。
    mockCmd("sessions", "search_messages", [
      {
        kind: "content",
        session_id: SESSION_ID,
        session_title: "虚拟化种子会话",
        project_id: "e2e-project",
        project_name: "e2e-project",
        updated_at: "2026-01-01T00:00:00Z",
        seq: hitSeq,
        role: "user",
        speaker: null,
        snippet: "虚拟化 needle 命中上下文",
      },
    ]);
    await boot();
    await waitForListReady(page);
    // 命中前:中段行不在渲染窗口(虚拟化下它不在渲染窗口)。
    expect(await page.locator(`${MESSAGES} [data-seq="${hitSeq}"]`).count()).toBe(0);

    // Cmd/Ctrl+K 开搜索 → 输入 → 结果行 → 预览 →「在主窗口打开」。
    await page.keyboard.press("ControlOrMeta+k");
    await page.locator(".search-modal__input").fill("needle");
    await page.locator(".search-modal__row--snippet").first().click();
    await page.locator(".search-modal__open-btn").click();

    // 模态关闭 + 命中消息被滚进视口:元素存在(= 渲染窗口覆盖它)且
    // 与视口相交(scrollToIndex align:center 的结果断言,真 Chromium)。
    await expect(page.locator(".search-modal")).toHaveCount(0);
    const hit = page.locator(`${MESSAGES} [data-seq="${hitSeq}"]`);
    await expect(hit).toHaveCount(1, { timeout: 15_000 });
    await expect(hit).toBeVisible();

    // AC4 后半(N4 PR3):命中行短暂高亮 —— flash 类落在 wrapper 上、
    // 1.5s 窗口内可观测,随后自动摘除(SEARCH_FLASH_MS 驱动)。
    const flashRow = page.locator(
      `${MESSAGES} .vrow.search-hit [data-seq="${hitSeq}"]`,
    );
    await expect(flashRow).toHaveCount(1);
    await expect(page.locator(`${MESSAGES} .vrow.search-hit`)).toHaveCount(
      0,
      { timeout: 4_000 },
    );

    const inCenter = await page.evaluate(
      ({ sel, seq }) => {
        const scroller = document.querySelector<HTMLElement>(sel)!;
        const hit = document.querySelector<HTMLElement>(
          `${sel} [data-seq="${seq}"]`,
        )!;
        const s = scroller.getBoundingClientRect();
        const h = hit.getBoundingClientRect();
        const scrollerMid = s.top + s.height / 2;
        // 命中行与视口垂直中线距离在半个视口内(center 对齐的宽松界)。
        return Math.abs(h.top + h.height / 2 - scrollerMid) < s.height / 2;
      },
      { sel: MESSAGES, seq: hitSeq },
    );
    expect(inCenter).toBe(true);
  });

  test("视口内展开摘要行:自身位置不跳屏(高度校正不扰动阅读位)", async ({
    page,
    boot,
    mockCmd,
  }) => {
    await seed10k(mockCmd, true);
    await boot();
    await waitForListReady(page);

    // 滚到顶找摘要行(种子.splice 注入的 compaction_summary 行)。
    await page.evaluate((sel) => {
      (document.querySelector(sel) as HTMLElement).scrollTop = 0;
    }, MESSAGES);
    const summaryRow = page.locator(`${MESSAGES} .msg-compact-summary`).first();
    await expect(summaryRow).toBeVisible();
    const rowTop = () =>
      summaryRow.evaluate((el) => el.getBoundingClientRect().top);

    // 基线取自静默布局(estimate→actual 校正尾流排除,见 helper 注)。
    const before = await waitForReadingQuiet(rowTop);

    await summaryRow.click();
    // 展开体真的出现(高度确实变了)。
    await expect(
      page.locator(`${MESSAGES} .msg-compact-summary__body`).first(),
    ).toBeVisible();
    // 展开后行顶应不动(向下生长不推自身;±4px 容测量噪声)。
    await expect
      .poll(rowTop, { timeout: 10_000 })
      .toBeLessThan(before + 4);
    await expect
      .poll(rowTop, { timeout: 10_000 })
      .toBeGreaterThan(before - 4);
  });

  test("视口上方项异步变高:不跳屏探针(stickToBottom 退役证据,D2)", async ({
    page,
    boot,
    mockCmd,
  }) => {
    await seed10k(mockCmd, true);
    await boot();
    await waitForListReady(page);

    // 滚到顶渲染摘要行,等estimate→actual 校正静默后量它的内容坐标
    // 底缘,再滚到「摘要行整体在视口上方一格」的位置:它仍在渲染窗口
    // (overscan)内但完全越过折线(库校正路径的触发前提)。
    await page.evaluate((sel) => {
      (document.querySelector(sel) as HTMLElement).scrollTop = 0;
    }, MESSAGES);
    const summaryRow = page.locator(`${MESSAGES} .msg-compact-summary`).first();
    await expect(summaryRow).toBeVisible();
    const rowBottomRel = () =>
      summaryRow.evaluate(
        (el, sel) => {
          const scroller = document.querySelector<HTMLElement>(sel)!;
          return (
            el.getBoundingClientRect().bottom -
            scroller.getBoundingClientRect().top
          );
        },
        MESSAGES,
      );
    const rowBottom = await waitForReadingQuiet(rowBottomRel);
    const scrollTarget = rowBottom + 12;
    await page.evaluate(
      ({ sel, top }) => {
        (document.querySelector(sel) as HTMLElement).scrollTop = top;
      },
      { sel: MESSAGES, top: scrollTarget },
    );
    await page.waitForTimeout(400); // 测量/调整静默窗

    // 折线下第一条消息(seq 3 = 摘要行(注入于 seq2 后)的后继,身份固定):
    // 用 data-seq 固定身份当哨兵 —— 按位置取 rows[0] 会因窗口重排换行
    // 而产生假稳定(首轮实测踩过)。
    const sentinel = page.locator(`${MESSAGES} [data-seq="3"]`);
    await expect(sentinel).toBeVisible();
    const sentinelTop = () =>
      sentinel.evaluate((el) => el.getBoundingClientRect().top);
    const sentinelBefore = await waitForReadingQuiet(sentinelTop);

    await summaryRow.evaluate((el) => (el as HTMLElement).click());
    // 展开体出现(增量真实发生)。
    await expect(
      page.locator(`${MESSAGES} .msg-compact-summary__body`).first(),
    ).toBeVisible();

    // 不跳屏:哨兵稳定(库的 offset 校正 = scrollTop 增加同等增量)。
    await expect
      .poll(sentinelTop, { timeout: 10_000 })
      .toBeLessThan(sentinelBefore + 4);
    // 校正真实发生(非侥幸):scrollTop 前移越过原目标。
    const scrollTopAfter = await page.evaluate(
      (sel) => (document.querySelector(sel) as HTMLElement).scrollTop,
      MESSAGES,
    );
    expect(scrollTopAfter).toBeGreaterThan(scrollTarget);
  });
});

test.describe("N4 PR2 慢滚锚定 + 流式跟滚(AC2)", () => {
  test("慢滚 10k 全程(顶→底→顶):固定哨兵行视口漂移 ≤ 一行高", async ({
    page,
    boot,
    mockCmd,
  }) => {
    test.setTimeout(120_000);
    await seed10k(mockCmd);
    await boot();
    await waitForListReady(page);

    // 哨兵 = seq 5(顶部首个 401 字符 assistant 文本行,~142px,身份
    // 固定;ghost user 行 .msg 高 0,不能当可见性哨兵)。基线取自静默
    // 布局。
    await page.evaluate((sel) => {
      (document.querySelector(sel) as HTMLElement).scrollTop = 0;
    }, MESSAGES);
    const sentinel = page.locator(`${MESSAGES} [data-seq="5"]`);
    await expect(sentinel).toBeVisible();
    const sentinelTop = () =>
      sentinel.evaluate((el) => el.getBoundingClientRect().top);
    const before = await waitForReadingQuiet(sentinelTop);

    // 全程慢滚:顶→底(全部行首次实测,estimate→actual 校正与滚动
    // 交错)→ 静默 → 底→顶(重挂 + 重测校正方向反向)。
    await slowScroll(page, "bottom");
    await waitForReadingQuiet(() => distBottom(page));
    await slowScroll(page, "top");
    const after = await waitForReadingQuiet(sentinelTop);

    // 锚定漂移 ≤ 一行高(验收线;回归 = 校正补偿失灵或测量缓存被
    // 重置,哨兵会大幅位移)。
    expect(Math.abs(after - before)).toBeLessThanOrEqual(320);
  });

  test("流式中上滚不被抢 + 回底按钮;点回底瞬跳并重挂跟滚", async ({
    page,
    boot,
    mockCmd,
    stream,
  }) => {
    await seed10k(mockCmd);
    await boot();
    await waitForListReady(page);

    // 开流(跨客户端认领):占位行不可见,首 delta 入列表并钉底。
    const rid = "rid-ac2-stream";
    const delta = (t: string) => ({
      request_id: rid,
      session_id: SESSION_ID,
      kind: "delta",
      text: t,
    });
    await stream.emit("chat-event", {
      request_id: rid,
      session_id: SESSION_ID,
      kind: "start",
    });
    await stream.emit("chat-event", delta(" AC2段1 "));
    await stream.emit("chat-event", delta(" AC2段2 "));
    await expect(page.locator(".chat-input__row--streaming")).toBeVisible();
    await expect
      .poll(() => distBottom(page), { timeout: 10_000 })
      .toBeLessThan(80);

    // 用户上滚到顶:按钮出现;流式继续(delta 落在视口外的行上),
    // scrollTop 不被抢(留在顶部)。
    await page.evaluate((sel) => {
      (document.querySelector(sel) as HTMLElement).scrollTop = 0;
    }, MESSAGES);
    await expect(page.locator(".scroll-to-bottom")).toBeVisible();
    await stream.emit("chat-event", delta(" AC2段3 "));
    await stream.emit("chat-event", delta(" AC2段4 "));
    await page.waitForTimeout(1200);
    expect(await page.evaluate(
      (sel) => (document.querySelector(sel) as HTMLElement).scrollTop,
      MESSAGES,
    )).toBe(0);
    await expect(page.locator(".scroll-to-bottom")).toBeVisible();

    // 点回底:瞬跳(behavior auto,流式)落底;段 3/4 的文本在视口内
    // 可见(它们只存在于视口外增长的行 —— 命中即真跳到了末端)。
    await page.locator(".scroll-to-bottom").click();
    await expect
      .poll(() => distBottom(page), { timeout: 10_000 })
      .toBeLessThan(80);
    await expect(
      page.locator(`${MESSAGES} .msg__markdown`, { hasText: "AC2段4" }),
    ).toBeVisible();

    // 重挂跟滚:force-follow 已重挂,后续 delta 到达仍钉底。
    await stream.emit("chat-event", delta(" AC2段5 "));
    await expect(
      page.locator(`${MESSAGES} .msg__markdown`, { hasText: "AC2段5" }),
    ).toBeVisible({ timeout: 10_000 });
    await expect
      .poll(() => distBottom(page), { timeout: 10_000 })
      .toBeLessThan(80);

    // 收流(turn_complete + done):done 触发 reloadAfterFinalize,
    // 权威重拉替换缓冲后仍落底(F4 语义,AC2「reload 落底」项)。
    await stream.emit("chat-event", {
      request_id: rid,
      session_id: SESSION_ID,
      kind: "turn_complete",
      seq: 99_999,
      ttfb_ms: null,
      gen_ms: null,
      total_ms: 5,
      thinking_ms: null,
    });
    await stream.emit("chat-event", {
      request_id: rid,
      session_id: SESSION_ID,
      kind: "done",
      stop_reason: "end_turn",
      usage: null,
    });
    await expect
      .poll(() => distBottom(page), { timeout: 15_000 })
      .toBeLessThan(80);
    // 无弹跳:静默窗内持续贴底。
    const d1 = await distBottom(page);
    await page.waitForTimeout(700);
    const d2 = await distBottom(page);
    expect(d1).toBeLessThan(80);
    expect(d2).toBeLessThan(80);
  });

  test("F5 badge 变高跟滚:done 收官 badge 渲染不脱底,reload 后保持", async ({
    page,
    boot,
    mockCmd,
    stream,
  }) => {
    // 尾行带 ttfb/total ms 字段:rehydrate 会附 latency(F5),badge
    // 在 done 收官(流式态熄灭)与 reloadAfterFinalize 重拉后都成立
    // —— 探针面 = badge 渲染的行变高不破坏钉底。
    const loaded = (readFixture as (n: number) => {
      messages: Array<Record<string, unknown>>;
    })(10000);
    const lastRow = loaded.messages.at(-1)!;
    lastRow.ttfb_ms = 1234;
    lastRow.gen_ms = 100;
    lastRow.total_ms = 1300;
    await seed10k(mockCmd);
    mockCmd("sessions", "load_session", loaded);
    await boot();
    await waitForListReady(page);

    const rid = "rid-ac2-badge";
    await stream.emit("chat-event", {
      request_id: rid,
      session_id: SESSION_ID,
      kind: "start",
    });
    await stream.emit("chat-event", {
      request_id: rid,
      session_id: SESSION_ID,
      kind: "delta",
      text: " F5 badge 钉底探针 ",
    });
    await expect
      .poll(() => distBottom(page), { timeout: 10_000 })
      .toBeLessThan(80);

    // done:末行 streaming 熄灭 → footer badge 渲染(行变高);
    // reloadAfterFinalize 用带 ms 字段的种子整体替换,badge 仍在。
    await stream.emit("chat-event", {
      request_id: rid,
      session_id: SESSION_ID,
      kind: "turn_complete",
      seq: 99_999,
      ttfb_ms: 1234,
      gen_ms: 100,
      total_ms: 1300,
      thinking_ms: null,
    });
    await stream.emit("chat-event", {
      request_id: rid,
      session_id: SESSION_ID,
      kind: "done",
      stop_reason: "end_turn",
      usage: null,
    });
    await expect(page.locator(`${MESSAGES} .msg__latency`).first()).toBeVisible({
      timeout: 15_000,
    });
    await expect
      .poll(() => distBottom(page), { timeout: 10_000 })
      .toBeLessThan(80);
  });

  test("会话切换落底 + 回底按钮复位", async ({ page, boot, mockCmd }) => {
    await seed10k(mockCmd);
    // 第二个会话(load_session 注册表按 cmd 键应答,两 session 共用同
    // 一种子 —— 断言面是「切换动作落底 + 按钮复位」,不依赖内容差异)。
    // updated_at 取当下:落「今天」分组(默认展开;更早分组默认折叠,
    // 行不出 DOM)。第一个 session 同样改今天 —— boot 后它成为当前
    // 会话,列表里两行都可见。
    const today = new Date().toISOString();
    mockCmd("sessions", "list_sessions", [
      {
        id: SESSION_ID,
        title: "会话甲",
        updated_at: today,
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
        color_tag: null,
        mode: "edit",
        workflow_enabled: false,
        plugin_name: "",
        session_type: "chat",
        metadata: null,
      },
      {
        id: "e2e-session-2",
        title: "会话乙",
        updated_at: today,
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
        color_tag: null,
        mode: "edit",
        workflow_enabled: false,
        plugin_name: "",
        session_type: "chat",
        metadata: null,
      },
    ]);
    await boot();
    await waitForListReady(page);

    // 上滚:按钮出现。切换会话 → 重挂落底(isAtBottom 复位,按钮消失)。
    await page.evaluate((sel) => {
      (document.querySelector(sel) as HTMLElement).scrollTop = 0;
    }, MESSAGES);
    await expect(page.locator(".scroll-to-bottom")).toBeVisible();

    await page.locator(".session-item", { hasText: "会话乙" }).click();
    await expect
      .poll(() => distBottom(page), { timeout: 15_000 })
      .toBeLessThan(80);
    await expect(page.locator(".scroll-to-bottom")).toHaveCount(0);
    // 确实切换了(active 态落在乙上)。
    await expect(
      page.locator(".session-item--active", { hasText: "会话乙" }),
    ).toBeVisible();
  });
});

// ---------------------------------------------------------------------------
// N4 PR3 移动端 390px 冒烟(prd「仅回归不退化」):虚拟化下
// ① 页面与消息行不溢出横向(行右缘不越过滚动容器 —— overflow-x:hidden
//    会静默裁剪,这里断言的是「没有东西被裁」);
// ② 回底按钮 44px 触控目标 + 8-13 档的显式避让(right 8 / bottom 64)在
//    虚拟化行几何上仍成立(S6a 契约,桌面块零改动)。
// ---------------------------------------------------------------------------
test.describe("N4 PR3 移动端 390px 冒烟", () => {
  test.use({ viewport: { width: 390, height: 844 } });

  test("390px:无横向溢出 + 回底按钮 44px 避让仍成立", async ({
    page,
    boot,
    mockCmd,
  }) => {
    await seed10k(mockCmd);
    await boot();
    await waitForListReady(page);

    // 页面级:无横向滚动(文档宽不超视口)。
    const overflow = await page.evaluate(() => ({
      doc: document.scrollingElement?.scrollWidth ?? 0,
      inner: window.innerWidth,
    }));
    expect(overflow.doc).toBeLessThanOrEqual(overflow.inner);

    // 消息行级:渲染窗口内所有行根的右缘都在滚动容器右缘内(±1px 测量
    // 噪声)—— 虚拟化行(translateY 定位 + 全宽 wrapper + .msg 对齐)
    // 不因绝对定位结构引入横向裁剪。
    const rowOverflow = await page.evaluate((sel) => {
      const container = document.querySelector<HTMLElement>(sel)!;
      const cRight = container.getBoundingClientRect().right;
      const rows = Array.from(
        container.querySelectorAll<HTMLElement>(".msg"),
      );
      const maxRight = Math.max(
        0,
        ...rows.map((r) => r.getBoundingClientRect().right),
      );
      return { cRight, maxRight };
    }, MESSAGES);
    expect(rowOverflow.maxRight).toBeLessThanOrEqual(rowOverflow.cRight + 1);

    // 回底按钮:滚离末端出现;390 视口命中 max-width:767px 移动块 ——
    // 44×44 触控目标 + 避让位(right 8 / bottom 64,±2px 测量容差)。
    // 避让基准 = 定位上下文 .messages-wrap(S6a 的 right/bottom 写在
    // wrap 坐标系;wrap 外侧还有布局自身留白,不与视口重合)。
    await page.evaluate((sel) => {
      (document.querySelector(sel) as HTMLElement).scrollTop = 0;
    }, MESSAGES);
    const button = page.locator(".scroll-to-bottom");
    await expect(button).toBeVisible();
    const box = await button.boundingBox();
    const wrap = await page
      .locator(".messages-wrap")
      .evaluate((el) => el.getBoundingClientRect());
    expect(box).not.toBeNull();
    expect(box!.width).toBe(44);
    expect(box!.height).toBe(44);
    expect(
      Math.abs(wrap.right - 8 - (box!.x + box!.width)),
    ).toBeLessThanOrEqual(2);
    expect(
      Math.abs(wrap.bottom - 64 - (box!.y + box!.height)),
    ).toBeLessThanOrEqual(2);

    // 按钮行为在移动视口下同样可用(点击回底落底)。
    await button.click();
    await expect
      .poll(() => distBottom(page), { timeout: 15_000 })
      .toBeLessThan(80);
  });
});
