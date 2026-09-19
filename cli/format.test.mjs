// format.test.mjs — 表格列选取 / JSON 单行 / 空列表 / 截断。
import test from 'node:test';
import assert from 'node:assert/strict';
import { toJsonLine, formatTable, ensureTrailingNewline, truncate } from './lib/format.mjs';

test('toJsonLine: 单行(无换行无缩进),可 JSON.parse', () => {
  const s = toJsonLine({ a: 1, b: [2, 3], c: 'x' });
  assert.equal(s, '{"a":1,"b":[2,3],"c":"x"}');
  assert.ok(!s.includes('\n'));
  assert.deepEqual(JSON.parse(s), { a: 1, b: [2, 3], c: 'x' });
});

test('formatTable: 列宽取表头与内容最大值,两空格分隔', () => {
  const rows = [
    { id: 'a', busy: false },
    { id: 'bbbb', busy: true },
  ];
  const out = formatTable(rows, [
    { header: 'id', key: 'id' },
    { header: 'busy', getValue: (r) => (r.busy ? 'busy' : '') },
  ]);
  const lines = out.split('\n');
  assert.equal(lines.length, 3);
  assert.equal(lines[0], 'id    busy');
  assert.equal(lines[1], 'a');
  assert.equal(lines[2], 'bbbb  busy');
});

test('formatTable: 空 rows → 只出表头(空列表 text 形态)', () => {
  const out = formatTable([], [{ header: 'id', key: 'id' }]);
  assert.equal(out, 'id');
});

test('formatTable: null/undefined 单元格以空串呈现', () => {
  const out = formatTable([{ id: null, x: undefined }], [
    { header: 'id', key: 'id' },
    { header: 'x', key: 'x' },
  ]);
  // 行体全空白 → trimEnd 后为空行
  assert.equal(out, 'id  x\n');
});

test('ensureTrailingNewline: 空串不加,已有不重复', () => {
  assert.equal(ensureTrailingNewline(''), '');
  assert.equal(ensureTrailingNewline('a'), 'a\n');
  assert.equal(ensureTrailingNewline('a\n'), 'a\n');
});

test('truncate: 未超长原样;超长以 … 收尾且总长 = max', () => {
  assert.equal(truncate('abc', 5), 'abc');
  const t = truncate('abcdef', 4);
  assert.equal(t, 'abc…');
  assert.equal(t.length, 4);
  assert.equal(truncate(undefined, 3), '');
});
