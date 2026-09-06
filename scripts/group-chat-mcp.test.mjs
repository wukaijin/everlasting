// group-chat-mcp 纯逻辑 + SDK 接线单测(node:test;shape 断言非快照)。
// 跑法:node --test scripts/group-chat-mcp.test.mjs
// 分层:纯逻辑测试零 SDK import(AC2);末尾一组走真 SDK InMemoryTransport
// 验协议接线(registerTool 参数形状/返回 content 形状),不 spawn 进程。
import { test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

import {
  TOOLS, TOOLS_BUDGET_CHARS, buildToolShapes, createLedger, isTerminal,
  coreStart, coreStatus, coreResult, coreCancel, ensureTranscript, realDeps,
} from './group-chat-mcp.mjs';

const MODELS = [
  { id: 'uuid-glm53', modelName: 'glm-5.3', displayName: 'glm-5.3' },
  { id: 'uuid-flash', modelName: 'glm-5.3-flash', displayName: 'GLM-5.3-Flash' },
  { id: 'uuid-deepseek', modelName: 'deepseek-v4-flash', displayName: 'deepseek-v4-flash' },
  { id: 'uuid-m3', modelName: 'MiniMax-M3', displayName: 'MiniMax-M3' },
];

/** mock deps:按场景注入;记录调用供断言。session 形状 = list_sessions 行。 */
function makeMockDeps({ session, loaded, project = { id: 'proj-1', created: false } } = {}) {
  const calls = { createSession: [], fireChat: [], cancelChat: [], pollSession: [], loadSession: [], listModels: 0 };
  const deps = {
    base: 'http://mock',
    resolveProject: async () => { calls.resolveProject = (calls.resolveProject || 0) + 1; return project; },
    listModels: async () => { calls.listModels++; return MODELS; },
    createSession: async (body) => { calls.createSession.push(body); return { id: 'sess-1', ...body }; },
    fireChat: async (body) => { calls.fireChat.push(body); return {}; },
    pollSession: async (projectId, sessionId) => { calls.pollSession.push([projectId, sessionId]); return session ? { ...session } : null; },
    loadSession: async (sessionId) => { calls.loadSession.push(sessionId); return loaded; },
    cancelChat: async (requestId) => { calls.cancelChat.push(requestId); return {}; },
  };
  return { deps, calls };
}

function tmpLedger() {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'gcmcp-test-'));
  return { ledger: createLedger({ file: path.join(dir, 'state.json') }), dir };
}

// ---------------------------------------------------------------------------
// 工具面(AC4 预算 + AC2 完整性)
// ---------------------------------------------------------------------------

test('AC4(文案层):四工具面完整,成本闸/不阻塞语义在 start 描述里(R3)', () => {
  assert.deepEqual(TOOLS.map((t) => t.name),
    ['start_discussion', 'discussion_status', 'discussion_result', 'cancel_discussion']);
  const desc = TOOLS[0].description;
  assert.match(desc, /5-15 min/);
  assert.match(desc, /tokens/);
  assert.match(desc, /Returns immediately/);
  // 字符预算的地面真值在 SDK wire 测试里按 listTools 实测(见下)
});

// ---------------------------------------------------------------------------
// 记账(createLedger)
// ---------------------------------------------------------------------------

test('记账:内存命中 → 进程「重启」后文件兜底 → 损坏文件容错', () => {
  const { ledger, dir } = tmpLedger();
  ledger.set('s1', { request_id: 'r1', project_id: 'p1', cwd: '/a', topic: 't', started_at_ms: 1 });
  assert.equal(ledger.get('s1').request_id, 'r1');

  const reborn = createLedger({ file: path.join(dir, 'state.json') }); // 模拟 server 重启
  assert.equal(reborn.get('s1').request_id, 'r1', '重启后从文件恢复');
  assert.equal(reborn.get('nope'), null);

  fs.writeFileSync(path.join(dir, 'corrupt.json'), '{broken');
  const bad = createLedger({ file: path.join(dir, 'corrupt.json') });
  assert.equal(bad.get('s1'), null, '损坏文件按空账本处理不抛错');
});

test('终态判定:busy/stop_reason 组合(四值终态;running/not-ran 非终态)', () => {
  for (const r of ['group_chat_end', 'max_rounds', 'cancelled', 'error']) {
    assert.equal(isTerminal({ busy: false, stop_reason: r }), true, r);
  }
  assert.equal(isTerminal({ busy: true, stop_reason: null }), false);
  assert.equal(isTerminal({ busy: false, stop_reason: null }), false, '从未跑过');
  assert.equal(isTerminal(null), false);
});

// ---------------------------------------------------------------------------
// coreStart(AC5:created_via=mcp;P1-1:moderator 恒取预设)
// ---------------------------------------------------------------------------

test('coreStart:全链 + created_via=mcp + 记账含 project_id + request_id 前缀', async () => {
  const { deps, calls } = makeMockDeps();
  const { ledger } = tmpLedger();
  const out = await coreStart(deps, ledger, { topic: '议题X', cwd: '/work/p1', preset: 'review' });

  assert.equal(out.session_id, 'sess-1');
  assert.match(out.request_id, /^gcmcp-\d+-/);
  assert.match(out.hint, /discussion_status/);
  // wire:moderator UUID、created_via 盖戳
  const body = calls.createSession[0];
  assert.equal(body.model, 'uuid-m3');
  assert.equal(body.metadata.created_via, 'mcp');
  assert.equal(body.session_type, 'group_chat');
  // participants 模型全部解析成 UUID
  assert.deepEqual(body.metadata.participants.map((p) => p.model), ['uuid-glm53', 'uuid-flash', 'uuid-deepseek']);
  // fire-and-forget 一次 + 记账五要素
  assert.equal(calls.fireChat.length, 1);
  const entry = ledger.get('sess-1');
  assert.deepEqual(
    [entry.request_id === out.request_id, entry.project_id, entry.cwd, entry.topic, typeof entry.started_at_ms],
    [true, 'proj-1', '/work/p1', '议题X', 'number'],
  );
});

test('coreStart:P1-1 participants 整名单只换名单,moderator 恒取预设值', async () => {
  const { deps, calls } = makeMockDeps();
  const { ledger } = tmpLedger();
  await coreStart(deps, ledger, {
    topic: 't', cwd: '/w', preset: 'arch',
    participants: [{ name: '甲', model: 'glm-5.3' }],
  });
  assert.equal(calls.createSession[0].model, 'uuid-m3', 'moderator 仍是 arch 预设的 MiniMax-M3');
  assert.deepEqual(calls.createSession[0].metadata.participants, [{ name: '甲', model: 'uuid-glm53' }]);
});

test('coreStart:校验错误路径(缺 topic / 未知 preset / 模型失配报清单)', async () => {
  const { deps } = makeMockDeps();
  const { ledger } = tmpLedger();
  await assert.rejects(coreStart(deps, ledger, { topic: ' ', cwd: '/w' }), /缺议题/);
  await assert.rejects(coreStart(deps, ledger, { topic: 't', cwd: '/w', preset: 'nope' }), /未知预设/);
  await assert.rejects(
    coreStart(deps, ledger, { topic: 't', cwd: '/w', preset: 'review', participants: [{ name: '甲', model: '不存在模型' }] }),
    /不存在模型/,
  );
});

// ---------------------------------------------------------------------------
// coreStatus / coreResult(P1-2 兜底链;P2-1 惰性转录;P2-2 无轮次字段)
// ---------------------------------------------------------------------------

test('coreStatus:running 态无转录动作;终态首次观测惰性导出 + 幂等', async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'gcmcp-status-'));
  const session = (busy, stop_reason) => ({ busy, stop_reason, id: 'sess-1', project_id: 'proj-1' });
  const loaded = {
    session: { id: 'sess-1', project_id: 'proj-1', current_cwd: dir, created_at: new Date().toISOString(), model: 'uuid-m3', metadata: JSON.stringify({ participants: [{ name: '甲', model: 'uuid-glm53' }] }), discussion_summary: 'S', stop_reason: 'group_chat_end' },
    messages: [{ seq: 0, role: 'user', text: '议题X', has_tool_calls: false, has_tool_results: false }],
  };
  {
    const { deps, calls } = makeMockDeps({ session: session(true, null), loaded });
    const { ledger } = tmpLedger();
    ledger.set('sess-1', { request_id: 'r1', project_id: 'proj-1', cwd: dir, topic: '议题X', started_at_ms: Date.now() - 5000 });
    const out = await coreStatus(deps, ledger, 'sess-1');
    assert.deepEqual([out.busy, out.stop_reason], [true, null]);
    assert.equal(typeof out.elapsed_s, 'number');
    assert.equal('transcript_path' in out, false, 'running 态不导转录');
    assert.equal(calls.loadSession.length, 0, '记账命中时不需要 load_session');
  }
  {
    const { deps, calls } = makeMockDeps({ session: session(false, 'group_chat_end'), loaded });
    const { ledger } = tmpLedger();
    ledger.set('sess-1', { request_id: 'r1', project_id: 'proj-1', cwd: dir, topic: '议题X', started_at_ms: Date.now() - 5000 });
    const out = await coreStatus(deps, ledger, 'sess-1');
    assert.ok(out.transcript_path?.startsWith(path.join(dir, 'out')), `转录落讨论 cwd:${out.transcript_path}`);
    assert.ok(fs.existsSync(out.transcript_path));
    // 幂等:第二次调用直接吃记账里的路径,不再 loadSession(可能连 session 都没了)
    const deps2 = makeMockDeps({ session: session(false, 'group_chat_end'), loaded: null });
    const out2 = await coreStatus(deps2.deps, ledger, 'sess-1');
    assert.equal(out2.transcript_path, out.transcript_path);
    assert.equal(deps2.calls.loadSession.length, 0, '转录已导出 → 记账命中路径,零 loadSession');
  }
});

test('coreStatus:记账全 miss(手抄 id)→ load_session 取 project_id → 回查(P1-2 兜底链)', async () => {
  const session = { busy: false, stop_reason: 'max_rounds', id: 'sess-9', project_id: 'proj-9' };
  const loaded = {
    session: { id: 'sess-9', project_id: 'proj-9', current_cwd: os.tmpdir(), model: 'uuid-m3', metadata: '{"participants":[]}', discussion_summary: null, stop_reason: 'max_rounds' },
    messages: [{ seq: 0, role: 'user', text: 't', has_tool_calls: false, has_tool_results: false }],
  };
  const { deps, calls } = makeMockDeps({ session, loaded });
  const { ledger } = tmpLedger();
  const out = await coreStatus(deps, ledger, 'sess-9');
  assert.deepEqual([out.busy, out.stop_reason], [false, 'max_rounds']);
  assert.equal(calls.loadSession.length >= 1, true);
  assert.ok(calls.pollSession.some(([p]) => p === 'proj-9'), '回查走了 load_session 拿到的 project_id');
});

test('coreStatus:导出失败降级不抛错(P2-1)——目录只读时仍返回 busy/stop_reason', async () => {
  const roDir = fs.mkdtempSync(path.join(os.tmpdir(), 'gcmcp-ro-'));
  fs.chmodSync(roDir, 0o500); // r-x:mkdir out/ 会 EACCES(root 下也能拦?linux 非 root 生效)
  try {
    const session = { busy: false, stop_reason: 'error', id: 'sess-1', project_id: 'proj-1' };
    const loaded = {
      session: { id: 'sess-1', project_id: 'proj-1', current_cwd: roDir, model: 'uuid-m3', metadata: '{"participants":[]}', discussion_summary: null, stop_reason: 'error' },
      messages: [],
    };
    const { deps } = makeMockDeps({ session, loaded });
    const { ledger } = tmpLedger();
    ledger.set('sess-1', { request_id: 'r1', project_id: 'proj-1', cwd: roDir, topic: 't', started_at_ms: 1 });
    const out = await coreStatus(deps, ledger, 'sess-1');
    if (process.getuid?.() === 0) {
      assert.equal(out.transcript_warning, undefined, 'root 环境权限拦截不生效,跳过降级断言');
    } else {
      assert.equal(out.transcript_path, null);
      assert.match(out.transcript_warning, /转录导出失败/);
      assert.deepEqual([out.busy, out.stop_reason], [false, 'error'], '轮询契约字段不受导出失败污染');
    }
  } finally {
    fs.chmodSync(roDir, 0o700);
    fs.rmSync(roDir, { recursive: true, force: true });
  }
});

test('coreResult:非终态明确报错;终态返回 summary/roster/stats/转录;summary 缺失警告', async () => {
  const mk = (stop_reason, summary) => ({
    session: { busy: false, stop_reason, id: 'sess-1', project_id: 'proj-1', current_cwd: os.tmpdir(), model: 'uuid-m3', metadata: JSON.stringify({ participants: [{ name: '甲', model: 'uuid-glm53' }] }), discussion_summary: summary },
    messages: [{ seq: 0, role: 'user', text: 't', has_tool_calls: false, has_tool_results: false }],
  });
  {
    const { deps } = makeMockDeps({ session: { busy: true, stop_reason: null }, loaded: mk('group_chat_end') });
    const { ledger } = tmpLedger();
    await assert.rejects(coreResult(deps, ledger, 'sess-1'), (e) => e.isToolError && /still running/.test(e.message));
  }
  {
    const { deps } = makeMockDeps({ session: { busy: false, stop_reason: 'group_chat_end' }, loaded: mk('group_chat_end', '共识:做A') });
    const { ledger } = tmpLedger();
    const out = await coreResult(deps, ledger, 'sess-1');
    assert.equal(out.summary, '共识:做A');
    assert.equal(out.stop_reason, 'group_chat_end');
    assert.deepEqual(out.roster, { moderator: 'MiniMax-M3', participants: ['甲/glm-5.3'] });
    assert.equal(out.stats.messages, 1);
    assert.ok(out.transcript_path);
    assert.equal(out.summary_warning, undefined);
  }
  {
    const { deps } = makeMockDeps({ session: { busy: false, stop_reason: 'group_chat_end' }, loaded: mk('group_chat_end', null) });
    const { ledger } = tmpLedger();
    const out = await coreResult(deps, ledger, 'sess-1');
    assert.equal(out.summary, null);
    assert.match(out.summary_warning, /discussion_summary 缺失/);
  }
});

// ---------------------------------------------------------------------------
// coreCancel
// ---------------------------------------------------------------------------

test('coreCancel:记账命中传 request_id;cancel 撞已终态幂等成功;记账 miss 明确报错', async () => {
  {
    const { deps, calls } = makeMockDeps({ session: { busy: true, stop_reason: null } });
    const { ledger } = tmpLedger();
    ledger.set('sess-1', { request_id: 'r-abc', project_id: 'p' });
    const out = await coreCancel(deps, ledger, 'sess-1');
    assert.equal(out.cancelled, true);
    assert.deepEqual(calls.cancelChat, ['r-abc']);
  }
  {
    const deps = {
      ...makeMockDeps({ session: { busy: false, stop_reason: 'cancelled' } }).deps,
      cancelChat: async () => { throw new Error('no active request'); },
    };
    const { ledger } = tmpLedger();
    ledger.set('sess-1', { request_id: 'r-abc', project_id: 'p' });
    const out = await coreCancel(deps, ledger, 'sess-1');
    assert.equal(out.already_finished, true, '已终态后 cancel 幂等成功而非报错');
  }
  {
    const { deps } = makeMockDeps();
    const { ledger } = tmpLedger();
    await assert.rejects(coreCancel(deps, ledger, 'ghost'), (e) => e.isToolError && /记账/.test(e.message));
  }
});

// ---------------------------------------------------------------------------
// SDK 协议接线(真 SDK + InMemoryTransport,不 spawn 进程)
// ---------------------------------------------------------------------------

test('SDK 接线:InMemoryTransport 全链——tools/list 四工具 + callTool 走 handler', async () => {
  const { McpServer } = await import('@modelcontextprotocol/sdk/server/mcp.js');
  const { Client } = await import('@modelcontextprotocol/sdk/client/index.js');
  const { InMemoryTransport } = await import('@modelcontextprotocol/sdk/inMemory.js');
  const { createServer } = await import('./group-chat-mcp.mjs');

  const { deps } = makeMockDeps({ session: { busy: true, stop_reason: null, id: 'sess-1', project_id: 'proj-1' } });
  const { ledger } = tmpLedger();
  const server = await createServer({ server: new McpServer({ name: 't', version: '0' }), deps, ledger });
  const client = new Client({ name: 't-client', version: '0' });
  const [ct, st] = InMemoryTransport.createLinkedPair();
  await Promise.all([server.connect(ct), client.connect(st)]);

  const listed = await client.listTools();
  assert.equal(listed.tools.length, 4);

  // AC4 预算地面真值:宿主注入 LLM context 的就是这份 wire schema
  const wireChars = JSON.stringify(listed.tools.map(({ name, description, inputSchema }) => ({ name, description, inputSchema }))).length;
  assert.ok(wireChars <= TOOLS_BUDGET_CHARS,
    `wire 预算超支:${wireChars} > ${TOOLS_BUDGET_CHARS}(≈${Math.round(wireChars / 4)} token;改文案或扩预算需过评审)`);
  assert.ok(wireChars >= 1200, `wire 内容异常偏少:${wireChars}(schema 被误删?)`);

  const res = await client.callTool({ name: 'start_discussion', arguments: { topic: '议题', cwd: '/w', preset: 'arch' } });
  assert.equal(res.isError, undefined);
  const payload = JSON.parse(res.content[0].text);
  assert.match(payload.session_id, /sess-1/);
  assert.match(payload.hint, /discussion_status/);

  const err = await client.callTool({ name: 'discussion_result', arguments: { session_id: 'sess-1' } });
  assert.equal(err.isError, true, '非终态走 isError 语义(roadmap:明确报错而非空值)');
  assert.match(JSON.parse(err.content[0].text).error, /still running/);

  await client.close();
  await server.close();
});

test('realDeps 默认不炸(仅构造;BASE 环境变量语义与 M1 单源)', () => {
  const d = realDeps();
  assert.equal(typeof d.pollSession, 'function');
  assert.ok(/^http/.test(d.base));
});
