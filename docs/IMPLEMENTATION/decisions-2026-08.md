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

### 2026-08-14 — C7 tools[] 上下文 token 治理 + C7D tools Stub 注册(渐进式披露)

- **Context**:`tools[]` 数组是与 messages 并列的上下文治理对象(省窗口预算,provider 无关;cache 断点只省钱不省窗口)。live 实测 tools_token=6773 / context_input=17602 = **38.5%**,远超 15% 触发线。
- **决策 — R1 度量先行,先量后裁**:`turn_tool_defs` freeze 后对 post-filter ToolDef JSON 跑 cl100k,落 `turn_trace.tools_token` 新列(幂等 migration helper + `upsert_turn_trace_token` 扩参);占比口径 = tools_token / context_input,**不 double-count**(context_input 已含 tools)。
- **决策 — R2(Anthropic cache 断点)不做**:relay 实测 `cache_creation=0` 零收益,等原生 Claude provider;D(Stub 注册)触发线 = tools 占窗口 >15%。
- **决策 — R3 静态裁剪**:`filter_tools_for_session_type` 经典聊天砍群聊专属 `nominate_speaker`/`end_discussion`(~465 tok/轮);drive.rs 过滤链第三环 mode→workflow→session_type。
- **决策 — C7D 原地 stub 替换 + 元工具按需取回**:`STUB_CANDIDATES` 大 schema 工具首轮原地替换为 stub(真名 + 一句话摘要 + 宽松外壳),另注册常驻 `load_tool_schemas` 元工具取回完整契约;session 粘性 `StubRegistry`(loaded-set,`delete_session` 清空);gate = `tools_stub_enabled` && 非 worker && 非群聊(群聊白名单语义 / worker 自主可靠性豁免)。live 两轮复验:tools_token 6773→**3677**(-45.7%),AC1 阈值 3000→3700 校准。
- 任务:`08-14-c7-tools-token-governance` / `08-14-c7d-tools-stub-registration`(均 archive)。spec:`token-usage-tracking §C7` + `tool-contract`。

### 2026-08-15 — memory-gov memory 指令块窗口治理(WP1 度量 + WP2 分级注入 digest)

- **Context**:C7D 后 memory 指令块(CLAUDE.md/AGENTS.md 4 层,~7-8k ≈ 42%)反超为上下文最大头。
- **决策 — WP1 度量与 tools_token 同契约**:`turn_trace.memory_token` 列(幂等 backfill,同 Done 写点/同 upsert);init.rs 对实际注入 blocks cl100k 估算经 `LoopInit`→`drive_turn` 落库。
- **决策 — WP2 分级注入(同构 C7D 渐进披露)**:`memory/digest.rs` fence-aware 切节 + 目录(标题+首句,纯机械);tier = AGENTS.md(primary)永不 digest / CLAUDE.md(reference)且 tokens>600 才 digest;`load_memory_sections` 元工具按需取回(banner label 命名空间寻址);粘性 `MemoryDigestRegistry`(OnceLock 单例,`delete_session_inner` 清理);gate `memory_digest_enabled`(缺省 on,fail-open)&& !worker && !群聊。
- **live 实测(08-15)**:memory 10124→**2080**(-79.5%,占首轮 72%→28%),首轮 context 14079→7421(**-47%**);双轮 cache 率 99.8% vs off 99.7% 不劣化。
- 任务:`08-15-memory-block-governance`。spec:`memory/decisions` + `token-usage-tracking`。

### 2026-08-17 — D2② Agent 驱动 `search_history` tool

- **Context**:D2① 用户驱动搜索落地后,agent 侧检索同库但复用 DB 层即可,不经 IPC。
- **决策 — 薄封装复用 `db::search::search_messages` SQL 层**:`tools/search_history.rs` 零 SQL/IPC/前端改动;`{query, scope: all|current_project, limit≤50}` → 紧凑一行一 hit 文本。权限链零改动(`ToolKind::Other` Tier 5 silent Allow);`READONLY_TOOL_ALLOWLIST` 第 6 员。
- **决策 — 新工具先评估扩 STUB_CANDIDATES 守则**:search_history 非 C7D stub 候选但注册 +178 tok → C7D AC1 预算线二次校准 3700→3900。
- 任务:`08-17-cross-session-search`(与 D2① 同任务)。spec:`tool-contract/15-search-history`。前端专属卡片 `08-17-search-history-card`。

### 2026-08-19 — unified-context-budget 统一 token 预算 + MAX_TURNS 软卡 + 手动 /compact + handoff

- **Context**:C7/memory-gov/B1 切片齐备,但缺统一预算收口;去硬卡前提是上下文可语义无损续(C3+ 已具备)。
- **决策 — WP1 度量补齐三切片**:`turn_trace` 新列 `at_files_token`(@文件注入体,span 寻址单一定义)/ `system_token`(system prompt 体 + skill-listing 合成消息)/ `context_window`(请求时窗口快照,前端预算行分母)。
- **决策 — 压缩口径统一切换为"发送部件加法"**:触发线 / 摘要 postcheck / 机械 compact 三处全用 `estimate_request_tokens`(system + tools_json + messages),修复旧口径只数 messages 漏计 tools/system 的洞;归因切片与总量**永不互相加计**。
- **决策 — WP2 硬卡引擎**:`BUDGET_LINE_RATIO=0.95`×window 触发静默裁剪,裁尽仍超才 fail-fast;触发落 `AuditKind::ContextBudgetTrim`。
- **决策 — MAX_TURNS 软卡**:撞线不再无条件硬停,改 QuestionStore 询问(继续 +200 / 压缩后续跑(`force_compaction=true`,trigger_label="softcap")/ 停止,10 分钟超时兜底);break 门 = `effective_is_worker || group_chat_state.is_some()`(worker 有 C1 resume、群聊 speaker 段保持硬卡,漏群聊门会让 tool_use 结尾的 speaker 轮挂满超时)。新增 `AuditKind::TurnLimitSoftcap`;前端复用 `AskUserQuestionCard` 浮动卡(`tool_use_id` 前缀 `turn_limit_softcap_{turn}`,**绝不能 tag 成 `Question`**)。
- **决策 — 手动 /compact**:空闲期(loop 非活跃)主动触发 LLM 摘要压缩,替代"只能等触发线自动压";`CompactResult.method`(Summary/Mechanical/None)观测;`summary_usage` 不混入 `update_last_turn_usage`。
- **决策 — handoff 跨 session 接力**:C3+ 的 handoff 前缀落地为产品功能,接力摘要带进下一 session + 修复 HUD 按 session 隔离(接力状态不再串 session)。
- 任务:`08-19-unified-context-budget` / `08-18-max-turns-softcap` / `08-18-manual-compact-command` / `08-18-handoff-mechanism`(均 archive)。spec:`pattern-budget-gate` / `pattern-turn-limit-softcap` / `token-usage-tracking`。

### 2026-08-20 — worker per-turn 度量(turn_trace 并入 run 维度)

- **Context**:E2 turn-level trace 只记主 loop,worker per-run 的逐轮 token 不可见。
- **决策 — 表重建迁移 `UNIQUE(session_id, seq)` → `UNIQUE(session_id, run_id, seq)`**:`''` 哨兵 = 主 loop 行,worker 行 = `subagent_runs.id`;**不用 NULL** 因 SQLite UNIQUE 视 NULL 互异,会插出第二行破坏 upsert;partial index `idx_turn_trace_run`(`WHERE run_id != ''`);老库走 `schema_helpers::rebuild_turn_trace_with_run_id`(表约束加宽迁移守则)。
- **决策 — 写点归位**:Done 臂 run 行落值 + compaction / loop_hint 旁路写点带 run_id;前端 SubagentDrawer「Token 明细」per-run 折叠区 + `runTracesByRunId` 粘性缓存。
- 任务:`08-20-worker-turn-trace-persist`。spec:worker per-turn 行语义 + subagent_runs×turn_trace 关联。

### 2026-08-21 — B1 收尾:图片自动压缩 + 拖拽 + read_file 工具读图

- **Context**:B1(08-16/17)正向视觉路径与工具读图留作 follow-up。
- **决策 — 前端 canvas 压缩(fail-open)**:长边>1568 降采样;无透明且>1MB 重编码 JPEG q0.85;压后判 5MB。
- **决策 — `read_file` 读图**:白名单 + 魔数 + 5MiB 闸 → attachments 副本 + `ToolResult.images` 引用;`ToolResultData` **双形态 serde**(DB=refs / wire=Anthropic tool_result content block array;无图路径逐字节不变 fixture 锁)。
- **live 实证**:MiniMax-M3 read_file UI 截图准确描述内容(**正向视觉路径首次 live 实证**),images_token=1728 精确入账。
- 任务:`08-21-b1-image-followups`。spec:`llm-contract §Tool-Result Image Blocks` + `tool-contract/16` + `token-usage-tracking 工具图计费`。

### 2026-08-25 — F1 消息队列·用户连发档(输入侧排队 + 续轮批量注入)

- **Context**:turn 串行且流式期间编辑器整体只读,连打字都不行。补输入侧队列最小闭环:turn 进行中可打字/发送/撤销/修改,轮结束批量注入下一轮;为 F2 定时 / F6 异步预留统一入口(生产者不实现)。双外部评审(review-glm / review-d4f)+ 三轮修复收口,live 冒烟与 curl REST 排队分支真机实测通过。
- **决策 — 统一入队 vs 忙时特判**:选统一入队(所有发送一律先入队,「查忙 + 入队 + 注册/spawn」收进单一路由锁临界区,区内零 await,锁序 queues → active 全仓文档化)——一条路径让 Tauri IPC 与 daemon REST 天然一致(AC7),且错误终止后滞留项与下次新发送的 FIFO 顺序由构造保证;代价是空闲路径也过一次锁+队列入出(纳秒级)。
- **决策 — `TurnContinuation` 独立事件 vs 泛化 `start` 门控**:续轮渲染边界必须是新事件(驱动器每次续轮内层 run 前 emit,群聊 `Speaker` 同位置同角色)。不能泛化 streamEvents 的 `groupChat` start 门控——`start` 是 run 内每次 LLM 调用的边界(tool_use 后下一轮也发),泛化会把经典多工具轮错误拆泡。规划评审时此点被漏掉,实现者修正(原"中间态零改动"断言作废)。
- **决策 — uuid 寻址 vs position**:队列项与前端占位一律按 uuid 寻址(position 仅展示)。评审 Round 1 抓到 position 漂移三连缺陷(撤销后右移全部错位 → 撤错条/静默 no-op);`ChatAcceptance::Queued` 补 `id` 字段(wire additive),撤销/退回按 id 直达后端。
- **决策 — 取消矩阵不对称**:user 主动终止(Stop/edit/resend/retry/defense-in-depth 替换)→ 清空 + `clearedQueued` 计数 toast;provider 错误/续轮触顶(50)→ 队列**保留**,下次发送统一入队自然消化(非 user 过错不丢输入)。
- **决策 — 闲路径口径修正**(评审 Round 2):原"闲且队空逐字节对齐现状"作废——统一路径下闲时发送同样入队由驱动器消费,LLM 请求历史从客户端 `messages` 改为 DB reload(群聊 D-B 同构)。语义等价非逐字节;若未来出现"仅存于客户端 history、未落库"的请求内容会被 reload 丢弃(当前无此形态,记录在案)。
- **偏差记录**:① P0 DriverSink 丢事件——`emit_chat_event` 未按 `forward` 返回值转发且 Error 分支自转发(双发),续轮 Delta 整链不可见,单测 2 例 + 集成 Delta 断言锁死;② F1 三命令漏 `CMD_TO_DOMAIN` 映射,开 session 即 unknown cmd(transport 层路由同步守卫断根);③ ChatInput 两道旧守卫(`sendDisabled`/readOnly 判定)吞掉流式发送,AC1 物理不可达——评审 d4f 称"AC1 未要求流式中可发送"系误读,实现者驳回正确;④ 跨设备可见性走 `list_queued_messages` 水合而非事件广播(省 wire 变体,代价非实时,MVP 接受)。
- 任务:`08-25-f1-message-queue`(已归档)。spec:agent-loop-architecture "pattern-message-queue-driver"(注入契约 + TurnContinuation 事件语义 + 锁纪律)。B 档(优先级分档/抢占)与 C 档(daemon 统一入口服务化,F2/F6 生产者)留后续。

### 2026-08-25 — F4 `web_search` 工具(搜索 → 取前 N 条结果)

- **Context**:与 `web_fetch`(全文抓取)两段式分工,同 Claude Code WebSearch/WebFetch split;搜索场景不必抓全文。
- **决策 — enum dispatch 双后端(Tavily keyed / DDG 兜底)+ 30s 整体预算重试环**:固定端点无用户可控 URL → **无 SSRF 面**,`ToolKind::Other` Tier 5 silent Allow(同 `search_history`)。
- **决策 — key 三态 AEAD 配置**:`app_config` 存 web_search key(aad=web_search),Settings 第 7 tab masked 回显;Tauri command / daemon route / CMD_TO_DOMAIN 多处 IPC。
- **决策 — 开闸多面 + 运行时断言**:`READONLY_TOOL_ALLOWLIST` 第 7 员 / builtin + dev plugin researcher / 群聊调研白名单 / 用户+项目 frontmatter 层,配运行时断言防 builtin-only 假绿;C7D `STUB_CANDIDATES` 第 11 员(token 线零平移)。
- live 冒烟经 debug daemon 实跑 DDG 搜索全链路通(attribution / 审计两行)。
- 任务:`08-25-web-search-tool`(已归档)。spec:`tool-contract/16-web-search`。

### 2026-08-26 — F5 PDF/docx 原生文本提取(@文件注入第一档)

- **Context**:B2 @文件对 PDF/Office 占位降级(提示用户自己跑 pdftotext/pandoc)。brainstorm 六决议(D1-D6)+ 业界对照搜证(Claude Code=平台内置视觉读 PDF;Codex=零内置 + openai/skills curated skills 教 agent 自助)。
- **决策 — 平台内置 + agent 自助分层(D3),不引入 Node.js(D2)**:高频路径(文本型 PDF + docx)平台内置即时注入;长尾(OLE2 老格式/odf/rtf/扫描件/提取失败)Degraded 占位文案从"教用户跑命令"升级为**指令式**("agent 可自行转换:pdftotext <path> - 后读取")——占位文案即 prompt,LLM 有 shell 工具读到指令即自助,Codex skills 式路线的零成本形态。拒绝 Node.js:daemon 单二进制零运行时依赖是架构不变量,提取发生在后端请求构造时(前端 node 仅构建工具链)。
- **决策 — PDF 库走 spike 闸门(D4),pdf-extract 过关不买 pdfium**:headless Chromium 打印中文 HTML 制样本 + pdftotext 对照。实证:中文零乱码零丢字(段落/表格/代码全保留)、英文长文档 33,957 vs 34,104 chars 语义等价、扫描件(位图页)返回 0 字符 → "<32 字符判扫描件"判据成立。pdfium(工业级 + 渲染扫描件)留 follow-up 档,四平台动态库分发成本不提前付。
- **决策 — 提取是注入的一种形态**:doc_extract 纯函数模块(bytes 进文本出,零 IO),at_file 在 Degraded 兜底前分流;成功走与 Text 注入同构的 span 通道(`<doc>` marker + D10 同请求 span + at_files_token),失败落占位 turn 不死(B1 fail-open 同构)。三级 cap:源 20 MiB fail-fast → 扫描件 <32 字符 → 文本 150k 字符保头截断(≈CJK 50k tok)。
- **决策 — wire 字段名避开 serde tag**:`InjectionAction` 内部 tag 即 `kind`,`Extracted` 变体的来源字段必须叫 `format`(撞名编译失败);TS 镜像同步。页数/段落数/原文规模只进 LLM marker 不进 wire。
- **偏差记录**:① quick-xml 0.42 实体是独立 `GeneralRef` 事件(payload=实体名),不显式映射预定义五实体 + 字符引用会静默丢字("A & B" → "A B");空段 `<w:p/>` 走 Empty 事件同样要计数;② zip 默认 features 拉 zstd-sys(C 编译依赖)——收紧 `default-features=false, features=["deflate"]`;③ cargo init 在 workspace 目录内建 spike 项目会把自己挂进根 workspace members(污染 lock + 解析失败),spike 工程须仓库外或用后清理 members;④ `daemon.sh start` 是前台命令(后台用 `bg`),链式命令里误用 start 卡死后续冒烟;⑤ pdf-extract 有 unwrap 路径,catch_unwind 兜底为硬约束。
- 任务:`08-26-f5-doc-reading`。spec:agent-loop-architecture "pattern-doc-extraction"。follow-up:**xlsx/xlsm 提取已落地(同日,`08-26-f5-xlsx-extraction`,calamine 0.36 + 每 sheet CSV 块形态;pptx 用户裁定不做)**、pdfium 渲染扫描件走 B1 通道、正式 document skill(B4 体系)。

### 2026-08-26 — F5 follow-up:xlsx/xlsm 原生提取(CSV 形态)

- **Context**:F5 PRD D1 明确「xlsx 表格→文本需单独形态设计」后增量。用户三选一拍板:**每 sheet 一段 CSV 块**(RFC4180 转义,sheet 标题行带维度;markdown 表格 token 翻倍被否,行记录式稠密表更啰嗦被否)。**pptx 用户裁定不做**。
- **决策 — calamine 0.36 而非手搓 quick-xml**:xlsx 比 docx 多三层复杂度(workbook rels 解析/sharedStrings 间接寻址/序列日期 + numFmt 样式系统),手搓 bug 面大。依赖树核验:calamine 的 zip 同为 default-features=false + deflate-only(与项目契约一致,zstd-sys 不回归),chrono feature 启用后零新增 crate(chrono 已是直接依赖)。
- **决策 — 单元格渲染契约(D5)**:字符串原样 / 数字最短表示 / bool true·false / 错误值保留 `#REF!` 形态 / 公式取缓存值 / 序列日期 chrono 转 ISO(`%Y-%m-%d`,非零点补时间)。xlsx 路径**不做 normalize_whitespace**(压空行/trim 破坏 CSV 行语义);全 sheet 无数据 → Err 走 Degraded 兜底。marker `sheets="N"`(units=sheet 数);`.xlsm` 复用 xlsx 通道,`.xls`(OLE2)/`.ods` 保持占位降级。
- 坑:测试断言 needle 手写 RFC4180 转义多打一个引号——实现输出正确、断言写错(转义后的期望串应逐字符对照生成,别手抄)。
- 任务:`08-26-f5-xlsx-extraction`。spec:pattern-doc-extraction(硬约束 +7 号 xlsx 节)。

### 2026-08-27 — F6 异步 agent 任务可观测性 + F3 全局并发闸

- **Context**:detach 运行时语义早已免费成立(loop 是 fire-and-forget spawn,客户端断开非 cancel 源)——本任务交付**编排面**三件套,session 即载体、隐式普遍化(无「后台发」概念)。
- **决策 — `SessionSummary.busy` 运行时 enrich**:daemon 层单点 `list_sessions_inner`,双 transport 一致;冷启动/跨端侧栏红点。
- **决策 — 轮次终结跨 session toast**:当前-session 抑制 + cancelled 抑制 + `turn_complete_notify_enabled` 开关。
- **决策 — F3 最小档全局信号量 `max_concurrent_loops`**(缺省 4):spawn 闭包头 acquire 排队不拒绝,等闸取消完整回滚 claim 注册。
- **决策 — Tauri 壳关闭确认**:仅 `isTauriWebview` 生效,Web/PWA 关闭不影响任务。
- **决策 — 零新表零 migration**:跨重启终态复用 messages.status 恢复链。F1-C 移出归 F2(两个消费者:cron + LLM detached dispatch)。
- 任务:`08-27-f6-async-agent-task`。spec:`agent-loop-architecture/pattern-global-loop-semaphore`。

### 2026-08-27 — stream chat 事件 payload 补 `session_id`(跨客户端实时认领)

- **Context**:remote PWA / 多客户端并发连接时,`chat-event` 只有 `request_id` 没有 session 维度,非发起端无法按 session 认领事件。
- **决策 — 事件 payload 回填 `session_id`(additive)**:`daemon/sse.rs` 事件注释契约化,支持跨客户端按 session 认领;向后兼容,老客户端忽略新字段。
- 任务:`08-27-stream-session-id`(已归档)。

### 2026-08-27 — workflow-plugin builtin 提示词脱栈通用化

- **Context**:内置 dev/review 插件提示词含 `cargo`/`pnpm` 硬编码与 `.trellis` 残留,换栈/换目录即误导 agent。
- **决策 — 提示词内容三约定**:栈中立 / 无 dogfood 泄漏 / 不承诺 ask(权限动作交给 permission 层)。零 `.trellis` 残留;等价性测试机制 + 镜像范围口径收编 spec。
- **决策 — model 漏传教训去标识化收编**:错误示例不复述个人栈细节。
- 任务:`08-27-builtin-agent-prompt-generalize`。spec:`workflow-plugin-builtin`(提示词三约定 + 等价性测试机制)。

### 2026-08-28 — F2 定时任务(本地 cron 式)+ F2b 调度模型扩展

- **Context**:ROADMAP F1-C(cron 消费者)落地;F2 把「detached LLM dispatch」与「定时触发」两个消费者统一到一条调度通道。GUI Full 零 timer 硬约束保持,调度仅 daemon 进程。
- **决策 — daemon 常驻调度器 30s tick + 单一扫描算法**:每 tick 重算「自 `max(created_at, last_fired_at)` 以来最近到期点」,catch-up(停机补跑一次)与常规触发同一判定;落账记理论到期点 `due` 保证 interval 无相位漂移;同 session 每 tick 至多一 fire。
- **决策 — origin 载体链**:fire = 构造带 origin 的 user message 走 chat_inner 同源路径,`ChatEntry → QueuedMessage.origin → ChatLoopRequest → persist 门控` 落 `messages.metadata.scheduled`(additive)——F1 队列入口统一。
- **决策 — 审计 + kill switch**:`ScheduledTaskFired` 六动作(fired/catchup/skipped_dedup/skipped_queue_disabled/lost/error);`scheduled_tasks_enabled` kill switch fail-open。管理面 Settings 第 8 tab,PWA 可用;前端「定时」chip 零 rehydrate 改动。
- **决策 — F2b 调度模型 additive 扩展**:preset 6 档(固定时间类新增 hourly/weekdays/monthly;interval 加单位换算,**纯 UI 换算**成 every_min,后端零感知零迁移);结束条件 `max_runs`/`ends_at` 通用(completed 审计 reason=max_runs/end_date,恰好一次);`run_count` 只计真正送入 chat_inner 的 fire(dedup 跳过不计数);ends_at **含当日**(判定用 `due > ends_at` 而非 now,保 catch-up 补跑);重新启用计数清零;wire update 双层 Option(显式 null = 清空为不限)。
- 用户三裁定:短月跳过(monthly 无该日跳过该月,cron 语义)/ 自动停用保留 / 当天仍触发。
- 任务:`08-28-f2-scheduled-tasks` / `08-28-f2b-schedule-extension`。spec:`backend/scheduled-tasks.md`(§F2 + §F2b)。F1-C cron 消费者交付,**LLM detached dispatch(`schedule_task` tool)仍开放**。

### 2026-08-29 — LLM `schedule_task` 工具家族(detached dispatch 落 LLM 面)

- **Context**:F2 后 daemon 调度器已就位,但只有 Settings UI 能建任务;ROADMAP F1/F2 点名的 follow-up「LLM detached dispatch」把调度器暴露给 agent——对话里一句话自排/查看/取消未来任务。零新表、零调度语义改动,纯粹在 F2 基建之上加 LLM 入口。
- **决策 — 三工具单模块(命名家族镜像 L1a 三件套)**:`schedule_task`(创建)/ `schedule_status`(列本项目 `created_by='agent'` 的任务)/ `schedule_cancel`(按 id 硬删,仅限 agent 自建行);plain dispatch,注册追加 `builtin_tools()` 尾部(provider prefix cache 契约);创建复用 `create_scheduled_task_inner` 校验矩阵全量白拿。工具名 `schedule_task`/`schedule_status`/`schedule_cancel`。
- **决策 — `created_by` 参数化(用户/agent 两作者面互不越界)**:agent 创建路径落 `created_by='agent'`(db 参数化,沿用 F2 预留),用户 UI/IPC 恒 `'user'` 零变化;列表/取消只碰 agent 自建行;`ScheduledTaskPayload` 暴露 `created_by`,Settings 任务列表加来源徽标(silent Allow 的可见性补偿)。
- **决策 — 三面全部 silent Allow + 反滥用上限 20(用户定案 Q2/Q3)**:create/list/cancel 均 `ToolKind::Other` Tier 5(创建零立即副作用,真正执行在 fire 时刻走完整 mode/permission 链,可逆可禁);补偿控制 = 同 project `enabled=1` 且 `created_by='agent'` ≥20 拒绝(上限 gate 在 tool 侧,不进 `_inner`,UI/IPC 路径不受限)+ worker `STRUCTURALLY_DISABLED` + 群聊穷举白名单天然隔离(kill switch 沿用调度器侧)。
- 任务:`08-29-schedule-task-tool`。spec:`backend/tool-contract/17-schedule-task-family.md` + `backend/scheduled-tasks.md`。

### 2026-08-30 — C6 大输出截断统一(截断契约三模式 + spill 落点迁出项目树)

- **Context**:大输出截断散落各工具,上限口径/标记格式/恢复通路三者均不统一(web_fetch 完全无恢复、grep 行级截断无指引、标记至少 4 种格式、truncate_output 三份重复实现 + background_shell 第四份镜像);评审实锤 shell 裸切片 UTF-8 panic(全库唯一违反已收编 RULE-E-009 char-boundary 规则);shell/background 的 spill 落 `<cwd>/.everlasting/outputs/` 造成 agent 自我污染 + 语义混杂。
- **决策 — 统一的是「截断契约」不是「上限数字」**:三恢复模式(A 落盘 spill + read_file offset/limit / B range 参数 / C 收窄 pattern)+ 统一 `<truncated>` 标记、machine-parsable;数字留 per-tool(语义不同)但集中一张常量表;shell/background/read_file/web_fetch/grep 五工具共用新 `tools/tool_output.rs` 契约模块。web_fetch 走落盘不走重取(两次 fetch 内容可漂移)。
- **决策 — spill 迁 `app_data_dir/outputs/<session>/` 而非 `~/.everlasting/`**:app 单根约定(DB/attachments/worktrees 同在 app_data_dir),session-keyed 与 attachments 同构;home 点目录是前 XDG 风格且 macOS/Windows 无对应物;**迁移捆绑 trusted carve-out**(不加 `outputs/**` 的 read_file 恢复每次走 ask_path,恢复通路名存实亡)。
- **决策 — 统一实现以 RULE-E-009 为准绳**:char-boundary 安全为已收编规则,shell 是唯一违例,统一后 panic→安全截断属预期修复,不为字面「等价」保留裸切片;spill 字节入口 + session_id 走 ToolContext(background_shell 的 `&[u8]` 入口更忠实,`&str` 调用方适配)。
- 任务:`08-30-c6-output-truncation`。spec:`backend/agent-loop-architecture/pattern-output-truncation.md`。C6 前 ROADMAP 标注含 web_fetch 硬 5MiB 上限与 >100KB 转换截断,一并走新契约。

### 2026-08-30 — ShellCard 专属卡 + shell 一体化审批(顺带 shell tool description 参数)

- **Context**:shell/run_background_shell 的 tool_use input 只有 command/working_directory/timeout,折叠态卡片不可扫读(连续多 shell 调用视觉同质)、审批是「盲签」(ask body 无命令原文,用户看不到命令就做允许/拒绝决策)。用户定案:顺带重设计 shell 卡片与审批卡(三问三答)。
- **决策 — shell/run_background_shell 加可选 `description` 参数(display-only)**:LLM 填写短句描述;chip 数据源抽纯函数 `messageFormat.ts::toolHeaderChip`(path → shell 家族 string description → 命令首非空行 → null)+ `isShellFamilyTool` 封闭名单;ShellCard 与 DrawerToolCallCard 共用。
- **决策 — 专用 ShellCard 组件(EditFileCard 先例)**:命令块常驻(`$` + command、pre-wrap、max-height 200px 滚动)+ 一体化审批(命令块 + 风险条 + 按钮融为一个状态,去掉独立「需要权限」盒子;pendingAsk 判定与 ToolCallCard 同源 `permStore.getPending(sid)` + `toolUseId` 匹配)+ 输出默认收起(错误红框常显);MessageItem resolver 按 tool name 替换通用卡。
- **决策 — `PermissionActions.vue` 审批按钮组抽取**:ShellCard/ToolCallCard 共用(放行/撤销/超时三态按钮列),toolHeaderChip helper 同批落地 + header chip 更名(done 态成功色)。
- 任务:`08-30-shell-description`。spec:`frontend/chat/shell-card.md` + `backend/tool-contract.md`(description 条目)。

### 2026-08-30 — RULE-PERM-001 审计事件查询 keyset 分页

- **Context**:C4 审计事件查询 MVP 全量拉取(无 LIMIT),长 session 行数到千级时单次 IPC 载荷/首屏渲染/内存驻留线性涨;DEBT P3 债源登记,PRD Edge Cases 原标「>500 条事件的 session」。两个消费方:AuditLogModal(客户端过滤,可改服务端)与 traceStore(按 turnSeq 分组,**语义需要全部行,不动**)。
- **决策 — 新增 keyset 分页命令,旧全量命令原样保留**:`list_session_audit_events_page`(RUST + daemon HTTP 双 transport,wire additive);keyset 游标 `ts DESC, id DESC`(同秒 tie 由 id 决定,**SQL 保证,前端不再重排**,分页期间新行插入不重复不跳行);类别过滤/仅 critical 下推 SQL + 服务端计数(总数/critical/filtered 对未加载行也准确);弹窗首屏一页 100 行 +「加载更多」续拉,已到末尾入口消失;`payload_json` 畸形行容错对齐客户端视为非 critical);e2e route-mock 清单与 all_command_names 同步登记新命令。
- 任务:`08-30-rule-perm-001-audit-pagination`。spec:加页码语义 `backend/database-guidelines.md` + `frontend/state-management.md`(audit store 分页状态);销债 DEBT P3(1→0)。

### 2026-08-30 — RULE-TEST-001 浏览器交互回归流水线(Playwright 选型 + CI blocking 门禁)

- **Context**:jsdom 结构性测不到真实交互(真实键盘/指针、滚动+store 联动、弹窗层叠)——BUGLIST CH5-1(Shift+Enter)、CH14-1(焦点环)被迫人工复核;MarkdownDetailModal pointerdown-outside 仅占位;ui-review.sh 是视觉评审(静态截图看不见 hit area/hover/动效),不是交互回归。仓库已有 playwright-core 先例(ui-review 截图)。
- **决策 — Playwright 单任务全链交付 + 三试点盲区各一**:真实 Chromium 驱动真实前端(vite dev server :1420,route-mock 驱动无 daemon/无 LLM/无网络);试点 ① 键盘类 Shift+Enter vs Enter(CH5-1 原型)② 滚动+store 联动提问卡强制回底(CH8-2,mock SSE 流)③ 指针+弹窗放行撤销确认(CH7-4,pointerdown-outside);用例放 `app/e2e/*.spec.ts`,vitest include 天然隔离。
- **决策 — CI gate 为 blocking 硬门禁 + 确定性准入标准**:进 frontend job 作 merge 门禁;CI 只收确定性用例(route-mock 无 daemon/LLM/网络)+ Playwright retry 兜底;时序不确定标 local-only 不进 CI;「进 CI」是每条用例的准入标准而非默认;devDep `@playwright/test ^1.62.1`。
- 任务:`08-30-rule-test-001-browser-pipeline`。spec:`frontend/browser-regression.md`(分层问询序 + fixture 契约 + testid 登记)。销债 DEBT P3。

### 2026-08-31 — Sandbox 执行期沙盒主路线定案(Landlock+seccomp)+ P3b 落地

- **Context**:A2+ 判定层(P1+P2 复合命令拆分 + 写重定向检测)只覆盖静态可判定的命令;变量展开 / `$()` / `eval` / alias / 间接副作用是静态分析永远堵不上的盲区(把 `FOO=rm x` 误判 ReadOnly 即静默放行)。P3 定位为判定层**之下**的执行期限损层。spike(P3a)实测 WSL2 两条候选路线并探查泛化性。
- **决策 — 主路线 = 自研 Landlock + seccomp,弃 bubblewrap(零外部二进制依赖)**:bwrap 依赖 userns(WSL2 可用性不稳)+ 二进制分发 + interop 逃逸面;Landlock(EXECUTE + 写族 handled,读不控)+ seccomp BPF(拦 `socket(AF_INET/AF_INET6)`,AF_UNIX 放行)+ `PR_SET_NO_NEW_PRIVS` 全在 Linux 内核 syscall,纯 Rust + 既有 libc crate,单二进制零新增依赖;泛化硬约束(C2)。
- **决策 — P3b 落地范围:ReadOnly 档 shell 默认进沙盒,能力探测失败 fail-open,单一 kill-switch**;规则集:可写根 = session cwd + /tmp + `outputs/<session>`(C6 spill 目录,**全部服务端解析,永不采信 tool 参数路径**——CVE-2025-59532 铁律);exec 允许面 = PATH 解析目录 ∪ /dev ∪ /tmp ∪ 可写根 ∪ 探测工具链目录,**显式不含 /init 与 /mnt/c**(WSL interop 收口);设备节点 per-file WRITE_FILE(/dev/null 等六节点);前台 shell + 后台 run_background_shell 两条 spawn 路径 pre-exec 施加,超时/管道排空/PGID/safe env/截断契约全保持。
- **决策 — 泛化性:能力探测 + fail-open 阶梯(spec)**:内核侧事实(有无 Landlock ABI/seccomp)用户态探测,不在部署面假设;SBX-001 跨平台编译债登记(P3b 非 WSL 环境开发时再启动);审计落 `SandboxedShellExecution`(AuditKind 第 29 变体,payload 带 command_sha256_12 前缀——不存全命令,全文由 tool_executed 行承载)。
- 任务:`08-31-a2-p3a-sandbox-spike` / `08-31-a2-p3b-sandbox-executor`。spec:`backend/sandbox-executor.md`;spill 目录交集引用 `agent-loop-architecture/pattern-output-truncation.md`。

### 2026-08-31 — 定时任务目标 session 三档(per_run 每次执行新建 session)

- **Context**:用户直接请求:定时任务目标 session 多一档「每次执行都是新的 session」+ 目标 session 前端 UI 重设计。现状两档(指定既有 chat session / 创建时新建专用固定 session),概念并列不清、专用档模型选择悬在远处。
- **决策 — `scheduled_tasks` 加三列 + 不变式**:`target_mode`('fixed'|'per_run',默认 'fixed')+ `model_id`(per_run 每次建 session 绑定的模型)+ `last_run_session_id`(无 FK,最近一次 run session,审计锚点 + 列表展示);`target_session_id` 可空化(per_run 恒 NULL,**不指向 run session**——删除旧 run session 不得级联删任务);CHECK `(target_mode='per_run' OR target_session_id IS NOT NULL)`。
- **决策 — 存量库表重建迁移(去 NOT NULL 无法 ALTER)**:沿 `rebuild_turn_trace_with_run_id` 先例,事务内 rename→create→copy(target_mode='fixed')→drop→reindex(崩溃残留 `scheduled_tasks_old` 守卫)。
- **决策 — fire 链零改动 + 审计锚点分流**:per_run 的 session 创建发生在调度 tick 内 fire seam 之前(`FireContext.target_session_id` 保持 String,seam 类型与全部既有测试替身零改动);fired/catchup/error 审计挂新 session,`error` reason 增 `session_create_failed`(不重试风暴);per_run 不受「同 session 每 tick 一 fire」与队列去重约束(message_queue_enabled=false 照常 fire——legacy cancel+replace 危害对全新 session 不存在);LLM `schedule_task` tool 路径不暴露 per_run(恒 fixed 语义)。
- **决策 — 前端 radio 卡片三档(创建态)/ 两档(编辑态)+ 就近模型选择**:AC11-15(reka-ui RadioGroupRoot 先例 DefaultTab,移动端 320-430px 无溢出,触控目标合规;切档清空/回填规则:切 per_run 清空固定绑定,切回 fixed 必须选 session)。
- 任务:`08-31-sched-per-run-session`。spec:`backend/scheduled-tasks.md`(per_run 契约)+ `frontend/`(表单三档 UI)。LLM schedule_task 面不变(AC10)。
