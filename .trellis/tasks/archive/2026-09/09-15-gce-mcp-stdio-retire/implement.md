# 执行清单(P4 stdio 壳退役)

顺序执行;每步后可独立验证。

1. [x] P3 前置已完成(挂载 HTTP-only + 备份)——本任务开工前复核过。
2. [x] 删 6 文件:`git rm scripts/group-chat-mcp{.mjs,.test.mjs,-smoke.mjs,-deploy.mjs,-deploy.test.mjs,-standalone-entry.mjs}`。
3. [x] `scripts/package.json`:zod 删;**SDK 保留**(http-smoke dynamic import SDK client——初版「依赖清零」判断误,grep 排除模式吃掉同前缀文件,见 research 勘误);npm 残留 package-lock.json 删,pnpm install 再生 lockfile。
4. [x] 机器面:`rm ~/.local/share/dev.everlasting.app/bin/everlasting-group-chat-mcp{,.build-info}`。
5. [x] `mcp.rs` 两处注释去 JS 源行号悬空引用(仅注释,行为零改动)。
6. [x] 文档:DAEMON-API.md §6.1(挂载换 HTTP / 删部署面段)+ §6.5(P3 翻转 + P4 记录);AGENTS.md 两块;roadmap §5 补记。
7. [x] spec:删 `.trellis/spec/scripts/group-chat-mcp-deploy.md` + index 重写;改 `group-chat-presets.md` / `pattern-group-chat-structured-summary.md` 引用。
8. [x] 验证:`node --test scripts/group-chat-run.test.mjs` 20/20 绿;`node scripts/group-chat-mcp-http-smoke.mjs` 全探针绿(预算 3609<4200);残留 grep 与 git status 见任务收尾记录。

回滚点:全部为 git 管理文件,`git checkout` 即回;机器面 bin/deploy 脚本仍在 git 历史。
