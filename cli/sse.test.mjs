// sse.test.mjs — SSE 帧切分纯函数(parseSseChunk):
// 整帧 / 跨 chunk 拆两半 / 多行 data / CRLF 容忍 / 注释行忽略 / JSON 解析。
import test from 'node:test';
import assert from 'node:assert/strict';
import { parseSseChunk } from './lib/sse.mjs';

test('整帧:event + data(JSON),rest 为空', () => {
  const { frames, rest } = parseSseChunk('event: chat-event\ndata: {"kind":"delta","text":"hi"}\n\n');
  assert.deepEqual(frames, [{ event: 'chat-event', data: { kind: 'delta', text: 'hi' } }]);
  assert.equal(rest, '');
});

test('一个 chunk 多帧', () => {
  const { frames } = parseSseChunk(
    'event: a\ndata: {"n":1}\n\n event:ignore-leading-space\ndata: {"n":2}\n\n'.replace(
      ' event:ignore-leading-space',
      'event: b'
    )
  );
  assert.equal(frames.length, 2);
  assert.equal(frames[1].event, 'b');
});

test('跨 chunk 拆两半:第一半无完整帧,尾部进 rest;第二半补全', () => {
  const whole = 'event: chat-event\ndata: {"kind":"done"}\n\n';
  const cut = whole.indexOf('done');
  const first = parseSseChunk(whole.slice(0, cut));
  assert.deepEqual(first.frames, []);
  const second = parseSseChunk(first.rest + whole.slice(cut));
  assert.deepEqual(second.frames, [{ event: 'chat-event', data: { kind: 'done' } }]);
  assert.equal(second.rest, '');
});

test('多行 data 按 \n 拼接;非 JSON 原样字符串', () => {
  const { frames } = parseSseChunk('data: line1\ndata: line2\n\n');
  assert.deepEqual(frames, [{ event: 'message', data: 'line1\nline2' }]);
});

test('CRLF 容忍(\\r\\n\\r\\n 切帧)', () => {
  const { frames, rest } = parseSseChunk('event: x\r\ndata: {"a":1}\r\n\r\n');
  assert.deepEqual(frames, [{ event: 'x', data: { a: 1 } }]);
  assert.equal(rest, '');
});

test('注释行(keepalive)与 id/retry 行忽略', () => {
  const { frames } = parseSseChunk(': keep-alive\nid: 42\nretry: 1000\nevent: e\ndata: 1\n\n');
  assert.deepEqual(frames, [{ event: 'e', data: 1 }]);
});

test('无 data 的帧丢弃(纯 event 行不产帧)', () => {
  const { frames } = parseSseChunk('event: ping\n\n');
  assert.deepEqual(frames, []);
});

test('data 与 event 顺序无关', () => {
  const { frames } = parseSseChunk('data: {"k":1}\nevent: later-name\n\n');
  assert.deepEqual(frames, [{ event: 'later-name', data: { k: 1 } }]);
});

test('rest 保留不完整尾部(帧内只有 event 行,未到 \\n\\n)', () => {
  const { frames, rest } = parseSseChunk('event: chat-event\ndata: {"kind":"del');
  assert.deepEqual(frames, []);
  assert.equal(rest, 'event: chat-event\ndata: {"kind":"del');
});
