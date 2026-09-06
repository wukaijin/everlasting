# GCE-M2 实施计划

> 含评审(review.md P1-1/P1-2/P2-1/P2-2/P2-4 + 采纳的 P3)修订;驳回项:P3-1 行号判定(引用本就有效,仅消歧)、P3-5 jsonl gate(inline 工作流跳过,workflow.md:186/196-205)、P3-4 mcp.json 内注释(JSON 无注释语法,警示落文档)。

## 前置

- [ ] trellis-before-dev:读 `.trellis/spec/` 相关条目(无 scripts 层 spec 则以 M1 代码风格为准)
- [ ] `scripts/package.json`(type: module)+ 安装 `@modelcontextprotocol/sdk`(钉精确版本,提交 lockfile;pnpm 在 scripts/ 内执行)

## Step 1:M1 侧两个向后兼容微扩(先改先测)

- [ ] `buildCreateSessionBody` 增可选 `createdVia` 参数 → metadata.created_via;run 主流程传 `'script'`
- [ ] `defaultTranscriptPath(topic, rootDir = REPO_ROOT)` 支持显式根目录(默认不变,M1 行为不动)
- [ ] `scripts/group-chat-run.test.mjs` 补两处断言
- 验证:`node --test scripts/group-chat-run.test.mjs`

## Step 2:MCP server 核心(`scripts/group-chat-mcp.mjs`)

- [ ] 纯逻辑区(零 SDK import,可独立单测):
  - 记账模块:内存 Map + XDG state 文件(`~/.local/state/dev.everlasting.app/mcp-discussions.json`)tmp+rename 原子写;**schema 含 project_id**(design §3.2);内存 miss 读文件兜底
  - `isTerminal(session)` 终态判定(四值枚举;轮级跳轮值 nominee_unknown/participant_unresolved 不入白名单,design §3.3)
  - start 编排链:组合 M1 导出;moderator 恒取 `PRESETS[preset].moderator_model`(participants 只换名单,镜像 run.mjs:416);created_via:'mcp';requestId `gcmcp-<ts>-<rand>`
  - status 兜底链:记账命中 → list_sessions(project_id);全 miss → load_session 取 project_id → 回 list_sessions(design §3.2 写死链)
  - `ensureTranscript` 共享纯逻辑:幂等,status/result 双入口;导出失败降级 `transcript_path: null + transcript_warning`,status 永不因导出报错
  - result 组装:非终态报错;summary 缺失警告 + 转录兜底;stats = 消息数/耗时(**status 不含轮次/消息数字段**,design §3.3 裁决)
  - cancel:记账查 request_id,已终态幂等成功
- [ ] 工具定义区:4 工具 schema+description 按 design §2 草稿;**AC4 字符上限 N 实现后校准**(实测 token 定 N 留 ≥10% 余量,预期 ~2200-2300 字符),子字段 description 从简
- [ ] SDK 接线区:McpServer + StdioServerTransport;handler 薄壳调纯逻辑;daemon 未跑报可操作错误(复用 M1 api() 文案)
- 验证:`node --test scripts/group-chat-mcp.test.mjs`(AC2/AC4/AC6;含 participants-only moderator 语义档、记账含 project_id、ensureTranscript 双入口与降级、重启兜底链)

## Step 3:挂载 + 冒烟

- [ ] `.agents/mcp.json`(顶层 mcpServers,command: node scripts/group-chat-mcp.mjs;纯 JSON 无注释——fallback 约束警示落 DAEMON-API §6)
- [ ] `scripts/group-chat-mcp-smoke.mjs`:SDK Client over stdio spawn,连通 + tools/list 断言 4 工具 + 预算复检;`--live` 真跑 start→poll→result(headless 回归选项,**非门禁**)
- 验证:`node scripts/group-chat-mcp-smoke.mjs`(daemon 在跑)

## Step 4:文档接线

- [ ] DAEMON-API.md §6:MCP 入口小节(宿主 agent = MCP 工具优先;everlasting 内部 agent = 脚本 + M1 纪律;hosting 一行;`.agents/mcp.json` fallback 警示一行)
- [ ] `.agents/skills/group-chat/SKILL.md`:嵌套消费章节改述(MCP 工具优先 + 分工声明;双发警告改述「后台壳句柄向量消失,残留 = 宿主工具重试」)
- [ ] docs/GROUP-CHAT-API-ROADMAP.md:M2 状态行与交付记录(收官时)
- [ ] AGENTS.md:测试速查区补 group-chat-mcp 冒烟一行

## Step 5:验收

- [ ] AC1(归属写死,评审 P2-4):**门禁 = ZCode 宿主实跑全链**(挂载后新会话见 4 工具,经工具真跑 start→poll→result 一场小规模审议——宿主本身就是 MCP client,一次真跑同时验挂载+协议+全链);smoke 非 live = 廉价回归门;smoke --live 可选
- [ ] AC2/AC4/AC6:单测绿;AC4 的 N 值按校准流程锁定
- [ ] AC3:`git diff --stat app/src-tauri` 为空
- [ ] AC5(评审修订):单测断言 script/mcp 双通道 stamp;「GUI 无字段」改 headless 验法——daemon 侧无 metadata create_session 抽查 + 单测断言缺失语义(不依赖 GUI 环境)
- [ ] journal + roadmap 收官记录

## 风险点/回滚

- SDK API 变动 → 封装隔离,纯逻辑零 SDK import;出问题 revert 单 commit 即净(全增量文件 + M1 两个兼容微扩)
- ZCode 挂载不生效 → 查 diagnosing-mcp 陷阱 12(本仓库 .zcode/ 无 config.json,理论不触发;实测为准)
- 工作流:inline(Phase 2 走 trellis-before-dev);implement/check.jsonl 保持 seed 合法(workflow.md:186,sub-agent 平台专属 gate 不适用)

## 顺序依赖

Step 1 → Step 2(微扩是 MCP 侧 import 的签名);Step 2 → Step 3/4 可并行;Step 5 收口。
