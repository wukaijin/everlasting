# Design — V2-2+ 自主记忆可观测性与管理面板

> 配套 `prd.md`。技术决策与执行边界。PRD 4 决策已定:O1 历史+实时 / O2 扩展 MemoryPreview / O3 允许编辑 / O4 详情 modal 统一。

## 1. 架构与边界

```
后端层(app/src-tauri/src/)
  db/memories.rs        update_memory(新)+ validate_memory_text(提取)+ update_status(已存在)
  db/migrations.rs      autonomous_memories 加 edited_by_user 列
  commands/memory.rs    2 新 IPC:update_autonomous_memory_status / update_autonomous_memory
  agent/chat_loop.rs    recall event emit(:1385 / :2453 / :3205 三处接入点)
  state.rs              ChatEvent 加 Recall 变体 + RecallHit payload
  lib.rs                注册 2 新 IPC

前端层(app/src/)
  stores/memory.ts      updateMemoryStatus / updateMemory actions + lastRecallHits + listener
  components/memory/MemoryPreview.vue       runtime row 扩展(hitCount chip + 点击开 modal)
  components/memory/RuntimeMemoryModal.vue  新建(统计 + 状态下拉 + 编辑 + 删除)
  components/chat/ChatPanel.vue             实时召回 chip(本次对话命中)
```

**边界**(不动):
- P3/P4/P5 自动闭环(recall / reflect / 晋升 / 卫生 job)行为不变。
- recall event 是 **read-only 旁路**(同 P3 footnote 语义,不参与决策链,不动 `permissions::check()`)。
- 不动 worker subagent 的 recall 逻辑(worker 仍 recall,只是 event 不 emit 到主 chat)。

## 2. 关键决策

### D1 — provenance 标记:新列 `edited_by_user`

- **决策**:`autonomous_memories` 加 `edited_by_user BOOLEAN NOT NULL DEFAULT 0`,用户编辑 title/content 时置 1。
- **理由**:`source_ref` 已承载 P4 provenance(`<request_id>:<tool_name>`),复用会污染语义;新列查询简单(`WHERE edited_by_user = 1`)、migration 是加列默认 0(**非破坏**)。
- **trade-off**:加列 = 一次 migration(对齐项目 v? migration 编号);复用 `source_ref` 零 migration 但语义混。选清晰。

### D2 — `update_memory` + 安全网提取

- **决策**:新 `db::memories::update_memory(pool, memory_id, title, content) -> Result<(), MemoryUpdateError>`,复用 `insert_memory` 的安全网。
- **重构**:把 `insert_memory` 的 415-436 行校验(char count + sensitive regex + sensitive path)提取为 `fn validate_memory_text(title, content) -> Result<(), MemoryWriteError>`,`insert_memory` 与 `update_memory` 共用(单源安全网,对齐 P4 "must route through insert_memory" 原则)。
- `MemoryUpdateError` 复用 `MemoryWriteError` 变体(Overlength / SensitiveContent / SensitivePath)+ `NotFound`。
- title 也走 sensitive regex(对齐 insert_memory:423 行 title+content 都查)。

### D3 — `update_status` IPC 暴露

- **决策**:IPC `update_autonomous_memory_status(memory_id: String, new_status: String, demoted_reason: Option<String>)`。
- 直接包 `db::memories::update_status`;`StatusTransitionError` → `AppCommandError`(InvalidRequest for Illegal / NotFound for NotFound)。
- **project 隔离**:`list_autonomous_memories` 已 project 隔离,前端只能看到/操作本 project(user-scope + 当前 project-scope)的 row;`memory_id` 全局唯一,update 不需额外 project_id 校验(同 `delete_autonomous_memory` 模式)。

### D4 — recall event:复用 `chat-event` channel(非新 channel)

- **决策**:不扩展 `ChatEventSink` trait(避免改 trait + AppHandleSink/SubagentBufferSink 2 impl + 测试 mock);复用 `emit_chat_event` 走 chat-event channel,在 `ChatEvent` 加 `Recall` 变体(对齐 `emit_retrying` 把 RetryingEvent 作 chat-event notice 的先例)。
- **payload**:`ChatEvent::Recall { hits: Vec<RecallHit> }`,`RecallHit { memory_id, title, kind, source: "fts"|"pitfall" }`。
- **emit 位置**:
  - `chat_loop.rs:1385` — `build_recall_text` 返回 `Some(_)` 后(session-start FTS 命中)
  - `chat_loop.rs:2453` / `:3205` — `recall_pitfall` 返 `Footnote(_)` 或 `SoftBlock{..}` 后(pre-tool pitfall 命中;`None` 不 emit)
- **worker 隔离**(免费):worker subagent 的 sink 是 `SubagentBufferSink`(不 forward 到主 chat IPC,同 L3 模式);recall event 走 `sink.emit_chat_event`,worker 天然不污染主 chat。AC7 由 sink 抽象保证,零额外代码。
- **实现注记**(取证明:`build_recall_text` 现返 `Option<String>` 不含 row 详情;`PitfallRecall::Footnote(String)` 同样不含):FTS 路新增 sibling `build_recall_text_with_rows -> Option<(String, Vec<MemoryRow>)>`(原 `build_recall_text` 变 wrapper,4 测试零回归);pitfall 路扩展 `recall_pitfall` 返 `(PitfallRecall, Vec<MemoryRow>)`(`PitfallRecall` 变体不变,只加伴随 rows;SoftBlock 的 `memory_id` 已在变体里)。详见 implement.md A8/A9。

### D5 — `RuntimeMemoryModal.vue` 新组件

- 区别于现有 `memory/MemoryModal.vue`(后者是 reka-ui Dialog 包 MemoryPreview,服务指令文件 layer)。
- reka-ui Dialog,样式参考 MemoryModal(80vw / min 640 / max 900 / 80vh)。
- 内容布局:title input(可编辑)+ content textarea(可编辑)+ 统计区(`hitCount` / `lastUsedAt` / `confidence` / `createdAt` / `sourceSessionId` / `sourceRef` / `edited_by_user` 徽标)+ 状态下拉(矩阵驱动合法目标,见 D6)+ 操作区(保存 / 取消 / delete)。

### D6 — 状态转换下拉(矩阵驱动)

- 前端复刻 P5 转换矩阵(spec 已锁):`candidate→{active,verified,demoted}`,`active→{verified,demoted}`,`verified→{demoted}`,`demoted→{active}`。
- 下拉只渲染当前 status 的合法目标;后端 `update_status` 二次校验作硬墙(防御前端 bug)。
- 转 `demoted` 时弹 reason 输入(对齐 `demoted_reason` 字段);其他转换不需 reason。

### D7 — 实时召回 chip 形态

- ChatPanel 区加"本次召回"chip(`hitCount>0` 命中时显示),点开展开命中 title 列表(按 source 分组:FTS / pitfall)。
- store `lastRecallHits` ref,per-turn 累积(一个 turn 内 FTS + 多次 pitfall 都累计);新 user message 时清空。
- listener:复用现有 `chat-event` IPC listener,过滤 `kind === 'recall'` 分发。

## 3. 数据流

```
状态转换:
  RuntimeMemoryModal 下拉 → store.updateMemoryStatus(id, status, reason?)
    → IPC update_autonomous_memory_status → db::update_status(矩阵校验)
    → 成功:store 本地 patch row badge + modal 同步
    → 非法:StatusTransitionError::Illegal → 前端报错(不崩溃)

编辑:
  RuntimeMemoryModal 保存 → store.updateMemory(id, title, content)
    → IPC update_autonomous_memory → db::update_memory(validate_memory_text 安全网)
    → 成功:store 本地 patch row + modal + edited_by_user=1 徽标
    → 安全网拒绝:Overlength/Sensitive → 前端报错

实时召回(旁路,read-only):
  chat_loop build_recall_text / recall_pitfall 命中
    → sink.emit_chat_event(Recall { hits })  [worker sink 天然隔离]
    → 前端 chat-event listener 过滤 recall → lastRecallHits push
    → ChatPanel chip 渲染

历史统计(零后端):
  row 渲染 AutonomousMemory 已有 hitCount / lastUsedAt(纯前端模板)
```

## 4. 兼容性 / migration

- **migration**:加 `edited_by_user BOOLEAN NOT NULL DEFAULT 0`(非破坏,旧行默认 0)。
- **ChatEvent 变体**:加 `Recall` 变体(前端 `ChatEvent` union 扩展;旧前端不处理 = 无 chip,向后兼容)。
- **IPC**:2 新命令注册到 `lib.rs::run`.invoke_handler(additive)。
- **wire**:`AutonomousMemory` 已含所有字段(hitCount/lastUsedAt/confidence/demotedReason),只需加 `editedByUser: boolean`(camelCase,后端 serde)。

## 5. trade-offs

- **实时召回 event 的成本**(中等):chat_loop 3 处 emit + ChatEvent 变体 + 前端 listener + chip UI。用户已选"历史+实时都做";若 review 时觉得过重,可分期——**Phase 1 做历史统计 + 状态转换 + 编辑(R1/R2a/R3/R4/R5),Phase 2 做实时 event(R2b)**。两期之间无依赖(实时 event 是纯 additive 旁路)。
- **D4 复用 channel vs 新 channel**:复用省 trait 改动,代价是 chat-event channel 多一种 notice(可接受,emit_retrying 先例已立)。
- **D2 安全网提取**:重构 insert_memory 提取 helper 有回归风险(P4/P5 依赖 insert_memory 安全网);用 `cargo test --lib` 现有记忆测试覆盖兜底。

## 6. 回滚

- migration 加列非破坏;回滚 = 保留列不用(或 drop,但项目 migration 是 append-only,不回滚)。
- ChatEvent Recall 变体 additive;前端不渲染 = 无 chip(回退历史统计)。
- 新 IPC 不调用 = 无功能。
- update_memory 安全网复用 insert_memory 提取 helper,回归点 = 现有 insert_memory 测试。

## 7. 风险文件

- `agent/chat_loop.rs`(加 3 处 emit;worker sink 隔离验证)
- `db/memories.rs`(update_memory + validate_memory_text 提取,insert_memory 回归)
- `db/migrations.rs`(新 migration,编号对齐)
- `state.rs`(ChatEvent Recall 变体 + RecallHit)
- `stores/memory.ts`(listener 注册时序 + lastRecallHits 生命周期)
