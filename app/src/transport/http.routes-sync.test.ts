// @vitest-environment node
// 结构性守卫(2026-08-25,F1 三条 queue 命令漏加映射第三次翻车后落地):
// 解析 daemon Rust 路由源码,断言每条 POST 路由在 httpTransport 的
// CMD_TO_DOMAIN 中有 domain 映射。此前同类遗漏:save_attachment(B1)、
// get_web_search_config(F4)、list/remove/recall_queued_messages(F1)——
// 全是"daemon 路由 + Tauri IPC 都通了,浏览器/sidecar/remote 模式打开
// 即断"的同一模式;Tauri IPC 不经过该表,单测侥幸测不出。
//
// 约定(routes/mod.rs):`POST /api/v1/{domain}/{cmd}`,cmd 名与 Tauri
// 命令 1:1。本测试只校验 `post(...)` 路由 —— GET 路由(附件下载
// `/:session_id/:file`、session 快照 `/:id/snapshot`、health、SSE
// `/api/v1/stream`)走 `<img>`/EventSource 直连,不进 CMD_TO_DOMAIN。
import { describe, it, expect } from "vitest";
import { CMD_TO_DOMAIN } from "./http";

// node:fs 等不能静态 import:主 tsconfig 的 types 白名单只有
// `vitest/globals`(刻意不装 @types/node,避免 node 全局类型漏进 app
// 代码),vue-tsc 会对静态 `import "node:fs"` 报 TS2307。非字面量
// specifier 的动态 import TS 按 any 放行;vitest 运行时是 Node,原生
// 可用。守卫测试不应倒逼 app tsconfig 引入 @types/node。
const fsSpecifier = "node:fs";
const pathSpecifier = "node:path";
const urlSpecifier = "node:url";
const { readFileSync } = await import(/* @vite-ignore */ fsSpecifier);
const { join, dirname } = await import(/* @vite-ignore */ pathSpecifier);
const { fileURLToPath } = await import(/* @vite-ignore */ urlSpecifier);

const ROUTES_DIR = join(
  dirname(fileURLToPath(import.meta.url)),
  "../../src-tauri/src/daemon/routes",
);

/** 从 routes/mod.rs 提取 nest("/api/v1/{domain}", {module}::router) 映射。 */
function parseNestMap(): Map<string, string> {
  const src = readFileSync(join(ROUTES_DIR, "mod.rs"), "utf8");
  const map = new Map<string, string>();
  const re = /\.nest\(\s*"\/api\/v1\/(\w+)"\s*,\s*(\w+)::router/g;
  for (const m of src.matchAll(re)) map.set(m[1], m[2]);
  // mod.rs 的 nest 覆盖所有 command domain —— 解析不到说明重构了路由
  // 组织方式,守卫需要跟上而不是静默放行。
  expect(map.size).toBeGreaterThanOrEqual(20);
  return map;
}

/** 提取某域文件里所有 `.route("/{cmd}", post(...))` 的命令名。 */
function parsePostedCmds(module: string): string[] {
  const src = readFileSync(join(ROUTES_DIR, `${module}.rs`), "utf8");
  return [...src.matchAll(/\.route\(\s*"\/(\w+)"\s*,\s*post\(/g)].map(
    (m) => m[1],
  );
}

describe("httpTransport CMD_TO_DOMAIN ↔ daemon routes sync", () => {
  it("every daemon POST route has a CMD_TO_DOMAIN entry with the right domain", () => {
    const nest = parseNestMap();
    const missing: string[] = [];
    const wrongDomain: string[] = [];
    for (const [domain, module] of nest) {
      for (const cmd of parsePostedCmds(module)) {
        const mapped = CMD_TO_DOMAIN[cmd];
        if (!mapped) missing.push(`${domain}/${cmd}`);
        else if (mapped !== domain)
          wrongDomain.push(`${domain}/${cmd} → mapped "${mapped}"`);
      }
    }
    expect(
      `missing CMD_TO_DOMAIN entries: ${missing.join(", ") || "(none)"}`,
    ).toBe("missing CMD_TO_DOMAIN entries: (none)");
    expect(
      `wrong domain mappings: ${wrongDomain.join(", ") || "(none)"}`,
    ).toBe("wrong domain mappings: (none)");
  });

  it("every CMD_TO_DOMAIN domain is a real daemon nest prefix (no typos)", () => {
    const nest = parseNestMap();
    const unknown = [...new Set(Object.values(CMD_TO_DOMAIN))].filter(
      (d) => !nest.has(d),
    );
    expect(`domains with no daemon router: ${unknown.join(", ") || "(none)"}`).toBe(
      "domains with no daemon router: (none)",
    );
  });
});
