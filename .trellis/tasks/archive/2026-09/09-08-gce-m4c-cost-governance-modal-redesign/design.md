# design.md — gce-m4c 成本治理消费面 + 弹窗重设计

## 1. 架构总览(四通道声明 + 三层核算)

```
预算声明(写侧,全 additive)                    核算(读侧,零新存储)
─────────────────────────────                ─────────────────────────────
GUI 弹窗(重设计)──┐                          turn_trace.token_usage_json
M1 script --token-budget ─┤                   × messages.speaker (JOIN on
MCP start_discussion 参数 ─┼→ sessions.metadata.  session_id, seq)
M4a 定时(表单+fire)──┘   token_budget            │
                          (键已存在,09-08)        ├→ db 查询 group_chat_token_usage
                                                    │   (新,total + by_speaker)
硬停链路(已就绪,零改动):                          ├→ GUI edit 弹窗成本区(Tauri cmd)
轮头检查 → HaltReason::Budget                       ├→ 讨论库 hit.total_tokens
→ stop_reason="budget"                              └→ script/MCP 客户端聚合
                                                      (list_turn_traces × load_session)
```

原则:写侧四通道全部只在「显式声明」时写 metadata 键(缺省不带键,与 09-08 语义逐字节一致);读侧核算不建表不加列,一次 join 模式三处消费。

## 2. 写侧各通道设计

### 2.1 M1 script(`scripts/group-chat-run.mjs`)

- `run` 子命令加 `--token-budget <N>`:parse-int 校验(`Number.isInteger && > 0`,否则 usage 报错退出 1);透传 `buildCreateBody({..., tokenBudget})` → metadata 在 Some 时带 `token_budget` 键(不写 undefined/null)。
- `--dry-run` 模板打印建群 body,预算键随模板可见(冒烟即验证)。
- 纯函数区:`buildCreateBody` 增参 + 单测两态(带/不带)。

### 2.2 MCP(`scripts/group-chat-mcp.mjs`)

- `buildToolShapes.start_discussion` 加:
  `token_budget: z.number().int().positive().optional().describe('Billed-token ceiling (input+output+cache_creation+cache_read); exceeded → stop_reason=budget')`
- `coreStart` 把参透传给既有建群调用(metadata 加键逻辑与 script 同:Some 才写)。
- **wire 预算锁**:TOOLS_BUDGET_CHARS=3200,AC4 单测按 wire 实测锁——加参后重跑实测;预计 +~140 chars(2876→~3020)仍在 3200 内,不动锁;若实测超锁,升锁值并在测试注释记录新实测。
- 单测:shape 断言(参数存在、类型、可选)、AC4 重测、coreStart 透传断言(mock deps)。

### 2.3 M4a 定时(daemon Rust + GUI 表单)

- `db/scheduled_tasks.rs`:`GroupChatTaskConfig` 加 `#[serde(default)] pub token_budget: Option<u64>`;`parse_group_chat_task_config` 校验(有值时正整数;additive,旧 JSON 反序列化不变)。
- `scheduler/mod.rs` fire:白名单 json! 改为先构造 `serde_json::Map`,token_budget 在 Some 时 insert(避免写 `"token_budget": null` 污染 metadata——转录导出按原始 JSON 读,保守不落 null 键)。
- TS 类型 `GroupChatTaskConfig`(stores/scheduledTasks.ts:59)加 `token_budget?: number`;第四档表单加预算 number 输入(留空 = 不限,校验同弹窗)。
- **编辑态预算独立提交**:现语义「未重选 preset = 不发 groupChatConfig(存档不动)」。预算输入单独 dirty 跟踪:dirty 时即使未重选 preset,也取存档 config(或当前展开 config)替换 token_budget 后整体重交;两处都无变化维持「不发」(缺省不动语义保留)。
- 端到端:fire → metadata 带键;预算声明真实生效(AC4 的 MockProvider 定时场剧本)。

## 3. 读侧核算设计

### 3.1 db 查询 + 命令面(daemon Rust)

- `db/trace.rs` 新增(紧邻 `list_speaker_cache_usage`,沿用其全部 join 约束:assistant 行、speaker 非空、token_usage_json 非空、`t.run_id = ''` 排除 worker 行、retry 覆盖取最后):

```rust
pub struct GroupChatTokenUsage { pub total: u64, pub by_speaker: Vec<SpeakerTokens> }
pub struct SpeakerTokens { pub speaker: String, pub tokens: u64 }
```

  SQL:同 join,但聚合而非 latest-turn——
  `SUM( COALESCE(json_extract(t.token_usage_json,'$.input_tokens'),0) + output_tokens + cache_creation_input_tokens + cache_read_input_tokens )`
  按 speaker GROUP BY,再程序侧求 total(与 C1.2 计费口径逐字段一致;context_input 是观测口径不计)。
  注意:**retry 覆盖语义**——`upsert_turn_trace_token` 按 (session_id, seq) 覆盖,SUM 不会重复计;latest-turn 查询里的 `m.seq = MAX(...)` 子查询在聚合版**不需要**(全部轮都算),但 worker 行排除(`t.run_id=''`)保留。
- 命令面镜像 `group_chat_cache_rates` 先例:`commands/sessions.rs` inner + Tauri command + `daemon/routes/sessions.rs` route(`POST /api/v1/sessions/group_chat_token_usage`,body `{session_id}`)+ lib.rs invoke_handler 双注册。
- 讨论库:`db/search_group_chat.rs` 的 list/search 两查询加 `total_tokens` 输出列——correlated subquery(同 SUM 口径,run_id='' 过滤,无行 = NULL → Option;前端「—」)。`GroupChatSessionHit` 加 `pub total_tokens: Option<u64>`(serde additive)。

### 3.2 script / MCP 客户端聚合(零 daemon 改动)

- 共享纯函数(放 `group-chat-run.mjs` 导出,MCP 复用——两文件已共享实现层先例):
  `aggregateTokens(turnTraces, messages)` → `{total, per_speaker}`:traces(run_id='')按 seq 与 messages(speaker 非空)对齐,四字段求和。
- script:run 收官后导转录时统计段落带总消耗(engine 已有 daemonFetch + loadSession,加一次 list_turn_traces 调用;失败降级不阻塞转录,先例:M2 惰性转录降级)。
- MCP `coreResult`:stats 加 `tokens: {total, per_speaker}`(loadSession 已调,补 list_turn_traces;失败 → tokens 省略,不污染既有字段)。

## 4. 弹窗重设计(GroupChatConfigModal.vue)

### 4.1 create 模式结构(上→下)

1. **preset 单选卡**(三卡横排:评审团/架构/复盘;卡内名称 + 一句描述取 presets.json `description` 截断;选中态 accent 边框)。选中 → 立即展开预填阵容 + 主持人默认。无「自定义」卡——改任何阵容字段即自然偏离,不设显式状态。
2. **阵容微调区**(现有行交互保留:name/model/persona_md、+/- 按钮、2-3 上限、重名/空名校验)。preset 预填的 persona_md = 边界 + "\n\n" + persona_common(与 composePresets/gcPersonaMd 逐字同形);用户可编辑。
3. **主持人 Select**(create 可选):默认 = 当前 preset 的 moderator_model 解析;用户改选覆盖;解析失败显示 preset 原名并提示先去模型页添加(定时表单同款错误形态)。提交时 `createNewSession({..., modelId})` → `create_session` 的 model 参数(wire 已收)。
4. **token_budget 输入 + 参考量级提示**(静态文案「留空 = 不限;一场讨论通常 20-60 万 token」,D4:不进 presets.json)。

### 4.2 edit 模式结构

1. 阵容编辑照旧(存档回显;**不引入 preset 重选**——编辑对象是既成 session,重选预设覆盖阵容语义复杂且易误抹)。
2. 主持人只读区照旧。
3. **成本区**(新):per-speaker 一行「架构 12.3万 · 缓存 68%」+ 主持人行;有预算时顶部进度条「26万 / 40万(65%)」;数据 = `group_chat_token_usage` + 既有 `group_chat_cache_rates` 两次 invoke(或考虑合并端点——**决定:不合并**,cache_rates 已有消费面,新命令独立 additive,避免改既有契约)。失败降级:成本区显示「—」,不阻塞编辑(cache_rates 同款设计)。

### 4.3 共享逻辑提取

- `resolveModelRef`(ScheduledTasksTab:372)+ `gcPersonaMd`(:124)+ preset JSON 类型:提取到共享模块(候选 `app/src/utils/groupChatPresets.ts`),两消费方 import——避免三处复制;ScheduledTasksTab 改引用(纯搬家,行为零变化,vitest 既有用例守护)。

### 4.4 不变的边界

- 2-3 人数上限(D5);议题不进弹窗(D2);`gcfg-content` 根类名与移动端全屏覆盖块(style.css @media)不动;测试 testid 家族保留(`gcfg-*`),新增 preset 卡/主持人/成本区 testid 按同风格。

## 5. 兼容性 / 契约影响(全部 additive)

| 面 | 变更 | 兼容性 |
|---|---|---|
| sessions.metadata | 无新键(token_budget 已存在) | — |
| scheduled group_chat_config JSON | + 可选 token_budget | serde default,旧行反序列化不变;未知键丢弃行为不变 |
| daemon API | + POST group_chat_token_usage;list/search 响应 + total_tokens 可选字段;§6.3 config +键 | 新端点 + 可选字段,既有消费方零感知 |
| MCP wire | start_discussion + 可选参;result stats + tokens | 可选参/字段;wire 锁重测 |
| Tauri | + group_chat_token_usage command | 双注册 |

回滚:各通道独立提交(见 implement.md 顺序),任一层 revert 不影响其余层(metadata 键 additive)。

## 6. 权衡记录

- **核算查询放 daemon 而非前端三处各自 join**:GUI 弹窗/讨论库走 daemon(Tauri/HTTP 同构,一查询三消费);script/MCP 走客户端聚合是因为它们已有 load_session/list_turn_traces 通道且 M2「daemon 零改动」先例——两边口径由同一 spec 条目钉死(四字段、run_id=''、assistant+speaker 过滤),单测两侧各锁一份数字。
- **不合并 cache_rates 与 token_usage 端点**:cache_rates 已被 GUI 消费,改响应形状是破坏性风险;新命令 additive 更稳。
- **fire 不写 null 键**:转录导出/检索按原始 JSON 读 metadata,保守不落 null;键缺失 = 不限的既有语义不变。
- **wire 锁预计不升**:加参实测仍在 3200 内则不动锁;超了才升并记录实测值(锁的本意是防 description 膨胀,不是防合理参数)。
