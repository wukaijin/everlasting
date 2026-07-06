// Tests for `chat.ts` forced-dispatch helpers — B6+ B
// (task 07-06-b6plus-b-dispatch-model-arg).
//
// `parseForcedDispatchPrefix` + `resolveModelInput` are pure over
// `(trimmed, models)`, so they're unit-tested directly without
// mounting the pinia store. Covers:
//   1. `@@` prefix parses name + task (no model flag).
//   2. `--model=<id>` flag in the flag position → model_id = id.
//   3. `--model=<display_name>` flag → reverse-resolved to id.
//   4. `--model=` flag in the task body (NOT flag position) →
//      not extracted; stays in the task text.
//   5. `--model=` with no value / unknown name → model_id omitted
//      (dispatch degrades to agent default).
//   6. Duplicate display_name → first match wins + console.warn.
//   7. No `@@` prefix → forcedDispatch undefined, body = trimmed.
//   8. Empty task after `@@` prefix → null (caller aborts send).

import { describe, it, expect, vi, beforeEach } from "vitest";
import {
  resolveModelInput,
  parseForcedDispatchPrefix,
  type ForcedDispatchPayload,
} from "./chat";
import type { ModelWithProvider } from "./models";

/** Build a minimal ModelWithProvider for tests (only the fields
 *  `resolveModelInput` reads: id + displayName). */
function model(id: string, displayName: string): ModelWithProvider {
  return {
    id,
    providerId: "p",
    modelName: displayName,
    displayName,
    maxTokens: null,
    thinkingEffort: null,
    supportsThinking: false,
    contextWindow: 128000,
    createdAt: "",
    updatedAt: "",
    providerDisplayName: "P",
    providerProtocol: "openai",
  };
}

describe("resolveModelInput", () => {
  beforeEach(() => {
    vi.spyOn(console, "warn").mockImplementation(() => {});
  });

  it("exact id is passed through", () => {
    const models = [model("uuid-1", "GPT-4o"), model("uuid-2", "Claude")];
    expect(resolveModelInput("uuid-1", models)).toBe("uuid-1");
  });

  it("display_name reverse-resolves to id", () => {
    const models = [model("uuid-1", "GPT-4o"), model("uuid-2", "Claude")];
    expect(resolveModelInput("GPT-4o", models)).toBe("uuid-1");
  });

  it("duplicate display_name takes the first match", () => {
    const models = [model("uuid-1", "Dup"), model("uuid-2", "Dup")];
    expect(resolveModelInput("Dup", models)).toBe("uuid-1");
    expect(console.warn).toHaveBeenCalled();
  });

  it("unknown name returns undefined + warns", () => {
    const models = [model("uuid-1", "Real")];
    expect(resolveModelInput("ghost", models)).toBeUndefined();
    expect(console.warn).toHaveBeenCalled();
  });

  it("empty input returns undefined (no warn)", () => {
    const models = [model("uuid-1", "Real")];
    expect(resolveModelInput("", models)).toBeUndefined();
    expect(resolveModelInput("   ", models)).toBeUndefined();
    expect(console.warn).not.toHaveBeenCalled();
  });
});

describe("parseForcedDispatchPrefix", () => {
  beforeEach(() => {
    vi.spyOn(console, "warn").mockImplementation(() => {});
  });

  const MODELS = [model("uuid-gpt", "GPT-4o"), model("uuid-claude", "Claude")];

  it("no @@ prefix → undefined forcedDispatch, body = trimmed", () => {
    const got = parseForcedDispatchPrefix("just a normal message", MODELS);
    expect(got).toEqual({ forcedDispatch: undefined, body: "just a normal message" });
  });

  it("@@agent <task> with no model flag omits model_id", () => {
    const got = parseForcedDispatchPrefix("@@reviewer 审查这段代码", MODELS);
    expect(got).not.toBeNull();
    expect((got as { forcedDispatch: ForcedDispatchPayload }).forcedDispatch).toEqual({
      subagent: "reviewer",
      task: "审查这段代码",
    });
    // body = task (the prefix is stripped).
    expect((got as { body: string }).body).toBe("审查这段代码");
  });

  it("@@agent --model=<id> <task> threads model_id", () => {
    const got = parseForcedDispatchPrefix(
      "@@reviewer --model=uuid-gpt 审查这段代码",
      MODELS,
    );
    expect((got as { forcedDispatch: ForcedDispatchPayload }).forcedDispatch).toEqual({
      subagent: "reviewer",
      task: "审查这段代码",
      model_id: "uuid-gpt",
    });
  });

  it("@@agent --model=<display_name> <task> reverse-resolves to id", () => {
    const got = parseForcedDispatchPrefix(
      "@@reviewer --model=GPT-4o 审查这段代码",
      MODELS,
    );
    expect((got as { forcedDispatch: ForcedDispatchPayload }).forcedDispatch).toEqual({
      subagent: "reviewer",
      task: "审查这段代码",
      model_id: "uuid-gpt",
    });
  });

  it("--model= flag in the task body (not flag position) is NOT extracted", () => {
    // The flag must sit BETWEEN agent name and task. Here it appears
    // after the task text, so the whole thing is the task.
    const got = parseForcedDispatchPrefix(
      "@@reviewer 帮我看 --model=foo 的解析",
      MODELS,
    );
    expect((got as { forcedDispatch: ForcedDispatchPayload }).forcedDispatch).toEqual({
      subagent: "reviewer",
      task: "帮我看 --model=foo 的解析",
    });
  });

  it("--model= with unknown name omits model_id (warns, no crash)", () => {
    const got = parseForcedDispatchPrefix(
      "@@reviewer --model=ghost 审查这段代码",
      MODELS,
    );
    expect((got as { forcedDispatch: ForcedDispatchPayload }).forcedDispatch).toEqual({
      subagent: "reviewer",
      task: "审查这段代码",
      // model_id omitted: the reverse-lookup missed.
    });
    expect(console.warn).toHaveBeenCalled();
  });

  it("@@ prefix with no task separator is treated as a normal message", () => {
    // `@@reviewer` with no whitespace + task does not match the
    // forced-dispatch regex (`[ \t]+([\s\S]+)` requires a task), so
    // it's returned as a normal message — the caller sends it as-is.
    const got = parseForcedDispatchPrefix("@@reviewer", MODELS);
    expect(got).toEqual({
      forcedDispatch: undefined,
      body: "@@reviewer",
    });
  });

  it("@@ prefix with whitespace but empty task returns null (caller aborts)", () => {
    // `@@reviewer   ` — the regex matches (whitespace + task chars),
    // but the task trims to empty → null (the caller aborts the send,
    // matching the original `if (!task) return;` guard).
    expect(parseForcedDispatchPrefix("@@reviewer   ", MODELS)).toBeNull();
  });

  it("multi-line task after the prefix is preserved", () => {
    const got = parseForcedDispatchPrefix(
      "@@reviewer --model=GPT-4o line1\nline2\nline3",
      MODELS,
    );
    expect((got as { forcedDispatch: ForcedDispatchPayload }).forcedDispatch).toEqual({
      subagent: "reviewer",
      task: "line1\nline2\nline3",
      model_id: "uuid-gpt",
    });
  });
});
