#!/usr/bin/env node
// group-chat-mcp-standalone-entry.mjs — bun compile 专用入口(部署面,任务 09-06-gce-mcp-standalone)。
//
// 为什么不能直接拿 group-chat-mcp.mjs 当 entry:bun compile 下打包模块的
// import.meta.url 一律指向可执行文件自身(内嵌 bunfs 抹平模块身份),
// mcp.mjs / run.mjs 的 isMain 守卫(process.argv[1] === 自身路径)恒真 →
// 引擎 CLI 壳以空参数跑,usage 打上 stdout(MCP 协议通道)并 exit(0)。
// 解法 = 本入口先改写 process.argv[1] 为哨兵路径再动态 import,守卫恒不等,
// 引擎一行不改(探针实证,见任务 research/bun-compile-feasibility.md)。
// 顺序不可倒:静态 import 会提升到代码之前,哨兵必须赶在求值前生效。
//
// 无 isMain 守卫:本文件唯一存在形式是编译产物 main,无条件启动 stdio
// server(与 mcp.mjs 的 main 块同构);stdout 是协议通道,诊断只走 stderr。

process.argv[1] = '/nonexistent-everlasting-mcp-standalone'; // 哨兵:必在动态 import 之前

const { createServer } = await import('./group-chat-mcp.mjs');
const { DEFAULT_BASE } = await import('./group-chat-run.mjs');
const { McpServer } = await import('@modelcontextprotocol/sdk/server/mcp.js');
const { StdioServerTransport } = await import('@modelcontextprotocol/sdk/server/stdio.js');

const server = await createServer({ server: new McpServer({ name: 'everlasting-group-chat', version: '1.0.0' }) });
await server.connect(new StdioServerTransport());
process.stderr.write(`[group-chat-mcp] stdio server up (standalone bin, base=${process.env.EVERLASTING_BASE || DEFAULT_BASE})\n`);
