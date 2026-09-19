// discuss.test.mjs — discuss 纯函数全覆盖 + runDiscuss 注入 mock mcpCall 的
// 全链/超时/status --wait 路径(不打真 daemon;AC10)。
import test from 'node:test';
import assert from 'node:assert/strict';
import {
  VERBS,
  MCP_POLL_WAIT_S,
  stopReasonExitCode,
  stopReasonNote,
  diffProgressLine,
  waitSlice,
  runDiscuss,
} from './lib/discuss.mjs';
import { EvlError } from './lib/api.mjs';
import { UsageError } from './lib/args.mjs';

// ── 测试基建:捕获 stdout/stderr 的假 io + 可编程 mock mcpCall ──────────

function fakeIo() {
  const out = { stdout: '', stderr: '' };
  return {
    out,
    io: {
      stdout: { write: (s) => (out.stdout += s) },
      stderr: { write: (s) => (out.stderr += s) },
      stdin: null,
    },
  };
}

/** 按 tool 名分发的 mock:handlers = { toolName: (args, callIndex) => payload };
 * 记录每次调用到 calls(不真网络)。 */
function mockMcp(handlers) {
  const calls = [];
  const fn = async ({ tool, args }) => {
    calls.push({ tool, args });
    const h = handlers[tool];
    if (!h) throw new Error(`mock: 未注册工具 ${tool}`);
    if (Array.isArray(h)) return h[Math.min(calls.filter((c) => c.tool === tool).length, h.length) - 1];
    return typeof h === 'function' ? h(args) : h;
  };
  fn.calls = calls;
  return fn;
}

const TERMINAL_SNAP = { busy: false, stop_reason: 'group_chat_end', elapsed_s: 60 };
const RESULT_PAYLOAD = {
  stop_reason: 'group_chat_end',
  summary: '结论:两方一致。',
  roster: { moderator: 'glm', participants: ['a/m1', 'b/m2'] },
  stats: { messages: 12, elapsed_s: 480 },
  tokens: { total: 42000, per_speaker: [] },
  transcript_path: '/data/discussions/2026-09-19-x-sid8.md',
};

// ── 纯函数 ─────────────────────────────────────────────────────────────

test('VERBS: 七动词在册,顺序稳定', () => {
  assert.deepEqual(VERBS, ['start', 'status', 'result', 'cancel', 'interrupt', 'inject', 'presets']);
  assert.ok(MCP_POLL_WAIT_S === 25);
});

test('stopReasonExitCode: 具名档全表(开放集语义,design §6)', () => {
  assert.equal(stopReasonExitCode('group_chat_end'), 0);
  assert.equal(stopReasonExitCode('max_rounds'), 0);
  assert.equal(stopReasonExitCode('cancelled'), 0);
  assert.equal(stopReasonExitCode('preempted'), 0); // interrupt 自产真终态
  assert.equal(stopReasonExitCode('error'), 2);
  assert.equal(stopReasonExitCode('nominee_unknown'), 2);
  assert.equal(stopReasonExitCode('participant_unresolved'), 2);
  assert.equal(stopReasonExitCode('budget'), 6);
  assert.equal(stopReasonExitCode('interrupted'), 1); // 可续跑态
});

test('stopReasonExitCode: null / 空串 / 表外未知非空 → 1(防御档与开放集表外同码)', () => {
  assert.equal(stopReasonExitCode(null), 1);
  assert.equal(stopReasonExitCode(undefined), 1);
  assert.equal(stopReasonExitCode(''), 1);
  assert.equal(stopReasonExitCode('martian_reason'), 1);
  assert.equal(stopReasonExitCode('GROUP_CHAT_END'), 1); // 大小写敏感,表外处理
});

test('stopReasonNote: 表外值回显原值;interrupted 给 resume 提示;null 防御文案;具名正常档 null', () => {
  assert.match(stopReasonNote('martian_reason'), /martian_reason/);
  assert.match(stopReasonNote('interrupted'), /status/);
  assert.match(stopReasonNote(null), /null/);
  assert.equal(stopReasonNote('group_chat_end'), null);
  assert.equal(stopReasonNote('max_rounds'), null);
  assert.equal(stopReasonNote('preempted'), null);
  assert.equal(stopReasonNote('budget'), null);
});

test('diffProgressLine: 首拍(prev=null)产出行;同帧返回 null;变化才产出行', () => {
  const s1 = { busy: true, stop_reason: null, elapsed_s: 5, messages: 2, last_speaker: 'a', tokens: { total: 100 } };
  const line1 = diffProgressLine(null, s1);
  assert.match(line1, /\[discuss\]/);
  assert.match(line1, /msg 2/);
  assert.match(line1, /last a/);
  assert.match(line1, /tokens 100/);
  assert.match(line1, /elapsed 5s/);
  assert.equal(diffProgressLine(s1, { ...s1, elapsed_s: 6 }), null); // elapsed 不参与 diff
  const s2 = { ...s1, messages: 4, last_speaker: 'b', tokens: { total: 200 } };
  const line2 = diffProgressLine(s1, s2);
  assert.match(line2, /msg 4/);
  assert.match(line2, /last b/);
});

test('diffProgressLine: 缺 detail 字段的快照(last/tokens 缺)不炸,last 落 -', () => {
  const line = diffProgressLine(null, { busy: true, stop_reason: null, elapsed_s: 1 });
  assert.match(line, /last -/);
  assert.ok(!line.includes('msg'));
});

test('waitSlice: 裁到 1..25;剩余不足 1s 给 1(daemon 侧上界,略过线可接受)', () => {
  const t = 1_000_000;
  assert.equal(waitSlice(t + 600_000, t), 25);
  assert.equal(waitSlice(t + 10_000, t), 10);
  assert.equal(waitSlice(t + 25_000, t), 25);
  assert.equal(waitSlice(t + 1_000, t), 1);
  assert.equal(waitSlice(t, t), 1);
  assert.equal(waitSlice(t - 5_000, t), 1);
  assert.equal(waitSlice(t + 50_000, t, 30), 30); // cap 可注入
});

// ── runDiscuss:全链 ────────────────────────────────────────────────────

test('runDiscuss 全链:立即终态快路径 — json 平铺 {session_id, ...result},退 0', async () => {
  const { io, out } = fakeIo();
  const mcp = mockMcp({
    start_discussion: { session_id: 's1', request_id: 'r1' },
    discussion_status: TERMINAL_SNAP,
    discussion_result: RESULT_PAYLOAD,
  });
  const code = await runDiscuss({
    base: 'http://x', flags: { output: 'json', timeout: 540 }, positionals: ['议题文本'], io, mcpCall: mcp,
  });
  assert.equal(code, 0);
  const json = JSON.parse(out.stdout);
  assert.equal(json.session_id, 's1'); // result 载荷无 session_id 键,CLI 自补
  assert.equal(json.stop_reason, 'group_chat_end');
  assert.equal(json.summary, '结论:两方一致。');
  assert.deepEqual(json.roster.participants, ['a/m1', 'b/m2']);
  assert.ok(out.stderr.includes('session s1')); // 恢复锚点先出 stderr
  // 轮询走 MCP wait_seconds ≤ 25
  const statusCall = mcp.calls.find((c) => c.tool === 'discussion_status');
  assert.equal(statusCall.args.wait_seconds, 25);
  assert.equal(statusCall.args.session_id, 's1');
  // start 入参:缺省 flag 不带键(preview 缺省 review 由 daemon 定)
  const startCall = mcp.calls.find((c) => c.tool === 'start_discussion');
  assert.deepEqual(startCall.args, { topic: '议题文本', cwd: process.cwd() });
});

test('runDiscuss 全链:text 模式 — summary + roster/stats + transcript + 末行 stop_reason 标记', async () => {
  const { io, out } = fakeIo();
  const mcp = mockMcp({
    start_discussion: { session_id: 's1', request_id: 'r1' },
    discussion_status: { ...TERMINAL_SNAP, stop_reason: 'max_rounds' },
    discussion_result: { ...RESULT_PAYLOAD, stop_reason: 'max_rounds' },
  });
  const code = await runDiscuss({
    base: 'http://x', flags: { output: 'text', timeout: 540 }, positionals: ['议题'], io, mcpCall: mcp,
  });
  assert.equal(code, 0); // max_rounds → 0(text 末行标记让消费方可分辨轮帽截断)
  const lines = out.stdout.trimEnd().split('\n');
  assert.ok(out.stdout.includes('结论:两方一致。'));
  assert.ok(out.stdout.includes('participants(2)'));
  assert.ok(out.stdout.includes('tokens: 42000'));
  assert.ok(out.stdout.includes('transcript: /data/discussions'));
  assert.equal(lines[lines.length - 1], 'stop_reason: max_rounds');
});

test('runDiscuss 全链:多拍轮询 — busy(wait_timed_out:true)后续轮,进度行按变化打 stderr', async () => {
  const { io, out } = fakeIo();
  const mcp = mockMcp({
    start_discussion: { session_id: 's1', request_id: 'r1' },
    discussion_status: [
      { busy: true, stop_reason: null, elapsed_s: 5, messages: 2, last_speaker: 'a', tokens: { total: 100 }, wait_timed_out: true },
      { busy: true, stop_reason: null, elapsed_s: 30, messages: 2, last_speaker: 'a', tokens: { total: 100 }, wait_timed_out: true },
      { busy: true, stop_reason: null, elapsed_s: 60, messages: 4, last_speaker: 'b', tokens: { total: 200 }, wait_timed_out: true },
      TERMINAL_SNAP,
    ],
    discussion_result: RESULT_PAYLOAD,
  });
  const code = await runDiscuss({
    base: 'http://x', flags: { output: 'json', timeout: 540 }, positionals: ['议题'], io, mcpCall: mcp,
  });
  assert.equal(code, 0);
  assert.equal(mcp.calls.filter((c) => c.tool === 'discussion_status').length, 4);
  const progress = out.stderr.split('\n').filter((l) => l.startsWith('[discuss]'));
  assert.equal(progress.length, 2); // 首拍 + msg 变化拍;同帧不重复
  assert.match(progress[0], /msg 2/);
  assert.match(progress[1], /msg 4/);
});

test('runDiscuss 全链:--timeout 到点 — 不 cancel,退 7,json 带 error:timeout + recovery,stderr 勿重跑', async () => {
  const { io, out } = fakeIo();
  const t0 = 1_000_000;
  let n = 0;
  const nowFn = () => (n++ < 3 ? t0 : t0 + 600_000); // 前 3 次(deadline+首拍)在窗内,之后过窗
  const mcp = mockMcp({
    start_discussion: { session_id: 's1', request_id: 'r1' },
    discussion_status: { busy: true, stop_reason: null, elapsed_s: 5, wait_timed_out: true },
    cancel_discussion: () => {
      throw new Error('cancel 不应被调用(超时不 cancel,design §8)');
    },
  });
  const code = await runDiscuss({
    base: 'http://x', flags: { output: 'json', timeout: 540 }, positionals: ['议题'], io, mcpCall: mcp, nowFn,
  });
  assert.equal(code, 7);
  assert.ok(!mcp.calls.some((c) => c.tool === 'cancel_discussion')); // 关键反义:与 chat 不同
  const json = JSON.parse(out.stdout);
  assert.deepEqual(json, {
    session_id: 's1',
    stop_reason: null,
    error: 'timeout',
    recovery: 'evl discuss status s1 --wait 540',
  });
  // 防重跑文案(stderr 里超时报告行须以"勿重跑"领起;session 锚点行合法地在更早)
  const stderrLines = out.stderr.split('\n').filter((l) => l !== '');
  assert.match(stderrLines[stderrLines.length - 1], /^evl: timeout:/);
  assert.match(stderrLines[stderrLines.length - 1], /讨论仍在跑,勿重跑/);
  assert.match(out.stderr, /evl discuss status s1 --wait 540/);
});

test('runDiscuss 全链:轮询中途传输错 → 退 1(抛 EvlError)+ 续窗提示,不重试', async () => {
  const { io, out } = fakeIo();
  let statusCalls = 0;
  const mcp = mockMcp({
    start_discussion: { session_id: 's1', request_id: 'r1' },
    discussion_status: () => {
      statusCalls += 1;
      throw new EvlError('daemon 不可达');
    },
  });
  await assert.rejects(
    runDiscuss({ base: 'http://x', flags: { output: 'json', timeout: 540 }, positionals: ['议题'], io, mcpCall: mcp }),
    (e) => {
      assert.ok(e instanceof EvlError);
      assert.equal(e.exitCode, 1);
      assert.match(e.message, /勿重跑/);
      assert.match(e.message, /续窗:evl discuss status s1 --wait 540/);
      return true;
    }
  );
  assert.equal(statusCalls, 1); // 不重试(session 在 daemon 继续)
});

test('runDiscuss 全链:异常收场族 / budget / interrupted 的退出码翻译透传', async () => {
  for (const [reason, want] of [['error', 2], ['nominee_unknown', 2], ['budget', 6], ['interrupted', 1]]) {
    const { io, out } = fakeIo();
    const mcp = mockMcp({
      start_discussion: { session_id: 's1', request_id: 'r1' },
      discussion_status: { busy: false, stop_reason: reason, elapsed_s: 5 },
      discussion_result: { ...RESULT_PAYLOAD, stop_reason: reason },
    });
    const code = await runDiscuss({
      base: 'http://x', flags: { output: 'json', timeout: 540 }, positionals: ['议题'], io, mcpCall: mcp,
    });
    assert.equal(code, want, reason);
    assert.equal(JSON.parse(out.stdout).stop_reason, reason);
  }
});

test('runDiscuss 全链:表外未知 stop_reason → 退 1 + stderr 回显原值', async () => {
  const { io, out } = fakeIo();
  const mcp = mockMcp({
    start_discussion: { session_id: 's1', request_id: 'r1' },
    discussion_status: { busy: false, stop_reason: 'future_new_value', elapsed_s: 5 },
    discussion_result: { ...RESULT_PAYLOAD, stop_reason: 'future_new_value' },
  });
  const code = await runDiscuss({
    base: 'http://x', flags: { output: 'json', timeout: 540 }, positionals: ['议题'], io, mcpCall: mcp,
  });
  assert.equal(code, 1);
  assert.match(out.stderr, /future_new_value/); // 回显原值
});

test('runDiscuss 全链:--preset/--cwd/--token-budget/--roster 透传为 start 入参(命名翻译)', async () => {
  const { io } = fakeIo();
  const mcp = mockMcp({
    start_discussion: { session_id: 's1', request_id: 'r1' },
    discussion_status: TERMINAL_SNAP,
    discussion_result: RESULT_PAYLOAD,
  });
  await runDiscuss({
    base: 'http://x',
    flags: {
      output: 'json', timeout: 540, preset: 'arch', cwd: '/repo',
      tokenBudget: 1000, roster: [{ name: 'a', model: 'm1' }],
    },
    positionals: ['议题'], io, mcpCall: mcp,
  });
  assert.deepEqual(mcp.calls[0].args, {
    topic: '议题',
    cwd: '/repo',
    preset: 'arch',
    participants: [{ name: 'a', model: 'm1' }],
    token_budget: 1000,
  });
});

// ── runDiscuss:动词 ────────────────────────────────────────────────────

test('runDiscuss start:只建群不等待,json 出 {session_id, request_id},退 0', async () => {
  const { io, out } = fakeIo();
  const mcp = mockMcp({ start_discussion: { session_id: 's1', request_id: 'r1' } });
  const code = await runDiscuss({
    base: 'http://x', flags: { output: 'json', timeout: 540 }, positionals: ['start', '议题'], io, mcpCall: mcp,
  });
  assert.equal(code, 0);
  assert.deepEqual(JSON.parse(out.stdout), { session_id: 's1', request_id: 'r1' });
  assert.equal(mcp.calls.length, 1); // 不触碰 status/result
});

test('runDiscuss start:text 模式两行;动词名开头议题经 positionals 不被吞(AC8)', async () => {
  const { io, out } = fakeIo();
  const mcp = mockMcp({ start_discussion: { session_id: 's1', request_id: 'r1' } });
  await runDiscuss({
    base: 'http://x', flags: { output: 'text', timeout: 540 },
    positionals: ['start', 'status 这个词当议题'], io, mcpCall: mcp,
  });
  assert.equal(out.stdout, 's1\nr1\n');
  assert.equal(mcp.calls[0].args.topic, 'status 这个词当议题');
});

test('runDiscuss status:无 --wait 单次快照,busy 也退 0(busy 是数据不是错误)', async () => {
  const { io, out } = fakeIo();
  const mcp = mockMcp({
    discussion_status: { busy: true, stop_reason: null, elapsed_s: 5 },
  });
  const code = await runDiscuss({
    base: 'http://x', flags: { output: 'json', timeout: 540 }, positionals: ['status', 's1'], io, mcpCall: mcp,
  });
  assert.equal(code, 0);
  assert.deepEqual(JSON.parse(out.stdout), { busy: true, stop_reason: null, elapsed_s: 5 });
  const call = mcp.calls[0];
  assert.equal(call.args.wait_seconds, undefined); // 无 wait 不带 wait_seconds
  assert.equal(call.args.detail, undefined);
});

test('runDiscuss status:--detail 单次快照带 detail: true', async () => {
  const { io } = fakeIo();
  const mcp = mockMcp({
    discussion_status: { busy: true, stop_reason: null, elapsed_s: 5, messages: 1 },
  });
  await runDiscuss({
    base: 'http://x', flags: { output: 'json', timeout: 540, detail: true }, positionals: ['status', 's1'], io, mcpCall: mcp,
  });
  assert.equal(mcp.calls[0].args.detail, true);
});

test('runDiscuss status --wait:已终态秒返(不耗窗口,一次调用)', async () => {
  const { io, out } = fakeIo();
  const mcp = mockMcp({
    discussion_status: { busy: false, stop_reason: 'group_chat_end', elapsed_s: 60, messages: 9, last_speaker: 'mod', tokens: { total: 1 } },
  });
  const code = await runDiscuss({
    base: 'http://x', flags: { output: 'json', timeout: 540, wait: 540 }, positionals: ['status', 's1'], io, mcpCall: mcp,
  });
  assert.equal(code, 0);
  assert.equal(mcp.calls.filter((c) => c.tool === 'discussion_status').length, 1); // 终态短路先于窗口循环
  assert.equal(JSON.parse(out.stdout).stop_reason, 'group_chat_end');
});

test('runDiscuss status --wait:wait_timed_out 键缺失 = 变化(判据 !== true,== false 会反转)', async () => {
  const { io } = fakeIo();
  const mcp = mockMcp({
    // 第一拍:daemon wait 超时(键显式 true);第二拍:变化即返(键缺失)
    discussion_status: [
      { busy: true, stop_reason: null, elapsed_s: 5, wait_timed_out: true },
      { busy: true, stop_reason: null, elapsed_s: 30, messages: 3, last_speaker: 'a', tokens: { total: 9 } },
    ],
  });
  const code = await runDiscuss({
    base: 'http://x', flags: { output: 'json', timeout: 540, wait: 60 }, positionals: ['status', 's1'], io, mcpCall: mcp,
  });
  assert.equal(code, 0);
  assert.equal(mcp.calls.filter((c) => c.tool === 'discussion_status').length, 2);
});

test('runDiscuss status --wait:窗口尽(wait_timed_out 恒 true)返末次快照', async () => {
  const { io, out } = fakeIo();
  const t0 = 1_000_000;
  let n = 0;
  const nowFn = () => (n++ < 3 ? t0 : t0 + 600_000); // 前 3 次在窗内,之后过窗
  const mcp = mockMcp({
    discussion_status: { busy: true, stop_reason: null, elapsed_s: 5, wait_timed_out: true },
  });
  const code = await runDiscuss({
    base: 'http://x', flags: { output: 'json', timeout: 540, wait: 60 }, positionals: ['status', 's1'], io, mcpCall: mcp, nowFn,
  });
  assert.equal(code, 0);
  const json = JSON.parse(out.stdout);
  assert.equal(json.busy, true);
  assert.equal(json.wait_timed_out, true);
});

test('runDiscuss status --wait:text 按字段存在性输出(wait 隐含 detail,不看 --detail flag)', async () => {
  const { io, out } = fakeIo();
  const mcp = mockMcp({
    discussion_status: { busy: true, stop_reason: null, elapsed_s: 30, messages: 3, last_speaker: 'a', tokens: { total: 9 } },
  });
  await runDiscuss({
    base: 'http://x', flags: { output: 'text', timeout: 540, wait: 30 }, positionals: ['status', 's1'], io, mcpCall: mcp,
  });
  assert.ok(out.stdout.includes('messages: 3')); // flag 未给 --detail,字段在就输出
  assert.ok(out.stdout.includes('last_speaker: a'));
  assert.ok(out.stdout.includes('tokens: 9'));
});

test('runDiscuss result:成功恒退 0(stop_reason 是数据;budget 也 0——双口径)', async () => {
  const { io, out } = fakeIo();
  const mcp = mockMcp({
    discussion_result: { ...RESULT_PAYLOAD, stop_reason: 'budget' },
  });
  const code = await runDiscuss({
    base: 'http://x', flags: { output: 'json', timeout: 540 }, positionals: ['result', 's1'], io, mcpCall: mcp,
  });
  assert.equal(code, 0);
  const json = JSON.parse(out.stdout);
  assert.equal(json.session_id, 's1');
  assert.equal(json.stop_reason, 'budget');
});

test('runDiscuss result:运行中语义错(isError)→ EvlError 退 1 透传', async () => {
  const { io } = fakeIo();
  const mcp = mockMcp({
    discussion_result: () => {
      throw new EvlError('still running (busy=true) — poll discussion_status');
    },
  });
  await assert.rejects(
    runDiscuss({ base: 'http://x', flags: { output: 'json', timeout: 540 }, positionals: ['result', 's1'], io, mcpCall: mcp }),
    (e) => e instanceof EvlError && e.exitCode === 1 && /still running/.test(e.message)
  );
});

test('runDiscuss cancel/interrupt:载荷原样出,退 0(cancel 幂等 already_finished 也 0)', async () => {
  for (const [verb, tool, payload] of [
    ['cancel', 'cancel_discussion', { cancelled: true, session_id: 's1', note: 'stopping' }],
    ['cancel', 'cancel_discussion', { already_finished: true, stop_reason: 'group_chat_end', session_id: 's1' }],
    ['interrupt', 'interrupt_discussion', { interrupted: true, session_id: 's1', hint: 'wrapping up' }],
  ]) {
    const { io, out } = fakeIo();
    const mcp = mockMcp({ [tool]: payload });
    const code = await runDiscuss({
      base: 'http://x', flags: { output: 'json', timeout: 540 }, positionals: [verb, 's1'], io, mcpCall: mcp,
    });
    assert.equal(code, 0, verb);
    assert.deepEqual(JSON.parse(out.stdout), payload);
  }
});

test('runDiscuss inject:sid + text join;缺 text → 64', async () => {
  const { io, out } = fakeIo();
  const mcp = mockMcp({ inject_message: { injected: true, session_id: 's1' } });
  const code = await runDiscuss({
    base: 'http://x', flags: { output: 'json', timeout: 540 }, positionals: ['inject', 's1', '请', '聚焦'], io, mcpCall: mcp,
  });
  assert.equal(code, 0);
  assert.deepEqual(JSON.parse(out.stdout), { injected: true, session_id: 's1' });
  assert.equal(mcp.calls[0].args.text, '请 聚焦');
  await assert.rejects(
    runDiscuss({
      base: 'http://x', flags: { output: 'json', timeout: 540 }, positionals: ['inject', 's1'], io, mcpCall: mcp,
    }),
    (e) => e instanceof UsageError && e.exitCode === 64
  );
});

test('runDiscuss presets:text 表 + json 原样', async () => {
  const payload = {
    presets: [
      { key: 'arch', source: 'builtin', moderator: 'glm', participants: [{ name: 'a' }, { name: 'b' }], name: '架构' },
    ],
    degraded: false,
  };
  const j = fakeIo();
  const mcpJson = mockMcp({ list_presets: payload });
  await runDiscuss({
    base: 'http://x', flags: { output: 'json', timeout: 540 }, positionals: ['presets'], io: j.io, mcpCall: mcpJson,
  });
  assert.deepEqual(JSON.parse(j.out.stdout), payload);

  const t = fakeIo();
  const mcpText = mockMcp({ list_presets: payload });
  await runDiscuss({
    base: 'http://x', flags: { output: 'text', timeout: 540 }, positionals: ['presets'], io: t.io, mcpCall: mcpText,
  });
  assert.match(t.out.stdout, /key/);
  assert.match(t.out.stdout, /arch/);
  assert.match(t.out.stdout, /2/); // participants 人数列
});

// ── 用法错(64)─────────────────────────────────────────────────────────

test('runDiscuss 用法错:缺议题 / status 缺 sid / result 缺 sid / inject 缺 text → UsageError 64', async () => {
  const base = { base: 'http://x', flags: { output: 'json', timeout: 540 }, mcpCall: mockMcp({}) };
  for (const positionals of [
    [], // 全链缺议题
    ['start'], // start 缺议题
    ['status'], // 缺 sid
    ['result'], // 缺 sid
    ['cancel'], // 缺 sid
    ['interrupt'], // 缺 sid
    ['inject'], // 缺 sid + text
    ['inject', 's1'], // 缺 text
  ]) {
    const { io } = fakeIo();
    await assert.rejects(
      runDiscuss({ ...base, positionals, io }),
      (e) => e instanceof UsageError && e.exitCode === 64,
      JSON.stringify(positionals)
    );
  }
});
