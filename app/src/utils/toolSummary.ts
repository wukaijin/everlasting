// read 族工具(headline 紧凑卡)的 chip / meta 数据源。
//
// 09-19-tool-card-compact-read:`glob` / `list_dir` / `read_file` 三张卡从
// 通用 ToolCallCard 的三行形态压成 1 行(见
// `.trellis/tasks/09-19-tool-card-compact-read/design.md` §1)。压缩掉的是
// `▸ input` / `▸ output · N chars` 两行 summary,信息不能跟着丢 —— 这一层
// 把原先藏在展开区里的「搜了什么 / 命中多少 / 读了哪段」提到 headline 上:
//
//   [icon] NAME · chip(=readToolChip)      meta(=readToolMeta)   ✓ done 0.3s
//
// 与本文件同族的还有 `messageFormat.ts` 的 `toolHeaderChip` / `toolAccentVar`
// / `toolIcon`(既有 header 槽位与图标映射)。这里单独成文件是因为它多一层
// 「解析工具输出文本」的逻辑(计数 / 行号范围 / 截断提示),比 messageFormat
// 里那些纯字段映射重一档。
//
// 全部是纯函数、零 store、零 DOM:`input` 来自 LLM、`content` 来自后端,
// 两者都可能畸形,函数一律安全返回 `null` 而不是抛错。

import { extractToolResultDisplay } from "./messageFormat";

/** read 族工具名(封闭名单,switch 写法对齐 messageFormat 的家族判定)。
 *  只列本次紧凑化的三个只读检视工具:`grep` / `web_search` 仍走通用卡
 *  (PRD Non-Goals)。 */
export function isReadFamilyTool(name: string): boolean {
  switch (name) {
    case "read_file":
    case "glob":
    case "list_dir":
      return true;
    default:
      return false;
  }
}

/** 工具结果的渲染面(与 `ToolResultInfo` 的字段子集同形,便于单测直接喂)。 */
export interface ReadToolResultLike {
  content: string;
  isError: boolean;
}

/** headline 的 chip 槽:这次调用**读的是哪儿**。
 *
 *  优先级:
 *    - `glob`:`input.pattern`(匹配式;这是 glob 唯一的检索语义来源,
 *      通用卡的 `toolHeaderChip` 只认 `input.path`,所以 glob 此前在卡上
 *      完全看不出搜了什么)。`input.path` 非空且不是 `.` 时补
 *      `pattern in path`(搜索根非 cwd 时是重要差异)。
 *    - `list_dir`:`input.path`;缺省时字面量 `cwd`(工具语义:省略即列
 *      当前工作目录 —— 显示占位比留空诚实)。
 *    - `read_file`:`input.path`。
 *
 *  非 string / 空串一律按缺失处理(防御 LLM 畸形 input);全缺失返回
 *  `null`,调用方不渲染 chip。 */
export function readToolChip(
  name: string,
  input?: Record<string, unknown>,
): string | null {
  const path = strInput(input, "path");
  if (name === "glob") {
    const pattern = strInput(input, "pattern");
    if (!pattern) return null;
    if (path && path !== ".") return `${pattern} in ${path}`;
    return pattern;
  }
  if (name === "list_dir") return path ?? "cwd";
  if (name === "read_file") return path;
  return null;
}

/** headline 的 meta 槽:这次调用**拿了多少**(计数 / 行范围)。
 *
 *  按工具解析各自的输出文本形态(见各分支注释)。共同约定:
 *    - 先经 `extractToolResultDisplay` 剥掉 `{result, cwd}` 信封;
 *    - `isError` → `null`(失败没有「规模」可言;错误原文由 ✗ + 展开区承载);
 *    - 结果缺失(流式中)→ `null`。 */
export function readToolMeta(
  name: string,
  result?: ReadToolResultLike | null,
): string | null {
  if (!result || result.isError) return null;
  const text = trimOuterWhitespace(extractToolResultDisplay(result.content ?? ""));
  if (text.length === 0) return null;
  switch (name) {
    case "glob":
      return globMeta(text);
    case "list_dir":
      return listDirMeta(text);
    case "read_file":
      return readFileMeta(text);
    default:
      return null;
  }
}

// ---------------------------------------------------------------------------
// per-tool 解析
// ---------------------------------------------------------------------------

/** glob 结果:一行一个路径,尾部可能跟一条截断提示(两种措辞,
 *  见 `tools/glob.rs`):
 *    `(...and N more matches; narrow your pattern to see them)`
 *    `(showing the 100 most recent matches; narrow your pattern for the rest)`
 *  0 命中是单句 `No files matched pattern 'X' in Y.`(非错误)。
 *
 *  提示行统一以 `(` 开头 → 计数时剥除,存在即在计数后加 `+`。`+` 只声称
 *  「还有更多」,不把被截掉的数量当精确值报出去(总数是 `shown + N`,而 N
 *  在被截断时未必等于真实剩余量)。 */
function globMeta(text: string): string | null {
  if (/^No files matched pattern/.test(text)) return "no matches";
  const { items, capped } = splitItems(text);
  if (items.length === 0) return null;
  return `${items.length}${capped ? "+" : ""} ${plural(items.length, "match", "matches")}`;
}

/** list_dir 结果:一行一个条目(目录带尾 `/`),空目录是单句
 *  `(empty directory: <path>)`,超限时尾部跟
 *  `(...N more entries hidden by limit; …)`。 */
function listDirMeta(text: string): string | null {
  if (/^\(empty directory:/.test(text)) return "empty";
  const { items, capped } = splitItems(text);
  if (items.length === 0) return null;
  return `${items.length}${capped ? "+" : ""} ${plural(items.length, "entry", "entries")}`;
}

/** read_file 结果:成功时是 `\t<line>\t<text>` 的 cat -n 形态(行号是文件
 *  真实行号,带 offset 时从 offset 起);读图走 image block,文本结果退化成
 *  一行 `[image: <path> (W×H) — 已作为图片块发送]` 摘要。
 *
 *  headline 只报行范围 —— 它同时表达了「读了哪一段」和「文件多大」,比
 *  字符数更有信息量。大文件截断时输出是 head + 标记 + tail
 *  (`tool_output::truncation_marker` 的 `<truncated: omitted N of M bytes …>`),
 *  head/tail 两段的行号都取,范围即文件真实跨度,再追加 `· truncated`
 *  说明中间少了内容。
 *
 *  非行号输出(读图 / 工具集扩展前的历史行)→ 兜底字符数,词汇与
 *  `ToolOutputBody` 的 sizeLabel 对齐。 */
function readFileMeta(text: string): string | null {
  if (text.startsWith("[image:")) return imageMeta(text);
  const numbers = lineNumbers(text);
  if (numbers.length === 0) return `${text.length} chars`;
  const first = numbers[0]!;
  const last = numbers[numbers.length - 1]!;
  const range = first === last ? `L${first}` : `L${first}–${last}`;
  return text.includes("<truncated: omitted") ? `${range} · truncated` : range;
}

/** 读图结果的 headline:`image`(带尺寸时 `image 1234×567`)。尺寸段来自
 *  read_file.rs 的 `imagesize::blob_size` 摘要,解析不出就只说 image。 */
function imageMeta(text: string): string {
  const dims = /^\[image:.*?\((\d+)×(\d+)\)/.exec(text);
  return dims ? `image ${dims[1]}×${dims[2]}` : "image";
}

// ---------------------------------------------------------------------------
// 共享小工具
// ---------------------------------------------------------------------------

/** 去掉首部空行与尾部空白,**保留首行行首的制表符**。刻意不用
 *  `String.trim()`:read_file 的第一行是 `\t1\t<正文>`,trim 会啃掉那个
 *  前导制表符,行号正则随即失配 —— 范围会从第二行报起(L2–N 的实证
 *  就是这里踩出来的)。 */
function trimOuterWhitespace(s: string): string {
  return s.replace(/^(?:[ \t]*\n)+/, "").replace(/\s+$/, "");
}

/** `input` 里的字符串字段(非 string / 空串 → null;防御 LLM 畸形 input)。 */
function strInput(
  input: Record<string, unknown> | undefined,
  key: string,
): string | null {
  const v = input?.[key];
  return typeof v === "string" && v.length > 0 ? v : null;
}

/** 拆分「条目行」与「尾部提示行」:提示行前缀 `(`,被截断时返回 `capped`。
 *  空行(glob / list_dir 在提示前压了一个空行)不计。 */
function splitItems(text: string): { items: string[]; capped: boolean } {
  const items: string[] = [];
  let capped = false;
  for (const line of text.split("\n")) {
    const t = line.trim();
    if (t.length === 0) continue;
    if (t.startsWith("(")) {
      capped = true;
      continue;
    }
    items.push(t);
  }
  return { items, capped };
}

/** cat -n 输出里的行号序列(`\t<num>\t` 前缀)。逐行匹配、不中断扫描:
 *  截断态是 head + 标记 + tail,取 head 首行与 tail 末行才是文件真实跨度。
 *  误匹配不可能 —— cat -n 给每行都加前缀,正文里形如 `\t123\t` 的内容会
 *  落在该行前缀之后(`\t<真实行号>\t\t123\t…`),正则只吃前缀。 */
function lineNumbers(text: string): number[] {
  const out: number[] = [];
  for (const line of text.split("\n")) {
    const m = /^\t(\d+)\t/.exec(line);
    if (m) out.push(Number(m[1]));
  }
  return out;
}

function plural(n: number, one: string, many: string): string {
  return n === 1 ? one : many;
}
