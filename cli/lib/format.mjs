// lib/format.mjs — 输出格式化(纯函数,node --test 覆盖)。
//
// 契约(design §7.5):stdout 只出数据;json 一律单行(toJsonLine),可 | jq。

/** json 输出唯一入口:单行(无缩进无换行,换行由调用方补)。 */
export function toJsonLine(value) {
  return JSON.stringify(value);
}

/**
 * 简单文本表格。columns: [{header, key?, getValue?}];列宽取表头与各行最大
 * 长度,两空格分隔。空 rows 时只出表头(空列表的 text 形态)。
 */
export function formatTable(rows, columns) {
  const cell = (row, col) =>
    String((col.getValue ? col.getValue(row) : row[col.key]) ?? '');
  const widths = columns.map(
    (col) => Math.max(col.header.length, ...rows.map((row) => cell(row, col).length), 1)
  );
  const line = (cells) => cells.map((s, i) => s.padEnd(widths[i], ' ')).join('  ').trimEnd();
  const lines = [line(columns.map((col) => col.header))];
  for (const row of rows) lines.push(line(columns.map((col) => cell(row, col))));
  return lines.join('\n');
}

/** text 模式尾部换行规整(空串不加)。 */
export function ensureTrailingNewline(s) {
  if (s === '') return '';
  return s.endsWith('\n') ? s : `${s}\n`;
}

/** 定宽截断(超长以 … 收尾);非字符串先 String 化。 */
export function truncate(s, max) {
  const str = String(s ?? '');
  if (str.length <= max) return str;
  return `${str.slice(0, Math.max(0, max - 1))}…`;
}
