// N9 F1 前端渲染基准主体(任务 09-19-n9-perf-benchmark,design §5)。
//
// 三测量组 × 三量级档(100/1k/10k,与 B2 种子同 profile 同源):
// - f1 mount:goto 完成 → MessageList 渲染稳定(.messages 的
//   scrollHeight 连续 3 帧 rAF 不变且 >0)。N4 PR0(2026-09-19)换尺:
//   旧判据 childElementCount 连续帧不变在虚拟化下首帧即常数,效度失效
//   (评审推翻);scrollHeight 与列表实现/tag 结构无关,裸 v-for 与
//   虚拟化同尺可比(AC1 双行口径)。与 e2e waitForListReady 同思路,
//   因 f1 需 in-page 计时(且尺子要紧,不加大静默窗)故此处内联。
// - f2 滚动:.messages(programmatic)scroll 到底/顶/底,rAF 帧间隔序列。
// - f4 流式回放:mount 稳定后 stream.emit 推 start/delta×20/
//   turn_complete/done(fake EventSource),量「首 emit → 末尾 delta
//   文本上屏」。与 h3(后端 10k 整轮)构成前后端对子。
//
// 结构约束(实测教训):**每档独立 test()** —— Playwright 每 test 重建
// context,renderer 堆彻底释放;单 test 内 reload 复用同一 renderer,
// 10k 档 × 8 run 会把 WSL2 Chromium 直接崩掉(Target crashed)。
// N4 PR0(2026-09-19)加码:10k 档连 3 run + reload 也 2/2 崩(死点漂移
// 于 f2/f4 evaluate,Execution context destroyed = renderer 死亡特征),
// 改为**每 run 独立 test**(fresh context);单 run 测量协议不变
// (boot→f1→f2→f4),与 N9 基线同参可比。
//
// 数字口径:median + P75 + max;100/1k 档 8 run、10k 档 3 run;不做
// P99(名不实,评审结论 #11)。人工检查列(报告内提示,非自动判定):
// 滚动 thumb 稳定性 / 锚定漂移(帧分布量不出)。

import { test } from "../e2e/fixtures";
// Node IO(node:fs)走 io.mjs 薄层(app 无 @types/node,见 io.mjs 头注释)。
const { readFixture, writeReport } = await import("./io.mjs");

const SESSION_ID = "e2e-session-1";

// SessionList 行形态(出处 tool-card-compact.spec.ts SESSION_ROW;bench
// 不 import spec 文件,内联单一来源注释)。
const SESSION_ROW = {
  id: SESSION_ID,
  title: "bench 种子会话",
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

const stat = (xs: number[]) => {
  const s = [...xs].sort((a, b) => a - b);
  const at = (p: number) => s[Math.min(s.length - 1, Math.floor(p * s.length))];
  return {
    runs: s.length,
    median: +at(0.5).toFixed(1),
    p75: +at(0.75).toFixed(1),
    max: +s[s.length - 1].toFixed(1),
  };
};

// workers=1(bench config)保证 describe 级聚合无竞态。
const tiers: Record<string, unknown> = {};

test.describe("N9 F1 render bench", () => {
  // 跨 test 聚合的原始样本(10k 档每 run 独立 test,样本落这里,
  // afterAll 统一算 stat;workers=1 下无竞态)。
  const raw: Record<string, { mountMs: number[]; scrollFrameMs: number[]; streamMs: number[] }> = {};
  const sinkOf = (key: string) => {
    raw[key] ??= { mountMs: [], scrollFrameMs: [], streamMs: [] };
    return raw[key];
  };
  const finalize = (key: string) => {
    tiers[key] = {
      f1_mount_ms: stat(raw[key]!.mountMs),
      f2_scroll_frame_ms: stat(raw[key]!.scrollFrameMs),
      f4_stream_to_paint_ms: stat(raw[key]!.streamMs),
    };
    // eslint-disable-next-line no-console
    console.log(`${key}:`, JSON.stringify(tiers[key]));
  };

  test.afterAll(() => {
    const file = writeReport({
      date: new Date().toISOString(),
      note: "median/p75/max over runs;人工检查列:滚动 thumb 稳定性 / 锚定漂移",
      tiers,
    });
    // eslint-disable-next-line no-console
    console.log(`report: ${file}`);
  });

  for (const n of [100, 1000, 10000]) {
    const runs = n >= 10000 ? 3 : 8;
    const key = `n${n}`;
    // 单 run 测量协议(两档共用):mock 种子 → boot → f1 mount → f2 滚动
    // → f4 流式回放。run 末尾的 reload 已上移到「同 test 多 run」档的
    // 循环里;10k 每 run 独立 test,fresh context 天然隔离。
    const runOnce = async (
      r: number,
      fx: {
        page: import("@playwright/test").Page;
        boot: (path?: string) => Promise<void>;
        stream: { emit: (name: string, payload: unknown) => Promise<void> };
        mockCmd: (domain: string, cmd: string, payload: unknown) => void;
      },
      sink: { mountMs: number[]; scrollFrameMs: number[]; streamMs: number[] },
    ) => {
      fx.mockCmd("sessions", "list_sessions", [SESSION_ROW]);
      fx.mockCmd("sessions", "load_session", readFixture(n));
      await fx.boot("/");

      // f1 mount(页面内测:goto 后 app JS 启动 → 列表出现 → 稳定)。
      // 判据 = .messages scrollHeight 连续 3 帧不变(>0;h===0 的空表
      // 不算稳定,防 vite 冷变换期的假收敛)。实现中立(N4 PR0 换尺,
      // 见文件头)。
      sink.mountMs.push(
        await fx.page.evaluate(async () => {
          const t0 = performance.now();
          const list = () => document.querySelector(".messages");
          while (!list()) await new Promise(requestAnimationFrame);
          let prev = -1;
          let stable = 0;
          while (stable < 3) {
            await new Promise(requestAnimationFrame);
            const h = list()!.scrollHeight;
            if (h > 0 && h === prev) stable += 1;
            else {
              stable = 0;
              prev = h;
            }
          }
          return performance.now() - t0;
        }),
      );

      // f2 滚动:.messages 即滚动容器(MessageList.vue overflow-y:auto)。
      const stamps = await fx.page.evaluate(async () => {
        const out: number[] = [];
        const el = document.querySelector<HTMLElement>(".messages");
        if (!el) throw new Error(".messages missing");
        el.scrollTop = el.scrollHeight;
        await new Promise(requestAnimationFrame);
        out.push(performance.now());
        el.scrollTop = 0;
        await new Promise(requestAnimationFrame);
        out.push(performance.now());
        el.scrollTop = el.scrollHeight;
        await new Promise(requestAnimationFrame);
        out.push(performance.now());
        return out;
      });
      for (let i = 1; i < stamps.length; i++) sink.scrollFrameMs.push(stamps[i] - stamps[i - 1]);

      // f4 流式回放(start → 20×delta → turn_complete → done)。
      const s0 = Date.now();
      const rid = `rid-bench-${n}-${r}`;
      await fx.stream.emit("chat-event", {
        request_id: rid,
        session_id: SESSION_ID,
        kind: "start",
      });
      for (let i = 0; i < 20; i++) {
        await fx.stream.emit("chat-event", {
          request_id: rid,
          session_id: SESSION_ID,
          kind: "delta",
          text: ` bench-delta-${i} `,
        });
      }
      await fx.stream.emit("chat-event", {
        request_id: rid,
        session_id: SESSION_ID,
        kind: "turn_complete",
        seq: 99_999,
        ttfb_ms: null,
        gen_ms: null,
        total_ms: 5,
        thinking_ms: null,
      });
      await fx.stream.emit("chat-event", {
        request_id: rid,
        session_id: SESSION_ID,
        kind: "done",
        stop_reason: "end_turn",
        usage: null,
      });
      await fx.page.waitForFunction(
        () => document.body.innerText.includes("bench-delta-19"),
        undefined,
        { timeout: 30_000 },
      );
      sink.streamMs.push(Date.now() - s0);
    };

    if (n < 10000) {
      test(`tier ${n} (${runs} runs)`, async ({ mockCmd, boot, page, stream }) => {
        const sink = sinkOf(key);
        for (let r = 0; r < runs; r++) {
          await runOnce(r, { mockCmd, boot, page, stream }, sink);
          await page.reload();
        }
        finalize(key);
      });
    } else {
      // 10k 档:每 run 独立 test(fresh context,文件头「结构约束」);
      // 样本聚合进同一 key,afterAll 前最后一次 finalize 为全 run 统计。
      for (let r = 0; r < runs; r++) {
        test(`tier ${n} run ${r + 1}/${runs}`, async ({ mockCmd, boot, page, stream }) => {
          await runOnce(r, { mockCmd, boot, page, stream }, sinkOf(key));
          finalize(key);
        });
      }
    }
  }
});
