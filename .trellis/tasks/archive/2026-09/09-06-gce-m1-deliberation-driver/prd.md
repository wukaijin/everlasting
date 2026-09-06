# GCE-M1 群聊审议驱动:脚本引擎 + skill 指引路径

> 来源:[GROUP-CHAT-API-ROADMAP.md §2](../../../../docs/GROUP-CHAT-API-ROADMAP.md) M1;2026-09-06 立项讨论定形三层:**平台 = daemon 群聊原语(M0 已交付)/ 引擎 = 脚本(确定性、可测、M2 共享)/ 指引 = skill(LLM 门面,零逻辑)**。

## Goal

把「curl 建群 → 发题 → 轮询 busy/stop_reason → 读 summary → 导转录 → 清理」的人肉流程固化为一等驱动入口:LLM/人/cron 说清议题与参与角色,一条命令拿回有据可查的共识结论;skill 让不带先验知识的 agent 沿指引路径快速组装建群信息(目录/议题/参与者)。

## Confirmed Facts(代码/文档实证)

- **建群契约**:`create_session(project_id, initial_cwd, session_type="group_chat", metadata={participants:[{name, model, persona_md?}]})`;moderator 模型 = session 自身 `model` 字段,participants 列表不含 moderator。出处:`app/src-tauri/src/commands/sessions.rs:56-80`、`app/src-tauri/src/agent/group_chat.rs:11-12,59-69`。
- **发起编排**:`POST /api/v1/agent/chat` body `{request_id, session_id, messages:[{role:"user", content:<topic>}]}`(新 session 单条 wire 即可;turn-smoke.sh:140-176 先例)。编排运行在该 chat 请求内,终态信号见下。
- **轮询语义(独立于 SSE)**:`list_sessions` → `busy` + `stop_reason`(`group_chat_end`/`max_rounds`/`cancelled`/`error`,编排级粒度,轮间空隙不回落);`load_session` → `discussion_summary` 一等字段。契约:DAEMON-API.md §4。
- **中断停编排但保 session**:用 `cancel_chat`(commands/cancel.rs:88,daemon 路由 `cancel/cancel_chat`),**不是** turn-smoke 的 delete_session(那个会腰斩+删 session)。
- **GC3 语义约束**:有活跃 SSE 订阅者 → 权限 ask 等 120s;无订阅者 → 8s 快拒。驱动脚本**不挂 SSE 连接**即保住 8s 快拒,无人值守不被审批卡(第一场 10min 卡审批即此病灶)。
- **项目解析链先例**:turn-smoke.sh:119-138——`projects/list_projects` 按路径匹配 → 查不到 `create_project` → 建 session 传 `initial_cwd`。
- **后台 shell 基础设施已在**(2026-09-02 session 53):registry + `background_shell:update` 事件 + ActivityPanel 可观测——AC5 嵌套消费时外层 agent 用后台 shell 起 5-15min 的 run,不阻塞自己的轮次。
- **模型目录**:`providers/*` 端点存在(commands/providers.rs)。
- **转录导出无既有工具**:out/group-chat-*.md 三份均人肉制作 → 导出为本任务真正的新增工作。
- **两场 live 已验证输出质量**:`out/group-chat-gc-fix-verify-20260905.md`(session 60bcb778)、`out/group-chat-d1d2-verify-20260906.md`(session eb14d2df,9m11s,stop_reason=group_chat_end)。
- **约束**:M2 MCP 工具与 M1 驱动共享同一实现层(roadmap §3);v1 零 DB schema 变更(roadmap §7)。

## Requirements

### R1 脚本引擎 `scripts/group-chat-run.mjs`(零 npm 依赖,fetch 用 Node 内建)

- **内省三查询**(建群所需全部运行时事实;LLM 不读文档、查脚本):
  - `projects` — 列项目目录(转查 `projects/*`);
  - `models` — 列 provider 模型目录(转查 `providers/*`);
  - `presets` — 列内置预设 + 每个可覆盖的 participant/model/persona 字段。
- **`run` 子命令**:
  - `--project <path>`:**目录为建群第一要素**(证据基地);默认当前仓库;不在项目列表自动创建(turn-smoke 先例);
  - `--topic <text>` / `--topic-file <path>`;
  - `--preset review|arch|retro`(评审团/架构决策/复盘,脚本内置常量)+ `--participants <json>` 内联;`--moderator-model`(映射 session model);
  - 覆盖粒度三级(AC3):participant 增删 / 单人 model / 单人 persona_md;
  - 进度:周期轮询 `list_sessions` 打一行(轮次/当前 speaker/耗时),`--quiet` 静默给 cron;**全程不挂 SSE**;
  - 终态:退出码区分 stop_reason(0=group_chat_end / 2=max_rounds / 3=cancelled / 4=error);
  - 转录:导出 markdown 至 `out/group-chat-<slug>-<ts>.md` 默认,`--out` 覆盖;格式对齐既有三份(seqN speaker 正文);
  - session 处置:正常收官与中断退出均**默认保留**(`--cleanup` 仅成功路径删 session);中断现场是 post-mortem 依据,session 是 M4 讨论库 DB 主源;
  - 中断(默认 30min 超时,`--timeout` 可调 / SIGINT):`cancel_chat` 停编排 → 导出部分转录 → 按 stop_reason=cancelled 语义退出;
  - `--dry-run`:解析 project/preset/override 并打印将发出的请求体,不建 session 不发 chat——零成本冒烟,锁参数组装逻辑(回归闸 C3 机制层)。
- **预算参数不做**:硬停依赖群聊内部共识 C1.2(`stop_reason=budget`),未落地前 CLI 不占位。

### R2 skill 门面 `.agents/skills/group-chat/`(薄,零逻辑)

四块内容:① 慎用警告(一场 5-15min、数十万 token)+ 适用场景(评审/架构决策/复盘);② 议题与 persona 写法微指南(两场 live 核心经验:议题质量决定输出质量);③ 预设配方选择指引;④ 结果解读(`discussion_summary` 字段、转录落点、stop_reason 四值)+ 内省三查询速查。所有执行都调脚本,SKILL.md 不内嵌会漂移的事实(模型清单/预设字段以脚本内省输出为准)。

### R3 文档接线

DAEMON-API.md 加一节链接脚本与 skill(脚本即文档);AGENTS.md 冒烟速查区补一行入口指引。

## Acceptance Criteria

- [ ] **AC1** 一条命令完成一场完整审议并落盘转录 markdown(live 实跑验收)。
- [ ] **AC2** 中断退出(超时/SIGINT)→ 编排经 `cancel_chat` 停止、部分转录落盘、session 默认保留;`--cleanup` 仅作用于成功路径。
- [ ] **AC3** 预设可被覆盖:`--participants` 整名单替换(增删参与者的唯一方式,语义超集)+ `--set` 单人 model / 单人 persona 两级 + `--moderator-model`。(2026-09-06 评审团 verdict 修订:原「--add/--drop 增删旗标」砍掉——9 个 throw 的主要来源,整名单替换已覆盖该语义。)
- [ ] **AC4** 两场已验证场景可复现为脚本冒烟(`--dry-run` 参数级 + 至少一场 live 实跑)。
- [ ] **AC5(最终验收:daemon 内「套娃」消费)**:在 daemon 里对一个**非 everlasting 项目**开普通单聊,用户仅给一句指引(脚本绝对路径 +「召集一场关于 X 的审议」),该 LLM 自行用内省命令组装并完成审议、读回结论——外层 agent 建议用后台 shell 起 5-15min 的 run,轮询输出拿转录路径。开发期自测臂:本仓库 ZCode 会话仅凭 skill 完成一场。
  - 架构依据:outer chat loop → shell → HTTP → 独立 session 的群聊编排,两层进程/API 解耦,非 loop 嵌套;daemon 多 session 并发(GUI/remote/F2 共存)是既有设计。并发两 session 同跑为该测试 live 验证点之一。

## Out of Scope

- MCP 四工具(M2);打断/注入/实时跟随与 SSE follow 透传(M3);任何 DB schema 变更;预算硬停(上游 C1.2);用户预设文件管理;GUI 改动。

## Decisions(brainstorm 2026-09-06)

1. 脚本 vs skill:**两者都要但分层**——脚本为核(冒烟/cron/MCP 共享/GC1-7 教训:生命周期语义锁死在一份确定性实现),skill 为薄门面(LLM 编排智能:何时召集/议题写法/结果解读);编排逻辑绝不进 SKILL.md 让 agent 裸跑 curl。
2. session 处置:默认保留 + `--cleanup` 成功后删(M4 讨论库资产视角;对照 turn-smoke 高频冒烟即删,审议低频高价值)。
3. 进度呈现:轮询打印 + `--quiet`;不挂 SSE(保 GC3 8s 快拒)。
4. 转录落点:`out/` 默认 + `--out`(延续既有三份现状)。
5. 预设存放:脚本内置常量(用户自定义文件 YAGNI,AC3 覆盖已满足个性化)。
6. skill 命名:`group-chat`(对齐 ui-review 按对象命名惯例)。
7. **AC5 测试法(用户提议,2026-09-06)**:daemon 内非 everlasting 项目单聊「套娃」消费——外层 LLM 仅凭一句指引 + 脚本内省完成审议。配套两个实现约束:转录 `out/` 默认解析为 everlasting 仓库根(按脚本位置,非 CWD),结束打印绝对路径;`--detach`/`status`/`result` 异步形状**不做进 M1**,划 M2(此测试即其需求验证)。

## 验证记录(2026-09-06)

- **AC1/AC4 ✅**:live 全程跑通(session `aeac878a`,13m51s,exit 0,`discussion_summary` 落库;转录 `out/group-chat-群聊审议驱动-…-20260905191258.md`)。议题 = 评审团评审 GCE-M1 本身(狗血测试),产出直接反哺本任务(见下 verdict)。
- **AC2 ✅**:60s 超时中断臂——cancel 停编排、部分转录落盘、session 保留、exit 3(session `45d2dffa`)。
- **AC3 ✅**:dry-run + 单测双重验证(整名单替换 / 单人 model / persona 文件覆盖;`node --test scripts/group-chat-run.test.mjs` 8/8)。
- **AC5 ✅(2026-09-06 二跑通过,用户令重试)**:daemon 单聊(vue3-cms 项目,session `511d62ed`)→ 外层 LLM 仅凭指引完成全链:内省组装 → 后台跑驱动脚本 → 沙箱升级链自动脱沙箱(errno→strerror 修复生效)→ 群聊编排与外层轮询**并发同跑**(live 验证点)→ 转录落盘 → discussion_summary 高质量转述(评审真 grep 了 vue3-cms,file:line 级证据)。首轮中止时挖出的两个真缺陷均已修复/澄清:①错误文案缺 OS 签名 → classify_block 死锁(已修);②prefix 授权语义 = **命令首词 basename**(存全路径是死数据,GUI AllowAlways 同语义)——授权改为 `match_value=node` 后直通。
- **AC5 衍生发现(记入 M2 论据)**:①**双重触发隐患**:后台壳升级重跑换新句柄,agent 轮询旧句柄见 Failed 后手动重发 → 两场审议并发(本次实际跑了 3 场:1 场有效 + 1 场过时排队消息触发 + 1 场双发);②**过时排队消息**:外层 busy 期间排队的用户消息在 turn 边界送达,可触发重复工作(M4 定时/嵌套设计须考虑消息时效);③`--out` 相对路径落调用方 cwd——语义正确(转录跟着审议对象项目走)。

## 评审团 verdict 采纳记录(2026-09-06 live,discussion_summary 见 AC1 转录)

- **三条马上修 → 已落地**:①失败路径落转录(pollLoop 错误 catch 后仍导出,stop_reason 不篡改,exit 1);②PERSONA_COMMON(公共纪律单源,persona 只留视角边界;删上版「BUGLIST」引用,persona 与议题无关约束入注释);③转录三修+单测(blockquote 隔离碎格式 / 工具轮证据链 `summarizeToolUses` / summary 缺失警告落文件;`node --test` 8 用例)。
- **最该砍 → 已落地**:--add/--drop 移除(--participants 整名单替换为增删唯一方式),cmdPresets / run --help / SKILL.md / AC3 四处同 diff 收口。
- **延后(follow-up)**:per-speaker token——turn_trace 按 LLM 调用段落落库,与 speaker 对齐有歧义,硬 join 会产错数;需群聊内部先在 trace 行打 speaker 标签(依赖矩阵记入 roadmap §6 同类)。8s 快拒计量落转录头,同类延后。
- **采纳小项**:--topic-file 主推(run --help + SKILL.md);议题写法补「别把答案写进问题」;轮询 ±10s 精度写 skill 边界;dry-run 双态简化为纯静态模板(回归职责移交单测);normalizeModelRef 两趟解析(精确名优先,防 catalog 内 glm-5.3/GLM-5.3 大小写撞车)。
