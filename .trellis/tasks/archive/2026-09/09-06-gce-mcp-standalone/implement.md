# Implement — 09-06-gce-mcp-standalone

> 轻量任务:PRD(D 决策)+ 本清单即全部。技术事实与坑见 research/bun-compile-feasibility.md(必读:CLI 壳误判根因 + argv[1] 哨兵写法)。

## 顺序清单

1. **部署入口** `scripts/group-chat-mcp-standalone-entry.mjs`
   - 无守卫、无条件启动的 stdio server 入口;**先** `process.argv[1] = '/nonexistent-everlasting-mcp-standalone'` 再动态 import(mcp.mjs / SDK);注释写明 bun compile import.meta.url 语义(根因)。
2. **部署器** `scripts/group-chat-mcp-deploy.mjs`(node stdlib 自足,零 npm 依赖)
   - 纯函数区(可单测):`mergeServersConfig(rawText, { serverName, command })` — 读-改-写 JSON,保序保其他键,幂等;`buildRevertEntry(repoScriptsDir)` — 产出 node 挂载体 `{ command:'node', args:[<scripts>/group-chat-mcp.mjs] }`。
   - CLI 面:`--revert`(回 node 挂载)/ `--uninstall`(删配置项 + 删 bin)/ 默认 = build+install+config。
   - 默认流程:调 `bun build --compile scripts/group-chat-mcp-standalone-entry.mjs --outfile <XDG bin 路径>`(bun 不可用时报错退出并提示安装命令)→ 写 sidecar `build-info`(git rev --short + ISO 时间)→ 写 D4 配置(写前备份 `config.json.mcp-deploy.bak` 单份覆盖)→ 打印后续冒烟命令。
   - 路径:XDG data 根 `~/.local/share/dev.everlasting.app/bin/`;config 默认 `~/.zcode/cli/config.json`,留 `--config` 覆盖。
3. **smoke 支持 bin** `scripts/group-chat-mcp-smoke.mjs`:加 `--bin <path>`(spawn 该 bin 替代 `node group-chat-mcp.mjs`),纯 additive,默认行为不变。
4. **单测** `scripts/group-chat-mcp-deploy.test.mjs`(node --test;vitest 不收 scripts/):merge 幂等/保其他键/缺文件建骨架、revert 体形状、uninstall 语义。≈5 用例。
5. **文档接线**:DAEMON-API.md §6.1 部署小节;GROUP-CHAT-API-ROADMAP.md §5 部署面条目落账;AGENTS.md 群聊 MCP 行补部署命令。

## 验证命令

```bash
node --test scripts/group-chat-mcp-deploy.test.mjs \
     scripts/group-chat-mcp.test.mjs scripts/group-chat-run.test.mjs   # 全绿
node scripts/group-chat-mcp-smoke.mjs                                  # node 直连回归不变
node scripts/group-chat-mcp-deploy.mjs                                 # AC1 一条命令
node scripts/group-chat-mcp-smoke.mjs --bin ~/.local/share/dev.everlasting.app/bin/everlasting-group-chat-mcp   # AC2
env -i HOME="$HOME" PATH=/usr/bin:/bin \
  bash -c 'printf "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-06-18\",\"capabilities\":{},\"clientInfo\":{\"name\":\"p\",\"version\":\"0\"}}}\n" | ~/.local/share/dev.everlasting.app/bin/everlasting-group-chat-mcp 2>/dev/null | head -1'   # 免 node 实证(回应含 serverInfo 即过)
git diff --stat app/src-tauri                                          # 必须为空(AC3)
```

## 回滚点

- 每步独立可回:`git checkout -- <file>`;配置写前有 `.mcp-deploy.bak`;`--revert`/`--uninstall` 是活回滚。
- 引擎与 MCP 实现零 diff(D5),无共享面回归风险。
