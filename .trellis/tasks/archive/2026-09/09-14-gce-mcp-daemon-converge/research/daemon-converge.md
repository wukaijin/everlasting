# MCP 能力收敛 daemon:架构映射与迁移调研

> 承接 [mcp-wire-protocol.md](./mcp-wire-protocol.md)(协议契约)与 09-14 子代理
> daemon 侧勘察(挂载点/原语/DB,证据行号引自该报告)。目标:daemon 在 :7456
> 原生暴露 MCP streamable-HTTP endpoint(`/mcp`),宿主挂载从 spawn 98MB bun bin
> 改为 url 型,子进程归零。

## 0. 动机:内存账(2026-09-14 实测)

| 项 | 现状 | 收敛后 |
|---|---|---|
| MCP 进程 | 每宿主会话 ×1(bun bin,RSS 58MB/PSS 40MB,第 2+ 实例增量 ~26MB) | **0** |
| daemon | 112MB | +数 MB(路由+schema 常量+serde,一次性) |
| 部署面 | 98MB bin + deploy 脚本 + standalone 冒烟 + bun 依赖 | 全链退役 |
| 乘数效应 | 会话数 × 58MB | 消失 |

能力上无损:JS 壳本就是 daemon HTTP API 的薄代理(8 工具零本地业务),
daemon 侧群聊原语齐全(GCE-M4a 定时任务已原生建群发题)。
roadmap 既定方向:docs/GROUP-CHAT-API-ROADMAP.md §5 path③。

## 1. daemon 现状要点(子代理勘察浓缩,行号见原文)

- **框架**:axum 0.7 + tower;两层路由(顶层 `build_router` server.rs:248-290
  + 域路由 `routes/mod.rs:106-153`);唯一 layer = CORS very_permissive(:289)。
- **绝对路径路由先例**:`stream.rs:87-91` 的 `/api/v1/stream` 走 merge 不走
  nest——`/mcp` 照抄此模式;SPA fallback 只兜未匹配路径,不吞显式路由。
- **鉴权**:全 API 零鉴权 + 0.0.0.0(DAEMON-API.md §8 用户裁定边界);`/mcp`
  继承同一前提,不新增暴露面(也不经 remote tunnel 前扩)。
- **SSE 先例**:`routes/stream.rs:52-69`(axum Sse + KeepAlive)可作 SSE 参考;
  但极简 profile 不需要 SSE(纯 JSON 响应 + GET 405)→ 不触发
  daemon-server.md 的 streaming endpoint shutdown 契约审查。
- **schema 机制**:无 schemars;手写 `json!` JSON Schema 是既有惯例
  (llm/types/chat.rs ToolDef:72-93;tools/end_discussion.rs 等先例)。
- **群聊原语全部现成**:open_new_group_discussion(scheduler/mod.rs:928-1162)、
  chat_inner 立即返回 ChatAcceptance(agent/chat.rs:344)、resume/preempt、
  group_chat_controls 注入缓冲(state.rs:147-149)、checkpoints、
  list_sessions_inner busy 富化(commands/sessions.rs:34-42)、
  group_chat_token_usage_inner(与 JS aggregateTokens 同口径 spec 条目)、
  group_chat_transcript.rs Rust 渲染器。
- **DB**:sqlx 0.8 WAL 池;sessions(project_id/current_cwd/created_at/metadata/
  stop_reason/discussion_summary/detail)、messages(seq/speaker)、
  group_chat_presets(builtin_key 覆盖行)、turn_trace。
- **模式**:`/mcp` 只落 daemon bin(Thin/standalone 全覆盖);Full 模式 GUI
  不起 axum,天然不带——与现状 stdio 壳独立于 GUI 一致。

## 2. 八工具语义映射(JS core* → daemon 原语)

| MCP 工具 | JS 实现(mcp.mjs) | daemon 侧落地 |
|---|---|---|
| start_discussion | coreStart :127-176 | 预设合并(D1)→ lookupPreset 三趟(key→精确名→忽略大小写)→ normalizeModelRef(名字/UUID 两趟,catalog 查询)→ resolveProject(路径→project,miss 则建)→ create_session_in_pool(session_type=group_chat, metadata{participants, created_via:'mcp', token_budget?})→ chat_inner(topic);回 session_id+hint |
| discussion_status | coreStatus :245-283 | busy = list_sessions_inner 富化;stop_reason/session 行直读;wait_seconds 长轮询 Rust 化(tokio interval 2s 拍,signal=`busy\|stop_reason\|消息数\|末seq`,到点前实查防边界空报);detail 富化(messages/last_speaker/tokens);终态观测触发惰性转录 |
| discussion_result | coreResult :297-347 | 终态 guard(运行中 → isError 工具错误)+ load_session + summary/detail(坏 JSON → null+warning 降级)+ roster disp(目录名解析)+ stats + group_chat_token_usage_inner(聚合失败整键省略)+ 惰性转录 |
| cancel_discussion | coreCancel :349-367 | rid 从 `state.session_active_request` 内存表取(D3;替代 XDG ledger);非 busy → already_finished 幂等语义 |
| interrupt_discussion | coreInterrupt :372-379 | preempt_group_chat_inner 1:1(routes/cancel.rs:44-56) |
| inject_message | coreInject :386-414 | 前置 busy guard(busy===true 才发起,防误重启编排器抹 summary)→ chat_inner → ChatAcceptance::Injected 判定;非 injected → cancel 自有 rid 止损 + 语义错误 |
| list_models | coreModels :504-515 | catalog 只读透传(id/name/model_name/provider) |
| list_presets | corePresets :521-537 | 内置四档(D1 编译期嵌入)⊕ group_chat_presets 表合并;degraded 键恒 false(daemon 即数据源,降级语义自然消失) |

**行为红线(勿降级)**:工具绝不阻塞(start 立即返,唯一有界例外
wait_seconds≤30);错误翻译两级(isToolError → `{error}`;daemon 链路错 →
`{error,hint}`);result 的 summary/detail/tokens 三处失败降级不炸主流程。

## 3. ledger 退役(XDG state 文件 → DB 派生)

JS 记账五要素全部可从 session 行派生,状态文件不再需要:

| 记账键 | 派生源 |
|---|---|
| request_id(cancel 用) | `session_active_request` 内存表(讨论活着就在;死了=无 running,cancel 无意义) |
| project_id(pollSession 用) | sessions.project_id 列 |
| cwd(转录落点根) | sessions.current_cwd |
| topic(转录命名) | 首条 user 消息 text |
| started_at_ms(elapsed) | sessions.created_at(与 fire 时刻差毫秒级,口径可接受) |

findSession 两级兜底链(记账→list→load)整体坍缩为一次 DB 读。
语义微调(D3):原「无记账的历史 session cancel」报工具错误 → 改为非 busy 即
`already_finished`(更贴近 cancel 幂等本义,不依赖跨进程记账)。

## 4. 设计决策点

- **D1 内置四档单一事实源**:`include_str!` 编译期嵌入
  `scripts/group-chat-presets.json`(含 persona_common/personas/presets 三节,
  persona kind → persona_md 展开逻辑随解析移植)+ serde 启动解析。单源保持
  (改 JSON 一处生效:M1 CLI 读文件、daemon 吃嵌入),免运行时文件依赖
  (standalone daemon 无 repo 检出也能跑)。JSON 内 moderator/participants
  引用是目录名,运行时经 normalizeModelRef 解析(重装换 UUID 不怕)。
- **D2 惰性转录渲染器**:复用 Rust `render_scheduled_transcript`
  (group_chat_transcript.rs:112+,带 conclusions/anchors 证据链,比 JS 版全),
  落点参数化:定时场 `{data}/discussions/` 不变,MCP 场落 `<cwd>/out/
  {date}-{slug}.md`(JS defaultTranscriptPath 规则:topic 前 40 字符 slug 化)。
  避免第三套渲染器;格式与 JS 版的轻微分叉(转录头部字段)接受,转录是
  人读产物非 wire 契约。幂等:已存在即返回(D2 落点规则本身可探测)。
- **D3 cancel rid 来源**:session_active_request;busy 判定前置。
- **D4 created_via**:保留 `'mcp'`(GUI/调度统计口径不破)。
- **D5 协议版本**:echo 策略(详见协议文档 §2)。
- **D6 mcp-protocol-version 头**:lenient 不校验只记日志(前向兼容)。
- **D7 AC4 预算锁 Rust 化**:tools/list 序列化(name+description+inputSchema
  合计)≤ 4200 字符单测锁,与 JS 31 用例的 AC4 同口径——宿主注入 LLM context
  的就是这份 wire schema,预算的地面真值。八工具 schema 从 JS buildToolShapes
  (mcp.mjs:429-457)逐字段平移(含描述文案,勿重写)。

## 5. 风险与边界

| 风险 | 评估 | 缓解 |
|---|---|---|
| 宿主 http 挂载未实测(连接管理/重连/懒连时序) | 中 | 阶段 2 双挂载并行(新名 `-http`)live 实测后才切;bin 保留回滚 |
| daemon-down = MCP 整体不可用 | 低(list_presets degraded 语义消失,但 daemon 不在群聊本就不可跑) | 工具错误文案已含「先确认 daemon 在跑」指引;宿主懒连接入下 daemon 后起也无妨 |
| 端口冲突/Thin sidecar 生命周期 | 与现有 API 同前提 | 无新增 |
| remote tunnel 把 /mcp 转发出去(catch-all proxy) | 暴露面扩张 | 本期不启用;/mcp 路由加注释标记「远程暴露须先过安全评审」(roadmap §5 前置) |
| Full 模式 GUI 无 /mcp | 逃生舱场景,现状 stdio 壳也独立于 GUI | 接受,文档注明 |
| wait_seconds 长轮询占 handler | 每 MCP 客户端 ≤1 个 30s 有界等待,tokio 异步无阻塞 | 与 JS 版 2s 拍语义一致 |

## 6. 迁移计划(四阶段)

1. **P1 实现**:daemon `routes/mcp.rs`(merge 式挂载,照抄 stream.rs 模式;
   不进 CMD_TO_DOMAIN,files 域先例)+ 协议层(initialize/ping/tools/通知/
   错误分派,D5/D6)+ 八工具映射 + D1 预设嵌入 + D2 转录复用。测试:协议层
   Router oneshot(group_chat_presets.rs:121-336 模板)+ 纯函数单测 + D7 预算锁。
2. **P2 验证**:新 `scripts/group-chat-mcp-http-smoke.mjs` 用 SDK
   StreamableHTTPClientTransport 直连 daemon /mcp(非 live 零成本:initialize/
   tools/list/预算/错误链;`--live` 烧 token 走 start→status→result 全链);
   宿主双挂载实测(临时名 `everlasting-group-chat-http`,本会话内验工具可见可用)。
3. **P3 切换**:user config 的 `everlasting-group-chat` 条目原位换成
   `{"type":"http","url":"http://127.0.0.1:7456/mcp"}`(**保同名保
   mcp__everlasting-group-chat__ 工具前缀**);stdio bin 保留一个观察期作回滚。
4. **P4 退役**(烧机一周后):group-chat-mcp-deploy.mjs / standalone 冒烟 /
   ~/.local bin 三件套退役;`group-chat-mcp.mjs` 及其 31 用例、stdio 冒烟随壳
   退役(M1 CLI `group-chat-run.mjs` 保留不动);文档回填:DAEMON-API.md 新 §、
   GROUP-CHAT-API-ROADMAP.md §5 path③ 勾销、AGENTS.md MCP 段改写、spec 增补。

## 7. 工作量估计

P1(协议层 + 八工具 + 预设/转录复用 + 测试)≈ 1.5-2 天当量;P2 ≈ 半天;
P3/P4 薄。主要不确定区:宿主 http 挂载实测(P2 才见分晓)与 D1 persona
展开逻辑的移植细度(JSON 三节结构,纯机械)。
