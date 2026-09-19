// mcp.test.mjs — MCP JSON-RPC 响应三态解析纯函数单测(AC10;不打真 daemon)。
// fetch 本身不 mock(端到端 live 验);覆盖:正常载荷二次 parse / isError 语义错 /
// -326xx 协议错 / malformed body(content 缺失、text 非 JSON、id 不匹配)。
import test from 'node:test';
import assert from 'node:assert/strict';
import { parseMcpResponse } from './lib/mcp.mjs';
import { EvlError } from './lib/api.mjs';

const okBody = (payload) => ({
  jsonrpc: '2.0',
  id: 1,
  result: { content: [{ type: 'text', text: JSON.stringify(payload, null, 2) }], isError: false },
});

const isEvl = (fn, matchRe) => {
  try {
    fn();
  } catch (e) {
    assert.ok(e instanceof EvlError, `expected EvlError, got ${e?.constructor?.name}`);
    assert.equal(e.exitCode, 1);
    if (matchRe) assert.match(e.message, matchRe);
    return e;
  }
  assert.fail('expected EvlError to be thrown');
};

test('parseMcpResponse: 正常载荷 — content[0].text pretty JSON 二次 parse', () => {
  const payload = { session_id: 's1', request_id: 'r1', hint: 'poll discussion_status' };
  assert.deepEqual(parseMcpResponse(okBody(payload)), payload);
});

test('parseMcpResponse: 正常载荷 — text 非 pretty(单行)也 parse;多 content 取 [0]', () => {
  const body = {
    jsonrpc: '2.0',
    id: 2,
    result: {
      content: [
        { type: 'text', text: '{"busy":true}' },
        { type: 'text', text: '{"ignored":true}' },
      ],
      isError: false,
    },
  };
  assert.deepEqual(parseMcpResponse(body), { busy: true });
});

test('parseMcpResponse: isError 语义错 — text 为 {error[, hint]},两者都进消息', () => {
  const body = {
    jsonrpc: '2.0',
    id: 3,
    result: {
      content: [{ type: 'text', text: JSON.stringify({ error: 'session 不存在:x' }) }],
      isError: true,
    },
  };
  isEvl(() => parseMcpResponse(body), /session 不存在:x/);
});

test('parseMcpResponse: isError infra 错 — hint(daemon 拉起提示)追加进消息', () => {
  const body = {
    jsonrpc: '2.0',
    id: 4,
    result: {
      content: [
        { type: 'text', text: JSON.stringify({ error: 'load_session failed', hint: './scripts/daemon.sh bg' }) },
      ],
      isError: true,
    },
  };
  const e = isEvl(() => parseMcpResponse(body), /load_session failed/);
  assert.match(e.message, /daemon\.sh bg/);
});

test('parseMcpResponse: isError 但 text 非 JSON — 原文兜底进消息(协议面异常不静默)', () => {
  const body = {
    jsonrpc: '2.0',
    id: 5,
    result: { content: [{ type: 'text', text: 'plain failure text' }], isError: true },
  };
  isEvl(() => parseMcpResponse(body), /plain failure text/);
});

test('parseMcpResponse: JSON-RPC 协议错(未知工具 -32602)— code + message 进消息', () => {
  const body = { jsonrpc: '2.0', id: 6, error: { code: -32602, message: 'Unknown tool: nope' } };
  const e = isEvl(() => parseMcpResponse(body), /-32602/);
  assert.match(e.message, /Unknown tool: nope/);
});

test('parseMcpResponse: JSON-RPC 协议错(未知方法 -32601)', () => {
  const body = { jsonrpc: '2.0', id: 7, error: { code: -32601, message: 'Method not found: x' } };
  isEvl(() => parseMcpResponse(body), /-32601/);
});

test('parseMcpResponse: malformed — 非对象/数组/null → 协议错', () => {
  isEvl(() => parseMcpResponse(null), /非 JSON-RPC 对象/);
  isEvl(() => parseMcpResponse([1, 2]), /非 JSON-RPC 对象/);
  isEvl(() => parseMcpResponse('oops'), /非 JSON-RPC 对象/);
});

test('parseMcpResponse: malformed — 缺 result / 缺 content / text 非字符串 → 协议错', () => {
  isEvl(() => parseMcpResponse({ jsonrpc: '2.0', id: 1 }), /缺 result/);
  isEvl(
    () => parseMcpResponse({ jsonrpc: '2.0', id: 1, result: { isError: false } }),
    /缺 content\[0\]\.text/
  );
  isEvl(
    () => parseMcpResponse({ jsonrpc: '2.0', id: 1, result: { content: [{ type: 'text' }], isError: false } }),
    /缺 content\[0\]\.text/
  );
});

test('parseMcpResponse: malformed — 正常态 text 非 JSON → 协议错(不静默吞)', () => {
  const body = {
    jsonrpc: '2.0',
    id: 1,
    result: { content: [{ type: 'text', text: 'not json {' }], isError: false },
  };
  isEvl(() => parseMcpResponse(body), /工具载荷非 JSON/);
});

test('parseMcpResponse: id 不匹配(expectId 传入时)→ 协议错;匹配则通过', () => {
  const body = okBody({ a: 1 });
  body.id = 6;
  isEvl(() => parseMcpResponse(body, { expectId: 7 }), /id 不匹配/);
  assert.deepEqual(parseMcpResponse(body, { expectId: 6 }), { a: 1 });
});
