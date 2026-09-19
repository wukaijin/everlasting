// N9 bench 的 Node IO 薄层(任务 09-19-n9-perf-benchmark)。
// 与 render.bench.ts 分离的原因:app 不装 @types/node(实测它会经
// vitest/globals 的 reference 污染 vue-tsc 面,permissions.ts 类型
// 冲突),而 tsc 编译门要查 bench .ts 文件——所以 node:fs 用法全部
// 收敛到本 .mjs(运行时动态 import,不进类型面),声明见 io.d.mts。

import { readFileSync, mkdirSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const HERE = dirname(fileURLToPath(import.meta.url));

/** 读 gen-fixtures.mjs 生成的 LoadedSession wire 种子。 */
export function readFixture(n) {
  return JSON.parse(readFileSync(join(HERE, "fixtures", `session-${n}.json`), "utf8"));
}

/** 报告写 out/bench-fe/<ts>/report.json(out/ 不进 git)。 */
export function writeReport(report) {
  const outDir = join(
    HERE,
    "../../out/bench-fe",
    new Date().toISOString().replace(/[:.]/g, "-"),
  );
  mkdirSync(outDir, { recursive: true });
  const file = join(outDir, "report.json");
  writeFileSync(file, JSON.stringify(report, null, 2));
  return file;
}
