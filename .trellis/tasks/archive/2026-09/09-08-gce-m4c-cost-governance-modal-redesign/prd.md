# M4 成本治理消费面 + GUI 建群弹窗重设计(gce-m4c)

## Goal

C1.2 的 token 预算硬停(`stop_reason=budget`)已就位,但只有 GUI 建群弹窗能声明预算——**最需要预算帽的无人值守通道(M1 script / MCP / M4a 定时)全都透传不了**。本任务:① 补齐三通道预算声明;② per-discussion token 核算三层消费面;③ GUI 建群弹窗整体重设计(预设优先单弹窗)。

上游:09-08-gc-c1-stoploss-regression-gate(C1.2 声明机制 + 硬停语义)。ROADMAP 记账:GROUP-CHAT-API-ROADMAP §5 M4「成本治理」条目(预算上限硬停已就绪,本任务交核算 + 全通道声明面)。

## Background(2026-09-08 勘察确认)

### 预算声明现状(四通道差集)

| 通道 | 建群 body / metadata 构造 | token_budget 现状 |
|---|---|---|
| GUI 弹窗 | `createNewSession({sessionType, participants, tokenBudget})`(stores/chatSessionActions.ts:98-103)→ metadata | ✅ 09-08 已落(create+edit 都有输入) |
| M1 script | `buildCreateBody`(scripts/group-chat-run.mjs:165)metadata 固定键 `{participants, created_via?}` | ❌ 不透传 |
| MCP | `buildToolShapes.start_discussion`(scripts/group-chat-mcp.mjs:323-332)topic/cwd/preset/participants 四参 | ❌ 不透传;wire 预算锁 TOOLS_BUDGET_CHARS=3200(AC4 单测按实测锁) |
| M4a 定时 | `GroupChatTaskConfig`(db/scheduled_tasks.rs:65-69)闭结构 {moderator_model_id, participants}(未知键 serde 静默丢弃)+ fire 白名单 json!(scheduler/mod.rs:1043-1048)四键 | ❌ 不透传;GUI 定时表单(ScheduledTasksTab 第四档)也无输入 |

### 后端已就绪、硬停链路无需改动的事实

- `GroupChatConfig.token_budget: Option<u64>`(agent/group_chat.rs:69,serde default additive)——metadata 键就位,各通道只需在构造 metadata 时带上。
- 硬停语义(轮头检查、四计费字段求和 input+output+cache_creation+cache_read、`stop_reason="budget"`、前端 finalize 白名单 + notice)09-08 已全绿。

### GUI 建群弹窗现状(GroupChatConfigModal.vue,881 行)

- create/edit 双模式;字段 = 2-3 参与者(name/model/persona_md)+ token_budget(底部裸 number 输入);edit 模式另有 per-speaker 缓存率只读行 + 主持人只读区。
- **无 preset 入口**——四通道里唯一要手配阵容的。preset 单一事实源 `scripts/group-chat-presets.json`(review/arch/retro;persona kind 展开 = 边界文本 + "\n\n" + persona_common,script `composePresets` 与定时表单 `gcPersonaMd` 逐字同形);vite import,目前只有 ScheduledTasksTab 消费。
- **建群时选不了主持人**:GUI `create_session` 调用从不传 model(commands/sessions.rs:98-102 注释明说),主持人 = 全局默认模型;script/MCP/定时表单都能选主持人。`create_session_in_pool` 本就收 `model: Option<String>`(scheduler fire 传的就是它)——wire-ready。
- 参与者 2-3 上限是纯前端 D5 MVP 边界(后端无人数校验,round-robin 支持任意 vec)。
- 宿主:Sidebar「新建群聊」(create)+ ChatPanel header(edit,ChatPanel.vue:1122)。样式已对齐 modal 家族 token;移动端全屏覆盖块命中 .gcfg-content。

### Per-speaker / per-discussion token 核算(零新存储)

- `turn_trace.token_usage_json` JOIN `messages.speaker` ON (session_id, seq)——`list_speaker_cache_usage`(db/trace.rs:324)已验证该 join 模式;按 speaker / 按场聚合四计费字段是同模式变体。
- M1 交付时记的「per-speaker token 延后(需 trace 行打 speaker 标签)」实际不需要打标签——messages.speaker 已够。
- daemon HTTP 已有 `list_turn_traces`(DAEMON-API §2,camelCase)——script/MCP 可客户端聚合,daemon 核算侧零改动。

## Decisions(2026-09-08 用户裁定)

- **D1 弹窗布局 = 预设优先单弹窗**:保持单弹窗单页;create 顶部 preset 单选卡(选中即预填阵容,可展开微调),预算区带参考量级提示;edit 追加只读成本区。否掉两步向导(老手多一步)与双 Tab(预算藏层,与止损主目的相悖)。
- **D2 议题不进弹窗**:弹窗保持纯配置面,建群后仍在聊天框发首条消息开场;script/MCP 的 topic-first 是无人值守需要,GUI 不缺。
- **D3 核算面 = 三层都做**:edit 弹窗成本区 + 讨论库面板每场总消耗列 + MCP `discussion_result` stats 加 tokens。
- **D4 默认档 = 不做**:预算只在每场显式声明时生效(零静默截断风险);弹窗/定时表单给静态参考量级提示文案(如「一场通常 20-60 万 token」),不进 presets.json(避免为展示文本动共享 schema)。
- **D5 人数上限 = 维持 2-3**:与三套预设阵容一致;预算硬停已是止损主线;放开人数 wall-time 线性变长。

## Requirements

### R1 — 三通道 token_budget 透传

- **M1 script**:`run` 子命令加 `--token-budget <N>`(正整数;缺省不传 = 不限),`buildCreateBody` metadata 带键;`--dry-run` 模板输出同步显示。
- **MCP**:`start_discussion` shape 加 `token_budget: number(int, positive, optional)`(description 一句:计费四字段口径 + 超限 stop_reason=budget);wire 预算锁 AC4 重测(现 3200,实测约 2876,预计仍余量)。
- **M4a 定时**:`GroupChatTaskConfig` 加 `token_budget: Option<u64>`(serde default,additive);fire 白名单 metadata 在 Some 时插键(不写 null 键);GUI 定时表单第四档加预算输入(留空 = 不限),编辑态预算改动可独立于重选 preset 提交(现语义「未重选 preset = 存档配置不动」,预算 dirty 时携带存档配置 + 新预算整体重交)。
- 语义统一:四通道全为「显式声明才限,缺省 = 不限」,口径 = 四计费字段(与 C1.2 一致)。

### R2 — GUI 建群弹窗整体重设计(预设优先单弹窗)

- **create 模式**:① preset 单选卡区(review/arch/retro,描述文案取自 presets.json;选中即展开预填阵容);② 阵容微调区(name/model/persona_md 可改,2-3 上限,加减参与者的现有交互保留);③ 主持人 Select(preset 默认 + 可改选,写 `create_session` 的 model 参数);④ token_budget 输入 + 静态参考量级提示(留空 = 不限)。
- **edit 模式**:阵容编辑照旧(存档阵容回显;不引入 preset 重选——编辑的是既成事实);预算输入照旧;追加只读成本区:per-speaker token 消耗(计费四字段)合计 + 预算进度条(有预算时;无预算只显示各 speaker 消耗);现有 per-speaker 缓存率行并入成本区同一行展示(核算 + 缓存率两次查询,行内「12.3万 · 缓存 68%」形态,均失败降级不阻塞编辑)。
- preset 展开 = persona kind → 边界 + "\n\n" + persona_common(与 script/定时表单逐字同形);模型名 → UUID 解析复用定时表单的 resolveModelRef 逻辑(抽共享模块或提取,不复制粘贴);模型缺失 → 用户可读错误,绝不静默降级。
- 移动端全屏覆盖块适配维持(style.css @media 块)。

### R3 — per-discussion token 核算三层消费面

- **读侧聚合**(新增一处,三处复用):db 层新查询 `group_chat_token_usage(session_id)` → `{total, by_speaker[]}`(turn_trace JOIN messages.speaker,SUM 四计费字段;沿用 list_speaker_cache_usage 的全部 join 约束:role='assistant'、speaker IS NOT NULL、token_usage_json IS NOT NULL、run_id='')。命令面镜像 cache_rates 先例:Tauri command + daemon route(`POST /api/v1/sessions/group_chat_token_usage`)+ lib.rs 双注册。
- **edit 弹窗成本区**(R2)消费上述命令。
- **讨论库面板**:`GroupChatSessionHit` 加 `total_tokens: Option<u64>`(list/search 两端点同构 subquery 聚合;turn_trace SUM,run_id='' 过滤);DiscussionLibraryModal 列表加消耗展示(格式化万单位;无数据 = 「—」)。
- **MCP `discussion_result`**:stats 加 `tokens: {total, per_speaker}`——客户端聚合 `list_turn_traces` × `load_session` messages.speaker(daemon 零 Rust diff,沿用 M2「纯暴露层」先例);M1 script 转录导出同步在统计段落带总消耗(同聚合逻辑,engine 内共享)。

### R4 — 文档接线

- DAEMON-API.md:§6.1 metadata token_budget 段补三通道声明面;§6.3 scheduled `group_chat_config` 加 token_budget 键;§3 两讨论端点响应加 total_tokens;新核算端点一节。
- GROUP-CHAT-API-ROADMAP §5 成本治理条目落账 ✅(含 M4 仅余远程暴露认证的表述更新);AGENTS.md 群聊速查行同步;GCE 相关章节的 M4 状态表更新。
- scripts/ 侧若沉淀新约定(如 wire 预算锁新值)更新对应 spec(`.trellis/spec/scripts/`)。

## Acceptance Criteria

- [ ] AC1(声明面):三通道各一条通路验证——script `--dry-run` 模板 + run 冒烟显示 token_budget 落建群 body;MCP smoke 断言 shape 含新参且 wire 锁 AC4 重测通过;定时场端到端 = 建任务(带预算)→ 手动触发 fire → sessions.metadata 带 token_budget(DB 直查或 daemon 读侧验证);未声明预算时四通道 metadata 均不带键(缺省零行为变更,既有测试全绿即证)。
- [ ] AC2(弹窗):vitest 覆盖——preset 选中预填阵容(persona 组装与 script composePresets 同形断言)、主持人 Select 落 create_session model 参数、预算输入留空/填数两态落 metadata、edit 成本区渲染(mock transport:per-speaker 消耗 + 进度条 + 缓存率合并行)、2-3 上限交互不回归。
- [ ] AC3(核算):后端单测——`group_chat_token_usage` 聚合正确(造多 speaker 多 turn 数据,SUM 口径 = 四字段;worker 行/无 usage 行排除);`GroupChatSessionHit.total_tokens` 两端点返回;MCP coreResult tokens 聚合单测(mock deps);script 转录含总消耗。
- [ ] AC4(预算硬停跨通道):MockProvider 单测——定时通道 metadata 声明的预算真实生效(fire → 编排 → 轮头硬停 stop_reason=budget),证明透传链不是只写键不生效。
- [ ] AC5(门):`cargo test -p everlasting --lib` 全绿(基线 2343+)+ `cd app && pnpm test` 全绿(基线 1652+)+ `node --test scripts/group-chat-run.test.mjs` + `scripts/group-chat-mcp.test.mjs` 全绿 + clippy/vue-tsc/fmt 净;wire 契约变更全部 additive(metadata 键、shape 可选参、响应可选字段)。
- [ ] AC6(视觉):弹窗重设计后跑 `scripts/ui-review.sh --screenshots-only` 确认无布局破版(桌面 + 移动全屏态);关键界面人工过一眼。

## Out of Scope

- **默认档**(D4 裁定不做):预设级推荐预算、全局缺省上限、$ 换算(models 表无价格列,需新价格输入面)——真实出险再立项。
- **C2 证据链**(结构化 summary)——群聊内部改进线另立任务。
- **议题进弹窗 / 创建即开场**(D2)。
- **人数上限放开**(D5)。
- **远程暴露认证**(M4 最后余项,立项前须安全评审)。
- MCP 部署面 follow-up(跨平台矩阵 / Tauri 分发 / 其他宿主)。

## Notes

- 规划方式:trellis-brainstorm 五问收敛(D1-D5),证据全部来自码上勘察(见 Background 锚点)。
- 技术设计见同目录 design.md;执行清单见 implement.md。
