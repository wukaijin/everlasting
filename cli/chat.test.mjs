// chat.test.mjs — chat 纯函数:project 匹配 / usage 归一 / json 终态形状 /
// 终态分类(退出码映射)/ 应答键位。
import test from 'node:test';
import assert from 'node:assert/strict';
import {
  pickProjectByPath,
  normalizeUsage,
  makeChatResult,
  terminalExitCode,
  mapDecisionKey,
  summarizeToolInput,
} from './lib/chat.mjs';

const PROJECTS = [
  { id: 'p-home', path: '/home/carlos' },
  { id: 'p-repo', path: '/usr/local/code/github/everlasting' },
  { id: 'p-hidden', path: '/tmp/hidden-proj/' },
];

test('pickProjectByPath: 精确命中', () => {
  assert.equal(pickProjectByPath(PROJECTS, '/home/carlos')?.id, 'p-home');
});

test('pickProjectByPath: 尾斜杠 / 相对段词法规整后命中(path.resolve 语义,不解析符号链接)', () => {
  assert.equal(pickProjectByPath(PROJECTS, '/home/carlos/')?.id, 'p-home');
  assert.equal(pickProjectByPath(PROJECTS, '/usr/local/code/github/./everlasting/../everlasting')?.id, 'p-repo');
});

test('pickProjectByPath: miss → undefined(调用方 create_project)', () => {
  assert.equal(pickProjectByPath(PROJECTS, '/no/such/dir'), undefined);
});

test('pickProjectByPath: 空入参防御', () => {
  assert.equal(pickProjectByPath(undefined, '/x'), undefined);
  assert.equal(pickProjectByPath(PROJECTS, undefined), undefined);
});

test('normalizeUsage: turn_usage 形状(五字段 + context_window 顶层)', () => {
  const u = normalizeUsage(
    {
      input_tokens: 100,
      output_tokens: 5,
      cache_creation_input_tokens: 0,
      cache_read_input_tokens: 90,
      context_input_tokens: 1234,
    },
    1000000
  );
  assert.deepEqual(u, {
    input_tokens: 100,
    output_tokens: 5,
    cache_creation_input_tokens: 0,
    cache_read_input_tokens: 90,
    context_input_tokens: 1234,
    context_window: 1000000,
  });
});

test('normalizeUsage: 缺字段补 0;无 usage → null', () => {
  assert.deepEqual(normalizeUsage({ input_tokens: 3 }, null).output_tokens, 0);
  assert.equal(normalizeUsage(null), null);
  assert.equal(normalizeUsage(undefined, 5), null);
});

const BASE_FIELDS = {
  sessionId: 'sid-1',
  requestId: 'rid-1',
};

test('makeChatResult: done 分支 — 恒定键集,无 error 键,text_chars = text 长度', () => {
  const r = makeChatResult({
    kind: 'done',
    text: '你好',
    usage: normalizeUsage({ input_tokens: 1 }),
    ...BASE_FIELDS,
    stopReason: 'end_turn',
    denials: 0,
  });
  assert.deepEqual(Object.keys(r).sort(), [
    'permission_denials',
    'request_id',
    'session_id',
    'stop_reason',
    'text',
    'text_chars',
    'usage',
  ]);
  assert.equal(r.text, '你好');
  assert.equal(r.text_chars, 2);
  assert.equal(r.stop_reason, 'end_turn');
  assert.equal(r.permission_denials, 0);
  assert.ok('usage' in r);
});

test('makeChatResult: error 分支 — 同一对象形状 + error 键,text="" / text_chars=0', () => {
  const r = makeChatResult({
    kind: 'error',
    text: '半截输出',
    ...BASE_FIELDS,
    errorKind: 'rate_limited',
    errorMessage: '429',
  });
  assert.deepEqual(Object.keys(r).sort(), [
    'error',
    'permission_denials',
    'request_id',
    'session_id',
    'stop_reason',
    'text',
    'text_chars',
    'usage',
  ]);
  assert.equal(r.text, '');
  assert.equal(r.text_chars, 0);
  assert.deepEqual(r.error, { kind: 'rate_limited', message: '429' });
});

test('makeChatResult: cancelled/timeout 同走 error 形状(design §7.5 一条分支)', () => {
  for (const kind of ['cancelled', 'timeout']) {
    const r = makeChatResult({ kind, ...BASE_FIELDS, errorMessage: 'x' });
    assert.equal(r.text, '');
    assert.equal(r.error.kind, kind);
  }
});

test('terminalExitCode: 0/2/3/7 映射;stream_lost 与未知 → 1', () => {
  assert.equal(terminalExitCode('done'), 0);
  assert.equal(terminalExitCode('error'), 2);
  assert.equal(terminalExitCode('cancelled'), 3);
  assert.equal(terminalExitCode('timeout'), 7);
  assert.equal(terminalExitCode('stream_lost'), 1);
  assert.equal(terminalExitCode('whatever'), 1);
});

test('mapDecisionKey: y/a/n(含 EOF null)= allow_once/allow_always/deny', () => {
  assert.equal(mapDecisionKey('y'), 'allow_once');
  assert.equal(mapDecisionKey('a'), 'allow_always');
  assert.equal(mapDecisionKey('n'), 'deny');
  assert.equal(mapDecisionKey(null), 'deny');
  assert.equal(mapDecisionKey('x'), 'deny');
});

test('summarizeToolInput: object → JSON 串;超长截断', () => {
  assert.equal(summarizeToolInput({ file_path: '/a' }), '{"file_path":"/a"}');
  assert.ok(summarizeToolInput({ command: 'x'.repeat(500) }).length <= 160);
  assert.equal(summarizeToolInput(undefined), '');
});
