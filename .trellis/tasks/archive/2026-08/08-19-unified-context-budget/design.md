# Design — 统一 token 预算表 + 关卡⑤硬卡

> 锚点相对 `app/src-tauri/src/`(后端)与 `app/src/`(前端)。PRD 见同目录 `prd.md`。

## 1. 架构与边界

```
prepare_loop_state (init.rs:92,每 request)
  ├─ C3 水位替换
  ├─ memory 指令块头对(insert 0/1,合成消息)  ── memory_token 归因计数(init.rs:499)
  ├─ skill listing(独立合成 user message)
  ├─ system prompt(与 messages 并列发送)
  ├─ @文件注入(at_file.rs:291,每 request 重展开) ── ★ at_files_token 计数 + 同请求 spans(新增)
  └─ 附件转 ImageRef

drive_turn (drive.rs:82,每 turn)
  ├─ head_sha / system prompt 刷新(drive.rs:180)
  ├─ ★ tools 过滤链 + stubify + tools_token 估算     ← 时序重排:从 :752 挪到压缩块前
  ├─ C3+ 压缩块(drive.rs:215)
  │    └─ 触发判断:estimate_request_tokens(total) vs 0.85   ← 口径修正(补 tools+system)
  ├─ turn_messages APPEND 组装(drive.rs:537,追加独立合成消息,不改既有文本)
  ├─ 图片 resolve(drive.rs:735)          ── images_token 计数点(drive.rs:743)
  ├─ ★ budget_gate(新增,send 前最后一道)
  │    ├─ total > 0.95×window? → 裁剪引擎(D3 优先级)
  │    └─ 裁尽仍超 → fail-fast(stop_reason=context_over_budget)
  ├─ ChatEvent::Done → turn_trace 落库(★ 新列一律记实发值 = 预裁 − freed)
  └─ provider.send(drive.rs:891)
```

新模块 `agent/budget.rs`:统一估算入口 + 阈值常量 + 裁剪引擎。`context.rs` 保留既有 per-part 估算函数(`estimate_messages_tokens` 等),budget.rs 组合它们。

## 2. WP1 度量契约

### turn_trace 新列(幂等 migration,`add_turn_trace_column_if_missing` 模式)

| 列 | 计数点 | 语义 |
|----|--------|------|
| `at_files_token INTEGER NULL` | `inject_at_tokens` 内对全部 user message 注入正文 count_tokens,经 loop state 传到 Done 写点 | **实发口径**(裁剪发生后记预裁 − freed) |
| `system_token INTEGER NULL` | system prompt 本体(发送部件)+ skill listing 合成消息(messages 内归因)count_tokens | 同上;归因口径,见下方估算公式 |
| `context_window INTEGER NULL` | 请求时 `context_window` 快照(drive_turn 已有参) | 前端预算行分母;旧行 NULL → 前端回退 200_000 |

- `upsert_turn_trace_token`(`db/trace.rs:96`)扩参 3 列;写点仍挂 `ChatEvent::Done` usage 分支、`!skip_persist` gate(与既有三列同点,worker 盲区维持现状,Out of Scope)。
- `turn-smoke.sh` 报告行加 3 列。

### 统一估算与口径修正(评审 F1 修正后)

**总量按发送部件加法,归因切片从 messages 内部归因,两类口径分开、永不互相加计**(PRD D8):

```
estimate_request_tokens(system_prompt, tools_json, messages)
  = count_tokens(system_prompt)          // 与 messages 并列的发送部件
  + count_tokens(tools_json)             // 与 messages 并列的发送部件
  + estimate_messages_tokens(messages)   // 已含 memory 头对 + skill listing + @文件正文
                                          // + 图片(无精估 pad 6400 字符 ≈1600 tok/张)+ 历史
```

- **不**单独加计 memory/@文件/图片切片——它们物理在 messages 里,已被第三项覆盖(原设计重复计数,评审 F1)。
- 归因切片列(memory_token / at_files_token / images_token / system_token 中的 skill listing 部分)只做 TurnCard 占比条归因,残差 = 总量 − tools − memory − @files − images − system,钳 0;归因之和 ≤ 总量是断言(AC1)。
- **口径切换点三处**,全部改调统一估算:
  1. C3+ 摘要触发(`drive.rs:221` `tokens_pre`);
  2. 摘要 postcheck 0.95(`SUMMARY_POSTCHECK_RATIO` 消费点);
  3. 机械 `compact_messages` 触发(`context.rs:285`,无 gate,群聊/worker 同受益)。
- 保留区/水位/摘要生成逻辑不动(那是 messages 内部操作,与口径无关)。

### @文件 spans(评审 F5 裁定:同请求临时产物)

- @文件注入是**每 request 重展开**(DB 存原始 `@relpath` 为 SoT,`init.rs:825-830`),因此 spans **不落 DB**——由 `inject_at_tokens` 在本次展开时直接产出:`Vec<AtFileSpan { msg_idx, start, end, path, tokens }>`(start/end 为该消息文本内偏移;@图注入致 Text→Blocks 形态时为首个 Text 块内偏移),经 loop state 传给 budget_gate 在**同一请求内**消费。
- APPEND 组装(checklist / 后台 shell 通知等)追加的是**独立合成消息**,不动既有消息文本 → 消息内偏移天然稳定(AC3 用例锁定);D3 编辑走 DB 重载 → 下次请求重新展开,无 stale 路径。
- 消费侧防御:span 超出当前文本长度(理论失配)→ 跳过该 span 不裁,fail-open。

### 时序重排(D7)

tools 过滤链(mode→workflow→session_type→stubify→元工具 append,`drive.rs:752-826`)整体挪到压缩块之前。依赖核查:过滤链只依赖 permissions + head_sha(已在 `drive.rs:180` 刷新)+ StubRegistry 粘性集合——与压缩块零依赖。重排后压缩触发判断即可拿到当轮 tools_token。回归:stub 粘性(load 后次轮 full schema)与既有 tools_token 计数语义不变,单测锁。

## 3. WP2 裁剪引擎

### budget_gate(drive_turn 内,图片 resolve 后、send 前)

```rust
// agent/budget.rs(示意)
const BUDGET_LINE_RATIO: f64 = 0.95; // 对齐 SUMMARY_POSTCHECK_RATIO

pub struct BudgetTrimReport { arms: Vec<TrimArm>, over_by: i64 }

pub fn enforce_budget(/* system, tools_json, messages(含指令头对), spans, loaded_sections, window, gate */) 
    -> Result<(), /* OverBudget + breakdown */>
```

- gate:`context_budget_enabled`(DB `app_config`,fail-open 缺省 on,读取惯例同 `tools_stub_enabled` `chat_loop.rs:622`)&& `!worker && !群聊`。gate 关 = 零行为变化(估算照算,只观测)。
- 超线判定用统一估算(与压缩同一把尺)。

### 裁剪三臂(D3 优先级,逐臂重估,直到达标或臂尽)

1. **@文件占位替换**:按 init 期产出的同请求 spans(见 §2)识别旧轮次注入正文——**当前 turn 的 user message(seq ≥ 本次发送)的 span 不裁**;其余 span 在 `turn_messages` **副本**上替换为单行占位 `[at-file {path}: {n} tokens trimmed by budget gate]`;span 失配 → 跳过 fail-open。
2. **旧图降级**:resolve 后的 Image content block(非当前 turn)替换为 B1 占位降级文案先例(`attachments.rs` caps=false 路径同款话术,模型知有图未发)。
3. **memory 节回退**:本请求的指令块视图用目录态重算(`build_instructions_blocks` 不带已加载节);`MemoryDigestRegistry` 不动(下轮仍可复用,窗口持续紧则每轮等效回退)。
4. 臂尽仍超 → `Done { stop_reason: "context_over_budget" }` + error turn,错误文本含 breakdown(tools/mem/system/@files/imgs/history 各 tok + window)。

每臂记录 `{kind, count, tokens_freed}`;臂后重估总量(消息文本已变,需重跑受影响部件估算——@文件臂只影响 messages 部件,memory 臂只影响指令块视图,增量成本低)。

### 非破坏性不变量(D6)

- 裁剪只作用于 `turn_messages`(本来就是 `messages.clone()` 副本,`drive.rs:537`)与请求局部视图;
- DB messages、StubRegistry、MemoryDigestRegistry、at_file manifest 均不写;
- 下轮重新评估(裁剪不记忆)——成本 = 每 turn 重算,量级与既有估算同阶,可接受。

### 可观测(R8,实发口径 D9)

- **trace 一律记实发值**:裁剪发生时各切片列 = 预裁值 − 该臂 `tokens_freed`(算术差,不重编码);预算行总量 = 实发统一估算,与 provider `context_input` 同量级可比;预裁值只活在 audit payload(`pre_total`)。无裁剪时(常态)实发 = 预裁,零特判。
- `AuditKind::ContextBudgetTrim`(enum 变体,无 migration 先例——`LoopIntervention` 同款):payload `{arms: [{kind: "at_file"|"image"|"memory_section", count, tokens_freed}], over_by, pre_total, post_total, window}`。
- `ChatEvent::BudgetTrim`(非持久化、只读,同 `Retrying` 先例):前端 ChatPanel 瞬时 chip(如"预算裁剪:-3.2k(@文件×2,图×1)"),streamController 一处路由。
- TurnCard 预算行:实发统一总量 vs `context_window`(行内值,NULL 回退 200_000)+ 五切片占比条(tools/mem/img/at_file/system,残差 = 实发总量 − 五切片,钳 0)。

## 4. 兼容性与迁移

- turn_trace 三新列走幂等 helper,老 DB 零迁移动作;旧行 NULL 前端回退。
- `AuditKind` enum 加变体无 DB migration(audit kind 存 TEXT)。
- 开关缺省 on(fail-open);关闭 = send 前行为逐字节同现状。
- 触发线口径切换是**有意的行为变化**(PRD AC2 锁定新语义);既有压缩测试若依赖 messages-only 口径需同步校准预期。

## 5. 权衡记录

- **全局线 vs per-slice 配额**:选全局线。切片事实治理已存在(stub/digest/图片张数闸),配额矩阵引入 N 个难调参数且互相耦合;全局线 + 优先级表一个参数(0.95)表达"最后防线"语义。
- **0.95 而非 0.9**:与 `SUMMARY_POSTCHECK_RATIO` 同尺;0.85 压缩是第一道防线,硬卡只在压缩失败/静态切片挤压时动作,线贴窗留余量吸收 cl100k 偏差。
- **请求副本裁剪 vs 持久化裁剪**:副本裁剪幂等、可逆、不污染 DB(裁剪后历史仍是全量,窗口松了自动恢复);持久化裁剪会把临时压力变成永久信息损失。
- **memory 节回退不动 registry**:回退是"本请求视图"级别,registry 粘性语义(加载过就省一次 load)保留;窗口松时零成本恢复。

## 6. 回滚

- WP1 独立回滚 = revert 单 commit(纯加列 + 口径切换,口径切换若需单独回退可临时注入更小 TRIGGER_RATIO)。
- WP2 总闸 `context_budget_enabled=false` 即回滚到 WP1 行为;代码级 revert 不涉及 schema。
