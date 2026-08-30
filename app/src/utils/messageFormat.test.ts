// Tests for `extractToolResultDisplay` — the bridge between the
// LLM-facing tool result envelope (`{ result, cwd }`) and the
// human-readable display. The envelope is the LLM's contract so
// it can see which on-disk path a tool ran against (REQ-16 in
// prd.md); the UI must NOT show the raw JSON to the user.
//
// Why a separate test file (vs. adding to messageFormat.ts as a
// private test): the helper is a single, pure function with
// multiple branches (envelope / no envelope / non-JSON / empty).
// A dedicated test file keeps the fixture readable.

import { describe, it, expect } from "vitest";
import {
  extractToolResultDisplay,
  isRealUserTurnStart,
  isShellFamilyTool,
  toolHeaderChip,
} from "./messageFormat";

describe("extractToolResultDisplay", () => {
  it("unwraps the cwd envelope to the result string", () => {
    const envelope = JSON.stringify({
      result: "hello world",
      cwd: "/data/worktrees/p1/s1",
    });
    expect(extractToolResultDisplay(envelope)).toBe("hello world");
  });

  it("preserves multi-line content in the result field", () => {
    const envelope = JSON.stringify({
      result: "line 1\nline 2\nline 3",
      cwd: "/data/wt",
    });
    expect(extractToolResultDisplay(envelope)).toBe("line 1\nline 2\nline 3");
  });

  it("preserves special characters (quotes, backslashes)", () => {
    const content = 'has "quotes" and \\ a backslash';
    const envelope = JSON.stringify({ result: content, cwd: "/data/wt" });
    expect(extractToolResultDisplay(envelope)).toBe(content);
  });

  it("returns the raw input when it's not JSON", () => {
    // Pre-follow-up sessions stored plain strings; on rehydrate
    // the content has no envelope, so the helper must pass it
    // through unchanged.
    const plain = "this is not JSON";
    expect(extractToolResultDisplay(plain)).toBe(plain);
  });

  it("returns the raw input when JSON lacks the envelope shape", () => {
    // A JSON object that doesn't have both `result` (string) and
    // `cwd` (string) is not an envelope — pass through. This
    // protects against false positives from random tool output
    // that happens to be valid JSON.
    const other = JSON.stringify({ output: "data", meta: "info" });
    expect(extractToolResultDisplay(other)).toBe(other);
  });

  it("returns the raw input when result field is non-string", () => {
    const wrongType = JSON.stringify({ result: 42, cwd: "/data/wt" });
    expect(extractToolResultDisplay(wrongType)).toBe(wrongType);
  });

  it("returns empty string for empty input", () => {
    expect(extractToolResultDisplay("")).toBe("");
  });

  it("fast-paths strings that don't start with '{'", () => {
    // Common case: short, non-JSON tool output ("ok", "success",
    // "Wrote file", etc). The helper should not try to parse.
    expect(extractToolResultDisplay("ok")).toBe("ok");
    expect(extractToolResultDisplay("Wrote /tmp/foo.txt")).toBe(
      "Wrote /tmp/foo.txt",
    );
  });
});

// 交错思考: renderGroups 分组判据。决定一条消息是否开启新的 agent run
// (MessageList 把同 run 的多条消息视觉连成一条流)。关键判据:
// 真·用户输入才开新 run;ghost user(tool_result) + orphan-repair synthetic
// 归入当前 run。详见 isRealUserTurnStart 头注释 + 设计文档 §3.4。
describe("isRealUserTurnStart (interleaved-thinking run grouping)", () => {
  it("a real user-typed message starts a new run", () => {
    expect(isRealUserTurnStart({ id: "s-0", role: "user" })).toBe(true);
    expect(
      isRealUserTurnStart({ id: "s-0", role: "user", toolResults: [] }),
    ).toBe(true);
  });

  it("an assistant message never starts a run", () => {
    expect(isRealUserTurnStart({ id: "s-1", role: "assistant" })).toBe(false);
  });

  it("a ghost user(tool_result) message does NOT start a run", () => {
    // rehydrate merge step copies (not moves) toolResults onto the
    // preceding assistant, so the ghost user row still carries
    // toolResults — that's the discriminator.
    expect(
      isRealUserTurnStart({
        id: "s-2",
        role: "user",
        toolResults: [{ toolUseId: "tu_1", content: "x", isError: false }],
      }),
    ).toBe(false);
  });

  it("an orphan-repair synthetic message does NOT start a run", () => {
    // id suffix `-orphan-repair`, spliced in by rehydrate's orphan repair.
    expect(
      isRealUserTurnStart({ id: "s-1-orphan-repair", role: "user" }),
    ).toBe(false);
    // Even if it somehow carried toolResults, the suffix still wins.
    expect(
      isRealUserTurnStart({
        id: "s-1-orphan-repair",
        role: "user",
        toolResults: [{ toolUseId: "tu_1", content: "x", isError: true }],
      }),
    ).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// 2026-08-30 (task `08-30-shell-description` PR2 / design D1): header chip
// 数据源优先级矩阵 + shell 家族封闭名单。
// ---------------------------------------------------------------------------

describe("isShellFamilyTool", () => {
  it("accepts both builtin shell tools", () => {
    expect(isShellFamilyTool("shell")).toBe(true);
    expect(isShellFamilyTool("run_background_shell")).toBe(true);
  });

  it("rejects everything else (closed list)", () => {
    expect(isShellFamilyTool("read_file")).toBe(false);
    expect(isShellFamilyTool("shell_status")).toBe(false);
    expect(isShellFamilyTool("shell_kill")).toBe(false);
    expect(isShellFamilyTool("")).toBe(false);
  });
});

describe("toolHeaderChip (priority matrix)", () => {
  it("priority 1: input.path wins over everything (non-shell, current behavior)", () => {
    expect(
      toolHeaderChip("read_file", { path: "/repo/a.ts", content: "x" }),
    ).toBe("/repo/a.ts");
  });

  it("priority 1: path wins even when shell input carries a path-shaped key", () => {
    // shell 家族 input 无 path(后端 schema 不含),但防御性输入仍按
    // 优先级链走 —— path 检查在家族检查之前。
    expect(
      toolHeaderChip("shell", { path: "/weird", command: "ls" }),
    ).toBe("/weird");
  });

  it("priority 2: shell + string description → description", () => {
    expect(
      toolHeaderChip("shell", {
        command: "cargo test",
        description: "Run unit tests",
      }),
    ).toBe("Run unit tests");
    expect(
      toolHeaderChip("run_background_shell", {
        command: "pnpm build",
        description: "全量构建前端",
      }),
    ).toBe("全量构建前端");
  });

  it("priority 2 skips non-string / empty-string description (畸形按缺失)", () => {
    expect(
      toolHeaderChip("shell", { command: "ls", description: 12345 }),
    ).toBe("ls");
    expect(toolHeaderChip("shell", { command: "ls", description: "" })).toBe(
      "ls",
    );
    expect(
      toolHeaderChip("shell", { command: "ls", description: null }),
    ).toBe("ls");
  });

  it("priority 3: shell fallback → first non-empty command line", () => {
    expect(toolHeaderChip("shell", { command: "ls -la" })).toBe("ls -la");
    expect(
      toolHeaderChip("shell", { command: "find . -name '*.ts' -print0 \\\n  | xargs -0 wc -l" }),
    ).toBe("find . -name '*.ts' -print0 \\");
  });

  it("priority 3: blank lines are skipped, the line is trimmed", () => {
    expect(
      toolHeaderChip("shell", { command: "\n\n  cargo build --release  \necho done" }),
    ).toBe("cargo build --release");
  });

  it("priority 4: null when shell input has no usable command", () => {
    expect(toolHeaderChip("shell", {})).toBeNull();
    expect(toolHeaderChip("shell", { command: "" })).toBeNull();
    expect(toolHeaderChip("shell", { command: "  \n  " })).toBeNull();
    expect(toolHeaderChip("shell", { command: 12345 })).toBeNull();
    expect(toolHeaderChip("run_background_shell", {})).toBeNull();
  });

  it("priority 4: null for non-shell tools without path", () => {
    expect(toolHeaderChip("grep", { pattern: "x" })).toBeNull();
    expect(toolHeaderChip("dispatch_subagent", { task: "x" })).toBeNull();
  });

  it("safe on undefined / malformed input", () => {
    expect(toolHeaderChip("shell", undefined)).toBeNull();
    expect(toolHeaderChip("read_file", undefined)).toBeNull();
  });
});
