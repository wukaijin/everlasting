// DiffView 单测 — 09-26-diffview-enhance 起建(此前无专属测试)。
//
// PR1 范围:unified 行内 seg(存在性 + 拼接还原)/ ctx 与纯增删不参与 /
// raw fallback 分支不回归(RULE-FrontDiff-001 间接锚)/ 折叠交互不回归。
// PR2 范围:split 网格(对齐/占位)/ 切换交互 / localStorage 记忆(三
// 纪律)/ matchMedia 选树(unmount 清理)/ 工具行三缺席 / allowSplit。
//
// DiffView 无 store 无 IPC,直接 mount。

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { nextTick } from "vue";
import { mount } from "@vue/test-utils";
import DiffView, { type FileDiff } from "./DiffView.vue";
// CSS 钉死用例读源用(vitest 默认 css:false,SFC 样式不注入 jsdom)。
import diffViewSource from "./DiffView.vue?raw";

const MODE_KEY = "everlasting:diffview.mode";

/** 标准修改文件:ctx + 一对配对 run + ctx + 一条独立纯新增 + ctx。 */
const PAIRED_DIFF = `--- a/foo.txt
+++ b/foo.txt
@@ -1,4 +1,5 @@
 ctx one
-del line with words
+add line with words
 ctx two
+pure added line
 ctx three
`;

function file(overrides: Partial<FileDiff> = {}): FileDiff {
    return {
        path: "foo.txt",
        status: "modified",
        added: 2,
        removed: 1,
        diff_text: PAIRED_DIFF,
        ...overrides,
    };
}

function mountView(files: FileDiff[], extraProps: Record<string, unknown> = {}) {
    return mount(DiffView, {
        props: { files, ...extraProps },
        global: { stubs: { Icon: true } },
    });
}

/** 展开默认收起的 modified 文件体,返回 wrapper。 */
async function mountExpanded(files: FileDiff[], extraProps: Record<string, unknown> = {}) {
    const w = mountView(files, extraProps);
    await w.find(".diff-file__header").trigger("click");
    return w;
}

beforeEach(() => {
    localStorage.clear();
});

describe("DiffView — unified 行内高亮(PR1)", () => {
    it("配对 del/add 行渲染 diff-mark 片段,数量正确", async () => {
        const w = await mountExpanded([file()]);
        const delLine = w.get(".diff-line--del");
        const addPaired = w.get(".diff-line--add");
        expect(delLine.findAll(".diff-mark--del").length).toBe(1); // del↔add
        expect(addPaired.findAll(".diff-mark--add").length).toBe(1);
    });

    it("不变量:每行 diff-line__text 文本串接 == 行文本(配对行也还原)", async () => {
        const w = await mountExpanded([file()]);
        // VTU text() 会 trim,拼接还原必须读原始 textContent(保留行首空格)。
        const texts = w.findAll(".diff-line__text").map(
            (n) => (n.element as HTMLElement).textContent ?? "",
        );
        expect(texts).toEqual([
            "@@ -1,4 +1,5 @@",
            "ctx one",
            "del line with words",
            "add line with words",
            "ctx two",
            "pure added line",
            "ctx three",
        ]);
    });

    it("纯新增行(独立 add-run)与 ctx 行保持整行染色,无 mark 片段", async () => {
        const w = await mountExpanded([file()]);
        const addLines = w.findAll(".diff-line--add");
        expect(addLines.length).toBe(2);
        // 第二条 add 是独立纯新增 run(前面无 del-run):无 mark。
        expect(addLines[1]!.findAll(".diff-mark").length).toBe(0);
        // ctx 行无 mark。
        for (const line of w.findAll(".diff-line--ctx")) {
            expect(line.findAll(".diff-mark").length).toBe(0);
        }
    });

    it("CRLF diff:hunk 行构建点归一,渲染文本无 \\r,配对照常", async () => {
        const crlf = PAIRED_DIFF.split("\n").join("\r\n");
        const w = await mountExpanded([file({ diff_text: crlf })]);
        for (const t of w.findAll(".diff-line__text")) {
            expect(t.text()).not.toContain("\r");
        }
        expect(w.get(".diff-line--del").findAll(".diff-mark--del").length).toBe(1);
    });

    it("多文件:第二文件的配对独立计算", async () => {
        const other = file({
            path: "bar.txt",
            added: 1,
            removed: 1,
            diff_text: `--- a/bar.txt\n+++ b/bar.txt\n@@ -1,1 +1,1 @@\n-old bar\n+new bar\n`,
        });
        const w = await mountExpanded([file(), other]);
        expect(w.findAll(".diff-file").length).toBe(2);
        expect(w.get(".diff-line--del").findAll(".diff-mark--del").length).toBe(1);
    });
});

describe("DiffView — raw fallback 不回归(RULE-FrontDiff-001 锚)", () => {
    const LLM_STYLE = ` foo
-x
+y
 bar`;

    it("LLM 风格无头片段:走 raw 分支,无行内 mark", async () => {
        const w = await mountExpanded([
            file({ diff_text: LLM_STYLE, added: 1, removed: 1 }),
        ]);
        expect(w.findAll(".diff-raw-line").length).toBe(4);
        expect(w.findAll(".diff-line").length).toBe(0);
        expect(w.findAll(".diff-mark").length).toBe(0);
    });

    it("空 hunks 形态防御:parsed=false 不产空 body 也不产 mark", async () => {
        // parsePatch 对无头 +/- 输入返回 [{hunks:[]}],out.parsed 必须 false。
        const w = await mountExpanded([file({ diff_text: "-x\n+y" })]);
        expect(w.findAll(".diff-raw-line").length).toBe(2);
        expect(w.findAll(".diff-mark").length).toBe(0);
    });
});

describe("DiffView — 折叠交互不回归", () => {
    it("modified 默认收起,点击展开;added 默认展开", async () => {
        const w = mountView([file()]);
        expect(w.find(".diff-file__body").exists()).toBe(false);
        await w.find(".diff-file__header").trigger("click");
        expect(w.find(".diff-file__body").exists()).toBe(true);

        const w2 = mountView([
            file({ path: "new.txt", status: "added", removed: 0, diff_text: PAIRED_DIFF }),
        ]);
        expect(w2.find(".diff-file__body").exists()).toBe(true);
    });

    it("再点收起:body 消失", async () => {
        const w = await mountExpanded([file()]);
        await w.find(".diff-file__header").trigger("click");
        expect(w.find(".diff-file__body").exists()).toBe(false);
    });
});

// --------------------------------------------------------------------
// PR2:side-by-side + 切换 + 持久化 + 窄屏降级 + allowSplit
// --------------------------------------------------------------------

/** matchMedia 替身:记录 listener,支持翻 matches 派发 change。 */
type MqlListener = (e: MediaQueryListEvent) => void;
function stubMatchMedia(initialMatches: boolean) {
    const listeners = new Set<MqlListener>();
    const mql = {
        media: "(max-width: 767px)",
        matches: initialMatches,
        addEventListener: (_t: string, fn: MqlListener) => {
            listeners.add(fn);
        },
        removeEventListener: (_t: string, fn: MqlListener) => {
            listeners.delete(fn);
        },
    };
    vi.stubGlobal("matchMedia", vi.fn(() => mql));
    return {
        setMatches(v: boolean) {
            mql.matches = v;
            for (const fn of [...listeners]) {
                fn({ matches: v } as unknown as MediaQueryListEvent);
            }
        },
        listenerCount: () => listeners.size,
    };
}

describe("DiffView — split 渲染(PR2)", () => {
    beforeEach(() => {
        localStorage.clear();
    });

    it("工具行默认渲染,默认 unified(aria-pressed 表态);点击双栏切 split 网格", async () => {
        const w = await mountExpanded([file()]);
        // get() 缺元素即抛,这里只需取到后断言按钮态。
        const toolbar = w.get(".diff-view__toolbar");
        const btns = toolbar.findAll("button");
        expect(btns.length).toBe(2);
        expect(btns[0]!.attributes("aria-pressed")).toBe("true");
        expect(btns[1]!.attributes("aria-pressed")).toBe("false");
        expect(w.findAll(".diff-srow").length).toBe(0);

        await btns[1]!.trigger("click");
        expect(btns[1]!.attributes("aria-pressed")).toBe("true");
        // unified 行退场,split 行登场。
        expect(w.findAll(".diff-line").length).toBe(0);
        // hunk 头通栏 1 + ctx×3 + pair×2。
        expect(w.findAll(".diff-sfull").length).toBe(1);
        expect(w.findAll(".diff-srow").length).toBe(5);
    });

    it("split 结构:ctx 两侧同行号对齐;del 仅左、add 仅右;行内 mark 在 split 生效", async () => {
        localStorage.setItem(MODE_KEY, "split");
        const w = await mountExpanded([file()]);
        const rows = w.findAll(".diff-srow");
        expect(rows.length).toBe(5);
        // 第 1 行 ctx:两侧行号一致(1 / 1)。
        expect(rowGutters(rows[0]!)).toEqual(["1", "1"]);
        // 第 2 行 pair:左 del(行号 2)右 add(行号 2)。
        expect(rowGutters(rows[1]!)).toEqual(["2", "2"]);
        expect(rowText(rows[1]!.findAll(".diff-srow__cell")[0]!)).toBe("del line with words");
        expect(rowText(rows[1]!.findAll(".diff-srow__cell")[1]!)).toBe("add line with words");
        // 行内高亮:配对行两侧各 1 个 mark。
        expect(rows[1]!.findAll(".diff-mark--del").length).toBe(1);
        expect(rows[1]!.findAll(".diff-mark--add").length).toBe(1);
        // 第 4 行 = 纯新增 pair:左占位空、右 "pure added line"(行号 4)。
        expect(rowGutters(rows[3]!)).toEqual(["", "4"]);
        expect(rowText(rows[3]!.findAll(".diff-srow__cell")[0]!)).toBe("");
        expect(rowText(rows[3]!.findAll(".diff-srow__cell")[1]!)).toBe("pure added line");
        // 无 +/- 前缀列:split 行内不应有 diff-line__prefix。
        expect(w.findAll(".diff-srow .diff-line__prefix").length).toBe(0);
    });

    it("del3+add2 不等长 run:3 行 pair,短侧补空占位格且同染", async () => {
        const TRIPLE = `--- a/triple.txt
+++ b/triple.txt
@@ -1,5 +1,4 @@
 keep
-del one
-del two
-del three
+add one
+add two
 keep end
`;
        localStorage.setItem(MODE_KEY, "split");
        const w = await mountExpanded([
            file({ path: "triple.txt", added: 2, removed: 3, diff_text: TRIPLE }),
        ]);
        const rows = w.findAll(".diff-srow");
        expect(rows.length).toBe(5); // ctx + 3 pair + ctx
        const pairRows = rows.slice(1, 4);
        // 前两行双侧都有内容。
        for (const row of pairRows.slice(0, 2)) {
            const cells = row.findAll(".diff-srow__cell");
            expect(cells.length).toBe(2);
            expect(rowText(cells[0]!)).not.toBe("");
            expect(rowText(cells[1]!)).not.toBe("");
        }
        // 第 3 行:左 "del three",右占位(空内容,占位格 DOM 在)。
        const last = pairRows[2]!;
        const cells = last.findAll(".diff-srow__cell");
        expect(cells.length).toBe(2);
        expect(rowText(cells[0]!)).toBe("del three");
        expect(rowText(cells[1]!)).toBe("");
        // 占位格同染(对齐线索):del3/add2 的 run 双侧铺 tint。
        expect(cells[0]!.classes()).toContain("diff-srow__cell--del");
        expect(cells[1]!.classes()).toContain("diff-srow__cell--add");
    });

    it("CSS 钉死:split 行网格 align-items: start(防 baseline 回归让 wrap 行号错位)", () => {
        // vitest 默认 css:false(SFC 样式不注入 jsdom),无法走 CSSOM/
        // getComputedStyle;退而钉源(?raw 导入):style 块内 .diff-srow
        // 规则必须带 align-items: start(结论 4 防回归锚;真观感由 e2e 截图兜)。
        const styleIdx = diffViewSource.indexOf("<style scoped>");
        expect(styleIdx).toBeGreaterThan(0);
        const styleBlock = diffViewSource.slice(styleIdx);
        const rule = /\.diff-srow\s*\{[^}]*\}/.exec(styleBlock);
        expect(rule).not.toBeNull();
        expect(rule![0]).toContain("align-items: start");
    });
});

describe("DiffView — 切换持久化(localStorage 三纪律)", () => {
    beforeEach(() => {
        localStorage.clear();
    });

    it("读路径永不写回:干净存储挂载后 key 仍缺省", async () => {
        await mountExpanded([file()]);
        expect(localStorage.getItem(MODE_KEY)).toBeNull();
    });

    it("点击才写:切双栏 → key=split;切单栏 → key=unified", async () => {
        const w = await mountExpanded([file()]);
        const btns = w.findAll(".diff-view__toolbar button");
        await btns[1]!.trigger("click");
        expect(localStorage.getItem(MODE_KEY)).toBe("split");
        await btns[0]!.trigger("click");
        expect(localStorage.getItem(MODE_KEY)).toBe("unified");
    });

    it("重挂载记忆生效:预置 split → 新建挂载直接渲染 split(跨入口同路径)", async () => {
        localStorage.setItem(MODE_KEY, "split");
        const w = await mountExpanded([file()]);
        expect(w.findAll(".diff-srow").length).toBe(5);
        w.unmount();
        const w2 = await mountExpanded([file()]);
        expect(w2.findAll(".diff-srow").length).toBe(5);
    });

    it("非法存储值 → unified;同屏多实例不联动(挂载时各读一次)", async () => {
        localStorage.setItem(MODE_KEY, "bogus");
        const w = await mountExpanded([file()]);
        expect(w.findAll(".diff-srow").length).toBe(0);
        // 实例 A 切 split 不影响已挂载的实例 B。
        const wA = await mountExpanded([file()]);
        const wB = await mountExpanded([file()]);
        const btns = wA.findAll(".diff-view__toolbar button");
        await btns[1]!.trigger("click");
        expect(wA.findAll(".diff-srow").length).toBeGreaterThan(0);
        expect(wB.findAll(".diff-srow").length).toBe(0);
    });
});

describe("DiffView — 窄屏选树(matchMedia)与工具行三缺席", () => {
    beforeEach(() => {
        localStorage.clear();
    });

    afterEach(() => {
        vi.unstubAllGlobals();
    });

    it("窄屏(matchMedia 命中)恒渲染 unified;工具行挂 mobile-hide 类(CSS 藏整行)", async () => {
        stubMatchMedia(true);
        localStorage.setItem(MODE_KEY, "split");
        const w = await mountExpanded([file()]);
        // 选树归 unified:split 网格不出,unified 行在。
        expect(w.findAll(".diff-srow").length).toBe(0);
        expect(w.findAll(".diff-line").length).toBeGreaterThan(0);
        // 工具行 DOM 在但带 mobile-hide-toolbar(responsive spec §1.4
        // 惯例类,CSS display:none;真视口语义交 Playwright)。
        expect(w.find(".diff-view__toolbar").classes()).toContain("mobile-hide-toolbar");
    });

    it("matchMedia change 事件选树:翻窄 → unified;翻宽 → 回 split", async () => {
        const mq = stubMatchMedia(false);
        localStorage.setItem(MODE_KEY, "split");
        const w = await mountExpanded([file()]);
        expect(w.findAll(".diff-srow").length).toBe(5);
        mq.setMatches(true);
        await nextTick(); // isNarrow 是 ref,等渲染 flush
        expect(w.findAll(".diff-srow").length).toBe(0);
        expect(w.findAll(".diff-line").length).toBeGreaterThan(0);
        mq.setMatches(false);
        await nextTick();
        expect(w.findAll(".diff-srow").length).toBe(5);
    });

    it("unmount 移除 matchMedia change 监听(不泄漏)", () => {
        const mq = stubMatchMedia(false);
        const w = mountView([file()]);
        expect(mq.listenerCount()).toBe(1);
        w.unmount();
        expect(mq.listenerCount()).toBe(0);
    });

    it("缺席条件① raw-only:无任何 parsed 文件不给工具行", async () => {
        const w = await mountExpanded([
            file({ diff_text: "-x\n+y", added: 1, removed: 1 }),
        ]);
        expect(w.find(".diff-view__toolbar").exists()).toBe(false);
        // raw 分支不受档位影响(无 split 化)。
        expect(w.findAll(".diff-raw-line").length).toBe(2);
    });

    it("缺席条件③ allowSplit=false:不给工具行;存储 split 也恒 unified", async () => {
        localStorage.setItem(MODE_KEY, "split");
        const w = await mountExpanded([file({})], { allowSplit: false });
        expect(w.find(".diff-view__toolbar").exists()).toBe(false);
        expect(w.findAll(".diff-srow").length).toBe(0);
        expect(w.findAll(".diff-line").length).toBeGreaterThan(0);
        // 点击路径不存在 → 不会写 key。
        expect(localStorage.getItem(MODE_KEY)).toBe("split");
    });

    it("缺席条件② 窄屏:工具行 CSS 藏整行(结构断言,真视口在 e2e)", async () => {
        stubMatchMedia(true);
        const w = await mountExpanded([file()]);
        expect(w.find(".mobile-hide-toolbar").exists()).toBe(true);
    });
});

// ---- split 用例的小工具(取行号/原始文本,VTU text() 会 trim)----
import type { DOMWrapper } from "@vue/test-utils";
function rowGutters(row: DOMWrapper<Element>): string[] {
    return row.findAll(".diff-srow__gutter").map((g) => g.text());
}
function rowText(cell: DOMWrapper<Element>): string {
    return (cell.element as HTMLElement).textContent ?? "";
}
