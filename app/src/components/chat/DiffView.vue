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

import { computed, ref } from "vue";
import { parsePatch } from "diff";
import Icon from "../Icon.vue";

export interface FileDiff {
    path: string;
    status: string;
    added: number;
    removed: number;
    diff_text: string;
}

const props = defineProps<{
    files: FileDiff[];
}>();

interface HunkLine {
    /** `+` for added, `-` for removed, ` ` for context, `@` for hunk header. */
    kind: "add" | "del" | "ctx" | "hunk" | "noeol";
    text: string;
    oldLine: number | null;
    newLine: number | null;
}

interface ParsedFile {
    file: FileDiff;
    hunks: HunkLine[][];
    /** True when the parser returned something we trust to render. */
    parsed: boolean;
}

const COLLAPSED_STATUSES = new Set(["modified"]);

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
                });
                let oldLine = hunk.oldStart;
                let newLine = hunk.newStart;
                for (const line of hunk.lines) {
                    const prefix = line[0];
                    const text = line.slice(1);
                    if (prefix === "+") {
                        lines.push({
                            kind: "add",
                            text,
                            oldLine: null,
                            newLine: newLine,
                        });
                        newLine += 1;
                    } else if (prefix === "-") {
                        lines.push({
                            kind: "del",
                            text,
                            oldLine: oldLine,
                            newLine: null,
                        });
                        oldLine += 1;
                    } else if (prefix === " ") {
                        lines.push({
                            kind: "ctx",
                            text,
                            oldLine: oldLine,
                            newLine: newLine,
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
        <div
            v-for="pf in parsedFiles"
            :key="pf.file.path"
            class="diff-file"
        >
            <button
                type="button"
                class="diff-file__header"
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
                <div v-if="pf.parsed" class="diff-file__hunks">
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
                            <span class="diff-line__text">{{ line.text }}</span>
                        </div>
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

.diff-file {
    border: 1px solid var(--color-bg-border);
    border-radius: var(--radius-md);
    background: var(--color-bg-surface);
    overflow: hidden;
}

.diff-file__header {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    padding: 6px 10px;
    background: var(--color-bg-elevated);
    border: 0;
    border-bottom: 1px solid var(--color-bg-border);
    cursor: pointer;
    text-align: left;
    font: inherit;
    color: inherit;
}

.diff-file__header:hover {
    background: var(--color-bg-border);
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
    color: var(--color-accent);
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
    color: var(--color-tool-error);
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
    background: rgba(239, 68, 68, 0.12);
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
    color: var(--color-tool-error);
}

.diff-line__text {
    padding: 0 8px;
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
    background: rgba(239, 68, 68, 0.12);
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
</style>
