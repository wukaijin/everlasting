// `utils/toolSummary.ts` 单测 —— read 族紧凑卡 headline 的 chip / meta
// 纯函数(2026-09-19, task `09-19-tool-card-compact-read`)。
//
// 这一层给 headline 供数,信息以前藏在展开区里;报错的数字会被用户当
// 事实读,所以每个分支都锁住契约,尤其是两类边界:
//   - 截断提示行(glob / list_dir 尾部 `(…)`)不能计入条目数;
//   - read_file 的 `\t<行号>\t` 前导制表符不能被 trim 吃掉(实证 bug:
//     范围从第二行报起,L2–4)。

import { describe, it, expect } from "vitest";
import {
  isReadFamilyTool,
  readToolChip,
  readToolMeta,
} from "./toolSummary";

/** 后端工具结果的 wire 信封(REQ-16):`{result, cwd}`。 */
function env(result: string, cwd = "/repo"): string {
  return JSON.stringify({ result, cwd });
}

/** read_file 的 cat -n 形态。 */
function numbered(lines: string[], offset = 1): string {
  return lines.map((l, i) => `\t${offset + i}\t${l}`).join("\n");
}

describe("isReadFamilyTool", () => {
  it("认 glob / list_dir / read_file", () => {
    expect(isReadFamilyTool("glob")).toBe(true);
    expect(isReadFamilyTool("list_dir")).toBe(true);
    expect(isReadFamilyTool("read_file")).toBe(true);
  });

  it("不认兄弟工具(它们仍走通用卡)", () => {
    for (const n of ["grep", "write_file", "edit_file", "shell", "web_search"]) {
      expect(isReadFamilyTool(n)).toBe(false);
    }
  });
});

describe("readToolChip", () => {
  it("glob 用 pattern(通用卡的 path-only chip 看不到它)", () => {
    expect(readToolChip("glob", { pattern: "src/**/*.vue" })).toBe("src/**/*.vue");
  });

  it("glob 的搜索根非 cwd 时补 ` in path`", () => {
    expect(
      readToolChip("glob", { pattern: "**/*.rs", path: "app/src-tauri" }),
    ).toBe("**/*.rs in app/src-tauri");
  });

  it("glob 的 path 为 `.` 时不补(等价默认 cwd)", () => {
    expect(readToolChip("glob", { pattern: "*.json", path: "." })).toBe("*.json");
  });

  it("list_dir 用 path;缺省显示 cwd 占位", () => {
    expect(readToolChip("list_dir", { path: "app/src" })).toBe("app/src");
    expect(readToolChip("list_dir", {})).toBe("cwd");
    expect(readToolChip("list_dir")).toBe("cwd");
  });

  it("read_file 用 path", () => {
    expect(readToolChip("read_file", { path: "app/src/App.vue" })).toBe(
      "app/src/App.vue",
    );
  });

  it("畸形 input(非 string / 空串 / 缺字段)→ null 或 cwd,不抛错", () => {
    expect(readToolChip("glob", { pattern: 42 })).toBeNull();
    expect(readToolChip("glob", { pattern: "" })).toBeNull();
    expect(readToolChip("read_file", { path: 7 })).toBeNull();
    expect(readToolChip("read_file", {})).toBeNull();
    expect(readToolChip("glob", undefined)).toBeNull();
  });
});

describe("readToolMeta — glob", () => {
  it("数路径行", () => {
    const out = ["a.rs", "b.rs", "c.rs"].join("\n");
    expect(readToolMeta("glob", { content: env(out), isError: false })).toBe(
      "3 matches",
    );
  });

  it("单数用 match", () => {
    expect(readToolMeta("glob", { content: env("a.rs"), isError: false })).toBe(
      "1 match",
    );
  });

  it("截断提示行不计入,计数加 `+`", () => {
    const out = `a.rs\nb.rs\n\n(...and 37 more matches; narrow your pattern to see them)`;
    expect(readToolMeta("glob", { content: env(out), isError: false })).toBe(
      "2+ matches",
    );
  });

  it("`showing the N most recent` 提示同样不计入", () => {
    const out = `a.rs\n(showing the 100 most recent matches; narrow your pattern for the rest)`;
    expect(readToolMeta("glob", { content: env(out), isError: false })).toBe(
      "1+ match",
    );
  });

  it("0 命中(非错误)→ no matches", () => {
    const out = "No files matched pattern '*.zzz' in /repo.";
    expect(readToolMeta("glob", { content: env(out), isError: false })).toBe(
      "no matches",
    );
  });
});

describe("readToolMeta — list_dir", () => {
  it("数条目(目录尾斜杠不影响计数)", () => {
    const out = ["chat/", "search/", "ChatPanel.vue"].join("\n");
    expect(readToolMeta("list_dir", { content: env(out), isError: false })).toBe(
      "3 entries",
    );
  });

  it("单数用 entry", () => {
    expect(readToolMeta("list_dir", { content: env("main.ts"), isError: false })).toBe(
      "1 entry",
    );
  });

  it("空目录 → empty", () => {
    expect(
      readToolMeta("list_dir", {
        content: env("(empty directory: /repo/app/src/empty)"),
        isError: false,
      }),
    ).toBe("empty");
  });

  it("limit 截断提示行不计入,计数加 `+`", () => {
    const out = `a\nb\n\n(...12 more entries hidden by limit; raise the limit or pass show_hidden: true)`;
    expect(readToolMeta("list_dir", { content: env(out), isError: false })).toBe(
      "2+ entries",
    );
  });
});

describe("readToolMeta — read_file", () => {
  it("报行范围(cat -n 的真实行号)", () => {
    const out = numbered(["a", "b", "c"]);
    expect(readToolMeta("read_file", { content: env(out), isError: false })).toBe(
      "L1–3",
    );
  });

  it("带 offset 时按输出行号报(不是从 1 起)", () => {
    const out = numbered(["a", "b"], 320);
    expect(readToolMeta("read_file", { content: env(out), isError: false })).toBe(
      "L320–321",
    );
  });

  it("首行行首的制表符不被 trim 吃掉(L1 不能报成 L2)", () => {
    const out = numbered(["only line"]);
    expect(readToolMeta("read_file", { content: env(out), isError: false })).toBe(
      "L1",
    );
  });

  it("截断(head + 标记 + tail)报真实跨度并标 truncated", () => {
    const head = numbered(["a", "b"]);
    const tail = numbered(["y", "z"], 4999);
    const out = `${head}\n<truncated: omitted 40000 of 51234 bytes | recover: read_file with offset/limit>\n${tail}`;
    expect(readToolMeta("read_file", { content: env(out), isError: false })).toBe(
      "L1–5000 · truncated",
    );
  });

  it("读图 → image(带尺寸时 image W×H)", () => {
    expect(
      readToolMeta("read_file", {
        content: env("[image: /repo/a.png (1280×720) — 已作为图片块发送]"),
        isError: false,
      }),
    ).toBe("image 1280×720");
    expect(
      readToolMeta("read_file", {
        content: env("[image: /repo/a.png — 超过 5MB 上限未读取；请压缩或转换后重试]"),
        isError: false,
      }),
    ).toBe("image");
  });

  it("非行号输出(工具集扩展前的历史行)→ 字符数兜底", () => {
    expect(
      readToolMeta("read_file", { content: env("plain text, no line numbers"), isError: false }),
    ).toBe("27 chars");
  });
});

describe("readToolMeta — 共性守卫", () => {
  it("报错 → null(错误由 ✗ + 展开区承载,不报规模)", () => {
    expect(
      readToolMeta("read_file", {
        content: env("Failed to read file '/x': No such file"),
        isError: true,
      }),
    ).toBeNull();
  });

  it("结果缺失(流式中)→ null", () => {
    expect(readToolMeta("glob", null)).toBeNull();
    expect(readToolMeta("glob", undefined)).toBeNull();
  });

  it("空输出 → null", () => {
    expect(readToolMeta("glob", { content: "", isError: false })).toBeNull();
    expect(readToolMeta("glob", { content: env("  \n "), isError: false })).toBeNull();
  });

  it("不认识的工具 → null(不猜)", () => {
    expect(readToolMeta("grep", { content: env("a\nb"), isError: false })).toBeNull();
  });

  it("裸文本(无 {result,cwd} 信封)照常解析(向后兼容历史行)", () => {
    expect(readToolMeta("glob", { content: "a.rs\nb.rs", isError: false })).toBe(
      "2 matches",
    );
  });
});
