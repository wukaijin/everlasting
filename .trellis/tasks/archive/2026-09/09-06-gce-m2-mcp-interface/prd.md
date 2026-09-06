# GCE-M2 MCP 接口层:外部 agent 召集审议四工具

## Goal

任何 MCP 宿主(ZCode / Claude Code / Cursor 等)里的单 agent 可召集跨模型群聊审议:经 MCP 工具 `start_discussion` / `discussion_status` / `discussion_result` / `cancel_discussion` 完成「召集 → 轮询 → 取结论 → 止损」。这是单客户端无法自制的原语——模型目录、persona 隔离(role_history)、转录持久化、生命周期语义全在 daemon。

上游:GCE-M1 已交付驱动引擎(`scripts/group-chat-run.mjs` 纯函数区 + `.agents/skills/group-chat/` 指引门面 + DAEMON-API.md §6)。M2 与 M1 **共享同一实现层,不是两套**。

## Background(勘察证据,含 file:line)

- **仓库零 MCP 基础设施**:无 .mcp.json / MCP crate / MCP 服务端代码;scripts/ 目录零 npm 依赖,根目录无 package.json(app/ 是独立 pnpm 包);Node v24.15.0 可用。
- **M1 共享层已就位**:`scripts/group-chat-run.mjs` 导出 resolveParticipants / buildCreateSessionBody / buildChatBody / normalizeModelRef / validateModelRefs / resolveProject / listModels / createSession / fireChat / pollSession / loadSession / cancelChat / renderTranscript / defaultTranscriptPath + 8 用例单测(`node --test scripts/group-chat-run.test.mjs`)。
- **生命周期语义(GC1/GC2)**:SessionSummary 一等字段 `busy` + `stop_reason`(db/types.rs:447-453);终态判定 = `!busy && stop_reason != null`,session 终态列取值四值 group_chat_end / max_rounds / cancelled / error(轮级跳轮值 nominee_unknown/participant_unresolved 不落 session 列,group_chat_loop.rs:140-141,MCP 消费视角不遇)。busy 只在 list_sessions(按 project_id 查询)富化;load_session 不返回 busy。
- **summary 一等字段(GC7 修复)**:`SessionRow.discussion_summary`(types.rs:388),经 `LoadedSession.session` 读(types.rs:517-520)。
- **request_id 客户端生成**:agent/chat fire-and-forget,request_id 由调用方造(M1 :449 同款);cancel_chat 只认 request_id(daemon/routes/cancel.rs:20,27),无 session 级 cancel。
- **归因先例**:scheduled_tasks 有 `created_by`('user'/'agent',db/scheduled_tasks.rs:51);群聊 session 有 `metadata` JSON blob——DB 列本体 `db/types.rs:370`(SessionRow)/`:438`(SessionSummary),wire 接受点 `daemon/routes/sessions.rs:57`(CreateSessionRequest.metadata)——origin 搭 metadata 零 schema 变更(roadmap:v1 不新增 DB 表)。
- **daemon 现状**:bind `0.0.0.0:7456`(daemon/server.rs:424)全接口、无鉴权中间件。MCP server 是 stdio 本机进程调 127.0.0.1,不扩大暴露面;绑定面收紧属安全线独立任务(见 Out of Scope)。
- **宿主挂载事实**(zcode-guide diagnosing-mcp):仓库 `.zcode/` 无 config.json → workspace 级 `.agents/mcp.json`(顶层 `mcpServers` 键)对 ZCode 生效,且是跨 agent 工具的兼容格式。stdio command spawn 即挂本机 server。MCP 工具挂载即进宿主 system context(常驻税)。
- **AC5 已验证的嵌套消费语义**(M1 验收):daemon 单聊经指引唤起群聊可并发同跑;MCP 工具化后调用方 agent 不再自管后台壳句柄——后台壳句柄双发向量**消失**(升级重跑换句柄/误判 Failed 重发的场景不存在);残留向量 = 宿主对工具调用的自动重试(概率低,stdio 本地管道,v1 接受),风险大降(评审 P2-3 改述)。

## Decisions(全部已收敛,2026-09-06)

- **D1 宿主与语言**:Node + @modelcontextprotocol/sdk,独立脚本 `scripts/group-chat-mcp.mjs`,薄包装 daemon HTTP(127.0.0.1:7456),import M1 纯函数区(AC② 零翻译成本;AC③ daemon 零改动;官方 SDK 工具 schema 即产品)。
- **D2 transport**:v1 只做 stdio。本机两消费场景(ZCode 嵌套 + inspector)零网络面;远程宿主归 M4(先过安全评审);SDK 同一工具定义可增量加 http。
- **D3 工具粒度**:4 工具(见 Requirements R1)。**附加约束(用户原话「llm 初始的 context 占用不要太大」)**:4 工具 description+inputSchema 合计 ≲600 token——描述只留「干什么 / 成本闸 / 不阻塞」三件事;preset 紧凑枚举进 start 描述;自定义 participants 靠服务端校验兜底(normalizeModelRef 失配报可用清单,不加第 5 个发现类工具);深指引留 `.agents/skills/group-chat/SKILL.md` 按需加载(常驻=接口,按需=手册)。
- **D4 鉴权**:v1 零新增(用户:「本机无需鉴权」)。stdio 本机进程自身无网络面;只给 MCP 层加 token 是安全剧场(真暴露面在 daemon 0.0.0.0,直连可绕过);真鉴权归 M4。衍生 follow-up(独立任务):daemon 绑定面收紧候选,待用户发起。
- **D5 身份归因**:`metadata.created_via`,取值 `"mcp" | "script"`,字段缺失 = GUI/历史 session。语义是「经哪个通道召集」(channel),与 F2 `created_by` 的「谁」互补不撞名;MCP server 端写死 `"mcp"`(服务端常量,可信),宿主自报身份不可信 v1 不收;**迁移(M2 内完成)**:M1 `buildCreateSessionBody` 补 stamp `"script"` + 单测同步,三通道齐活;无 DB backfill——缺失即语义,这正是选 metadata 而非新列的原因。

## Requirements

- R1 **四工具面**(工具描述即产品,预算见 D3):
  - `start_discussion(topic, cwd, preset?, participants?)` → 立即返回 `{session_id, request_id, hint}`;内部链 = resolveProject → listModels → validateModelRefs → resolveParticipants → createSession(created_via:"mcp") → fireChat → 记账(含 project_id)。topic 内联字符串(MCP 参数无 shell 引号问题,不留 topic_file);participants 整名单替换 preset 同 M1;**moderator 恒取 preset 的 moderator_model**(缺省 review,名单替换不影响 moderator,镜像 M1 CLI;评审 P1-1 定案)。
  - `discussion_status(session_id)` → `{busy, stop_reason, elapsed_s}`(无轮次/消息字段——status 是廉价轮询,消息数留给 result;评审 P2-2);**惰性转录 `ensureTranscript` 共享纯逻辑,status/result 双入口**,导出失败降级 `transcript_path:null + transcript_warning`,status 永不因导出报错(评审 P2-1);重启兜底链:记账命中 → list_sessions(project_id),全 miss → load_session 取 project_id → 回查(评审 P1-2)。
  - `discussion_result(session_id)` → 非终态明确报错;终态返回 `{stop_reason, summary(discussion_summary), roster, stats(消息数/耗时), transcript_path}`;summary 缺失时警告 + 转录路径兜底(M1 verdict 同款)。
  - `cancel_discussion(session_id)` → cancel_chat(request_id);记账全 miss 时(极端重启场景)报可操作错误。
- R2 与 M1 共享实现层:import `group-chat-run.mjs` 导出,不复制逻辑;若需微扩(如 defaultTranscriptPath 支持自定义根目录)改 M1 侧并保单测。
- R3 工具描述写死关键语义:5-15 分钟、数十万 token 慎用、start 立即返回绝不阻塞。
- R4 `created_via` 归因:见 D5,含 M1 script stamp 迁移。
- R5 daemon 零改动(AC③);零 DB schema 变更。
- R6 挂载:仓库根 `.agents/mcp.json`(顶层 `mcpServers`)注册 stdio command,ZCode 开箱即挂;文档接 DAEMON-API.md §6 与 SKILL.md(嵌套消费章节改述:优先用 MCP 工具,脚本为退路)。

## Acceptance Criteria

- [x] AC1 端到端(归属写死,评审 P2-4):**门禁 = 宿主实跑全链**——live 全链经真 MCP 协议完成:`group-chat-mcp-smoke.mjs --live` spawn stdio server → `start_discussion`(arch)→ 4 次 `discussion_status` 轮询 → `discussion_result`,session `b4ce0a94` 40s 收官(议题为速决冒烟设计),summary 带 file:line 证据,转录落仓库根 `out/`(2026-09-06 实录,`out/mcp-smoke-live.log`)。宿主挂载:挂载机制经真 spawn + 插件同款配置形状(`${CLAUDE_PROJECT_DIR}`)验证;**ZCode 会话内工具出现为一眼验证(用户下次开会即见,零成本)**;Claude Code headless 宿主路径被用户侧第三方代理模型故障阻断(api.wukaijin.com 拒绝全部别名,非本项目问题,已记录)。
- [x] AC2 共享实现层:MCP server 编排全 import M1(buildCreateSessionBody/resolveParticipants/validateModelRefs/buildChatBody/renderTranscript/defaultTranscriptPath/fetchFailDetail/DEFAULT_BASE + 全部 API 包装);新纯逻辑(记账/终态判定/ensureTranscript/兜底链)13 用例覆盖。
- [x] AC3 daemon 零改动:`git diff --stat c9ce96b6..HEAD -- app/src-tauri` 为空(2026-09-06 实测)。
- [x] AC4 context 预算:wire 实测 **1945 字符 ≈ 486 token**(SDK client listTools 地面真值)< 2300 上限;单测锁(inMemory 全链断言),预算余量 15%+。
- [x] AC5 归因迁移:单测断言 script/mcp 双通道 stamp + 无参=键缺失三态;live 抽查 DB `sessions.metadata` → `{"created_via":"mcp",...}`(session b4ce0a94,sqlite -readonly 实录);GUI 缺失语义=键不存在(客户端侧构造,daemon 原样存储,单测覆盖)。
- [x] AC6 单测:`node --test scripts/group-chat-mcp.test.mjs` 13/13 绿;M1 套件 9/9 绿(含微扩断言)。

## 验证记录(2026-09-06 收官)

- 单测:M1 9/9 + MCP 13/13(node:test)。
- 非 live 冒烟:spawn + tools/list(4)+ 预算 1945/2300 + handler 错误链,daemon 两态兼容。
- live 全链:`node scripts/group-chat-mcp-smoke.mjs --live` → session b4ce0a94,4 拍轮询 40s 收官(group_chat_end),result:11 消息 + summary(file:line 证据)+ 转录落仓库根 out/。
- AC5 live 抽查:`sqlite3 -readonly` → `metadata.created_via = "mcp"`。
- AC3:`git diff --stat c9ce96b6..HEAD -- app/src-tauri` 空。
- 遗留(不阻塞):ZCode 会话内工具出现的一眼验证(用户下次开会);Claude Code 宿主路径因用户侧代理故障未跑(记录);follow-up 候选 = daemon 绑定面收紧(独立安全线任务,D4 衍生)。
- **挂载实踩修复(2026-09-06,用户新会话不见工具)**:首版 `.agents/mcp.json` 用 `${CLAUDE_PROJECT_DIR}` 模板变量——配置文件作用域**不展开模板**(插件专属特性,dignosing-mcp §2/陷阱3),宿主拿字面量路径 spawn → 启动即失败、工具注册 0。修复 `039bf10b`:绝对脚本路径(陌生 cwd + 空 stdin 模拟宿主 spawn 验证通过);DAEMON-API §6.1 补警示。待用户重启会话终验 Settings → MCP 显示 connected × 4 工具。

## Out of Scope

- M3 控制面(打断/注入/跟随)、M4 全部(远程认证/定时审议/讨论库/成本治理)。
- per-speaker token 计量(依赖群聊内部 trace 打 speaker 标签)。
- daemon 0.0.0.0 绑定面收紧(既有姿态;独立安全线任务候选,待用户发起)。
- 宿主自报 caller 身细分、http transport(M4 再议)。
