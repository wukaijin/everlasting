# Design:MCP 能力收敛 daemon(/mcp endpoint)

> 承接 research/(协议契约 + 架构映射)。本文定实现边界与结构;语义映射表
> 不重复,见 research/daemon-converge.md §2/§3/§4。

## 0. 变更边界(Trellis before-dev step 7)

- **行为缺口**:宿主现在经 stdio spawn bun bin 消费 8 个群聊 MCP 工具
  (bin 内转发 daemon HTTP API);daemon 自身无 MCP 协议入口。目标是 daemon
  在 `POST /mcp` 上原生讲 MCP streamable-HTTP(极简 profile),行为面与
  JS 壳 1:1(工具语义/错误翻译/降级路径),宿主挂载换 url 型。
- **行为实际落点**:协议层 = 新 route module;工具语义 = daemon 现有
  `*_inner` 原语(Q0 单源,零业务复制);预设合并/模型解析/transcript 落点
  三个纯逻辑块在 mcp 模块内实现(对齐 JS 语义)。
- **改动文件**:
  - 新增 `app/src-tauri/src/daemon/routes/mcp.rs`(协议层 + 工具编排 +
    内置预设嵌入 + MCP 转录导出 + 单测;预估 ~1300 行,对齐仓库大文件惯例
    group_chat_loop.rs / scheduler/mod.rs)
  - 改 `app/src-tauri/src/daemon/routes/mod.rs`(挂载 + 模块注释)
  - 改 `docs/DAEMON-API.md`(新 §)、`docs/GROUP-CHAT-API-ROADMAP.md`(path③ 回填)、
    根 `AGENTS.md`(MCP 段)、`.trellis/spec/backend/daemon-server.md`(路由表)
  - 新增 `scripts/group-chat-mcp-http-smoke.mjs`(P2,SDK 客户端冒烟)
- **明确不做**:remote tunnel 远程暴露 /mcp(安全评审前置);SSE 响应/GET 流
  (极简 profile 不需要);Full 模式 GUI 带 /mcp(不起 axum);退役 stdio 壳
  (P3 切换 + 烧机后另起任务);M1 CLI 任何改动。

## 1. 协议层(routes/mcp.rs)

**挂载**:照抄 `stream.rs:87-91` 绝对路径 merge 模式,`routes/mod.rs` 加
`.merge(mcp::router(state.clone()))`;不进 CMD_TO_DOMAIN(files 先例,
绕开 http.routes-sync 守卫)。路由顶注释标记:远程暴露须先过安全评审
(roadmap §5)。

**端点矩阵**(契约 = research/mcp-wire-protocol.md §4):

| 请求 | 响应 |
|---|---|
| POST /mcp(Accept 含 json+sse,CT json,body 单条或数组) | 请求→200+JSON-RPC result;纯通知→202 空;校验失败→406/415/-32700 |
| GET /mcp | 405(JSON-RPC error body,无服务端主动流) |
| DELETE /mcp | 200 空(无 session,no-op) |

- 无状态:不分配/校验 `mcp-session-id`;`mcp-protocol-version` 头 lenient
  (不校验只 debug 日志)。
- **版本协商**:请求版本 ∈ {2024-10-07, 2024-11-05, 2025-03-26,
  2025-06-18, 2025-11-25} → echo;否则回 `2025-03-26`。
- initialize 结果:`{protocolVersion, capabilities:{tools:{}},
  serverInfo:{name:"everlasting-group-chat", version:"1.0.0"}}`。
- 方法分派:`initialize`/`ping`(→`{}`)/`tools/list`/`tools/call`;未知
  方法 → `-32601`;参数缺 name → `-32602`。
- 批处理:body 是数组 → 逐条处理;含请求 → 200 数组响应;纯通知 → 202。
- 错误体:传输级(406/415/405)带 JSON-RPC error body `{code:-32000/-32700,
  message}`;工具执行错误**不走** JSON-RPC error——200 + `isError:true`
  text result(JS 壳 errorResult 同款)。
- 工具结果统一:`{content:[{type:"text",text: pretty-JSON}], isError?}`;
  语义错误 `{error}`;infra 错误 `{error, hint:"daemon 调用链问题…"}`——
  mcp.rs 内 `ToolError{Semantic,Infra}` 两态承载。

## 2. 工具层(8 工具)

语义映射表见 research/daemon-converge.md §2,此处只记实现要点:

- **start_discussion**:预设合并 = 内置(embed,见 §3)⊕ `db::list_group_chat_presets`
  (merge 规则:覆盖行原位顶替/用户行追加 key=id/脏 builtinKey 跳过);
  lookup 三趟(key→display_name 精确→忽略大小写,歧义报错);
  `normalize_model_ref`(id→modelName/displayName 精确→忽略大小写,miss 报
  可用清单)——JS run.mjs:335-349 同构;participants 覆盖名单(moderator 恒取
  preset);项目解析 = list_projects+list_hidden_projects 物理路径比对,miss
  create;`create_session_in_pool(session_type=group_chat, metadata=
  {participants, created_via:'mcp', token_budget?})`;发题走 `chat_inner` +
  **HttpSseSink**(与 routes/agent.rs chat 完全同款——JS 壳现状就是打这条
  HTTP 路径,不用 scheduler 的 semi_sink);request_id `gcmcp-{uuid}`。
- **discussion_status**:`busy = state.session_active_request.contains_key`
  (list_sessions_inner 富化的同一真相源);`wait_seconds` 1..=30 有界长轮询
  (tokio 2s 拍,signal=`busy|stop_reason|消息数|末seq`,每拍先实查再判超时);
  `detail` 富化 messages/last_speaker/tokens;终态首次观测触发惰性转录(§4)。
- **discussion_result**:终态 guard(运行中→语义错误);detail 坏 JSON→null+
  warning;roster 用目录名解析;tokens 用 `group_chat_token_usage_inner`
  (失败整键省略);summary 缺失→summary_warning。
- **cancel_discussion**:busy→`session_active_request` 取 rid→`cancel_chat_inner`;
  非 busy→`{already_finished, stop_reason}`(D3:替代 JS 的 XDG ledger rid)。
- **interrupt_discussion**:`preempt_group_chat_inner` 1:1,无讨论→语义错误。
- **inject_message**:前置 busy guard(busy!=true 拒,防误重启编排器);
  `chat_inner` 后 acceptance 非 `Injected` → cancel 自有 rid 止损 + 语义错误。
- **list_models / list_presets**:`db::list_models` 透传;presets 合并视图
  (degraded 恒 false——daemon 即数据源,键保留以稳宿主 prompt 惯性)。

## 3. 内置预设嵌入(D1)

`include_str!("../../../../scripts/group-chat-presets.json")`(routes/mcp.rs
相对仓库根的路径,build 时嵌入)+ 启动惰性解析(OnceLock):
`persona kind → persona_md = personas[kind] + "\n\n" + persona_common`
(composePersonaMd 同构)→ `{description, moderator_model, participants}`
(composePresets 同构)。**单一事实源保持**:改 JSON 一处生效(M1 CLI 读文件,
daemon 吃嵌入需重编译——注释注明该约束)。解析失败 fail-loud(daemon 启动后
首次 MCP 调用时报 infra 错),不做降级。

## 4. MCP 惰性转录(D2)

复用 `render_scheduled_transcript`(group_chat_transcript.rs:112),新私有
helper `export_mcp_transcript`:
- 落点:`{session.current_cwd}/out/group-chat-{slug}-{ts}.md`,slug = topic
  前 40 字符非字母数字串折叠为 `-` + trim + lowercase(JS defaultTranscriptPath
  同构),ts = `YYYYMMDDhhmmss`;
- TranscriptRenderArgs:task_name=topic(截断同 slug 规则)、
  moderator/participants 直读 metadata(model 显示名不解析——与 scheduled
  导出行为一致,接受与 JS 版的分叉);幂等:落点含秒级 ts,不猜旧文件,
  重复导出 = 新文件(JS 版靠 ledger 记账幂等;收敛后 elapsed/记账从 session
  行派生,重复导出仅发生在用户反复调 result,成本一个 markdown 文件,接受);
- 失败降级 `{transcript_path:null, transcript_warning}`(P2-1:status 永不
  因导出报错)。

## 5. 测试计划

1. **协议层 oneshot**(group_chat_presets.rs:121-336 模板,真实
   `AppState::load_from_dir(tempdir)`):initialize echo 五值/未知版本回退/
   通知 202/ping/未知方法 -32601/Accept 缺失 406/CT 错 415/坏 JSON -32700/
   GET 405/DELETE 200/批处理数组。
2. **纯函数单测**:预设 compose/merge/lookup 三趟/normalize_model_ref/
   slug 落点/wait signal。
3. **AC4 预算锁**(D7):tools/list 序列化(name+description+inputSchema)
   ≤ 4200 字符断言——wire 地面真值,JS AC4 同口径。
4. **工具链 oneshot**:start→status(busy 路径 mock 不进 LLM:仅测参数
   校验/预设 miss/模型 miss 错误文案;真跑链路归 P2 --live)。
5. **P2 冒烟**:`scripts/group-chat-mcp-http-smoke.mjs` 用 SDK
   StreamableHTTPClientTransport 直连(非 live:initialize/tools/list/预算/
   未知工具/GET 405;--live:start→status wait→result 全链 + 内存验证
   `AC5`:daemon RSS 增量)。

## 6. 风险与回退

- 宿主 http 挂载实测(P2 双挂载)前不切配置;bin/stdio 壳全保留 = 即时回退。
- `/mcp` 继承零鉴权边界(DAEMON-API.md §8);tunnel catch-all 理论可达,
  路由注释 + 本期不启用。
- wait_seconds 最长占 handler 30s(tokio async,无阻塞;graceful shutdown
  drain 语义内,不触发 SSE 长连接契约)。
