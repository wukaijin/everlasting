### 2026-08-06 — 群聊 speaker 落库错位修复(废弃 round-robin fallback + wire 孤儿自愈)

- **Context**:moderator 常用自然语言引导("接下来请 D4F…")但不调 `nominate_speaker`,旧 round-robin fallback 按 `participants[round % len]` 机械派人 → 派错的人为回应语境模仿他人口吻发言 → 角色认知崩溃、speaker 标签错位(DB 取证 session a6c87247)。外围修复均失败:v1 加重试循环放大 orphan tool_use(D4F 28 个 400),v2 把 max_turns 1→6 破坏"nominate 后 turn 结束"语义(participant 完全不发言)。
- **决策 — 废弃 round-robin fallback,改为重试 moderator turn**:`next_speaker == None` 时不再机械轮转,而是 `no_nominate_streak += 1` 回到 for round 顶部**重跑 moderator turn**(不派任何人),超过 `MAX_NO_NOMINATE_STREAK` 才放弃。`nominate_speaker` 成为唯一调度机制;moderator `max_turns=1` 保持(nominate 后天然结束)。merge `fix/group-chat-speaker-desync`(`48960d7`)。

### 2026-08-07 — 群聊评审修复 + 工具集收敛 + per-role history 隔离 + 总结卡片

- **Context**:DB 取证(session 8be4687f)治理群聊三层缺陷:① `use_skill` 幻觉(moderator/participant 用 `update_checklist`/`use_skill` 自建调度,黑名单没拦住);② 参与者夺权(moderator-only 工具被调用);③ 同模型串台(多身份 assistant 共存同一上下文)。连续两个任务合并提交(评审修复 + 工具集收敛)。
- **决策 R1 — 工具白名单取代黑名单**:新增 `group_chat_tool_defs(tool_defs, is_moderator)`(取代 `participant_tool_defs`):moderator = 调研白名单(`read_file`/`grep`/`glob`/`list_dir`/`web_fetch`)+ 仲裁工具(`nominate_speaker`/`end_discussion`);participant = 仅调研白名单。**白名单穷尽** — 新增 builtin 工具默认不进群聊,防"群聊误用新工具"复发。
- **决策 R2 — 编排器静默路径变可见事件**:4 条静默 break/continue 改为 emit `Done{stop_reason}`。终态 max_rounds → finalize;非终态 nominee_unknown / participant_unresolved → 挂 notice 不 finalize(前端 `groupChatNotice` + `ChatMessage.notice` + MessageItem 提示行)。
- **决策 R3 — 参与者 max_turns 1→20**:允许参与者查询代码库取材实证(read_file 等工具后第二轮发言);moderator 保持 1。
- **决策 R4 — 删 `ParticipantConfig.order` 死字段**:round-robin 废弃后 order 不再用,UI ↑/↓ 重排按钮移除(防误导);serde 忽略旧 metadata 的 order(向后兼容 + 回归测试锁死)。
- **决策 R5 — `participant_view` 相邻性不变量**:显式不变量注释 + 参与者多轮非仲裁对混合场景测试(锁死 R3 改 max_turns 后不破坏)。
- **决策 — per-role history 隔离(`role_history`)**:`role_history(full, current_role)` 一遍扫描状态机取代 `participant_view`:每个角色只见自己的 assistant 行(verbatim,含 thinking + signature — Anthropic round-trip 安全),他人发言改写 `role:user`(保留 speaker 字段、content 不带 `@` 前缀 — 归属由 wire 层负责,防双重前缀),他人 thinking / 工具对剥离(工具结果不共享),moderator 仲裁对从参与者视角同样剥离。治第三层"同模型串台"。配套 D-D 守卫扩展(speaker 短路,防 rewrite 行重复落库 → DB 污染 + 前端 ghost rows)。
- **决策 — 参与者 research 指引(08-07-group-chat-participation-summary 前半)**:工具白名单已下发但 identity-guard 禁令过重,弱模型把"禁止夺权"过度泛化为"讨论轮不调任何工具"(参与者零工具调用,DB 取证 session 6c00f286);prompt 补"调研工具是你的,只用来佐证自己的观点"指引,恢复实证取材能力。
- **决策 — end_discussion 总结卡片(08-07-group-chat-participation-summary 后半)**:moderator 调 `end_discussion` 把完整总结放进 `input.summary`,前端新增 `DiscussionSummaryCard.vue` 提取渲染"讨论总结"卡片(200px 截断可滚动,markdown 渲染,与 ToolCallCard 共用 result 数据源),总结不再只藏在工具卡片里。
- 任务:`08-06-group-chat-speaker-desync` / `08-07-group-chat-review-fixes` / `08-07-group-chat-toolset-and-identity` / `08-07-group-chat-role-history-isolation` / `08-07-group-chat-participation-summary`(均 archive)。spec:`.trellis/spec/backend/agent-loop-architecture/pattern-worker-worktree-override.md` §"Group-chat transcript view" Pattern(role_history + 工具白名单 + D-D 守卫)。

### 2026-08-07 — agent-loop:tool_use 执行以 tool_calls 为准,不依赖 stop_reason

- **Context**:DB 取证(session d7fe451c)moderator 某轮发出 3 个 `read_file` tool_use,但 provider(Console Go,OpenAI 兼容)在 tool_calls-only 流上的 finish_reason 不是 `"tool_calls"`(`"stop"`→`"end_turn"` 或缺失)。旧判定 `should_continue = stop_reason == Some("tool_use") && !tool_calls.is_empty()` 不成立 → 直接 return:`assistant(tool_use)` 行已落库但工具未执行、`tool_result` 未落库 → 孤儿 tool_calls 违反 llm-contract §Pair Atomicity → 每次请求 400 → 群聊烧掉 `MAX_ORCHESTRATION_ROUNDS`(26 个连续错误 turn)。
- **决策**:工具执行 gate 只看 `tool_calls` 本身 — 模型发出任何 tool_use 就必须执行并回灌结果;`stop_reason` 只决定终态 `Done` 值。spec 补 `llm-contract.md` §Pair Atomicity 执行侧契约(When this bites),回归测试 `agent_loop_tool_use_with_non_tool_use_stop_reason_still_executes`。

### 2026-07-29~08-04 — 群聊 group chat(turn-taking 编排引擎 + 4 Phase)

- **Context**:经典 chat 是单 agent 循环;多 agent 协作(如多视角 review / 多角色讨论)需要多个 LLM 参与者在同一 session 内轮流发言,旧架构无此能力。
- **决策 — session_type 区分两种循环 + moderator 编排**:`sessions.session_type` 列区分 `'chat'`(经典单 agent,走 `chat_loop.rs`)/ `'group_chat'`(走新 `group_chat_loop.rs`)。群聊循环由 **moderator**(主持人)agent 协调多个**参与者**:moderator 用 `nominate_speaker` 工具点名下一发言者,参与者发言后回 moderator,任意参与者用 `end_discussion` 终止。每条 message 落库带 `speaker` 列(参与者标识),前端按 speaker 渲染独立气泡 + 实时发言人 chip。两个新工具 `nominate_speaker` / `end_discussion` 是 SIGNAL 工具(chat_loop 拦截记录信号,非真执行)。
- **4 Phase 落地**:① 数据层 + wire 层 speaker 维度(`d2fca90`);② turn-taking 编排引擎(`80ab4bd`);③ speaker 落库/读取(`e065a12`~`a75aa37`);④ 创建群聊 session + 参与者配置 UI + 逐轮流式(`35e631c`~`2b6ab8a`)。
- **08-04 编排重写**:入口持久化去重(防重复进入群聊循环)+ 参与者身份护栏(禁自名开头、允许 @点名别人)+ 终止/发言人事件 + 逐轮流式(群聊内容实时出现 + 发言人 chip 实时渲染)+ 人类抢占插话(send 在 group_chat streaming 时先 cancel 再发)。PRD 走 `.trellis/tasks/archive/2026-07/07-29-group-chat/`,08-04 重写见 `.trellis/tasks/archive/2026-08/08-04-group-chat-orchestration-rewrite/`。

### 2026-08-11~13 — remote-control epic S1~S6b(PC daemon 一等公民 + 手机远程通道,merge `94828cb` 于 08-13 合入 main)

- **Context**:大合并(feat/remote-control-epic-s1 合入 main,merge commit `94828cb`,2026-08-13)带来远程控制 epic + Cargo workspace 翻转。remote daemon(`crates/everlasting-remote`)跑国内 2C2G 服务器,仅中继不存 agent 数据;daemon 与 remote **两套独立 SQLite**(remote 存 nodes / devices / pairing_codes)。本节补齐 08-11~13 的 9 个决策点。
- **决策 ① — 中继方案变更:Cloudflare Workers + D1 → 自研 Rust remote daemon**:原定 Cloudflare Workers + D1 (SQLite) 中继被推翻并已落地为**国内 2C2G 服务器 + 自研 Rust remote daemon**(`crates/everlasting-remote`:axum 0.7 含 ws feature + sqlx + dashmap + subtle,**零系统库依赖**);HTTPS 由用户自理(nginx 反代),**非 Cloudflare Tunnel**。部署见 [docs/REMOTE-DEPLOY.md](../REMOTE-DEPLOY.md)。
- **决策 ② — Cargo workspace 翻转(2026-08-11)**:根 `Cargo.toml` 的 `members` 扩为 `app/src-tauri` + `crates/everlasting-remote` + `crates/everlasting-remote-protocol`;`default-members` **只含两个 remote crate**(根目录裸 cargo 不拉 Tauri 重依赖);`profiles` 上移;`Cargo.lock` 移到根;daemon 入口 `cargo build -p everlasting --bin everlasting-daemon`。
- **决策 ③ — 自研 WSS 隧道协议(Frame/StreamEvent)**:不用 frp / rathole / yamux,自研 WSS 隧道(PC 侧 tokio-tungstenite 0.24)。
- **决策 ④ — 安全模型**:配对码 60s 一次性 + per-IP 限速(`ratelimit.rs` 10 次/分)+ `device_token`(64-hex)+ `shared_secret`;MVP 阶段 token 存 localStorage(V2 评估 httpOnly cookie)。不做多用户、不做跨节点同步。
- **决策 ⑤ — 反向代理传 HTTP 原文、PC 打 loopback → agent core 零改动**:remote 侧转发层不侵入 agent 循环,PC daemon 侧把请求打到 loopback 即复用既有 handler。
- **决策 ⑥ — SSE 按 request_id 过滤 + 取消只停转发**:`sse_bridge` 用 `select!` 实现,取消只停转发不停 agent(commit `0485b73`)。
- **决策 ⑦ — PC daemon 一等公民 + 永久不做主动推送**:远程是 opt-in 附加层,不反过来绑架 PC 架构;主动推送永久不做。
- **决策 ⑧ — 移动端中限适配(DEC-1~7)**:滚动 tab / pill / 触控目标等适配,不引入 Tailwind。
- **决策 ⑨ — 测试专用 import 移入 `cfg(test)`**:commit `2a482eb`,release lib 构建 0 警告。

### 2026-08-16 — P3.3 读写不对称取消(远程权限分层不做)

- **Context**:REMOTE-ACCESS-ROADMAP P3.3 原规划"远程 client 默认只读、写操作需显式 grant + `Transport.isLocal` 属性区分本地/远程连接",epic 期间由 Q11 决策推后(PWA 全权、不做权限分层),2026-08-16 用户决策正式取消,不再作为候选。
- **决策**:PWA 全权模型接受为**最终形态**——远程设备(手机)与桌面 GUI 权限完全相同,不做远程只读档/写授权分层,`Transport.isLocal` 不引入。远程通道安全边界维持既有机制:配对码 60s 一次性 + per-IP 限速 + device_token + shared_secret + HTTPS(nginx 自理),token 存 localStorage(P3.4 MVP 决策不变)。`docs/REMOTE-ACCESS-ROADMAP.md` P3.3 状态同步为"取消"。
