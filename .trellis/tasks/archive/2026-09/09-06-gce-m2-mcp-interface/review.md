# Review — GCE-M2 MCP 接口层(planning 门评审)

> 评审对象:`prd.md` + `design.md` + `implement.md`(2026-09-06,基于 main `c9ce96b6`)。
> 任务状态:`planning`(`task.json.status`),尚未 start;implement/check.jsonl 均为 seed 占位。
> 证据核验:2026-09-06 对工作区实际代码逐一比对(M1 引擎、daemon Rust 源、宿主挂载事实)。

## 结论摘要

PRD / design / implement 质量高:共享 M1 实现层(AC②)、daemon 零改动(AC③)、context
预算(D3)、`created_via` 归因迁移(D5)四条主线决策收敛且与代码事实一致;XDG state
落点与既有 `daemon.log` 同构、`.agents/mcp.json` 挂载语义经 diagnosing-mcp skill 核实
成立。**核验发现 2 个 start 前需定案/修订项(P1)与 4 个实现定义缺口(P2)**——均不推翻
已定决策,修订量小(design 补几行 + implement.md 把数据源/兜底链写死)。

**建议状态:需修订后放行。**

---

## 1. 证据核验结果(PRD/design 引用逐条)

| 引用 | 实测 | 判定 |
|---|---|---|
| `db/types.rs:447-453` SessionSummary `busy` + `stop_reason` | `busy: bool` 447、`stop_reason` 453-454,注释(442-453)自证「list_sessions 轮询器可区分 running / ended-and-why」 | ✅ |
| 终态判定 = `!busy && stop_reason != null`,取值四值 | 四值 `group_chat_end/max_rounds/cancelled/error` 为会话级终态列写法(types.rs:373-375、group_chat_loop.rs:139-153);**另有两个轮级值** `nominee_unknown`/`participant_unresolved`(139-142),只出现在跳轮 `ChatEvent::Done` 后 `continue`,不落 session 终态列 | ⚠️ 会话级成立;建议补一句防 MCP 实现者误读(见 P3-2) |
| GC7:LoadedSession.`discussion_summary`,终态直接读 | 字段实为 `SessionRow::discussion_summary`(types.rs:388);`LoadedSession { session: SessionRow, messages }`(517-520) | ⚠️ 字段落点在 Row,经 `LoadedSession.session` 读;表述微偏,事实成立 |
| M1 `:449` request_id 客户端生成 | `group-chat-run.mjs:449` `group-chat-run-${Date.now()}-${rand}` | ✅ |
| cancel_chat 只认 request_id,无 session 级 cancel | `routes/cancel.rs:20,27`:请求体仅 `request_id`,调 `cancel_chat_inner` | ✅ |
| scheduled_tasks `created_by`('user'/'agent',`db/scheduled_tasks.rs:51`) | `:51` 字段、`:82-85` 注释、`:144-145` 载荷语义 | ✅ |
| 群聊 session 有 `metadata` JSON blob(`sessions.rs:57`) | 事实对(sessions.metadata 列,group chat 首消费,types.rs:360-374 注释);**行号错**:`db/sessions.rs` 是 33 行 hub(纯 re-export),字段在 `db/types.rs:370`(Row)/`438`(Summary) | ⚠️ 见 P3-1 |
| daemon bind `0.0.0.0:7456`(`server.rs:424`) | 实为 `src/daemon/server.rs:424` `SocketAddr::from(([0, 0, 0, 0], port))`;行号对、路径缺 `daemon/` 前缀 | ⚠️ 微偏 |
| 仓库零 MCP 基础设施 / scripts/ 零依赖 / 根无 package.json / Node v24.15.0 | 全部实测属实(无 `.agents/mcp.json`、scripts/ 无 package.json、根无 package.json 与 pnpm-workspace、`node v24.15.0`;pnpm 11.21.0 可用) | ✅ |
| `.zcode/` 无 config.json → `.agents/mcp.json`(顶层 `mcpServers`)对 ZCode 生效 | 实测 `.zcode/` 仅 `plans/`(md),无任何 json;diagnosing-mcp skill:workspace scope `.agents/mcp.json` 是 fallback,该 scope `.zcode` 无 MCP server 时生效,顶层 `mcpServers` 键,auto-connect | ✅(fallback 语义注意见 P3-4) |
| M1 导出清单(design §1) | 逐项核验存在:`resolveParticipants / buildCreateSessionBody / buildChatBody / normalizeModelRef / validateModelRefs / summarizeToolUses / defaultTranscriptPath / renderTranscript / resolveProject / listModels / createSession / fireChat / pollSession / loadSession / cancelChat / deleteSession` + `PRESETS`/`EXIT` | ✅ |
| M1 8 用例单测 | `node --test scripts/group-chat-run.test.mjs` 实测 8 pass | ✅ |
| PRESETS 与 start 描述一致 | review=架构+产品+后端、arch=架构+后端(2 人)、retro=产品+局外;moderator 三预设均 MiniMax-M3 | ✅ |
| 记账 XDG 落点 `~/.local/state/dev.everlasting.app/` | 与 `daemon.log` 同构:`disk/log_rotation.rs:14,54-68`(`${XDG_STATE_HOME:-~/.local/state}/dev.everlasting.app/daemon.log`) | ✅ |
| status 需 project_id(list_sessions 按 project_id 查) | `routes/sessions.rs:36,43`;busy 富化**只在** list_sessions(list_sessions_inner patch `AppState::session_active_request`,types.rs:442-447),load_session 无 busy | ✅(支撑 P1-2) |
| load_session 返回完整 SessionRow(含 project_id) | types.rs:517-520 + M1 CLI `:529-530` 实读 `session.discussion_summary/metadata` | ✅ |

## 2. 主要发现

### P1-1 自定义 participants 时 moderator 模型来源未定案,start 前必须定

`start_discussion` schema 无 moderator 参数(R1:preset? / participants? 二参),design §3.1
只写「moderator=model UUID」不写取值。M1 CLI 的既有语义可镜像(`group-chat-run.mjs:371,416`):
preset 默认 `'review'`,**participants 只替换名单**,moderator 恒取 `PRESETS[preset].moderator_model`
(MiniMax-M3,经 normalizeModelRef 解析成 UUID)。若 MCP 不写明这条,participants-only 调用要么
报「缺主持人」与契约冲突,要么隐含依赖实现者拍脑袋的默认值。

建议:design §3.1 写明「moderator 恒取(可能默认的)preset 的 moderator_model,名单替换不影响
moderator」,与 M1 CLI 对齐;单测补 participants-only 一档。

### P1-2 记账 schema 缺 project_id,「重启后兜底全覆盖」断言不成立

design §3.2 文件 schema 是 `{[session_id]: {request_id, cwd, topic, started_at_ms}}`,**无
project_id**;§3.3 却自证 status 需要 project_id(「list_sessions 按 project_id 查——需要
project_id」)。busy 只经 list_sessions 富化(types.rs:442-447),load_session 不返回 busy——
进程重启后内存 Map 丢失,若文件也无 project_id,`discussion_status` 无法查 busy,§3.2 的
「只有 cancel 需要 request_id——重启后兜底路径全覆盖」就不成立。

建议(修订量一行):文件 schema 加 `project_id`(与 §3.3「start 时把 proj.id 一并记账」对齐,
§3.2 文字与 §3.3 是同一记录的两种写法,属编辑遗漏);并在 implement.md 写死重启兜底链:
记账命中 → `list_sessions(project_id)`;记账 miss → `load_session(session_id).session.project_id`
→ `list_sessions`(load_session 返回完整 Row,含 project_id,已核验)。

### P2-1 惰性转录的触发点只定义在 status,result 直呼时暴露空路径;导出失败会拖垮轮询

(1) 「终态首次观测时惰性导出」挂在 status 语义下(§3.3);若调用方从未 poll status、终态后直接
`discussion_result`,result 返回的 `transcript_path` 指向未生成文件。建议把「终态 → 确保转录已导出」
抽成共享纯逻辑(天然满足 AC2),status 与 result 都走它。
(2) 惰性导出是写 `<讨论 cwd>/out/`,宿主 cwd 可能是任意目录(只读/沙箱/权限不足)。
**status 绝不能因导出失败而报错**——导出失败应降级为返回 `transcript_path: null` + 警告字段
(与 result 的 summary 缺失兜底同构),轮询契约不受影响。design 现无此容错说明,补一行。

### P2-2 rounds_hint / elapsed_s 数据源未定义

`discussion_status` 契约含 `rounds_hint`;SessionSummary 无轮次字段(list_sessions 摘要不可见
群聊内部 round,轮次是编排循环态)。M1 CLI 自身也从不显示轮次。elapsed_s 依赖记账 `started_at_ms`
(重启后文件里有,可行),但 rounds_hint 只能取 load_session messages 数的代理或删字段。
implement.md 需定义来源(建议:`rounds_hint` 由 load_session 消息数估,或砍掉换 `messages_hint`)——
不能在实现时临时发明。

### P2-3 PRD「双发隐患场景按设计消失」断言过强

AC5-M1 的双发根因是**后台壳升级重跑换句柄 + agent 手动重发**;MCP 化后后台壳句柄问题确消失,
但 LLM 宿主对工具调用的**自动重试**(响应丢失/超时)是残留双发向量:start 每次生成新 request_id
+ 新 session,宿主重试即并发双场、双倍成本——正是 M1 的 live 教训。stdio 本地管道可靠性高,
风险确实显著降低,但「消失」应改述为「转移到宿主工具重试,风险大降;v1 接受」;对抗手段(如
可选 caller 幂等 key / start 响应即刻写记账后短暂去重)留一句取舍即可,不必做。

### P2-4 AC1 的「冒烟脚本化全链」验收归属模糊,收官时必歧义

AC1 要求「SDK Client over stdio … 完成『召集→轮询→取结论』全链(冒烟脚本化)」+「ZCode 宿主
实跑」;但 implement Step 3 的冒烟 `--live` 标「按需」,Step 5 AC1 只验收「冒烟(非 live)+ 宿主
实跑」——即 SDK-client-over-stdio 的 **live 全链从未被强制验证**,host 实跑验证的是挂载而非
该 client 路径。建议:smoke `--live`(小规模,arch preset 2 人)列为 AC1 正式门禁之一,或明写
「SDK client 全链由 smoke --live 覆盖;宿主实跑只验工具出现与可用」,二选一写死。

### P3-1 `sessions.rs:57` 行号失配(事实正确)

`db/sessions.rs` 是 33 行 hub(纯 re-export);metadata 结构体字段在 `db/types.rs:370`
(SessionRow)/`438`(SessionSummary),注释 360-374 载明「sessions.metadata 列 group chat 首消费」。
建议改引用为 `db/types.rs:370`。

### P3-2 轮级 stop_reason 与四值枚举的边界

`group_chat_loop.rs:139-142` 另定义 `nominee_unknown`/`participant_unresolved`,语义是**跳轮**
(打轮级 Done 后 continue,群聊继续);session 终态列只写四值(clear/finalize 路径,
`session_crud.rs:868-899`)。以 MCP 消费的 list_sessions 视角四值成立,但 design 建议加一句
「轮级跳轮值不落 session 终态」,防实现者拿 summary.stop_reason 与固定表比对时困惑。

### P3-3 转录落点与 M1 的默认行为分叉,需确认意图

M1 CLI 转录**固定落 everlasting 仓库根 `out/`**(run.mjs:20-21,225-233:按 SCRIPT_PATH 推导,
不随 --project/cwd 变);design §3.3 让 MCP 落 `<讨论 cwd>/out/`(依赖 defaultTranscriptPath 的
根目录参数微扩)。两种消费者对同一共享函数给不同默认根是有意为之(记录留在被审项目里),但
PRD/design 未写明这是**分叉**。建议 design §6 兼容性声明补一行,且实现时确认:向任意宿主 cwd
写 `out/` 是预期副作用(见 P2-1 的容错)。

### P3-4 `.agents/mcp.json` 的 fallback 语义是未来的坑

diagnosing-mcp pitfall 12:`.agents/mcp.json` 是 **same-scope fallback**——若日后仓库 `.zcode/`
定义任何 MCP server,该文件被整体忽略(不是合并)。当前 `.zcode/` 无 config.json(实测),开箱即挂
成立;建议 `.agents/mcp.json` 顶部留注释警示,并把此约束写进 DAEMON-API §6 一行。

### P3-5 planning 完成度

`task.json.status=planning`、implement/check.jsonl 仍为 seed 占位(workflow 要求 complex 任务
start 前 jsonl 各含真实条目)。放行条件见 §5。

## 3. 设计决策核对(D1–D5)

五条决策与代码事实一致,无回退:

1. **D1 Node + SDK 薄包装** —— scripts/ 零依赖属实,独立 `scripts/package.json` 落点可行
   (根无 package.json、app/ 是独立 pnpm 包,已核验);M1 导出面齐全,import 零翻译成本。
2. **D2 只做 stdio** —— 本机两消费场景成立;M4 前不加网络面合理。
3. **D3 四工具 + context 预算** —— 结构合理(无第 5 发现类工具,失配靠服务端报清单)。
   ⚠️ 预算余量小:design §2 自估 ~500 token,AC4 上限 600。按 chars≈token×4 粗估,四工具
   description+schema 合计约 2100-2400 字符即到边界;participants 嵌套 array-of-object 是
   主要超支风险。建议:AC4 的字符上限 N 按「实现后先实测 token(可本地数),再定 N,留 ≥10%
   余量」校准(估 N≈2200-2300),并把校准写进 implement.md;子字段 description 从简。
4. **D4 v1 零新增鉴权** —— 「stdio 无网络面、真暴露面在 daemon 0.0.0.0」论证成立,不给 MCP
   层加 token 非安全剧场;真鉴权归 M4 合理。
5. **D5 created_via 归因** —— 服务端写死 `"mcp"`、M1 补 stamp `"script"`、缺失即 GUI/历史,
   三通道语义完整;metadata 增量键零 schema 变更、GUI 不感知,兼容性声明成立。

另确认 design §5 风险对策(回滚净、SDK 封装隔离、daemon 未跑可操作报错复用 M1 `api()`
文案)与 §6 兼容性声明均成立。

## 4. 对 Acceptance Criteria 的检查

| AC | 判定 | 备注 |
|---|---|---|
| AC1 端到端 + ZCode 宿主实跑 | ⚠️ | 可行性成立(挂载语义已核实);「全链由谁验」需按 P2-4 定死 |
| AC2 共享实现层 + 新增纯逻辑有单测 | ✅ | 结构满足;P2-1 的共享「惰性导出」正是应抽的纯逻辑 |
| AC3 daemon 零改动 | ✅ | created_via 是客户端 metadata 增量,M1 微扩在脚本侧;`git diff` 门禁可达成 |
| AC4 context 预算 ≲600 token | ⚠️ | 可行但余量小;字符上限 N 需实现后实测校准(见 D3 行) |
| AC5 created_via 两通道 + GUI 无字段 | ⚠️ | 单测覆盖 script/mcp 可行;GUI「无字段」的 live 抽查依赖 GUI 环境,headless 不便时可改「daemon 侧无参 create_session 抽查 + 单测断言缺失语义」 |
| AC6 `node --test` 单测 | ✅ | node:test + 注入 mock 模式与 M1 基线一致(8 用例已实测绿) |

## 5. 放行条件(修订项汇总)

1. **design §3.1**:写死 moderator 取值(镜像 M1 CLI:preset 默认 review,participants 只替换
   名单,moderator 恒取预设值,P1-1)。
2. **design §3.2/§3.3**:记账 schema 补 `project_id`;implement.md 写死重启兜底链
   (记账命中/load_session 两级,P1-2)。
3. **design §3.3 + implement.md**:惰性导出抽共享逻辑、status/result 都走;导出失败降级
   (transcript_path null + 警告),status 永不因导出报错(P2-1)。
4. **implement.md**:定义 rounds_hint/elapsed_s 数据源或删字段(P2-2);PRD Background 双发
   表述改「宿主工具重试,风险大降,v1 接受」(P2-3);AC1 验收归属二选一写死(P2-4)。
5. **P3 细节**:metadata 引用改 `types.rs:370`;补轮级 stop_reason 一句;转录落点分叉声明;
   `.agents/mcp.json` 顶部注释 fallback 约束;AC4 的 N 值实现后实测校准。

完成上述(修订量小,均不推翻已定决策)后,填充 implement/check.jsonl 真实条目,可执行
`task.py start` 进入 Phase 2。
