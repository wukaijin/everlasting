# scripts 层 Spec

仓库 `scripts/` 工具脚本:node 直跑、零构建(vitest 不收此目录,单测走 `node --test`;npm 依赖只进 `scripts/package.json`)。

| 文件 | 主题 |
|---|---|
| [group-chat-mcp-deploy.md](./group-chat-mcp-deploy.md) | MCP server standalone bin 部署契约(双运行形态/配置写盘/错误矩阵)+ bun compile isMain 误判 gotcha(哨兵入口模式) |
