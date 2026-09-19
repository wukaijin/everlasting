// api.test.mjs — 错误翻译纯函数(errno 映射表)与退出码表。
// fetch 本身不 mock(端到端 live 验);只测 fetchFailDetail 与 EXIT。
import test from 'node:test';
import assert from 'node:assert/strict';
import { fetchFailDetail, EXIT, EvlError } from './lib/api.mjs';

test('fetchFailDetail: EPERM → "Operation not permitted (EPERM)" 字面串(沙箱分类器按它触发)', () => {
  const e = new Error('fetch failed', { cause: Object.assign(new Error('Permission denied'), { code: 'EPERM' }) });
  const detail = fetchFailDetail(e);
  assert.ok(detail.includes('Operation not permitted (EPERM)'), detail);
});

test('fetchFailDetail: EACCES → "Permission denied (EACCES)"', () => {
  const e = new Error('fetch failed', { cause: Object.assign(new Error('nope'), { code: 'EACCES' }) });
  assert.ok(fetchFailDetail(e).includes('Permission denied (EACCES)'));
});

test('fetchFailDetail: 未知 code 原样透传', () => {
  const e = new Error('fetch failed', { cause: Object.assign(new Error('getaddrinfo fail'), { code: 'ENOTFOUND' }) });
  assert.ok(fetchFailDetail(e).includes('getaddrinfo fail'));
  assert.ok(fetchFailDetail(e).includes('[ENOTFOUND]'));
});

test('fetchFailDetail: 无 cause → message 透传', () => {
  assert.equal(fetchFailDetail(new Error('boom')), 'boom');
  assert.equal(fetchFailDetail(undefined), 'undefined');
  assert.equal(fetchFailDetail('str'), 'str');
});

test('EXIT: 退出码契约(design §6)', () => {
  assert.deepEqual(EXIT, {
    ok: 0,
    scriptError: 1,
    chatError: 2,
    cancelled: 3,
    timeout: 7,
    usage: 64,
  });
});

test('EvlError: 默认 exitCode=1,可覆盖', () => {
  assert.equal(new EvlError('x').exitCode, 1);
  assert.equal(new EvlError('x', 7).exitCode, 7);
});
