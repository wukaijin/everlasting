# P4 波及面调研(2026-09-15,主会话 grep 实测)

## 依赖方向(删除安全性)

- `group-chat-run.mjs`(M1 引擎)**不** import `group-chat-mcp.mjs`;依赖单向:stdio 壳 →
  引擎。删壳不动引擎。
- 引擎 import 面:node 内建 + `./group-chat-presets.json`(json import)。零第三方依赖。
- `group-chat-mcp-http-smoke.mjs`:node 内建 only。
- ~~全 scripts/ 目录除被删 6 文件外,无任何 `zod` / `@modelcontextprotocol` import~~
  **勘误(实施中发现)**:初版 grep 的排除模式 `grep -v "group-chat-mcp"` 把
  `group-chat-mcp-http-smoke.mjs` 一并排除了——它 dynamic import SDK **client**
  (`client/index.js` + `streamableHttp.js`,L31-32),用真宿主 SDK 验 daemon wire,
  正是其设计目的。结论修正:SDK 依赖保留,zod 删除(全目录确认零消费);npm 残留
  package-lock.json 删,pnpm-lock 经 `pnpm install` 再生(lockfile 收敛单源)。
  **教训:排除式 grep 的模式会误伤同前缀文件,验证依赖面应用白名单(逐文件列 import)。**
- 仓库根无 package.json(只有 Cargo.toml);无 `.github/workflows/`(零 CI 引用)。

## 测试所有权(删 31 用例为何不丢覆盖)

- `group-chat-mcp.test.mjs` 31 用例全部针对 stdio 实现层:core* 编排组合、XDG 记账
  (HTTP 路径已退役该机制——rid 由 `session_active_request` 派生)、SDK InMemoryTransport
  接线、wire 预算锁(buildToolShapes)。该层行为所有权已移交 Rust `mcp.rs`(mod tests,
  09-14 平移)+ `group-chat-mcp-http-smoke.mjs`(非 live 探针 + --live 全链)。
- 引擎级覆盖(`lookupPreset` / `loadEffectivePresets` / 渲染节各臂)在
  `group-chat-run.test.mjs`(20 用例)独立存在,不受删除影响。
- `group-chat-mcp-deploy.test.mjs` 测 deploy 纯函数,随脚本一起退役。

## 引用清单(改动面)

| 位置 | 内容 | 动作 |
|---|---|---|
| `AGENTS.md` ~L60 | GCE-M2 块:stdio 壳为主入口、deploy/smoke/单测指引 | 重写:HTTP 唯一挂载 + 已退役 |
| `AGENTS.md` ~L61 | 收敛端点块:「挂载切换(P3)未做…P4 另立」 | 改:P3 ✅ 09-15 + P4 ✅ 本任务 |
| `docs/DAEMON-API.md` §6.1 L197-237 | stdio 挂载 JSON + 绝对路径警示 + 部署面段 | 挂载段换 HTTP;部署面段删;记账/冒烟行同步 |
| `docs/DAEMON-API.md` §6.5 L401-407 | 「stdio 壳冒烟不变」+「挂载切换(P3,未切)」 | 冒烟只留 http-smoke;P3 状态翻转 + P4 记录 |
| `docs/GROUP-CHAT-API-ROADMAP.md` L90 | 路径③ 段尾「P3…P4 另立」 | 补 P3 ✅(09-15,config 原位换 + 备份)+ P4 ✅ |
| `app/src-tauri/src/daemon/routes/mcp.rs` L15-17 | 「平移自 scripts/group-chat-mcp.mjs(该文件随 stdio 壳退役后本文即唯一实现…)」 | 微调为已完成时态,保留 research 指向 |
| 同上 L293-295 | 「TOOLS 表(scripts/group-chat-mcp.mjs:429-500)」行号引用 | 去行号悬空引用,语义自足 |
| `.trellis/spec/scripts/group-chat-mcp-deploy.md` | 部署契约全文 | 删(契约随产物退役) |
| `.trellis/spec/scripts/index.md` L7 | deploy spec 索引行 | 删行 |
| `.trellis/spec/backend/group-chat-presets.md` L13/L163/L213 | 引 mcp.test.mjs 覆盖 / buildToolShapes | 改指现存覆盖 |
| `.trellis/spec/backend/agent-loop-architecture/pattern-group-chat-structured-summary.md` L76 | 「+ group-chat-mcp.test.mjs」 | 删该引用 |

## 机器面(仓库外)

- bin:`~/.local/share/dev.everlasting.app/bin/everlasting-group-chat-mcp` + `.build-info`。
- 挂载:`~/.zcode/cli/config.json` 已 HTTP-only(P3);备份两份保留。
