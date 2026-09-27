// intraLineDiff 单测 — util 级不变量 + 边界语料(design §7 util 行)。
//
// 两条钉死的不变量(impl 契约):
//   I1 两侧输出数组长度 == 入参数组长度;
//   I2 每行 segments 的 text 拼接 == 该行入参文本(util 不做 CRLF
//      归一,\r 是普通字符透传——归一在消费点,design §2 结论 6)。
// 语料必含:空行对 / 行尾空白 / CRLF 输入 / del3+add2 不等长 /
// 超长护栏 null / 异常 catch null / 一致性校验失败 null。

import { describe, it, expect, vi, beforeEach } from "vitest";

// 部分 mock jsdiff:真实现兜底,仅三个防御路径用例翻开关——
//   throwOnCall   → 路径②(diffWordsWithSpace 抛错);
//   dropANewline  → 路径③(片段流被吞一个 \n,join 结构破坏,
//                    「\n 切分推进」落不进最后一行 → 一致性校验失败)。
const diffMockState = { throwOnCall: false, dropANewline: false };
vi.mock("diff", async (importOriginal) => {
    const actual = await importOriginal<typeof import("diff")>();
    return {
        ...actual,
        diffWordsWithSpace: (a: string, b: string) => {
            if (diffMockState.throwOnCall) {
                throw new Error("injected: diffWordsWithSpace blew up");
            }
            const changes = actual
                .diffWordsWithSpace(a, b)
                .map((c) => ({ ...c }));
            if (diffMockState.dropANewline) {
                for (const c of changes) {
                    if (!c.added && c.value.includes("\n")) {
                        c.value = c.value.replace("\n", "");
                        break;
                    }
                }
            }
            return changes;
        },
    };
});

import {
    pairWordSegments,
    pairRunSegments,
    MAX_PAIR_LEN,
    type WordSeg,
} from "./intraLineDiff";

/** I1 + I2 不变量断言:非 null 时长度相等 + 逐行拼接还原。 */
function expectInvariants(
    delLines: string[],
    addLines: string[],
    out: { del: (WordSeg[] | null)[]; add: (WordSeg[] | null)[] } | null,
): void {
    expect(out).not.toBeNull();
    expect(out!.del.length).toBe(delLines.length);
    expect(out!.add.length).toBe(addLines.length);
    for (let i = 0; i < delLines.length; i++) {
        expect(out!.del[i]).not.toBeNull();
        expect(out!.del[i]!.map((s) => s.text).join("")).toBe(delLines[i]);
    }
    for (let i = 0; i < addLines.length; i++) {
        expect(out!.add[i]).not.toBeNull();
        expect(out!.add[i]!.map((s) => s.text).join("")).toBe(addLines[i]);
    }
}

describe("pairWordSegments", () => {
    it("单行对:变更片段标 changed,上下文片段不变,两侧拼接还原", () => {
        const out = pairWordSegments("const foo = 1;", "const foo = 2;");
        expect(out).not.toBeNull();
        expect(out!.a.map((s) => s.text).join("")).toBe("const foo = 1;");
        expect(out!.b.map((s) => s.text).join("")).toBe("const foo = 2;");
        expect(out!.a.some((s) => s.changed)).toBe(true);
        expect(out!.b.some((s) => s.changed)).toBe(true);
        // 上下文片段(未变更)两侧一致:前缀 + 尾缀 `;`。
        const ctx = out!.a.filter((s) => !s.changed);
        expect(ctx.map((s) => s.text).join("")).toBe("const foo = ;");
        expect(ctx[0]!.text).toBe("const foo = ");
    });

    it("完全相同的行:无 changed 片段", () => {
        const out = pairWordSegments("same line", "same line");
        expect(out).not.toBeNull();
        expect(out!.a.every((s) => !s.changed)).toBe(true);
        expect(out!.b.every((s) => !s.changed)).toBe(true);
    });

    it("空串对:返回空片段流", () => {
        const out = pairWordSegments("", "");
        expect(out).not.toBeNull();
        expect(out!.a).toEqual([]);
        expect(out!.b).toEqual([]);
    });
});

describe("pairRunSegments", () => {
    beforeEach(() => {
        diffMockState.throwOnCall = false;
        diffMockState.dropANewline = false;
    });

    it("单行 del/add 配对:片段与低层一致", () => {
        const out = pairRunSegments(["old value"], ["new value"]);
        expectInvariants(["old value"], ["new value"], out);
        expect(out!.del[0]!.some((s) => s.changed)).toBe(true);
        expect(out!.add[0]!.some((s) => s.changed)).toBe(true);
    });

    it("多行块 join+「\\n 切分推进」重分布:跨行变更落到正确的行", () => {
        const delLines = ["keep a", "drop old one", "drop old two", "keep b"];
        const addLines = ["keep a", "brand new one", "keep b"];
        const out = pairRunSegments(delLines, addLines);
        expectInvariants(delLines, addLines, out);
        // 首尾 ctx 邻接行无变更片段。
        expect(out!.del[0]!.every((s) => !s.changed)).toBe(true);
        expect(out!.add[0]!.every((s) => !s.changed)).toBe(true);
        expect(out!.del[3]!.every((s) => !s.changed)).toBe(true);
        expect(out!.add[2]!.every((s) => !s.changed)).toBe(true);
        // 中间变更行有 changed 片段。
        expect(out!.del[1]!.some((s) => s.changed)).toBe(true);
        expect(out!.add[1]!.some((s) => s.changed)).toBe(true);
    });

    it("空行对:空行 segments 拼接 == 空串,不变量不破", () => {
        const delLines = ["a", "", "b"];
        const addLines = ["a", "", "c"];
        expectInvariants(delLines, addLines, pairRunSegments(delLines, addLines));
    });

    it("行尾空白:保留且拼接还原", () => {
        const delLines = ["foo   ", "bar"];
        const addLines = ["foo", "bar   "];
        expectInvariants(delLines, addLines, pairRunSegments(delLines, addLines));
    });

    it("CRLF 输入:\\r 普通字符透传(util 不剥,归一在消费点)", () => {
        const del = ["one\r", "two\r"];
        const add = ["one\r", "TWO"];
        expectInvariants(del, add, pairRunSegments(del, add));
    });

    it("del3+add2 不等长 run:逐行对应 + 拼接还原(尾部形态钉死)", () => {
        const delLines = ["del one", "del two", "del three"];
        const addLines = ["add one", "add two"];
        expectInvariants(delLines, addLines, pairRunSegments(delLines, addLines));
    });

    it("超长护栏:join 后任侧 > MAX_PAIR_LEN 整 run null(路径①)", () => {
        const long = "x".repeat(MAX_PAIR_LEN + 1);
        expect(pairRunSegments([long], ["y"])).toBeNull();
        expect(pairRunSegments(["y"], [long])).toBeNull();
        // 多行 join 拼起来超帽同样触发(长度口径 = join 后)。
        const pieces = Array.from({ length: 5 }, () => "x".repeat(900));
        expect(pairRunSegments(pieces, ["y"])).toBeNull();
        // 帽内不误伤。
        const ok = "x".repeat(MAX_PAIR_LEN);
        expectInvariants([ok], ["y"], pairRunSegments([ok], ["y"]));
    });

    it("diffWordsWithSpace 抛错 → 整 run null(路径②)", () => {
        diffMockState.throwOnCall = true;
        try {
            expect(pairRunSegments(["a", "b"], ["a", "c"])).toBeNull();
        } finally {
            diffMockState.throwOnCall = false;
        }
    });

    it("重分布一致性校验失败 → 整 run null(路径③,防御分支)", () => {
        // dropANewline:jsdiff 输出的非 added 侧片段流被吞掉一个 \n,
        // join 补的行界丢失 → 推进停不到最后一行 → 校验失败 → null。
        diffMockState.dropANewline = true;
        try {
            expect(pairRunSegments(["a", "b"], ["a", "c"])).toBeNull();
        } finally {
            diffMockState.dropANewline = false;
        }
    });

    it("空 run 防御:任侧 0 行 → null(调用方不该喂空 run)", () => {
        expect(pairRunSegments(["x"], [])).toBeNull();
        expect(pairRunSegments([], ["y"])).toBeNull();
    });
});
