# 群聊外部调用 / 审议原语 — 实施路线图

> **状态(2026-09-06 立项;2026-09-08 更新)**:目标与验收已定。**M0-M3(地基 / 流程固化 / MCP 接口层 / 控制面)已交付并逐项定案;M4 三子项定时审议(M4a)/ 讨论库检索(M4b)✅ 2026-09-07、成本治理(M4c)✅ 2026-09-08 交付**——各里程碑的「待定决策」已全部定案;仅余远程暴露认证(立项前须安全评审)与部署面 follow-up。本文回答「做什么 / 为什么 / 怎么算完成」;「怎么做」在各里程碑定案后记入交付段。
> **定位**:群聊对外部调用方(GUI 之外的 daemon API 消费者、脚本、其他 AI agent)成为**可编程的多模型审议原语**:一个调用方说清议题与参与角色,拿回一份有据可查的共识结论。两场 live 实跑(session `60bcb778` 09-05 / `eb14d2df` 09-06,后者 9m11s 收官并现场抓出代码缺陷)已验证该原语的输出质量与驱动可行性。
> **关联**:[DAEMON-API.md](./DAEMON-API.md)(API 契约,本路线图的地基)/ [BUGLIST-group-chat.md](./BUGLIST-group-chat.md)(GC1-GC7 + §4 D1-D3,地基缺陷修复记录)/ [BACKLOG.md 附录 B](./BACKLOG.md)(N-x 候选;第二场讨论的止损包/证据链/回归闸共识由本文 §6 吸收)/ [ROADMAP.md §2 第四档](./ROADMAP.md)

---

## 0. 总览

| 里程碑 | 一句话 | 状态 | 规模感 |
|--------|--------|------|--------|
| M0 地基 | lifecycle 三态机 + summary 一等字段 + 无人值守安全 + API 契约文档 | ✅ 2026-09-05/06(见 §1) | — |
| M1 流程固化 | 驱动脚本 + 角色预设:一场 headless 审议 = 一条命令 | ✅ 2026-09-06(见 §2;含嵌套消费验收) | 小(1-2 天) |
| M2 MCP 接口层 | 外部 AI agent 可召集审议:四工具 `start/status/result/cancel`(M3 扩 `interrupt_discussion`/`inject_message` 成六) | ✅ 2026-09-06(见 §3) | 中(2-4 天 + 协议细节) |
| M3 控制面 | 打断 / 注入 / 实时跟随——讨论可驾驶(上游依赖群聊内部 P0 共识) | ✅ 2026-09-06(见 §4;P0 前置同日落地) | 中 |
| M4 运营治理 | 定时审议、讨论库检索、成本核算与上限、远程暴露认证 | 🟡 前三子项 ✅(M4a 定时审议 / M4b 讨论库检索 2026-09-07;成本治理 M4c 2026-09-08,§5);仅余远程暴露认证(立项前须安全评审) | 大(多子项) |

推进原则(沿用 remote-access 先例):每个子阶段 ① 能独立提交 ② 有明确验证标准 ③ GUI/经典聊路径零行为变化。

## 1. M0 已完成地基(2026-09-05/06)

全部 live 验证过,外部调用的**读侧**契约已闭环:

- **lifecycle 三态机**(GC1/GC2):`busy=true` 进行中(编排级,轮间空隙不回落);`busy=false + stop_reason≠null` 终态(`group_chat_end` / `max_rounds` / `cancelled` / `error`);复用 session 二跑先清残留。
- **结论一等字段**(GC7):`discussion_summary` 经 `load_session` 一次调用可得,共识清单无需解析 tool_result。
- **无人值守安全**(GC3 + D1):无 SSE 观察者时权限 ask 8s 快拒;群聊 prompt 已注入 working directory(第二场 moderator 全程相对路径、零审批,开场调研 14s vs 第一场 10min 卡审批)。
- **工具层正确性**(D2):grep 相对 glob 修复,公共工具层单 agent 同样受益。
- **契约文档**:[DAEMON-API.md](./DAEMON-API.md)——命名约定、字段对照、群聊生命周期消费指南。

两场实录为 gitignored 本地产物(M1 落账后已随 out/ 清理),live 记录见任务目录 [09-06-gce-m1-deliberation-driver](../.trellis/tasks/archive/2026-09/09-06-gce-m1-deliberation-driver/)(implement.md / review.md)。

## 2. M1 流程固化(✅ 2026-09-06 交付)

**交付**:`scripts/group-chat-run.mjs`(引擎:内省三查询 `projects`/`models`/`presets` + `run` 一条命令全链路 + 中断 cancel + 转录导出 + `--dry-run` 纯静态模板;纯函数区 8 用例 `node --test`)+ `.agents/skills/group-chat/`(LLM 指引门面)+ DAEMON-API.md §6。验收 AC1-AC5 全部 live 通过——**AC5 嵌套消费**(2026-09-06 二跑):daemon 单聊经指引唤起群聊,沙箱升级链自动脱沙箱(两个前置修复:错误文案 errno→strerror 翻译喂 classify_block;prefix 授权按**命令首词 basename** 匹配),群聊与外层并发同跑,summary 转述回单聊。嵌套衍生发现记 M2 论据:后台壳升级重跑换句柄致 agent 双发重试(跑出并发双场)、过时排队消息可触发重复工作。评审团 live verdict 已消化(砍 --add/--drop、PERSONA_COMMON、转录三修、失败路径落转录);per-speaker token 延后(需群聊内部 trace 行打 speaker 标签,见 §6 依赖表)。待定决策定夺记录在任务 PRD(`.trellis/tasks/09-06-gce-m1-deliberation-driver/`)。

- **目标**:把「curl 建群 → 发题 → 轮询 busy/stop_reason → 读 summary → 导转录 → 清理」的人肉验证流程固化为一等驱动入口;任何消费方(人 / cron / 未来的 MCP 层)复用同一实现。
- **交付物**:驱动脚本 + 2-3 个角色预设(评审团 / 架构决策 / 复盘)+ 流程指引文档(脚本即文档,DAEMON-API.md 加一节链接)。
- **验收标准**:① 一条命令完成一场完整审议并落盘转录 markdown;② 中断退出(超时/取消)不留脏 session(或按参数保留并说明);③ 预设可被覆盖(participant/model/persona 级);④ 两场已验证场景可复现为脚本冒烟。
- **待定决策**:
  - 脚本形态:`scripts/group-chat-run.mjs`(Node,随 remote-e2e-smoke 先例)vs Rust 小 bin(进 crates/)vs daemon 内置子命令(如 `everlasting-daemon group-chat run`);
  - 预设存放:脚本内置常量 vs `~/.config` 用户文件 vs DB 表;
  - 转录导出格式与落点(延续 `out/group-chat-*.md` 现状 vs 可配置);
  - 讨论中立的进度呈现(轮询打印 vs 可选 SSE follow 透传)。

## 3. M2 MCP 接口层(✅ 2026-09-06)

**交付**:`scripts/group-chat-mcp.mjs`(stdio server,SDK 1.30;四工具与 M1 引擎共享实现层,纯逻辑区零 SDK import)+ 宿主挂载(2026-09-06 迁**用户级** `~/.zcode/cli/config.json` 的 `mcp.servers`——仓库级 `.agents/mcp.json` 已移除,workspace 作用域跨项目不可见)+ `scripts/group-chat-mcp-smoke.mjs`(非 live 零成本 / `--live` 全链)+ 单测 13 用例(含 AC4 wire 预算锁:实测 1945 字符 ≈486 token < 2300)。五决策收敛:Node+SDK 薄包装 / stdio(M4 前不加网络面)/ 四工具+context 预算约束(用户原话「llm 初始 context 占用不要太大」)/ v1 零鉴权(本机)/ `metadata.created_via` 三通道归因("mcp"/"script"/缺失=GUI)。关键设计:记账(session→request_id/project_id)XDG state 写穿兜底宿主会话生灭;惰性转录 status/result 双入口、导出失败降级不污染轮询;重启兜底链两级(busy 只在 list_sessions 富化)。验收 AC1-AC6 全过:live 全链(spawn→start→poll→result,session b4ce0a94,40s 收官,summary 带 file:line 证据,转录落仓库根 out/)+ created_via live 落库抽查 + daemon 零改动(git diff 空)。宿主挂载余项:ZCode 新会话见工具为一眼验证(挂载机制经真 spawn + 插件同款配置形状验证);Claude Code 宿主路径被用户侧代理模型故障阻断(非本项目问题)。评审(另一模型 planning 门)9 成采纳、3 处驳回有据(P3-1 行号判定误读/P3-5 jsonl gate 规则套错平台/P3-4 JSON 内注释不可行),见任务目录 review.md。**已知边界**:部署面绑定源码检出(挂载配置绝对路径指向仓库),无源码机器不可用——记 §5「MCP 部署面」follow-up;**该边界已于 2026-09-06 由 §5 路径②(standalone bin,`scripts/group-chat-mcp-deploy.mjs`)收口**。(注:本段工具数/预算/用例数为 M2 交付时快照——2026-09-09 现状:M3 起六工具、wire 预算锁 3200(09-08 gce-m4c 加 `token_budget` 参后 3115;09-09 加 `fe_review` 预设后 3162)、单测 18 用例,见 §4。) 

- **目标**:任何 MCP 宿主(ZCode / Claude Code / Cursor 等)里的单 agent 可召集跨模型审议——**这是单客户端无法自制的原语**:模型目录、persona 隔离(role_history)、转录持久化、生命周期语义全在 daemon。
- **工具面**(草案即定稿):
  | 工具 | 语义 |
  |---|---|
  | `start_discussion` | topic + cwd + preset?/participants?(名单替换,moderator 恒取预设)→ **立即**返回 session_id |
  | `discussion_status` | busy / stop_reason / elapsed_s(廉价轮询,无轮次字段),供调用方轮询 |
  | `discussion_result` | 终态读 summary + roster + stats + 转录路径(未终态时明确报错而非空值) |
  | `cancel_discussion` | 复用现有 cancel 端点(M3 preempt 落地前的唯一止损) |
- **关键语义**(已写死在工具描述):讨论耗时 5-15 分钟,**工具调用绝不阻塞**——start 立即返回,状态靠轮询;一场消耗数十万 token,调用方 agent 应慎用、议题要值得。
- **五项「待定决策」全部定案**(2026-09-06 brainstorm,记录在任务 PRD):Node+SDK / stdio / 四工具+context 预算 ≲600 token / v1 零鉴权 / created_via 归因;后续项挂 M4(transport 扩 http、鉴权、宿主自报身份)。*(M3 09-06 起工具面扩为六,wire 预算锁同步上调 3200,见 §4)*

## 4. M3 控制面:打断 / 注入 / 跟随(✅ 2026-09-06)

**交付(task `09-06-gce-m3-control-plane`,daemon 零 Rust diff 达成)**:MCP 六工具面
新增 `interrupt_discussion`(preempt 端点 1:1 + 轮询指引)/ `inject_message`(fireChat
薄包装 + **前置 busy guard**);GUI 打断按钮(群聊 chip 区,讨论进行中可见,与 API 同权同
语义;Stop/Esc 硬取消与打字注入零回归);[DAEMON-API §6.2](./DAEMON-API.md) SSE follow
消费专章(裸透传:全局流 / chat-event kind 全集 / session_id 过滤 / Last-Event-ID 重放 /
stream-resync→snapshot / follow 连接使 ask 走 120s attended 的注意事项)。wire 预算
2300→3200 两侧锁同步(实测 ≈2876 chars ≈719 token)。**关键设计(评审 P1-1)**:已收官
群聊误注入会重启编排器抹旧场终态(cancel 救不回)→ guard 在客户端层根本不发起;
fireChat 后 acceptance 非 `injected` → 自有 rid cancel 竞态兜底。验收 live 两场(session
`2ba779ec` / `dc9f518b`):preempt 受理 ✓、注入 `[用户插入]` + `metadata.kind=user_inject`
落库且被 moderator 收束轮吸收 ✓、终态分别实测 `preempted` 与 `group_chat_end`(后者正是
P2-1 预测的同刻自然收官竞态,AC 断言按此放宽)✓、summary 一等字段完整 ✓、SSE 按文档
消费(含参与者 speaker)✓。

- **目标**:讨论从「放电影」变「可驾驶」——外部调用方(与 GUI 用户同权)能打断当前 speaker、注入新议题、实时跟随。
- **上游依赖(2026-09-06 更新)**:**P0 打断最小语义已落地**(task `09-06-gc-p0-preempt-min-semantics`,spec [pattern-group-chat-preempt-inject](../.trellis/spec/backend/agent-loop-architecture/pattern-group-chat-preempt-inject.md)):注入 = busy 消息进 controls 缓冲非破坏投递(wire `ChatAcceptance::Injected`);打断 = `preempt_group_chat(session_id)` 收束式(`stop_reason=preempted` + summary,失败兜底立断)——M3 `interrupt_discussion` 的内核即此命令 1:1。**P1a checkpoint 落库 + P1b 续跑已落地(✅ 2026-09-06,task `09-06-gc-p1a-checkpoint-resume`;提交 09-07 凌晨,沿用 task 日 09-06)**:`group_chat_checkpoints` 表(轮头 upsert round+GC5 streak)+ boot sweep(load_inner 标 `interrupted` / 清终局孤儿行,不动 updated_at)+ `resume_group_chat` 命令五类校验(daemon 路由 + Tauri 同权)+ 轮预算继承 + moderator 恢复指令 + GUI 续跑按钮;live 三链验证(SIGKILL → interrupted → resume → group_chat_end;转录为本地产物不入库,任务目录 [09-06-gc-p1a-checkpoint-resume](../.trellis/tasks/archive/2026-09/09-06-gc-p1a-checkpoint-resume/) implement.md Step 记录完整链路)。
- **验收标准**:① headless 打断后 stop_reason 可区分且 summary 不丢;② 注入消息的 role 归属符合 P0 schema 决议;③ GUI 与 API 两条路同权同语义。
- **待定决策全部定案(2026-09-06 brainstorm)**:~~preempt 到达后的收束策略~~ 已决(P0):等在途 speaker 完 → moderator 收束轮 + 兜底立断;~~follow 暴露形态~~ **已决:裸 SSE 透传**(仅补消费文档,daemon 零改动;聚合进度事件无真实消费方撑需求,不做);~~打断的权限粒度~~ **已决:沿用 v1 零鉴权全域可打断**(与 M2「v1 零鉴权(本机)」同构;粒度机制推迟 M4 随远程认证一体议,记 §5)。

## 5. M4 运营治理(🔴)

多子项,各自独立立项:

- **定时审议**:cron 定期召集(每周架构复盘 / 发布前评审)——**✅ 2026-09-07 交付(task `09-07-gce-m4a-scheduled-deliberation`;同日 live 验证通过:完整周期 fire→收官→转录落盘→SSE done、SIGKILL 中断自动 resume 续跑无缝、僵尸场零 token 恢复,记录见任务 implement.md Step 7)**:trigger 拓扑 = **daemon 原生**(scheduled_tasks 新增 `target_mode="group_chat"` + `group_chat_config` 展开配置,fire 直接建群发题,零外部 cron);容错 = 四态路由(busy 跳过+审计不计数 / interrupted 自动续跑(P1a 地基的预期消费方)/ interrupted 无 checkpoint 审计 error 本期不动绝不双开场 / 僵尸 round≥30 与停摆场补 finalize(error) 恢复 / 终态开新)+ catalog 预检;计数矩阵对齐 F2b(全臂消费 due,run_count 只计真开跑);产物 = **落盘 + GUI 通知**(转录自动导 `{app_data_dir}/discussions/`,收官单 toast;飞书推送不在本期,归 B10);议题 = v1 静态文本;preset 单一事实源抽 `scripts/group-chat-presets.json`(M1 脚本 + 前端 vite import 共享,daemon 零 preset 概念);LLM `schedule_task` 工具不开放群聊档。评审(MCP 跨模型审议,session `f60e1212`)5 P1 全部织入设计。契约见 [DAEMON-API §6.3](./DAEMON-API.md)。
- **讨论库与检索**:历史审议(转录 + summary)可检索复用——**✅ 2026-09-07 交付(task `09-07-gce-m4b-discussion-search`)**:场级检索 = `sessions` 行直读 + 程序 LIKE,不建 FTS/触发器/冗余表(数据量低;场数上 10^3 再按 database-guidelines FTS5 模板升,查询层契约不变);消费入口 = **GUI 独立「讨论库」面板**(Sidebar 群聊区入口;空关键词浏览全部场 / 关键词命中 title/summary/task_name/participants / 项目 + stop_reason 筛选 / 点行跳回完整会话)。查询层与入口分离(daemon API + Tauri cmd 薄暴露,agent/M1/MCP 后续可复用)。契约见 [DAEMON-API §3](./DAEMON-API.md) 两新端点。
- **成本治理**:per-discussion token 核算(turn_trace 已有 per-turn 数据)+ 预算上限硬停——**✅ 2026-09-08 交付(task `09-08-gce-m4c-cost-governance-modal-redesign`)**:①硬停半 = C1.2 的 `stop_reason=budget`(09-08 止损包交付,metadata 键 `token_budget`);②声明面 = **四通道全通**(GUI 建群弹窗【随弹窗重设计:preset 优先单弹窗 + 主持人可选 + 成本区】/ M1 script `--token-budget` / MCP `start_discussion` 可选参【wire 实测 3115 < 锁 3200】/ M4a 定时 `group_chat_config.token_budget`【fire 仅声明时写键】),语义统一「显式声明才限,缺省无键」;③核算半 = 零新存储三层消费面(daemon 查询 `group_chat_token_usage`【`turn_trace` JOIN `messages.speaker`,四计费字段求和,GUI edit 弹窗成本区】+ 讨论库 hit `total_tokens`【GUI 列】+ script/MCP 客户端聚合【`aggregateTokens` 共享纯函数,转录统计行 / `discussion_result.stats.tokens`】);两场实测:既有场 live 核算 243.4万 billed tokens 端到端贯通。默认档($ 换算 / preset 级推荐预算 / 全局上限)共识缓做,真实出险再立。GUI 弹窗重设计同任务交付:preset 单选卡(选中预填阵容,persona 组装与 script 逐字同形)+ 主持人 Select(wire 原生支持,此前 GUI 从未传)+ 预算输入带量级提示 + edit 成本区(per-speaker 消耗 + 缓存率合并行 + 预算进度条,超额红告警)。
- **远程暴露认证**:MCP/API 走 remote/tunnel 时的鉴权与降级——必须吸收 BACKLOG 附录 B「隧道来源降级」条目的安全论据与既有用户决策(PWA 全权 vs 分层),**立项前先过一次安全评审**。M3 决议(2026-09-06)落账:打断/注入的权限粒度机制在此一并议(本机零鉴权前提下全域可打断;远程暴露时粒度才有意义)。
- **MCP 部署面(2026-09-06 记,M2 收官后用户指认;✅ 路径② 2026-09-06 落地,task `09-06-gce-mcp-standalone`)**:M2 四工具是**仓库产物**(脚本 + node_modules + 指向源码检出内绝对路径的挂载配置),不随 daemon 分发——只有 daemon bin、无源码的机器上 MCP 层为零(仅剩 M0 裸 HTTP 原语)。这是 D1 的有意识取舍(v1 消费语境 = 本机 dev 工作流),但构成「任何宿主」终态的部署缺口。收口路径:①零成本接受(现状边界);②**单文件可执行(推荐,可独立小项提前做)**:bun/deno compile 打 standalone 二进制随 app 分发,安装器写 user-scope MCP 配置(免 node、绝对路径由安装期产生不进 git,JS 单实现保留,M2 纯逻辑零改动);③daemon 内置 streamable-http MCP endpoint(终态最干净,零外部依赖,也是远程暴露的天然载体;代价 = Rust 背协议 rmcp + 与 JS 引擎双实现,届时 JS 层降级为 dev 工具)。**落地记录(路径②)**:`scripts/group-chat-mcp-deploy.mjs` 一条命令 = bun compile standalone bin(落 XDG data 根 `bin/`,免 node/免 node_modules/免源码;sidecar `build-info` 记 git rev + 时间戳诊断 stale bin)→ ZCode user-scope 配置原位替换(备份 + 幂等);`--revert` 回 node 挂载(node 挂载保留为开发态默认)、`--uninstall` 清配置与 bin;引擎一行不改(CLI 壳误判根因与 argv[1] 哨兵解法见任务 research)。**follow-up 收窄为**:跨平台编译矩阵(macOS/Windows,交叉编译 flag 未验)+ Tauri app 分发 + 其他宿主(Claude Code/Cursor)配置写入;路径③仍挂 M4。

## 6. 与群聊内部改进的关系(依赖矩阵)

外部调用体验的上游是群聊内部质量。第二场讨论(2026-09-06)的止损包/证据链/回归闸共识**在群聊内部改进线推进**,本文只记账依赖:

| 内部共识项(第二场 discussion_summary) | 外部路线图受益方 |
|---|---|
| ~~P0 打断最小语义 + preempt/inject schema~~ **✅ 2026-09-06 落地**(task `09-06-gc-p0-preempt-min-semantics`:controls 注册表 + 注入双轨标记 + preempt 收束轮 + `stop_reason=preempted`;spec [pattern-group-chat-preempt-inject](../.trellis/spec/backend/agent-loop-architecture/pattern-group-chat-preempt-inject.md)) | ~~M3 全部~~ **✅ M3 2026-09-06 交付**(`interrupt_discussion` 内核 = `preempt_group_chat` 命令 1:1;guard 补记见 spec) |
| ~~P1a checkpoint 落库 / P1b 续跑~~ **✅ 2026-09-06 落地**(task `09-06-gc-p1a-checkpoint-resume`):checkpoint 表 + boot sweep + resume 命令 + GUI 按钮;M4 定时审议容错的地基就绪 | M3 信任底座(**已收口**);M4 定时审议的容错 |
| ~~C1.1 ask-free~~ **✅ 2026-09-08 落地**(task `09-08-gc-c1-stoploss`,全讨论范围裁定) | M1/M2 的确定性(**已兑现**:外部跑零权限等待) |
| ~~C1.2 token 预算(`stop_reason=budget`)~~ **✅ 2026-09-08 落地**(同上任务) | ~~M4 成本治理~~ **✅ M4c 2026-09-08 交付**(四通道声明面 + 三层核算,§5) |
| ~~C2.1 结构化 summary(锚点 + 推测标注)~~ **✅ 2026-09-09 落地**(task `09-09-gc-c2-evidence-summary`:`end_discussion` 结构化参数 + 锚点后校验「只标注不修改」+ `sessions.discussion_detail` 列 + 四消费面——GUI 收官卡 stance 徽章 / MCP `discussion_result.detail` / M1+定时双转录 `## conclusions` 节;依赖矩阵至此清零) | ~~M2 `discussion_result` 的结论可信度~~ **已兑现**(result 带 `detail`,外部 agent 可信度分层消费) |
| ~~C3.1 机制层 CI~~ **✅ 2026-09-08 落地**(MockProvider 剧本:ask-free 拒绝路径 / budget 三破坏剧本) | 全里程碑的回归闸(已进 `cargo test --lib`) |
| ~~RULE「假注释是毒数据」~~ **✅ 2026-09-08 进 spec**(`.trellis/spec/backend/quality-guidelines.md`) | 全部(三层生效) |

## 7. 明确不做(本路线图边界)

- **同步阻塞工具**:MCP 工具永不内置「等讨论跑完再返回」的形态(等价于把 5-15 分钟的不可中断等待塞进宿主 agent 的工具调用)。
- **跨节点/多 daemon 编排**:参与者模型仍解析自本 daemon 目录,不做联邦。
- **v1 不新增 DB 表**:M0-M2 零 schema 变更;M3 checkpoint 需要时随内部 P1a 立项,不挂本文档。
- **不做完整 OpenAPI 生成**:DAEMON-API.md 手册式契约已覆盖消费场景,自动化 schema 生成待真实需求出现再议。
