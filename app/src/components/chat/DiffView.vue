<script setup lang="ts">
// DiffView — render a per-file unified diff. The backend's
// `diff_worktree` IPC returns a `FileDiff[]`; this component takes
// that list and renders it as a vertical list of file cards, each
// with a header (path + status + +/- counts) and a collapsible
// body showing the unified diff text. New files / deletions are
// shown open by default; modifications are shown collapsed since
// they're usually large.
//
// jsdiff's `parsePatch` is used to convert the backend's unified
// diff text into structured `Hunk` data we can render with
// per-line `+/-/space` coloring. Falls back to a plain
// `<pre>`-rendered diff for files where the parser bails (rare —
// only happens for malformed patch text).

import { computed, onMounted, onUnmounted, ref } from "vue";
import { parsePatch } from "diff";
import Icon from "../Icon.vue";
import { pairRunSegments, type WordSeg } from "../../utils/intraLineDiff";

export interface FileDiff {
    path: string;
    status: string;
    added: number;
    removed: number;
    diff_text: string;
}

const props = withDefaults(
    defineProps<{
        files: FileDiff[];
        /** inline 窄容器(ToolCallCard ~250px)显式关双栏 + 藏工具行
         *  (design §3.4a:视口级降级保护不到窄容器);缺省 true 行为
         *  不变。 */
        allowSplit?: boolean;
    }>(),
    { allowSplit: true },
);

interface HunkLine {
    /** `+` for added, `-` for removed, ` ` for context, `@` for hunk header. */
    kind: "add" | "del" | "ctx" | "hunk" | "noeol";
    text: string;
    oldLine: number | null;
    newLine: number | null;
    /** 行内 word-diff 片段(09-26-diffview-enhance)。null = 整行染色
     *  (ctx / 未配对纯增删 / 配对失败的原子 null 退化);仅 add/del 且
     *  配对成功非 null。渲染时 segments 的 text 拼接 == line.text。 */
    segments: WordSeg[] | null;
}

interface ParsedFile {
    file: FileDiff;
    hunks: HunkLine[][];
    /** True when the parser returned something we trust to render. */
    parsed: boolean;
}

const COLLAPSED_STATUSES = new Set(["modified"]);

// Run 数预算帽(view 级保险丝,design §2 性能护栏补充):单个文件的
// 配对 run 对数超过此值后,余下 run 一律跳过行内计算(整行染色)。
// 防病态大 diff 逐 run 全算卡死渲染;数值 ≈200 即评审 OQ2 采纳值,
// design §2 承诺的落定位置就是这条常量注释。
const MAX_PAIR_RUNS = 200;

/**
 * 行内配对 pass(09-26-diffview-enhance,design §3.1):对每个 hunk 扫
 * 「极大连续 del-run 紧跟极大连续 add-run」并调 pairRunSegments 回填两
 * 侧行 segments。纯删除 run 后无 add-run / 纯新增 run 前无 del-run 不配
 * 对(保持整行染色);ctx / hunk / noeol 行不参与;超预算帽的 run 跳过。
 * 行文本已在 hunk 行构建点做过 CRLF 归一(见下),直接进 util。
 */
function applyIntraLinePairs(hunks: HunkLine[][]): void {
    let runBudget = MAX_PAIR_RUNS;
    for (const lines of hunks) {
        let i = 0;
        while (i < lines.length) {
            if (lines[i].kind !== "del") {
                i += 1;
                continue;
            }
            const delStart = i;
            while (i < lines.length && lines[i].kind === "del") i += 1;
            const delEnd = i;
            const addStart = i;
            while (i < lines.length && lines[i].kind === "add") i += 1;
            if (i === addStart) continue; // 纯 del-run:无配对,整行染色
            const addEnd = i;
            if (runBudget <= 0) continue; // 预算帽:跳过(整行染色)
            runBudget -= 1;
            const paired = pairRunSegments(
                lines.slice(delStart, delEnd).map((l) => l.text),
                lines.slice(addStart, addEnd).map((l) => l.text),
            );
            if (!paired) continue; // 原子 null:整 run 退化整行染色
            for (let j = delStart; j < delEnd; j += 1) {
                lines[j].segments = paired.del[j - delStart];
            }
            for (let j = addStart; j < addEnd; j += 1) {
                lines[j].segments = paired.add[j - addStart];
            }
        }
    }
}

const parsedFiles = computed<ParsedFile[]>(() => {
    return props.files.map((f) => {
        const out: ParsedFile = { file: f, hunks: [], parsed: false };
        if (!f.diff_text) {
            return out;
        }
        try {
            const patches = parsePatch(f.diff_text);
            const patch = patches[0];
            if (!patch) {
                return out;
            }
            // Flatten every hunk's lines into a 2-D structure
            // (array of hunks, each hunk is array of lines). One
            // hunk per file is the common case.
            out.hunks = patch.hunks.map((hunk) => {
                const lines: HunkLine[] = [];
                lines.push({
                    kind: "hunk",
                    text: `@@ -${hunk.oldStart},${hunk.oldLines} +${hunk.newStart},${hunk.newLines} @@`,
                    oldLine: null,
                    newLine: null,
                    segments: null,
                });
                let oldLine = hunk.oldStart;
                let newLine = hunk.newStart;
                for (const line of hunk.lines) {
                    const prefix = line[0];
                    // CRLF 归一(消费点,design §3.1 结论 6):CRLF diff 的
                    // `\r` 挂在行内容尾部,在此剥成 LF 口径——行内配对与
                    // 渲染共用这份文本,保证「segments 拼接 == 行文本」。
                    // raw fallback 分支不经过这里,逐字节不动。
                    const text = line.slice(1).replace(/\r$/, "");
                    if (prefix === "+") {
                        lines.push({
                            kind: "add",
                            text,
                            oldLine: null,
                            newLine: newLine,
                            segments: null,
                        });
                        newLine += 1;
                    } else if (prefix === "-") {
                        lines.push({
                            kind: "del",
                            text,
                            oldLine: oldLine,
                            newLine: null,
                            segments: null,
                        });
                        oldLine += 1;
                    } else if (prefix === " ") {
                        lines.push({
                            kind: "ctx",
                            text,
                            oldLine: oldLine,
                            newLine: newLine,
                            segments: null,
                        });
                        oldLine += 1;
                        newLine += 1;
                    } else if (prefix === "\\") {
                        // "\ No newline at end of file" — render as
                        // a small italic note, no line number.
                        lines.push({
                            kind: "noeol",
                            text: text,
                            oldLine: null,
                            newLine: null,
                            segments: null,
                        });
                    }
                }
                return lines;
            });
            // Only mark parsed when we actually have renderable hunks.
            // parsePatch can return patches with zero hunks for inputs
            // that look like +/- fragments but lack `---`/`+++`
            // headers — without this guard we'd set parsed=true and
            // render an empty body (DiffPrimitive's raw fallback would
            // be bypassed). See DiffPrimitive "allHunksEmpty" branch.
            out.parsed = out.hunks.length > 0;
            if (out.parsed) {
                applyIntraLinePairs(out.hunks);
            }
        } catch (e) {
            // parsePatch throws on truly malformed input. We
            // treat this as a render-with-raw-text fallback and
            // log so we notice if the backend ever produces bad
            // patches.
            console.warn("DiffView: parsePatch failed", e);
        }
        return out;
    });
});

function statusLabel(status: string): string {
    switch (status) {
        case "added":
            return "added";
        case "deleted":
            return "deleted";
        case "modified":
            return "modified";
        case "renamed":
            return "renamed";
        default:
            return status;
    }
}

// --------------------------------------------------------------------
// side-by-side(split)行模型(09-26-diffview-enhance,design §3.2)
// --------------------------------------------------------------------

/** split 单元格:一侧的内容(或 null = 空占位格,保持网格对齐)。 */
interface SplitCell {
    lineNo: number | null;
    text: string;
    segments: WordSeg[] | null;
}

/** split 行:pair = 并排行(ctx 是两侧同文的对齐行);hunk / noeol =
 *  通栏行(文本放左格,right 恒 null,渲染跨全宽)。tintLeft/tintRight
 *  = 该侧铺 run tint(内容格与空占位格同染,占位格是行对齐线索)。 */
interface SplitRow {
    left: SplitCell | null;
    right: SplitCell | null;
    kind: "pair" | "ctx" | "hunk" | "noeol";
    tintLeft: boolean;
    tintRight: boolean;
}

function splitCellOf(line: HunkLine, side: "old" | "new"): SplitCell {
    return {
        lineNo: side === "old" ? line.oldLine : line.newLine,
        text: line.text,
        segments: line.segments,
    };
}

/** 从同一份 flat HunkLine[] 派生 split 行(与 unified 共用一次 parse):
 *  ctx 两侧同行号对齐;del-run + add-run zip 逐行并排、短侧补 null 占位
 *  (del3/add2 形态天然表达);纯 del/add 单侧、对侧占位;hunk 头/noeol
 *  通栏。 */
function splitRows(lines: HunkLine[]): SplitRow[] {
    const rows: SplitRow[] = [];
    let i = 0;
    while (i < lines.length) {
        const line = lines[i]!;
        if (line.kind === "hunk" || line.kind === "noeol") {
            rows.push({
                kind: line.kind,
                left: { lineNo: null, text: line.text, segments: null },
                right: null,
                tintLeft: false,
                tintRight: false,
            });
            i += 1;
            continue;
        }
        if (line.kind === "ctx") {
            rows.push({
                kind: "ctx",
                left: splitCellOf(line, "old"),
                right: splitCellOf(line, "new"),
                tintLeft: false,
                tintRight: false,
            });
            i += 1;
            continue;
        }
        const dels: HunkLine[] = [];
        while (i < lines.length && lines[i]!.kind === "del") {
            dels.push(lines[i]!);
            i += 1;
        }
        const adds: HunkLine[] = [];
        while (i < lines.length && lines[i]!.kind === "add") {
            adds.push(lines[i]!);
            i += 1;
        }
        const rowCount = Math.max(dels.length, adds.length);
        for (let j = 0; j < rowCount; j += 1) {
            rows.push({
                kind: "pair",
                left: dels[j] ? splitCellOf(dels[j]!, "old") : null,
                right: adds[j] ? splitCellOf(adds[j]!, "new") : null,
                tintLeft: dels.length > 0,
                tintRight: adds.length > 0,
            });
        }
    }
    return rows;
}

// --------------------------------------------------------------------
// 档位切换 + 持久化 + 窄屏降级(design §3.4 / §3.4a)
// --------------------------------------------------------------------

/** localStorage key(结论 9:前缀 `everlasting:`,useTheme/config 先例)。 */
const MODE_KEY = "everlasting:diffview.mode";

function readStoredMode(): "unified" | "split" {
    try {
        const raw = window.localStorage.getItem(MODE_KEY);
        return raw === "split" ? "split" : "unified";
    } catch {
        // localStorage 不可用(私隐模式等)→ 默认 unified。
        return "unified";
    }
}

function writeStoredMode(mode: "unified" | "split"): void {
    try {
        window.localStorage.setItem(MODE_KEY, mode);
    } catch {
        // 写失败静默:内存值仍正确(config.ts writeLastActive 同款)。
    }
}

/** 用户档位。三纪律(结论 9):仅点击写;读路径永不写回;各实例挂载时
 *  读一次 —— 同屏多实例不联动是最终裁定,后续仅自身点击可变。 */
const userMode = ref<"unified" | "split">(readStoredMode());

function setMode(mode: "unified" | "split") {
    userMode.value = mode;
    writeStoredMode(mode);
}

/** 窄屏(<768px 单断点)选树:matchMedia + change 监听(responsive spec
 *  备案例外「仅用于选树,不用于显隐」;工具行本身的显隐走 CSS)。
 *  ModeSelect.vue 的 jsdom 守卫先例:无 matchMedia 的环境跳过,恒 false
 *  (桌面语义),真窄屏语义交 Playwright setViewportSize。 */
const isNarrow = ref(false);
let narrowMq: MediaQueryList | null = null;
function onNarrowChange(e: MediaQueryListEvent): void {
    isNarrow.value = e.matches;
}
onMounted(() => {
    if (typeof window.matchMedia !== "function") return;
    narrowMq = window.matchMedia("(max-width: 767px)");
    isNarrow.value = narrowMq.matches;
    narrowMq.addEventListener("change", onNarrowChange);
});
onUnmounted(() => {
    narrowMq?.removeEventListener("change", onNarrowChange);
    narrowMq = null;
});

/** 实际渲染档位:split 需同时满足 用户选了 split + allowSplit + 非窄屏。 */
const effectiveMode = computed<"unified" | "split">(() =>
    userMode.value === "split" && props.allowSplit && !isNarrow.value
        ? "split"
        : "unified",
);

/** 有任一 parsed 文件才给工具行(raw-only 不渲染,缺席条件①)。 */
const hasParsedFile = computed(() => parsedFiles.value.some((pf) => pf.parsed));

/** 工具行渲染条件:有文件 + 有 parsed + allowSplit(缺席条件③;窄屏
 *  缺席条件②走 CSS 藏整行,见样式区 mobile-hide-toolbar)。 */
const showToolbar = computed(
    () => props.files.length > 0 && hasParsedFile.value && props.allowSplit,
);

/** Whether a file should be initially open. Added/deleted are
 *  small-ish and high-signal; modifications are usually noisy. */
function defaultOpen(status: string): boolean {
    return !COLLAPSED_STATUSES.has(status);
}

const collapsedMap = ref<Record<string, boolean>>({});

function isCollapsed(filePath: string, status: string): boolean {
    if (filePath in collapsedMap.value) {
        return collapsedMap.value[filePath];
    }
    return !defaultOpen(status);
}

function toggleCollapsed(filePath: string) {
    collapsedMap.value = {
        ...collapsedMap.value,
        [filePath]: !(collapsedMap.value[filePath] ?? !defaultOpen(getStatusFor(filePath))),
    };
}

// Reverse-lookup for default-open: needed because the toggle
// closure captures the file path, not the file itself. Cache the
// status of every file by path at render time.
const statusByPath = computed<Record<string, string>>(() => {
    const m: Record<string, string> = {};
    for (const f of props.files) {
        m[f.path] = f.status;
    }
    return m;
});

function getStatusFor(path: string): string {
    return statusByPath.value[path] ?? "modified";
}

/** Per-line classification for the raw fallback path (used when
 *  jsdiff couldn't form real hunks — typically LLM-style +/- fragments
 *  lacking `---`/`+++` headers). Splits on "\n" and tags each line by
 *  its first character so we can paint add/del backgrounds without
 *  re-invoking the parser. Lines that don't look like diff lines
 *  ("other") render plain — common when the LLM emits a heading or
 *  short summary before the +/- block. */
type RawLineKind = "add" | "del" | "ctx" | "other";
function classifyRawLine(line: string): RawLineKind {
    if (line.startsWith("+") && !line.startsWith("+++")) return "add";
    if (line.startsWith("-") && !line.startsWith("---")) return "del";
    if (line.startsWith(" ")) return "ctx";
    return "other";
}
function rawLines(pf: ParsedFile): { kind: RawLineKind; text: string }[] {
    return pf.file.diff_text.split("\n").map((text) => ({
        kind: classifyRawLine(text),
        text,
    }));
}
</script>

<template>
    <div class="diff-view">
        <div v-if="files.length === 0" class="diff-view__empty">
            No file changes in this session yet.
        </div>
        <!-- 档位工具行(每实例一次,非 per-file)。三缺席条件:
             ①raw-only(v-if 的 hasParsedFile)②窄屏 CSS 藏整行
             (mobile-hide-toolbar 惯例类)③allowSplit=false。 -->
        <div v-if="showToolbar" class="diff-view__toolbar mobile-hide-toolbar">
            <button
                type="button"
                class="btn btn--ghost btn--sm"
                :aria-pressed="effectiveMode === 'unified'"
                @click="setMode('unified')"
            >
                单栏
            </button>
            <button
                type="button"
                class="btn btn--ghost btn--sm"
                :aria-pressed="effectiveMode === 'split'"
                @click="setMode('split')"
            >
                双栏
            </button>
        </div>
        <div
            v-for="pf in parsedFiles"
            :key="pf.file.path"
            class="diff-file"
        >
            <button
                type="button"
                class="diff-file__header btn btn--muted"
                @click="toggleCollapsed(pf.file.path)"
            >
                <Icon
                    :name="isCollapsed(pf.file.path, pf.file.status) ? 'chevron-right' : 'chevron-down'"
                    :size="12"
                    icon-class="diff-file__chevron"
                />
                <span class="diff-file__path">{{ pf.file.path }}</span>
                <span
                    :class="['diff-file__status', `diff-file__status--${pf.file.status}`]"
                >
                    {{ statusLabel(pf.file.status) }}
                </span>
                <span class="diff-file__counts">
                    <span v-if="pf.file.added > 0" class="diff-file__add">
                        +{{ pf.file.added }}
                    </span>
                    <span v-if="pf.file.removed > 0" class="diff-file__del">
                        −{{ pf.file.removed }}
                    </span>
                </span>
            </button>
            <div
                v-if="!isCollapsed(pf.file.path, pf.file.status)"
                class="diff-file__body"
            >
                <div v-if="pf.parsed && effectiveMode === 'unified'" class="diff-file__hunks">
                    <div
                        v-for="(hunk, hi) in pf.hunks"
                        :key="hi"
                        class="diff-hunk"
                    >
                        <div
                            v-for="(line, li) in hunk"
                            :key="li"
                            :class="['diff-line', `diff-line--${line.kind}`]"
                        >
                            <span class="diff-line__gutter diff-line__gutter--old">
                                {{ line.oldLine ?? "" }}
                            </span>
                            <span class="diff-line__gutter diff-line__gutter--new">
                                {{ line.newLine ?? "" }}
                            </span>
                            <span class="diff-line__prefix">
                                <template v-if="line.kind === 'add'">+</template>
                                <template v-else-if="line.kind === 'del'">−</template>
                                <template v-else-if="line.kind === 'ctx'">&nbsp;</template>
                                <template v-else>&nbsp;</template>
                            </span>
                            <span class="diff-line__text"><template v-if="line.segments"><span v-for="(seg, si) in line.segments" :key="si" :class="seg.changed ? ['diff-mark', `diff-mark--${line.kind}`] : undefined">{{ seg.text }}</span></template><template v-else>{{ line.text }}</template></span>
                        </div>
                    </div>
                </div>
                <!-- split 双栏(左旧右新,无 +/- 前缀列,色即语义)。 -->
                <div v-else-if="pf.parsed" class="diff-file__hunks">
                    <div
                        v-for="(hunk, hi) in pf.hunks"
                        :key="hi"
                        class="diff-hunk"
                    >
                        <template
                            v-for="(row, ri) in splitRows(hunk)"
                            :key="ri"
                        >
                            <div
                                v-if="row.kind === 'hunk' || row.kind === 'noeol'"
                                :class="['diff-sfull', `diff-sfull--${row.kind}`]"
                            >
                                {{ row.left?.text }}
                            </div>
                            <div v-else class="diff-srow">
                                <span class="diff-srow__gutter">
                                    {{ row.left?.lineNo ?? "" }}
                                </span>
                                <span :class="['diff-srow__cell', { 'diff-srow__cell--del': row.tintLeft }]"><template v-if="row.left"><template v-if="row.left.segments"><span v-for="(seg, si) in row.left.segments" :key="si" :class="seg.changed ? 'diff-mark diff-mark--del' : undefined">{{ seg.text }}</span></template><template v-else>{{ row.left.text }}</template></template></span>
                                <span class="diff-srow__gutter">
                                    {{ row.right?.lineNo ?? "" }}
                                </span>
                                <span :class="['diff-srow__cell', { 'diff-srow__cell--add': row.tintRight }]"><template v-if="row.right"><template v-if="row.right.segments"><span v-for="(seg, si) in row.right.segments" :key="si" :class="seg.changed ? 'diff-mark diff-mark--add' : undefined">{{ seg.text }}</span></template><template v-else>{{ row.right.text }}</template></template></span>
                            </div>
                        </template>
                    </div>
                </div>
                <div v-else class="diff-file__raw">
                    <div
                        v-for="(rl, ri) in rawLines(pf)"
                        :key="ri"
                        :class="['diff-raw-line', `diff-raw-line--${rl.kind}`]"
                    >{{ rl.text }}</div>
                </div>
                <div
                    v-if="pf.file.diff_text === ''"
                    class="diff-file__raw diff-file__raw--empty"
                >
                    <em>(binary or empty diff - no inline preview)</em>
                </div>
            </div>
        </div>
    </div>
</template>

<style scoped>
.diff-view {
    display: flex;
    flex-direction: column;
    gap: 6px;
    font-family: var(--font-mono);
    font-size: var(--text-sm);
    color: var(--color-text-primary);
}

.diff-view__empty {
    padding: 16px;
    text-align: center;
    color: var(--color-text-muted);
    font-size: var(--text-sm);
}

/* 档位工具行:根部右对齐,每实例一次(非 per-file)。窄屏整行 CSS 藏
 * (缺席条件②,mobile-hide 惯例,responsive spec §1.4)——显隐不走路
 * 径,matchMedia 只选树。 */
.diff-view__toolbar {
    display: flex;
    justify-content: flex-end;
    gap: 4px;
}

/* --------------------------------------------------------------------
 * split 双栏(PR2,design §3.3):4 列 = 左行号 / 旧文 / 右行号 / 新文。
 * align-items: start 必改(结论 4)—— 现状 .diff-line 的 baseline 在
 * 单侧长行 wrap 后会把行号 gutter 拉错位;栏内 pre-wrap + anywhere,
 * 长行 wrap 不裁切不横滚(与 unified 的 pre + 行级横滚两档各自策略)。
 * ------------------------------------------------------------------ */
.diff-srow {
    display: grid;
    grid-template-columns: 44px minmax(0, 1fr) 44px minmax(0, 1fr);
    align-items: start;
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    line-height: 1.5;
    min-width: 0;
}

.diff-srow__gutter {
    text-align: right;
    padding: 0 8px;
    color: var(--color-text-muted);
    user-select: none;
    border-right: 1px solid var(--color-bg-border);
}

.diff-srow__cell {
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    min-width: 0;
    padding: 0 8px;
}

/* 行底 tint(行底 0.12 族,与 unified 的 --add/--del 行同色):
 * 内容格与空占位格同染(tintLeft/tintRight),短侧补位是行对齐线索。 */
.diff-srow__cell--del {
    background: color-mix(in srgb, var(--color-tool-error) 12%, transparent);
}

.diff-srow__cell--add {
    background: rgba(16, 185, 129, 0.12);
}

/* 通栏行(hunk 头 / noeol):跨全宽,样式沿用 unified 对应行语义。 */
.diff-sfull {
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    line-height: 1.5;
    white-space: pre;
    padding: 0 8px;
}

.diff-sfull--hunk {
    background: var(--color-bg-surface);
    color: var(--color-text-muted);
}

.diff-sfull--noeol {
    color: var(--color-text-muted);
    font-style: italic;
}

.diff-file {
    border: 1px solid var(--color-bg-border);
    border-radius: var(--radius-md);
    background: var(--color-bg-surface);
    overflow: hidden;
}

/* 文件头折叠行由全局 .btn 家族承载(muted);此处仅通栏几何
 * (下边框是 diff 区分隔线,保留)。hover 由 elevated 转 accent-muted
 * 方向收敛。 */
.diff-file__header {
    gap: 8px;
    width: 100%;
    padding: 6px 10px;
    border: 0;
    border-bottom: 1px solid var(--color-bg-border);
    text-align: left;
    color: inherit;
}

.diff-file__chevron {
    flex-shrink: 0;
    color: var(--color-text-muted);
}

.diff-file__path {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--color-text-primary);
}

.diff-file__status {
    flex-shrink: 0;
    font-size: var(--text-2xs);
    text-transform: uppercase;
    letter-spacing: 0.05em;
    padding: 1px 6px;
    border-radius: 3px;
    background: var(--color-bg-app);
    color: var(--color-text-muted);
}

.diff-file__status--added {
    background: var(--color-tool-write);
    color: var(--color-bg-app);
}
.diff-file__status--deleted {
    background: var(--color-tool-error);
    color: var(--color-bg-app);
}
.diff-file__status--modified {
    background: var(--color-accent-muted);
    color: var(--color-accent-text);
}
.diff-file__status--renamed {
    background: var(--color-tool-read);
    color: var(--color-bg-app);
}

.diff-file__counts {
    flex-shrink: 0;
    display: inline-flex;
    gap: 4px;
    font-size: var(--text-xs);
    font-weight: var(--weight-semibold);
}

.diff-file__add {
    color: var(--color-tool-write);
}
.diff-file__del {
    color: var(--color-tool-error-text);
}

.diff-file__body {
    background: var(--color-bg-app);
    max-height: 480px;
    overflow-y: auto;
}

.diff-file__hunks {
    display: flex;
    flex-direction: column;
}

.diff-hunk {
    display: flex;
    flex-direction: column;
}

.diff-line {
    display: grid;
    grid-template-columns: 48px 48px 16px 1fr;
    align-items: baseline;
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    line-height: 1.5;
    white-space: pre;
    overflow-x: auto;
}

.diff-line--add {
    background: rgba(16, 185, 129, 0.12);
}
.diff-line--del {
    background: color-mix(in srgb, var(--color-tool-error) 12%, transparent);
}
.diff-line--hunk {
    background: var(--color-bg-surface);
    color: var(--color-text-muted);
}
.diff-line--noeol {
    color: var(--color-text-muted);
    font-style: italic;
}

.diff-line__gutter {
    text-align: right;
    padding: 0 8px;
    color: var(--color-text-muted);
    user-select: none;
    border-right: 1px solid var(--color-bg-border);
}

.diff-line__prefix {
    text-align: center;
    color: var(--color-text-muted);
    user-select: none;
}

.diff-line--add .diff-line__prefix {
    color: var(--color-tool-write);
}
.diff-line--del .diff-line__prefix {
    color: var(--color-tool-error-text);
}

.diff-line__text {
    padding: 0 8px;
}

/* 行内变更片段 tint(09-26-diffview-enhance,design §5):沿既有行底
 * 色族同 hue 加深(行底 0.12 → 片段 0.28),scoped 内联不入全局 token
 * 表(generative-ui spec「diff 染色同色族不立新 token」先例)。 */
.diff-mark--add {
    background: rgba(16, 185, 129, 0.28);
}

.diff-mark--del {
    background: color-mix(in srgb, var(--color-tool-error) 28%, transparent);
}

.diff-file__raw {
    display: flex;
    flex-direction: column;
    font-size: var(--text-xs);
    line-height: 1.5;
    color: var(--color-text-secondary);
}

.diff-raw-line {
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    line-height: 1.5;
    padding: 0 12px;
    white-space: pre;
    overflow-x: auto;
}

.diff-raw-line--add {
    background: rgba(16, 185, 129, 0.12);
    color: var(--color-text-primary);
}

.diff-raw-line--del {
    background: color-mix(in srgb, var(--color-tool-error) 12%, transparent);
    color: var(--color-text-primary);
}

.diff-raw-line--ctx {
    color: var(--color-text-secondary);
}

.diff-raw-line--other {
    color: var(--color-text-secondary);
}

.diff-file__raw--empty {
    color: var(--color-text-muted);
    text-align: center;
    padding: 16px;
}

@media (max-width: 767px) {
    /* 窄屏降级(缺席条件②):split 渲染树选树归 unified 之后,工具行
     * 也藏掉整行——窄屏没有可切的意义。走 mobile-hide-<what> 惯例类。 */
    .mobile-hide-toolbar {
        display: none;
    }
}
</style>
