// N9 F1 前端渲染基准主体(任务 09-19-n9-perf-benchmark,design §5)。
//
// 三测量组 × 三量级档(100/1k/10k,与 B2 种子同 profile 同源):
// - f1 mount:goto 完成 → MessageList 渲染稳定(.messages 子元素数
//   连续帧不变——组件 stick-to-bottom 收敛的代理信号,评审结论 #9;
//   双 rAF 系统性偏短)。
// - f2 滚动:.messages(programmatic)scroll 到底/顶/底,rAF 帧间隔序列。
// - f4 流式回放:mount 稳定后 stream.emit 推 start/delta×20/
//   turn_complete/done(fake EventSource),量「首 emit → 末尾 delta
//   文本上屏」。与 h3(后端 10k 整轮)构成前后端对子。
//
// 结构约束(实测教训):**每档独立 test()** —— Playwright 每 test 重建
// context,renderer 堆彻底释放;单 test 内 reload 复用同一 renderer,
// 10k 档 × 8 run 会把 WSL2 Chromium 直接崩掉(Target crashed)。
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
    test(`tier ${n} (${runs} runs)`, async ({ mockCmd, boot, page, stream }) => {
      const loaded = readFixture(n);
      const mountMs: number[] = [];
      const scrollFrameMs: number[] = [];
      const streamMs: number[] = [];

      for (let r = 0; r < runs; r++) {
        mockCmd("sessions", "list_sessions", [SESSION_ROW]);
        mockCmd("sessions", "load_session", loaded);
        await boot("/");

        // f1 mount(页面内测:goto 后 app JS 启动 → 列表出现 → 稳定)。
        mountMs.push(
          await page.evaluate(async () => {
            const t0 = performance.now();
            const list = () => document.querySelector(".messages");
            while (!list()) await new Promise(requestAnimationFrame);
            let prev = -1;
            let stable = 0;
            while (stable < 3) {
              await new Promise(requestAnimationFrame);
              const c = list()!.childElementCount;
              if (c === prev && c > 0) stable += 1;
              else {
                stable = 0;
                prev = c;
              }
            }
            return performance.now() - t0;
          }),
        );

        // f2 滚动:.messages 即滚动容器(MessageList.vue overflow-y:auto)。
        const stamps = await page.evaluate(async () => {
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
        for (let i = 1; i < stamps.length; i++) scrollFrameMs.push(stamps[i] - stamps[i - 1]);

        // f4 流式回放(start → 20×delta → turn_complete → done)。
        const s0 = Date.now();
        const rid = `rid-bench-${n}-${r}`;
        await stream.emit("chat-event", {
          request_id: rid,
          session_id: SESSION_ID,
          kind: "start",
        });
        for (let i = 0; i < 20; i++) {
          await stream.emit("chat-event", {
            request_id: rid,
            session_id: SESSION_ID,
            kind: "delta",
            text: ` bench-delta-${i} `,
          });
        }
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
        await page.waitForFunction(
          () => document.body.innerText.includes("bench-delta-19"),
          undefined,
          { timeout: 30_000 },
        );
        streamMs.push(Date.now() - s0);

        await page.reload();
      }

      tiers[`n${n}`] = {
        f1_mount_ms: stat(mountMs),
        f2_scroll_frame_ms: stat(scrollFrameMs),
        f4_stream_to_paint_ms: stat(streamMs),
      };
      // eslint-disable-next-line no-console
      console.log(`tier ${n}:`, JSON.stringify(tiers[`n${n}`]));
    });
  }
});
