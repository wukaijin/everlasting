# 群聊外部调用 / 审议原语 — 实施路线图

> **状态(2026-09-06 立项)**:目标与验收已定,**技术方案待定**——每个里程碑带「待定决策」小节列出开放问题,立项实施时逐项定夺后再补 [REMOTE-ACCESS-ROADMAP.md](./REMOTE-ACCESS-ROADMAP.md) 式的可执行拆解。本文只回答「做什么 / 为什么 / 怎么算完成」,不锁「怎么做」。
> **定位**:群聊对外部调用方(GUI 之外的 daemon API 消费者、脚本、其他 AI agent)成为**可编程的多模型审议原语**:一个调用方说清议题与参与角色,拿回一份有据可查的共识结论。两场 live 实跑(session `60bcb778` 09-05 / `eb14d2df` 09-06,后者 9m11s 收官并现场抓出代码缺陷)已验证该原语的输出质量与驱动可行性。
> **关联**:[DAEMON-API.md](./DAEMON-API.md)(API 契约,本路线图的地基)/ [BUGLIST-group-chat.md](./BUGLIST-group-chat.md)(GC1-GC7 + §4 D1-D3,地基缺陷修复记录)/ [BACKLOG.md 附录 B](./BACKLOG.md)(N-x 候选;第二场讨论的止损包/证据链/回归闸共识由本文 §6 吸收)/ [ROADMAP.md §2 第四档](./ROADMAP.md)

---

## 0. 总览

| 里程碑 | 一句话 | 状态 | 规模感 |
|--------|--------|------|--------|
| M0 地基 | lifecycle 三态机 + summary 一等字段 + 无人值守安全 + API 契约文档 | ✅ 2026-09-05/06(见 §1) | — |
| M1 流程固化 | 驱动脚本 + 角色预设:一场 headless 审议 = 一条命令 | ✅ 2026-09-06(见 §2;含嵌套消费验收) | 小(1-2 天) |
| M2 MCP 接口层 | 外部 AI agent 可召集审议:`start/status/result/cancel` 四工具 | ✅ 2026-09-06(见 §3) | 中(2-4 天 + 协议细节) |
| M3 控制面 | 打断 / 注入 / 实时跟随——讨论可驾驶(上游依赖群聊内部 P0 共识) | 🟠 等 P0 | 中 |
| M4 运营治理 | 定时审议、讨论库检索、成本核算与上限、远程暴露认证 | 🔴 远期 | 大(多子项) |

推进原则(沿用 remote-access 先例):每个子阶段 ① 能独立提交 ② 有明确验证标准 ③ GUI/经典聊路径零行为变化。

## 1. M0 已完成地基(2026-09-05/06)

全部 live 验证过,外部调用的**读侧**契约已闭环:

- **lifecycle 三态机**(GC1/GC2):`busy=true` 进行中(编排级,轮间空隙不回落);`busy=false + stop_reason≠null` 终态(`group_chat_end` / `max_rounds` / `cancelled` / `error`);复用 session 二跑先清残留。
- **结论一等字段**(GC7):`discussion_summary` 经 `load_session` 一次调用可得,共识清单无需解析 tool_result。
- **无人值守安全**(GC3 + D1):无 SSE 观察者时权限 ask 8s 快拒;群聊 prompt 已注入 working directory(第二场 moderator 全程相对路径、零审批,开场调研 14s vs 第一场 10min 卡审批)。
- **工具层正确性**(D2):grep 相对 glob 修复,公共工具层单 agent 同样受益。
- **契约文档**:[DAEMON-API.md](./DAEMON-API.md)——命名约定、字段对照、群聊生命周期消费指南。

两场实录:[out/group-chat-gc-fix-verify-20260905.md](../out/group-chat-gc-fix-verify-20260905.md) / [out/group-chat-d1d2-verify-20260906.md](../out/group-chat-d1d2-verify-20260906.md)。

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

**交付**:`scripts/group-chat-mcp.mjs`(stdio server,SDK 1.30;四工具与 M1 引擎共享实现层,纯逻辑区零 SDK import)+ `.agents/mcp.json` 宿主挂载 + `scripts/group-chat-mcp-smoke.mjs`(非 live 零成本 / `--live` 全链)+ 单测 13 用例(含 AC4 wire 预算锁:实测 1945 字符 ≈486 token < 2300)。五决策收敛:Node+SDK 薄包装 / stdio(M4 前不加网络面)/ 四工具+context 预算约束(用户原话「llm 初始 context 占用不要太大」)/ v1 零鉴权(本机)/ `metadata.created_via` 三通道归因("mcp"/"script"/缺失=GUI)。关键设计:记账(session→request_id/project_id)XDG state 写穿兜底宿主会话生灭;惰性转录 status/result 双入口、导出失败降级不污染轮询;重启兜底链两级(busy 只在 list_sessions 富化)。验收 AC1-AC6 全过:live 全链(spawn→start→poll→result,session b4ce0a94,40s 收官,summary 带 file:line 证据,转录落仓库根 out/)+ created_via live 落库抽查 + daemon 零改动(git diff 空)。宿主挂载余项:ZCode 新会话见工具为一眼验证(挂载机制经真 spawn + 插件同款配置形状验证);Claude Code 宿主路径被用户侧代理模型故障阻断(非本项目问题)。评审(另一模型 planning 门)9 成采纳、3 处驳回有据(P3-1 行号判定误读/P3-5 jsonl gate 规则套错平台/P3-4 JSON 内注释不可行),见任务目录 review.md。

- **目标**:任何 MCP 宿主(ZCode / Claude Code / Cursor 等)里的单 agent 可召集跨模型审议——**这是单客户端无法自制的原语**:模型目录、persona 隔离(role_history)、转录持久化、生命周期语义全在 daemon。
- **工具面**(草案即定稿):
  | 工具 | 语义 |
  |---|---|
  | `start_discussion` | topic + cwd + preset?/participants?(名单替换,moderator 恒取预设)→ **立即**返回 session_id |
  | `discussion_status` | busy / stop_reason / elapsed_s(廉价轮询,无轮次字段),供调用方轮询 |
  | `discussion_result` | 终态读 summary + roster + stats + 转录路径(未终态时明确报错而非空值) |
  | `cancel_discussion` | 复用现有 cancel 端点(M3 preempt 落地前的唯一止损) |
- **关键语义**(已写死在工具描述):讨论耗时 5-15 分钟,**工具调用绝不阻塞**——start 立即返回,状态靠轮询;一场消耗数十万 token,调用方 agent 应慎用、议题要值得。
- **五项「待定决策」全部定案**(2026-09-06 brainstorm,记录在任务 PRD):Node+SDK / stdio / 四工具+context 预算 ≲600 token / v1 零鉴权 / created_via 归因;后续项挂 M4(transport 扩 http、鉴权、宿主自报身份)。

## 4. M3 控制面:打断 / 注入 / 跟随(🟠)

- **目标**:讨论从「放电影」变「可驾驶」——外部调用方(与 GUI 用户同权)能打断当前 speaker、注入新议题、实时跟随。
- **上游依赖(群聊内部共识,见 BUGLIST 附录与第二场 discussion_summary)**:**P0 打断最小语义**(SharedTurnState preempt 信号 + preempt/inject schema 区分)是前置;P1a checkpoint 落库(「中断于 X」可拾起)是打断的信任底座。M3 本体只是把这些能力**经 API/MCP 暴露**,不含内部实现。
- **交付物**:daemon 端点 + MCP 工具 `interrupt_discussion` / `inject_message`(schema 遵循 P0 决议)+ SSE follow 的外部消费文档。
- **验收标准**:① headless 打断后 stop_reason 可区分且 summary 不丢;② 注入消息的 role 归属符合 P0 schema 决议;③ GUI 与 API 两条路同权同语义。
- **待定决策**:preempt 到达后的收束策略(moderator 收束 vs 立断);follow 暴露形态(裸 SSE 透传 vs 聚合进度事件);打断的权限粒度(谁能打断谁)。

## 5. M4 运营治理(🔴)

多子项,各自独立立项:

- **定时审议**:cron 定期召集(每周架构复盘 / 发布前评审)——复用 F2 定时任务基础设施 + M1 驱动;待定:产物推送形态(飞书通知走 B10 收窄形态?落地文件?)。
- **讨论库与检索**:历史审议(转录 + summary)可检索复用——待定:FTS(messages_fts 已有)够不够、要不要独立 discussion 视图表。
- **成本治理**:per-discussion token 核算(turn_trace 已有 per-turn 数据)+ 预算上限硬停——上游依赖第二场共识 C1.2(`stop_reason=budget`);待定:预算声明位置(开群参数 vs MCP 工具参数 vs 默认档)。
- **远程暴露认证**:MCP/API 走 remote/tunnel 时的鉴权与降级——必须吸收 BACKLOG 附录 B「隧道来源降级」条目的安全论据与既有用户决策(PWA 全权 vs 分层),**立项前先过一次安全评审**。

## 6. 与群聊内部改进的关系(依赖矩阵)

外部调用体验的上游是群聊内部质量。第二场讨论(2026-09-06)的止损包/证据链/回归闸共识**在群聊内部改进线推进**,本文只记账依赖:

| 内部共识项(第二场 discussion_summary) | 外部路线图受益方 |
|---|---|
| P0 打断最小语义 + preempt/inject schema | M3 全部 |
| P1a checkpoint 落库 / P1b 续跑 | M3 信任底座;M4 定时审议的容错 |
| C1.1 ask-free moderator 段 | M1/M2 的确定性(外部跑不受审批噪声干扰) |
| C1.2 token 预算(`stop_reason=budget`) | M4 成本治理 |
| C2.1 结构化 summary(锚点 + 推测标注) | M2 `discussion_result` 的结论可信度 |
| C3 机制层 CI / 行为层 turn-smoke | 全里程碑的回归闸(改 prompt/编排必跑) |
| RULE「假注释是毒数据」(进 .trellis/spec) | 全部(外部调用方读的是同一份代码) |

## 7. 明确不做(本路线图边界)

- **同步阻塞工具**:MCP 工具永不内置「等讨论跑完再返回」的形态(等价于把 5-15 分钟的不可中断等待塞进宿主 agent 的工具调用)。
- **跨节点/多 daemon 编排**:参与者模型仍解析自本 daemon 目录,不做联邦。
- **v1 不新增 DB 表**:M0-M2 零 schema 变更;M3 checkpoint 需要时随内部 P1a 立项,不挂本文档。
- **不做完整 OpenAPI 生成**:DAEMON-API.md 手册式契约已覆盖消费场景,自动化 schema 生成待真实需求出现再议。
