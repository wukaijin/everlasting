# PRD — GCE-MCP 部署面收口:单文件可执行 + 安装器写 user-scope 配置

> 来源:[GROUP-CHAT-API-ROADMAP.md §5「MCP 部署面」](../../../docs/GROUP-CHAT-API-ROADMAP.md) 路径②(推荐项,可独立小项提前做;M2 收官后用户指认的部署缺口)。任务定位:**轻量级**(纯 scripts/ 层 + 文档,Rust/前端零改动)。

## 背景与目标

M2/M3 的 MCP 六工具是**仓库产物**:`scripts/group-chat-mcp.mjs` + `scripts/node_modules` + 挂载配置里的源码检出绝对路径。只有 daemon bin、无源码的机器上 MCP 层为零(仅剩 M0 裸 HTTP 原语)——这是 D1 的有意识取舍,但构成「任何宿主」终态的部署缺口。

本任务把 MCP server 打成 **bun compile 单文件可执行**(内嵌运行时,免 node、免 node_modules、免源码),并提供**独立安装脚本**一条命令完成构建 → 安装 → user-scope 配置写入。

## 范围

**做**:部署入口文件 + 安装脚本(build/install/config-write/revert/uninstall)+ smoke 脚本支持对 bin 冒烟 + 配置合并纯函数单测 + 文档接线(DAEMON-API §6.1 / GCE-ROADMAP §5 / AGENTS.md)。

**不做**(记 follow-up):跨平台编译矩阵(macOS/Windows)、Tauri app 分发、其他宿主(Claude Code/Cursor)配置写入、bin 启动加速(--bytecode,与动态 import 兼容性未验)。

## 决策记录

| # | 决策 | 依据 |
|---|------|------|
| D1 | 工具链 **bun 1.3.6**(deno 未装;CLI `--define` 不支持下标键) | 环境勘察(见 research/bun-compile-feasibility.md) |
| D2 | 分发形态 **独立安装脚本**(用户 2026-09-06 拍板);Tauri 分发记 follow-up | 本项目无 release 流水线,单用户 dev 语境;路线图「独立小项提前做」 |
| D3 | bin 落点 `~/.local/share/dev.everlasting.app/bin/everlasting-group-chat-mcp`(XDG data 与 daemon/DB 同根;bin 走配置绝对路径,不依赖 PATH) | 与 app 现有 XDG 布局一致 |
| D4 | 配置写入 **ZCode user-scope** `~/.zcode/cli/config.json` 的 `mcp.servers."everlasting-group-chat"` **原位替换**为 `{ command: <bin 绝对路径> }`(当前唯一在用宿主)。写前留单份回滚备份;幂等。**勘误(2026-09-06 checker 实证)**:本行原文记「顶层键是 `servers`」有误——真实形状是嵌套 `mcp.servers`(本机 config.json 顶层键 = `mcp` + `plugins`),源误记来自 research;部署器双容器兼容、优先 `mcp.servers`(spec `.trellis/spec/scripts/group-chat-mcp-deploy.md` §3) | 当前挂载已是该形态(AGENTS.md:09-06 从仓库级迁用户级) |
| D5 | bun compile 下引擎 CLI 壳误判的解法 = **部署入口侧先改写 `process.argv[1]` 哨兵再动态 import**;`group-chat-run.mjs` / `group-chat-mcp.mjs` **一行不改**(守卫在 node 直连/测试下语义不变) | 探针实证:compile 下所有打包模块 import.meta.url = 可执行路径 → 守卫恒真 → usage + exit(0) 污染 stdout 协议通道 |
| D6 | 本机验证 = 真切换挂载到 bin(唯一诚实的端到端验收)→ ZCode 新会话一眼验六工具 → 用户裁定保持(dogfood)或 `--revert` 回 node 挂载(node 挂载保留为开发态默认:改 .mjs 无需重编译) | dev 环境改脚本频繁,重编译是开发税 |

## 验收标准

- **AC1 一条命令部署**:`node scripts/group-chat-mcp-deploy.mjs` = 构建 standalone bin → 装 D3 落点(附 build-info:git rev + 时间戳,诊断 stale bin)→ 原位替换 D4 配置(备份 + 幂等)。零 npm 新依赖(部署器 node stdlib 自足;bun 只是被调用的外部工具)。
- **AC2 bin 过冒烟 + 免 node 实证**:`node scripts/group-chat-mcp-smoke.mjs --bin <bin路径>` 非 live 全绿(spawn + tools/list + 预算 + 错误链);剔除 fnm node 的最小 PATH 下 spawn bin 仍可 handshake——自包含实证。既有 node 直连冒烟与全部单测(16+9)回归不变。
- **AC3 引擎零改动**:`group-chat-run.mjs` / `group-chat-mcp.mjs` git diff = 0;新增面 = 部署入口 + 部署器 + 其单测 + smoke 的 `--bin` 参数(additive)。
- **AC4 端到端挂载切换**:本机跑 AC1 后 ZCode 新会话六工具可见、工具可调(用户一键验证);`--revert` 能回 node 挂载,`--uninstall` 清配置项与 bin。
- **AC5 文档接线**:DAEMON-API §6.1 补部署小节(bin 形态/安装/回滚);GCE-ROADMAP §5「MCP 部署面」条目落账(路径② 落地,follow-up 收窄为跨平台矩阵 + Tauri 分发);AGENTS.md MCP 行补部署命令。

## 约束

- v1 只做宿主 linux-x64(交叉编译 flag 未验,记 follow-up);零鉴权前提不变(本机)。
- 记账 XDG state 文件(`~/.local/state/dev.everlasting.app/mcp-discussions.json`)与 node 直连共享——同机双形态记账互通,良性,不另做隔离。
- 配置文件可能被宿主进程并发写:部署器只做读-改-写(单用户 dev 语境可接受),写入前备份兜底。
