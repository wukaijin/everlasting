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

### 2026-08-16/17 — B1 图片支持(multimodal,5 PR)

- **Context**:输入层从纯文本升级图片通道。议程 8 决议 + 外部评审(1 P0 口径 + 4 P1 + 4 P2,含一项修法驳回)全部落 PRD/design/implement;5 PR 提交在 `feat/b1-image-multimodal`。
- **决策 — DB 只存文本 + metadata 引用,Image 块每轮磁盘即时物化**:`ContentBlock` 双形态(ImageRef 引用形态 tag `image_ref` 全管线轻量;Image resolved 形态 serde 即 Anthropic 原生 image 块,adapter 零转换)。resolve 在 `drive.rs` retry_open 前、per-turn 请求 clone 上(每图每轮一次读盘,主 Vec 保持轻量)。
- **决策 — 当轮新图与历史图统一走 `ChatMessage.attachments` 字段**(design §4.1 的 ChatRequest 独立参数取消):经典 chat 历史经前端回传(metadata.attachments → wire attachments),群聊经 `reload_messages` 从 metadata 重建——单一机制双路径。
- **决策 — 占位降级非静默丢弃(R3)**:caps=false 时 strip 对 UserBlocks 内 Image **替换**为 `[image: … 不支持图片,未发送]`;live 实证模型读到占位后明确拒答"图片没有送达",防幻觉达成。评审 P1-4 的">10 一刀切"修法被驳回(历史图累积误伤),改两级闸:新图/轮 ≤10 + 请求总量 ≤20(chat_inner 入口清晰报错)。
- **决策 — 顺手闭合 DOMPurify 外链图缺口**:原 `USE_PROFILES:{html:true}` 放行任意 `<img>`,LLM 输出外链图即发请求(tracker/IP 泄露)——BACKLOG §3.3"不渲染 LLM 之外的图"此前并不成立。现三前缀放行(相对 / 绝对 daemonBase / pwa-remote proxy+token),外链图降级 `[图片]` 链接。
- **决策 — images_token 口径 = 请求内全部 Image 块(含历史重建)**:评审 P0-1;@图 w/h 由后端 `imagesize` crate 读文件头(纯 Rust 非像素解析),粘贴图前端 FileReader。
- **偏差记录**:① 批量改 109 处 ChatMessage 字面量的脚本两轮误伤(`{` 丢失 / fn 返回类型括号错配),全部修复后全绿——此类机械改动应优先编译器反馈循环;② 正向视觉路径 live 未终验——catalog 无真实 vision 模型(MiniMax-M3 经 wukaijin 对 image 块静默忽略,in_tok=26 记账异常),降级路径 live 全验证;③ PR4 提交遗漏根 Cargo.lock(imagesize 锁),PR5 补上。
- 任务:`08-16-b1-image-multimodal`。spec:llm-contract "Image Blocks" + token-usage-tracking "images_token"。

### 2026-08-17 — D2① 跨 session 全文搜索(用户驱动 MVP)

- **Context**:ROADMAP 第三档 D2 双驱动的 ①(用户驱动)。brainstorm 四决议(入口=全局 Modal 接管 Cmd/Ctrl+K;范围=全部 project 默认;跳转=Modal 内只读预览+定位;<3 字符 LIKE 兜底)+ 外部评审 3 项(P1 GET→POST、P2 跨 project 打开语义方案甲、P3 kind 字段契约回写,独立核实后全采纳,P3 另修出 design §2 struct 自身缺 kind 的内部矛盾)。
- **决策 — docsize 守卫回填(本任务最有 transfer value 的实证)**:external-content FTS5 表的 `COUNT(*)` **穿透读内容表**(索引全空也返回基表行数),`integrity-check` 对从未索引的表**放行**——两者实证皆不可用作陈旧探针;`%_docsize` 影子表(每已索引文档恰一行,含空 text 行)才是精确探针。`run_migrations` 比对 messages 行数,分歧才 rebuild:升级后首启回填一次,后续跳过。live 实证:真实库 1192=1192。
- **决策 — update trigger 限定 `AFTER UPDATE OF text`**:memories 模板的裸 `AFTER UPDATE` 在 messages 上会造成每次 latency/metadata 落库的 FTS delete+insert 写放大;限定 text 列后只有 D3 编辑类真实改写触发同步(测试锁定:metadata/latency UPDATE 后 docsize 不变)。
- **决策 — 双路分派 + title 附带**:≥3 unicode 字符走 FTS(trigram phrase + bm25),<3 走 LIKE(`%_\` 转义)保 2 字中文词可搜;`sessions.title` LIKE 附带同程返回(`kind: title|content` 判别,单次 IPC 两类命中,content 专属字段 Option)。snippet Rust 统一切窗,前端 lowercased-indexOf 自行高亮——wire 不携带匹配偏移,消掉 Rust char index ↔ JS UTF-16 index 的语义漂移面。
- **决策 — 预览复用粒度 = MessageItem + buildRunGroups,不复用 MessageList 整壳**:MessageList 直读 `store.messages`(非 prop 驱动),整壳复用会绑死当前 session;改提取 20 行 run 分组纯函数 `buildRunGroups`(MessageList 行为等价调用,主聊天热路径唯一触点,既有测试兜底)。`MessageItem` 新增 `readonly` prop **结构禁用**(非 CSS 隐藏)hover 编辑菜单——预览里 Edit/Resend 会打到当前 session 的 store action,跨 session 预览时必然错靶。
- **决策 — `openSessionInProject` 组合 action(评审 P2 方案甲)**:`switchSession` 不碰 `currentProjectId`,裸跨 project 调用会 ① 把 B 的 session 记成 A 的 last active ② `sessions.value.find` miss → currentCwd 置空。组合顺序 `switchProject` → **显式** `await loadSessions` → `switchSession`(不依赖 chat.ts watcher 的异步 onProjectChange,消竞态);同 project 退化为裸 switchSession 零额外 IPC。
- **决策 — 路由 POST(评审 P1)**:`httpTransport.invoke` 对所有 CMD_TO_DOMAIN 命令硬编码 POST,sessions 域唯一 GET(`/:id/snapshot`)是 transport 特判 URL 非先例;GET 注册会 405。
- **决策 — limit 每类各自截断**(title N + content N):一类洪水不淹没另一类;测试锁定语义。
- **live 实证**:2 字中文"权限"跨 3 project 真实命中(LIKE 兜底)/ FTS "worktree" 命中 / title 命中按 updated_at 倒序 / project 过滤精确;"trigram""缓存率" 0 命中经 DB 直查证实为词本身不存在(非索引缺陷)。UI 交互层(浏览器点按)本 session 无浏览器后端未验,组件/路由测试覆盖 + 用户真机 Ctrl+K 复验。
- 任务:`08-17-cross-session-search`。spec:database-guidelines "messages_fts" Scenario。② Agent 驱动 `search_history` tool 为 follow-up(复用 `db::search::search_messages`,不经 IPC)。

### 2026-08-18 — C3+ 摘要式上下文压缩(LLM 摘要取代机械丢组)

- **Context**:C3(06-12)纯机械丢组无语义保留,长 session 早期决策彻底丢失;去 MAX_TURNS 硬卡的前提是上下文可语义无损续。7 家工具调研(Claude Code/Codex/Gemini CLI/opencode/Cline/Roo/OpenHands/Aider + Manus/Anthropic 通用原则)+ 外部评审(P1-1 位置假设、P1-2 前缀落库、P1-3 对齐依赖)+ check 独立复核(含 P1 级设计缺陷发现)全流程走完,3 PR + 1 修复。
- **决策 — 水位按 `cutoff_seq` 精确折叠,不是摘要行位置**:PR2 check 发现原设计"摘要行之前的全折叠"会把保留区(15-25k 逐字)与本请求提问一起吞掉——摘要行按 seq 游标插在全量行之后,按位置折叠恰丢最该保的东西,而摘要 transcript 从未覆盖它们。按 cutoff 折叠后保留区/提问/回答跨请求天然存活;旧摘要行被增量合并吸收(kind 过滤防重复)。`cutoff_seq` 因此从"审计冗余"升级为 load-bearing 字段,由 `compressible_cutoff_seq` 精确计算(无折叠 = `db_rows[cut - P - 1].seq`;有折叠 = 过滤后缀数行;退化 = 传递 prior.cutoff)。
- **决策 — 摘要行 content/text 两列同值写纯摘要,前缀话术不落库**:wire 对齐锚在 text 列(rehydrate 回发 text 列原文),in-context 折叠从 content 重建;前缀落库会进 `<prior-summary>` 滚雪球 + 污染 D2 搜索。不照抄 `insert_system_event` 的两列分叉先例。
- **决策 — 摘要行 insert 吃 loop 的 seq 游标,绝不独立 `MAX(seq)+1`**:messages 主键 `(session_id, seq)`,活跃 loop 内独立 MAX+1 会与 loop 后续 persist 撞号;`insert_system_event` 的 MAX+1 只在无活跃 loop 的 IPC 路径安全。配套 `permission_ctx.turn_seq` 重指到 assistant 行保审计引用。
- **决策 — prior-summary 检测用 `SummaryAnchor` 经 `DriveTurnOutcome` 循环内穿参,不用位置猜测**:合成头布局随 memory/skills 有无漂移(摘要实际落位 1/2/3);循环内穿参同时覆盖同 loop 二次压缩(LoopInit 单次穿参罩不住)。
- **决策 — 熔断走进程级 OnceLock 单例,不加 AppState**:`run_chat_loop` 24+ 参签名是硬约束,AppState 句柄穿不进 loop;同 `memory::digest::registry()` 先例,`delete_session_inner` 清理。连续 3 次失败跳摘要直达机械,成功清零。
- **决策 — 摘要失败路径必须把 `Ok(ChatEvent::Error)` 当失败**(check 修 bug):mid-stream 错误可能以任一形态到达,漏接 Ok(Error) 会把半截文本当完整摘要落库;降级链 = 摘要失败 → 机械丢组原样 → StillOver fail-fast(RULE-A-002 不变)。
- **决策 — 观测口径**:`CompactResult.method`(Summary/Mechanical/None)+ `summary_usage`(摘要 usage 不混入 `update_last_turn_usage`,只进 compaction_json;trace.rs 手工 json 三处联动)。TS 侧 `?? "none"` 兼容旧回看行。
- **偏差记录**:① `preserved_region_and_question_survive_across_requests` 断言一度过严(要求缺席非空文本行最大 seq == cutoff,但被压区末组可能是空 text 的工具配对)——修为"边界行在 DB + 缺席最大非空 seq ≤ cutoff";② live 烟测未跑(catalog 无超线长 session 现场,需真机重编 daemon 后构造),AC1/AC2 的 live 半边留待;③ 全量测试 1 个预存 flaky(`dispatch_main guard` 满并发超时,单发 0.56s 过,基线同挂)。
- 任务:`08-18-llm-context-compaction`(已归档)。spec:agent-loop-architecture "pattern-llm-compaction" + database-guidelines "compaction_summary" + token-usage-tracking "摘要旁路 usage"。后续任务:MAX_TURNS 软卡化 / 手动 `/compact` / handoff 接力(已立项)。

### 2026-08-25 — F1 消息队列·用户连发档(输入侧排队 + 续轮批量注入)

- **Context**:turn 串行且流式期间编辑器整体只读,连打字都不行。补输入侧队列最小闭环:turn 进行中可打字/发送/撤销/修改,轮结束批量注入下一轮;为 F2 定时 / F6 异步预留统一入口(生产者不实现)。双外部评审(review-glm / review-d4f)+ 三轮修复收口,live 冒烟与 curl REST 排队分支真机实测通过。
- **决策 — 统一入队 vs 忙时特判**:选统一入队(所有发送一律先入队,「查忙 + 入队 + 注册/spawn」收进单一路由锁临界区,区内零 await,锁序 queues → active 全仓文档化)——一条路径让 Tauri IPC 与 daemon REST 天然一致(AC7),且错误终止后滞留项与下次新发送的 FIFO 顺序由构造保证;代价是空闲路径也过一次锁+队列入出(纳秒级)。
- **决策 — `TurnContinuation` 独立事件 vs 泛化 `start` 门控**:续轮渲染边界必须是新事件(驱动器每次续轮内层 run 前 emit,群聊 `Speaker` 同位置同角色)。不能泛化 streamEvents 的 `groupChat` start 门控——`start` 是 run 内每次 LLM 调用的边界(tool_use 后下一轮也发),泛化会把经典多工具轮错误拆泡。规划评审时此点被漏掉,实现者修正(原"中间态零改动"断言作废)。
- **决策 — uuid 寻址 vs position**:队列项与前端占位一律按 uuid 寻址(position 仅展示)。评审 Round 1 抓到 position 漂移三连缺陷(撤销后右移全部错位 → 撤错条/静默 no-op);`ChatAcceptance::Queued` 补 `id` 字段(wire additive),撤销/退回按 id 直达后端。
- **决策 — 取消矩阵不对称**:user 主动终止(Stop/edit/resend/retry/defense-in-depth 替换)→ 清空 + `clearedQueued` 计数 toast;provider 错误/续轮触顶(50)→ 队列**保留**,下次发送统一入队自然消化(非 user 过错不丢输入)。
- **决策 — 闲路径口径修正**(评审 Round 2):原"闲且队空逐字节对齐现状"作废——统一路径下闲时发送同样入队由驱动器消费,LLM 请求历史从客户端 `messages` 改为 DB reload(群聊 D-B 同构)。语义等价非逐字节;若未来出现"仅存于客户端 history、未落库"的请求内容会被 reload 丢弃(当前无此形态,记录在案)。
- **偏差记录**:① P0 DriverSink 丢事件——`emit_chat_event` 未按 `forward` 返回值转发且 Error 分支自转发(双发),续轮 Delta 整链不可见,单测 2 例 + 集成 Delta 断言锁死;② F1 三命令漏 `CMD_TO_DOMAIN` 映射,开 session 即 unknown cmd(transport 层路由同步守卫断根);③ ChatInput 两道旧守卫(`sendDisabled`/readOnly 判定)吞掉流式发送,AC1 物理不可达——评审 d4f 称"AC1 未要求流式中可发送"系误读,实现者驳回正确;④ 跨设备可见性走 `list_queued_messages` 水合而非事件广播(省 wire 变体,代价非实时,MVP 接受)。
- 任务:`08-25-f1-message-queue`(已归档)。spec:agent-loop-architecture "pattern-message-queue-driver"(注入契约 + TurnContinuation 事件语义 + 锁纪律)。B 档(优先级分档/抢占)与 C 档(daemon 统一入口服务化,F2/F6 生产者)留后续。
