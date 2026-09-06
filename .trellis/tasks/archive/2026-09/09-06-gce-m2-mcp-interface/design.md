# GCE-M2 设计:MCP 接口层

## 1. 架构与边界

三层架构(M1 定形)不变,M2 是**引擎层的第二个消费者**:

```
平台  daemon 群聊原语(M0,0.0.0.0:7456)        ← 零改动(AC3)
引擎  scripts/group-chat-run.mjs 纯函数区(M1)  ← 微扩(created_via 参数、transcript 根目录参数)
指引  .agents/skills/group-chat/SKILL.md(M1)    ← 改述:优先 MCP 工具,脚本为退路
NEW   scripts/group-chat-mcp.mjs(MCP server)     ← 薄包装:工具定义 + 记账 + 惰性转录
NEW   scripts/package.json                       ← 唯一依赖 @modelcontextprotocol/sdk
NEW   .agents/mcp.json                           ← 宿主挂载(顶层 mcpServers)
```

边界铁律:MCP server 只做「翻译 + 记账」,编排语义(建群契约/模型解析/终态判定/渲染)全部 import M1;反向不依赖(M1 不感知 MCP)。

**依赖落点**:scripts/ 目前零依赖且根目录无 package.json(app/ 是独立 pnpm 包)。依赖装 `scripts/package.json`(scripts/node_modules 就地解析),不动根目录、不碰 app/ workspace。`pnpm install` 在 scripts/ 内执行。

## 2. 工具规格(产品本体,预算 ≲600 token)

四工具 inputSchema 全部极简(单参工具无嵌套对象,start 的 participants 是唯一数组)。描述草稿(实现时以此为准,不扩张):

```
start_discussion:
  "Convene a multi-LLM group deliberation on a topic. Costly: 5-15 min,
   hundreds of thousands of tokens. Returns immediately with session_id —
   poll discussion_status, results come later via discussion_result.
   Presets: review (arch+product+backend), arch (2-person), retro (product+outsider)."
  params: topic (string, required) — the question, don't bake the answer in;
          cwd (string, required) — project dir as evidence base;
          preset (enum review|arch|retro, optional, default review);
          participants (array {name, model, persona_md?}, optional — full
          roster, replaces preset; model by catalog name or UUID)

discussion_status:
  "Check discussion state: busy=true running; busy=false + stop_reason
   (group_chat_end|max_rounds|cancelled|error) = finished."
  params: session_id (string, required)

discussion_result:
  "Read a finished discussion's conclusion (errors while still running —
   poll discussion_status first). Returns summary, roster, transcript path."
  params: session_id (string, required)

cancel_discussion:
  "Stop a running discussion (orchestration stops, session kept)."
  params: session_id (string, required)
```

估算:4 描述合计 ~150 英文词 + schema ~1200 字符 ≈ 500 token,预算内。**AC4 用单测锁死**:所有工具 JSON.stringify(description+schema) 总长 < N 字符上限。**N 值校准流程(评审 P2/D3 补)**:实现后先本地实测 token 数(按 chars≈token×4 粗估 + 实数),再定 N 并留 ≥10% 余量(预期 N≈2200-2300),防 participants 嵌套 schema 超支;子字段 description 从简。

### 1.5 为什么 MCP 层消灭 M1 的权限链(评审补录)

M1 嵌套消费的权限问题是「agent Bash 工具 → 沙箱禁网 EPERM → classify_block → prefix-grant(basename)→ ask」四层链的产物。MCP 工具调用不经过 shell:server 是宿主 spawn 的自有进程,fetch `127.0.0.1:7456` 不进 agent 沙箱;没有命令要授权(工具授权 = 宿主配置级单轴决策);start 秒回 session_id,状态按 session_id 直查 daemon——「后台壳句柄」中间层(升级重跑换句柄 → agent 误判 Failed → 重发双场)不存在。**残留向量(诚实声明)**:宿主对工具调用的自动重试(响应丢失/超时)仍可能 double-start,stdio 本地管道可靠性高、风险大降,v1 接受不做幂等 key;对抗手段(可选 caller 幂等 key)留 M4 讨论库立项时再议。**边界**:此受益者仅 MCP 宿主 agent;everlasting 内部 agent(daemon 单聊)非 MCP client,嵌套消费仍走脚本路径,M1 全套纪律(errno 翻译/prefix basename/裸命令/双发警告)继续有效,SKILL.md 写清分工。

## 3. 状态与数据流

### 3.1 start_discussion(立即返回,绝不阻塞)

```
resolveProject(base, cwd)          # 路径 miss 自动建 project(M1 既有)
listModels → validateModelRefs     # 失配 → 报可用模型清单(M1 既有,发现类问题的兜底)
resolveParticipants                # preset 或 participants 整名单(M1 既有)
createSession                      # metadata.created_via = "mcp"
requestId = `gcmcp-${Date.now()}-${rand}`   # 客户端生成(M1 :449 同款)
fireChat(buildChatBody)            # fire-and-forget,daemon 后台编排
map.set(session_id, {requestId, projectId, cwd, topic, startedAtMs})  # 记账 + 持久化
return {session_id, request_id, hint: "poll discussion_status"}
```

**moderator 取值(评审 P1-1 定案,镜像 M1 CLI `run.mjs:416`)**:preset 缺省 `review`;participants 整名单替换**只动名单不动 moderator**;moderator 恒取 `PRESETS[preset].moderator_model`(三预设均 MiniMax-M3),经 normalizeModelRef 解析成 UUID 后入 create_session.model。MCP 面不暴露 moderator 参数——M1 的 `--moderator-model` 是 CLI 逃生门,工具面按 D3 最小化;单测补 participants-only 档断言此语义。

### 3.2 记账与进程重启(关键设计)

stdio server 生命周期 = 宿主会话(ZCode 重开会话即重 spawn)。讨论 5-15 分钟,跨 server 进程存活是常态:

- 内存 Map + **XDG state 文件镜像**:`~/.local/state/dev.everlasting.app/mcp-discussions.json`,start 时 write-through(tmp+rename 原子写),内存 miss 时读文件兜底。
- 文件是 `{[session_id]: {request_id, project_id, cwd, topic, started_at_ms}}` 映射(**project_id 必记**——busy 只在 list_sessions 按 project_id 查询时富化,types.rs:442-447;load_session 不返回 busy,评审 P1-2);并发多 server 进程 last-write-wins per key,可接受。
- **重启兜底链(写死,评审 P1-2)**:内存/文件记账命中 → `list_sessions(project_id)` 取 busy/stop_reason;记账全 miss(如手抄 session_id)→ `load_session(session_id)` 取完整 SessionRow(含 project_id,types.rs:517-520)→ 回 list_sessions。status/result 均走此链;只有 cancel 需要 request_id(仅记账可解,miss 时报可操作错误)。
- 不清理策略 v1 不做(文件极小,几百字节/场;挂 M4 讨论库检索时一起治理)。

### 3.3 discussion_status / result / 终态惰性转录

- status 返回 `{busy, stop_reason, elapsed_s}`——**无轮次字段**(评审 P2-2 裁决:SessionSummary 无轮次列,轮次是编排循环态;status 是廉价轮询,不为 hint 拉 load_session 全消息。消息数留给 result,它反正要 load_session)。elapsed_s 取记账 started_at_ms(重启后文件里有)。**stop_reason 枚举注意(评审 P3-2)**:session 终态列只有四值(group_chat_end/max_rounds/cancelled/error);group_chat_loop.rs:140-141 另有轮级跳轮值 nominee_unknown/participant_unresolved,不落 session 终态列,MCP 消费视角不会遇到,实现者勿误加白名单。
- **惰性导出抽共享纯逻辑 `ensureTranscript`(评审 P2-1)**:输入终态 session + messages + 记账,幂等(已导出/记账里有 transcript_path 则跳过);status 与 result **双入口都调**——防调用方不经 status 直呼 result 拿到悬空路径。
- **导出失败降级(评审 P2-1)**:写 `<讨论 cwd>/out/` 可能撞只读/权限不足目录——导出失败**绝不使 status 报错**(轮询契约不受污染),降级为 `transcript_path: null` + `transcript_warning` 字段;result 同构处理。导出机制 = M1 renderTranscript + defaultTranscriptPath 微扩显式根目录参数(默认仍 REPO_ROOT,M1 行为不变)。
- result:非终态 → MCP error("still running, poll discussion_status");终态 → `{stop_reason, summary: session.discussion_summary(缺失时警告+转录兜底——字段在 SessionRow,经 LoadedSession.session 读,GC7), roster, stats(消息数/耗时), transcript_path}`。
- cancel:cancelChat(记账查 request_id)→ 已终态时幂等成功返回提示。

### 3.4 created_via 迁移(D5)

- M1 侧:`buildCreateSessionBody({...  createdVia })` 新可选参数写入 metadata;`run` 主流程传 `"script"`;既有单测补断言。
- MCP 侧:恒传 `"mcp"`。
- 不动 daemon、不动 DB(缺失 = GUI/历史,语义即设计)。

## 4. 测试策略

| 层 | 手段 | 覆盖 |
|---|---|---|
| 单测 | `scripts/group-chat-mcp.test.mjs`(node:test) | 工具注册与 schema 预算(AC4);四 handler 行为——daemon 侧以注入 `deps`(fetch 包装)mock:start 全链含 created_via/status 终态判定与惰性转录触发/result 非终态报错与 summary 兜底/cancel 内存命中与文件兜底;记账文件原子写 |
| 单测 | M1 测试文件扩充 | buildCreateSessionBody createdVia stamp;defaultTranscriptPath 显式根目录 |
| 冒烟 | `scripts/group-chat-mcp-smoke.mjs` | SDK Client over stdio spawn 真 server + 真 daemon,跑「start → 轮询 → result」(AC1 前半,--dry 模式不真烧 token 时仅连通+工具列举) |
| live | ZCode 宿主实跑 | 挂 .agents/mcp.json 后新会话见 4 工具,真实召集一场小规模审议(AC1 后半;嵌套消费场景 = AC5-M1 的 MCP 版) |

SDK 自带 `InMemoryTransport`(Client↔Server 内存对)——单测不 spawn 进程即可走完整 MCP 协议路径。

## 5. 风险与对策

- **SDK 版本 API 漂移**(@modelcontextprotocol/sdk 的 registerTool 等近年变动大):lockfile 钉版本;封装层隔离 SDK 类型,纯逻辑(handler 核心)不 import SDK——单测可脱离 SDK 跑。
- **宿主重 spawn 丢内存态**:3.2 state 文件兜底。
- **context 超预算回归**:AC4 单测锁字符上限。
- **daemon 未跑**:start/status 首查失败即报可操作错误(checkDaemon 同款提示,引 scripts/daemon.sh)。
- **回滚**:MCP 层全增量文件 + M1 侧两个向后兼容微扩;revert 单 commit 即净。

## 6. 兼容性声明

- daemon:零改动、零新端口、零 schema 变更(AC3/R5)。
- M1 CLI:行为不变(created_via 是 metadata 增量字段,旧 daemon/新脚本、新 daemon/旧脚本均无破坏)。
- GUI:不感知 created_via(侧栏读 metadata.participants 不受增量键影响)。
- **转录落点有意分叉(评审 P3-3,非疏漏)**:M1 CLI 转录固定落引擎仓库根 out/(run.mjs:20-21 注释自证:按脚本位置推导,嵌套消费时外层 cwd 是别的项目);MCP 落讨论 cwd/out/(转录留在证据基地,调用方可发现)。同一共享函数经根目录参数分叉,两默认各服务其消费者。
- **`.agents/mcp.json` fallback 约束(diagnosing-mcp pitfall 12)**:同 scope `.zcode` 若日后定义任何 MCP server,本文件被**整体忽略**(非合并)。当前 `.zcode/` 无 config.json 故生效;警示落 DAEMON-API §6 一行——**不写进 mcp.json 本身**(JSON 无注释语法,`_comment` 键是依赖解析器宽容的 hack,评审建议此半条驳回)。
