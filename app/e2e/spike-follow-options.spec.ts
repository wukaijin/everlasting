// N4 PR0 spike(任务 09-19-n4-render-virtualization,design §9-1):
// @tanstack/vue-virtual 3.13.39(virtual-core 3.17.11)Vue 适配层对
// followOnAppend / anchorTo 的 **setOptions 响应式透传** 的运行时验证。
//
// # 判定标准(评审钉死):静态配置跑通不构成验证 —— 必须运行中动态
// 翻转 options,断言跟滚行为真的随之改变:
//   ① 初始 false → append 断言不跟;
//   ② 翻 'auto' 且视口在末端 → append 断言跟;
//   ③ 翻 true(设计文档称「强制」)→ 滚离末端后 append 断言仍跟;
//   ④ anchorTo:'end' → 末项 grow 时钉底。
// ③ 的实证结果与 PR1 对策见 implement.md PR0 段结论行(本文件 ③ 处
// 注释记录实证现实)。
//
// # 形态:被测页 app/bench/spike-follow-options.html(vite dev 直出的
// 静态 html + 内联 JS 模块,window.__spike 为唯一驱动面)。不经过 app
// mount / route-mock —— 用裸 @playwright/test,不挂 e2e/fixtures 的
// world(那些是产品 app 的 harness,spike 页不需要)。
import { expect, test, type Page } from "@playwright/test";

/** window.__spike.metrics() 的返回形状(spike 页定义)。 */
interface SpikeMetrics {
  ready: boolean;
  follow: false | "auto" | true;
  /** 库内侧写:virtualizer.options.followOnAppend —— 证明 setOptions
   *  真的收到新值(排除「spike 页自身没接上响应式」的假证伪)。 */
  followSeenByLib: false | "auto" | true | null;
  count: number;
  scrollTop: number;
  scrollHeight: number;
  clientHeight: number;
  distBottom: number;
}

const metrics = (page: Page): Promise<SpikeMetrics> =>
  page.evaluate(
    () =>
      (window as unknown as {
        __spike: { metrics(): SpikeMetrics };
      }).__spike.metrics(),
  );

const drive = (
  page: Page,
  fn: "append" | "growLast" | "setFollow" | "scrollToEnd" | "scrollUp",
  ...args: unknown[]
): Promise<unknown> =>
  page.evaluate(
    ({ fn, args }) => {
      const spike = (
        window as unknown as {
          __spike: Record<string, (...a: unknown[]) => unknown>;
        }
      ).__spike;
      return spike[fn](...args);
    },
    { fn, args },
  );

const distBottom = async (page: Page): Promise<number> =>
  (await metrics(page)).distBottom;

test("followOnAppend 三态翻转改变跟滚行为 + anchorTo:'end' 末项 grow 钉底", async ({
  page,
}) => {
  await page.goto("/bench/spike-follow-options.html");
  await page.waitForFunction(
    () =>
      (
        window as unknown as {
          __spike: { ready(): boolean };
        }
      ).__spike.ready() === true,
    undefined,
    { timeout: 30_000 },
  );

  // ---- ① 初始 followOnAppend=false:append → 不跟滚 --------------------
  await drive(page, "scrollToEnd");
  await expect.poll(() => distBottom(page)).toBeLessThan(40);
  const m0 = await metrics(page);

  await drive(page, "append", 2);
  // 不跟:距底拉开约 2×60px,scrollTop 一帧未动。
  await expect.poll(() => distBottom(page)).toBeGreaterThan(100);
  const m1 = await metrics(page);
  expect(m1.scrollTop).toBe(m0.scrollTop);
  expect(m1.followSeenByLib).toBe(false);

  // ---- ② 运行中翻 'auto'(setOptions)且视口在末端:append → 跟 --------
  await drive(page, "setFollow", "auto");
  // 透传直接证据:库实例的 options 已是新值(非页面侧影子变量)。
  expect((await metrics(page)).followSeenByLib).toBe("auto");
  await drive(page, "scrollToEnd");
  await expect.poll(() => distBottom(page)).toBeLessThan(40);
  await drive(page, "append", 2);
  // 跟:视口钉回末端,且 scrollTop 实际前进了。
  await expect.poll(() => distBottom(page)).toBeLessThan(40);
  const m2 = await metrics(page);
  expect(m2.scrollTop).toBeGreaterThan(m1.scrollTop);

  // ---- ③ 运行中翻 true(设计文档语义 =「强制」):滚离末端 append -------
  // 评审钉死的预期:仍跟。**实证结果(virtual-core 3.17.11)**:不跟 ——
  // core 的 setOptions 把 true 映射为 behavior:'auto',与 'auto' 完全
  // 同效;isAtEnd(scrollEndThreshold) 门对 true / 'auto' 一视同仁,
  // 滚离末端(>80px)后 append 一律不跟。库不存在「无视视口位置的强制
  // 跟滚」;断言按实证现实写,结论与 PR1 对策(手写 force-follow)见
  // implement.md PR0 段。
  await drive(page, "setFollow", true);
  expect((await metrics(page)).followSeenByLib).toBe(true);
  await drive(page, "scrollUp", 400);
  await expect.poll(() => distBottom(page)).toBeGreaterThan(80);
  const m3pre = await metrics(page);
  await drive(page, "append", 2);
  await expect.poll(() => distBottom(page)).toBeGreaterThan(100);
  const m3 = await metrics(page);
  expect(m3.scrollTop).toBe(m3pre.scrollTop);

  // ---- ③b 反向对照:'auto' 滚离末端 append 同样不跟(true ≡ 'auto')----
  await drive(page, "setFollow", "auto");
  await drive(page, "append", 2);
  await expect.poll(() => distBottom(page)).toBeGreaterThan(100);

  // ---- ④ anchorTo:'end':末项 grow 时钉底 ------------------------------
  await drive(page, "scrollToEnd");
  await expect.poll(() => distBottom(page)).toBeLessThan(40);
  const hBefore = (await metrics(page)).scrollHeight;
  await drive(page, "growLast", 300);
  // 先等内容真长高(测量落地)再断言钉底:resize 若落在滚动事件静默窗,
  // 库会推迟到 isScrolling 复位后补测 —— 直接 poll distBottom<40 会在
  // "仍在旧末端"时空转通过,什么都没验证到(repeat 实测踩过)。
  await expect
    .poll(async () => (await metrics(page)).scrollHeight, { timeout: 10_000 })
    .toBeGreaterThanOrEqual(hBefore + 200);
  // 钉底:ResizeObserver → measureElement → resizeItem 的 wasAtEnd 路径
  // 把视口贴住新末端(经 per-render _willUpdate 补偿完成 clamp 补写)。
  await expect.poll(() => distBottom(page)).toBeLessThan(40);
  const hAfter = (await metrics(page)).scrollHeight;
  expect(hAfter).toBeGreaterThanOrEqual(hBefore + 200);
});
