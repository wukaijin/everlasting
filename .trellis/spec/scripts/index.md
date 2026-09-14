# scripts 层 Spec

仓库 `scripts/` 工具脚本:node 直跑、零构建(vitest 不收此目录,单测走 `node --test`)。依赖纪律:曾为 stdio MCP 壳装过 SDK+zod,2026-09-15 P4 退役后仅剩 SDK 供 `group-chat-mcp-http-smoke.mjs` 客户端用(zod 已删,lockfile 收敛 pnpm 单源);依赖只进 `scripts/package.json`,勿装到根。

暂无 spec 子文档。原 [group-chat-mcp-deploy.md](standalone bin 部署契约:双运行形态/配置写盘/错误矩阵/bun compile isMain 哨兵入口)随 stdio 壳 P4 退役删除——原文见 git 历史或任务 `09-15-gce-mcp-stdio-retire`;MCP 面现契约在 [docs/DAEMON-API.md §6.5](../../../docs/DAEMON-API.md)。
