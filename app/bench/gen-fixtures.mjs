#!/usr/bin/env node
// N9 F1 种子生成器(任务 09-19-n9-perf-benchmark,design §5/§6)。
//
// 读后端种子 profile 单一出处(../src-tauri/benches/profile.json,
// 与 B2 seed_session 同读),生成 LoadedSession wire 形态的
// fixtures/session-{n}.json——形态出处 = e2e/tool-card-compact.spec.ts
// 的 seededSession()(MessageRow wire + tool_result 的 {result, cwd}
// 信封)。确定性伪语料,与 benches/support.rs 的 filler 同思路。
//
// 用法:node bench/gen-fixtures.mjs   (默认 100/1000/10000 三档)

import { readFileSync, writeFileSync, mkdirSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const HERE = dirname(fileURLToPath(import.meta.url));
const PROFILE = JSON.parse(
  readFileSync(join(HERE, "../src-tauri/benches/profile.json"), "utf8"),
);
const OUT_DIR = join(HERE, "fixtures");
const TIERS = [100, 1000, 10000];
const SESSION_ID = "e2e-session-1";

function filler(len, seed) {
  let s = "";
  let x = (BigInt(seed) * 6364136223846793005n + 1442695040888963407n) & 0xffffffffffffffffn;
  while (s.length < len) {
    x = (x * 6364136223846793005n + 1442695040888963407n) & 0xffffffffffffffffn;
    s += (x >> 33n).toString(16).padStart(8, "0") + " ";
  }
  return s.slice(0, len);
}

const r01 = (i) => Number((BigInt(i) * 2654435761n % 10000n)) / 10000;
const longPick = (i) => Number((BigInt(i) * 40503n % 10000n)) / 10000 < PROFILE.long_text_share;

function toolResultEnvelope(id, i) {
  return {
    type: "tool_result",
    tool_use_id: id,
    content: JSON.stringify({
      result: filler(PROFILE.tool_result_len, BigInt(i) * 23n + 13n),
      cwd: "/home/e2e/e2e-project",
    }),
    is_error: false,
    duration_ms: 12,
  };
}

function messageRow(i) {
  let role, content, text;
  if (i === 0) {
    role = "user";
    text = filler(60, 1n);
    content = [{ type: "text", text }];
  } else if (i % 2 === 1) {
    role = "assistant";
    const r = r01(i);
    const len = longPick(i) ? PROFILE.text_len_long : PROFILE.text_len_short;
    const body = filler(len, BigInt(i) * 13n + 7n);
    if (r < PROFILE.thinking_share) {
      content = [
        { type: "thinking", thinking: filler(400, BigInt(i) * 7n + 3n), signature: filler(80, BigInt(i) * 11n + 5n) },
        { type: "text", text: body },
      ];
      text = body;
    } else if (r < PROFILE.thinking_share + PROFILE.tool_pair_share) {
      const id = `tu_seed_${i}`;
      content = [
        { type: "text", text: filler(80, BigInt(i) * 17n + 9n) },
        { type: "tool_use", id, name: "read_file", input: { path: "src/main.rs" } },
      ];
      text = "";
    } else {
      content = [{ type: "text", text: body }];
      text = body;
    }
  } else {
    role = "user";
    const prevR = r01(i - 1);
    if (prevR >= PROFILE.thinking_share && prevR < PROFILE.thinking_share + PROFILE.tool_pair_share) {
      content = [toolResultEnvelope(`tu_seed_${i - 1}`, i)];
      text = "";
    } else {
      text = filler(PROFILE.text_len_short, BigInt(i) * 29n + 15n);
      content = [{ type: "text", text }];
    }
  }
  return {
    id: i,
    session_id: SESSION_ID,
    role,
    content,
    text,
    has_tool_calls: role === "assistant" && content.some((b) => b.type === "tool_use"),
    has_tool_results: content.some((b) => b.type === "tool_result"),
    created_at: "2026-01-01T00:00:00Z",
    seq: i,
    ttfb_ms: null,
    gen_ms: null,
    total_ms: null,
    thinking_ms: null,
  };
}

function loadedSession(n) {
  return {
    session: {
      id: SESSION_ID,
      title: `bench ${n}`,
      created_at: "2026-01-01T00:00:00Z",
      updated_at: "2026-01-01T00:00:00Z",
      model: "",
      project_id: "e2e-project",
      current_cwd: "/home/e2e/e2e-project",
      worktree_state: "none",
      worktree_path: null,
      last_worktree_path: null,
      model_id: null,
      input_tokens_total: null,
      output_tokens_total: null,
      cache_creation_total: null,
      cache_read_total: null,
      session_type: "chat",
      metadata: null,
    },
    messages: Array.from({ length: n }, (_, i) => messageRow(i)),
  };
}

mkdirSync(OUT_DIR, { recursive: true });
for (const n of TIERS) {
  const file = join(OUT_DIR, `session-${n}.json`);
  writeFileSync(file, JSON.stringify(loadedSession(n)));
  console.log(`wrote ${file} (${n} messages)`);
}
