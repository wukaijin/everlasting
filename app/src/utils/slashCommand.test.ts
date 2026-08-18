// slashCommand.test.ts — builtin slash-command input matching.
// 08-18-manual-compact-command AC5: the submit-path interception only
// fires on an exact builtin-name first token; everything else (unknown
// commands, escapes, plain text) must pass through to the LLM unchanged.
import { describe, expect, it } from "vitest";

import { BUILTIN_COMMAND_NAMES, matchBuiltinCommandInput } from "./slashCommand";

describe("matchBuiltinCommandInput", () => {
  it("matches every builtin name without args", () => {
    for (const name of BUILTIN_COMMAND_NAMES) {
      expect(matchBuiltinCommandInput(`/${name}`)).toEqual({ name, rest: "" });
    }
  });

  it("captures rest-of-line as focus text (whitespace-collapsed ends)", () => {
    expect(matchBuiltinCommandInput("/compact 聚焦 API 变更")).toEqual({
      name: "compact",
      rest: "聚焦 API 变更",
    });
    expect(matchBuiltinCommandInput("/compact   spaced   ")).toEqual({
      name: "compact",
      rest: "spaced",
    });
  });

  it("tolerates leading whitespace before the slash", () => {
    expect(matchBuiltinCommandInput("  /clear")).toEqual({ name: "clear", rest: "" });
  });

  it("does NOT prefix-match unknown slash tokens", () => {
    expect(matchBuiltinCommandInput("/comp")).toBeNull();
    expect(matchBuiltinCommandInput("/compactx")).toBeNull();
    expect(matchBuiltinCommandInput("/unknown args")).toBeNull();
  });

  it("passes through escaped slashes and plain text", () => {
    expect(matchBuiltinCommandInput("//compact")).toBeNull();
    expect(matchBuiltinCommandInput("use /clear in your answer")).toBeNull();
  });

  it("never matches empty or whitespace-only input", () => {
    expect(matchBuiltinCommandInput("")).toBeNull();
    expect(matchBuiltinCommandInput("   ")).toBeNull();
    expect(matchBuiltinCommandInput("/")).toBeNull();
    expect(matchBuiltinCommandInput("/ ")).toBeNull();
  });

  it("honors a custom name list (injection point for tests)", () => {
    expect(matchBuiltinCommandInput("/custom", ["custom"])).toEqual({
      name: "custom",
      rest: "",
    });
    expect(matchBuiltinCommandInput("/clear", ["custom"])).toBeNull();
  });
});
