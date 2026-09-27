// intraLineDiff — 行内 word-diff 配对 util(09-26-diffview-enhance,design §2)。
//
// DiffView(unified + split)与 EditFileCard 共享的纯函数层:把一对
// 「极大 del-run / add-run」的行文本喂进来,拿回逐行的行内片段
// (WordSeg[]),变更片段由渲染层加 tint 标记。零状态、零 DOM。
//
// 契约(design §2,评审结论 2/3/5/6 回填):
// - 原子 null:pairRunSegments 对整个 run 返回 null(调用方整 run
//   退化为整行染色),三条触发路径——①join 后任侧长度超过
//   MAX_PAIR_LEN;②diffWordsWithSpace 抛错;③重分布一致性校验失败。
//   不做「尾部 null」部分退化。
// - 多行块重分布用「\n 切分推进」:两侧按 \n join → word-diff → 逐段
//   按内含 \n 切开派发到对应行(seg 含 \n 就切),不是累计字符数。
// - 不变量(钉在 intraLineDiff.test.ts):两侧输出数组长度 == 入参
//   数组长度;每行 segments 的 text 拼接 == 该行入参文本。
// - CRLF 归一不放进 util(结论 6,保契约纯净):\r 是普通字符透传,
//   归一在消费点做(DiffView hunk 行构建 / EditFileCard split(/\r?\n/))。

import { diffWordsWithSpace } from "diff";

/** 行内高亮片段:changed = 该片段在配对侧有对应差异(渲染层加 mark)。 */
export interface WordSeg {
    text: string;
    changed: boolean;
}

/** 单次 word-diff 配对的任侧长度上限(join 后)。超长行/块跳过行内
 *  计算,整行(整 run)退化为行级染色(design §2 性能护栏)。 */
export const MAX_PAIR_LEN = 4000;

/** 低层:一对字符串的 word-diff。两侧输出都是全覆盖片段流(拼接 ==
 *  各自入参,含未被 diff 改动的上下文片段)。调用方应处理 null
 *  (jsdiff 抛错时——防御性,迄今未知有输入触发)。 */
export function pairWordSegments(
    a: string,
    b: string,
): { a: WordSeg[]; b: WordSeg[] } | null {
    let changes: ReturnType<typeof diffWordsWithSpace>;
    try {
        changes = diffWordsWithSpace(a, b);
    } catch {
        return null;
    }
    const aSegs: WordSeg[] = [];
    const bSegs: WordSeg[] = [];
    for (const ch of changes) {
        const changed = ch.added || ch.removed;
        if (!ch.added) aSegs.push({ text: ch.value, changed });
        if (!ch.removed) bSegs.push({ text: ch.value, changed });
    }
    return { a: aSegs, b: bSegs };
}

/** 高层:相邻 del-run / add-run 配对。多行块两侧按 \n join 后做一次
 *  word-diff,再按「\n 切分推进」把片段派发回各行。返回与入参逐行
 *  对应的 segments(null 行 = 整行染色);run 级 null = 整 run 退化
 *  (三条触发路径见文件头契约)。 */
export function pairRunSegments(
    delLines: string[],
    addLines: string[],
): { del: (WordSeg[] | null)[]; add: (WordSeg[] | null)[] } | null {
    const joinedDel = delLines.join("\n");
    const joinedAdd = addLines.join("\n");
    // 路径①:超长护栏(join 后任侧超帽,整 run 不算)。
    if (joinedDel.length > MAX_PAIR_LEN || joinedAdd.length > MAX_PAIR_LEN) {
        return null;
    }
    const paired = pairWordSegments(joinedDel, joinedAdd);
    if (!paired) {
        // 路径②:diffWordsWithSpace 抛错(catch 在低层)。
        return null;
    }
    const delRows: WordSeg[][] = delLines.map(() => []);
    const addRows: WordSeg[][] = addLines.map(() => []);
    // 「\n 切分推进」重分布:join 给每行之间恰好补了一个 \n,所以每侧
    // 片段流内含的 \n 总数必须恰好 == 行数 - 1;每个片段按内含 \n 切开,
    // 后续片段落进下一行。空串片段(行首/行尾或空行)不产空 seg。
    const redistribute = (
        segs: WordSeg[],
        rows: WordSeg[][],
    ): boolean => {
        let row = 0;
        for (const seg of segs) {
            const pieces = seg.text.split("\n");
            for (let k = 0; k < pieces.length; k++) {
                if (k > 0) {
                    row += 1;
                    if (row >= rows.length) return false;
                }
                if (pieces[k]) {
                    rows[row].push({ text: pieces[k], changed: seg.changed });
                }
            }
        }
        // 路径③:一致性校验——推进必须恰好落在最后一行(片段流的 \n
        // 数与行数不匹配 = 某侧字符被 diff 吃掉/多出,防御分支)。
        return row === rows.length - 1;
    };
    if (!redistribute(paired.a, delRows) || !redistribute(paired.b, addRows)) {
        return null;
    }
    return { del: delRows, add: addRows };
}
