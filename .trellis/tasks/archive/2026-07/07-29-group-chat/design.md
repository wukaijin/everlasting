# Design — group chat Phase 4 (后端 + 前端 + 人类插话)

> 技术设计。需求见 `prd.md`;Phase 4 待改文件清单与 TODO-A/B 见 `prd.md` 「Phase 4 接手指南」节。
> 本文聚焦 **跨层契约 + run_chat_loop 签名变更 + reload/落库一致 + UI 渲染 + 风险**。

## 1. 设计裁决(已固化进 PRD §设计裁决)

| Q | 决策 | 关键依据 |
|---|---|---|
| Q1 | `run_chat_loop` 加 `current_speaker: Option<String>` 参数(尾部追加,紧邻 `group_chat_state`) | 显式 > 隐式;34 参再加 1 参代价可控;现有 16 个 callsite 全部填 `None` |
| Q2 | 创建入口 + 简版改 modal(共一个 `GroupChatConfigModal`),不做 inline 实时增删 | D4 数据能力声明 ≠ D5 功能边界;MVP 边界 |
| Q3 | 主持人 persona 不配,固定 speaker 名 `"moderator"` | 模板已内置 |
| Q4 | 抢占语义,工具调用权限弹窗被 cancel 天然吃掉(oneshot 监听 cancel token) | D9 + 现有基建已支持 |

## 2. 数据契约

### 2.1 DB ↔ Rust(`MessageRow`)

`db/types.rs:419` `MessageRow` 加字段:
```rust
pub speaker: Option<String>,  // F4 group chat: messages.speaker 列透传
```
读取侧: `db/sessions.rs:284` (messages SELECT map) 加 `speaker: r.try_get("speaker")?`。
写入侧: `persist_turn` 已接 `speaker: Option<&str>` 参数(Phase 1 落地,line 729),INSERT 包含 speaker 列(Phase 1 migration 已加,line 747)。**无需改 persist_turn**。

### 2.2 Rust ↔ Frontend(`ChatMessage`)

`llm/types.rs:201` `ChatMessage.speaker: Option<String>` 已加,serde `skip_serializing_if = Option::is_none`(Phase 1)。
**前端 `SessionSummary` 要加 `session_type` + `metadata`**(`stores/chat.types.ts:310`),否则前端无法识别群聊 session 类型与读 participants。

### 2.3 主持人 vs 参与者(models 区分)

```rust
// agent/group_chat.rs::build_group_chat_ctx (已实现,Phase 1)
moderator_model_id: session.model_id.clone().unwrap_or(session.model.clone()),
participant.model:  participant.model_id,  // ProviderCatalog key
```
两路都是 ProviderCatalog key,语义与现有 `per_dispatch::resolve` 同形,**零新增**。

### 2.4 主持人 speaker 固定标识

`group_chat_loop.rs:181` 派发 moderator turn 时传 `current_speaker: Some("moderator")`(新参数)。落库 `messages.speaker = "moderator"`。
`group_chat_loop.rs` participant turn 派发传 `current_speaker: Some(participant.name.clone())`。

> **为什么不用更细的 `moderator:<session_id>`?** 同一 session moderator 是唯一固定角色,固定 `"moderator"` 字符串足够 UI 区分;多 session 并发时 session_id 已在 `messages.session_id` 列。

### 2.5 前端 UI 模型

```ts
// stores/chat.types.ts (扩)
export interface SessionSummary {
  // ... 既有字段
  session_type: "chat" | "group_chat";   // ← 新增
  metadata: Record<string, any> | null;  // ← 新增(解析后)
}

export interface ChatMessage {
  // ... 既有字段
  speaker?: string;  // ← 已加,前端 layer 透传
}

// 新增(放 chatGroup.config.ts 或 chat.types.ts)
export interface ParticipantConfig {
  name: string;
  model: string;          // model_id
  persona_md?: string;
  order?: number;
}
```

## 3. 后端变更(三处)

### 3.1 `run_chat_loop` 签名扩 1 参

`app/src-tauri/src/agent/chat_loop.rs:175` 在 `Some(turn_state)` 之后追加:
```rust
// F4 group chat (Phase 4): per-turn speaker. `None` for normal
// chat / subagent / review; `Some("moderator")` for moderator turn
// in group_chat; `Some(participant.name)` for participant turn.
// Carried into the assistant persist site (line 2118) so messages
// are stored with the originating speaker. Read-only — never
// affects tool routing, role mapping, or wire shape.
current_speaker: Option<String>,
```

**助手落库点(唯一写点)**:`chat_loop.rs:2118` 改:
```rust
speaker: current_speaker.clone(),
```

**其他 10 处 `speaker: None` 保持不变**(都是构造 inline 中间变量,非落库点)。grep 列表:757/768/818/903/1202/1212/1494/1521/2151/4217/4270。这些是 message construction / `msg.speaker.as_deref()` 只读,不动。

### 3.2 `MessageRow` ↔ messages SELECT

`db/types.rs:419` +`db/sessions.rs:284`(消息 map) + `db/sessions.rs:222`(SELECT)— 三处加 `speaker` 列。
**回归**: 现有 1616 单元测试若直接断言 `MessageRow` 字段数,可能塌;但 `MessageRow` 实际无 derives `Eq`/`Hash`,断言以具体字段为主。加 optional 字段不破坏 `FromRow` derive。

### 3.3 `reload_messages` 透传

`agent/group_chat_loop.rs:100` `reload_messages` 函数:
```rust
ChatMessage {
    role,
    content,
    speaker: m.speaker,  // ← 改: 直读 MessageRow.speaker
}
```
注释也同步更新(原 line 110-112 写 "speaker is carried in the row's metadata-less top-level" → 注释已写好,实际代码没接)。

### 3.4 `run_group_chat_loop` 派发点两处传 speaker

`group_chat_loop.rs:181` moderator 派发:
```rust
run_chat_loop(
    /* ... */
    Some(turn_state.clone()),
    Some("moderator".to_string()),  // ← 新增
)
```

participant 派发(grep `run_chat_loop` 在 group_chat_loop.rs 第二处):
```rust
run_chat_loop(
    /* ... */
    Some(turn_state.clone()),
    Some(participant.name.clone()),  // ← 新增
)
```

## 4. 前端变更(四层)

### 4.1 类型 + 序列化

`app/src/stores/chat.types.ts`:
- `SessionSummary` 加 `session_type` + `metadata: Record<string, any> | null`。
- `LoadedMessage`(若有)透传 `speaker` —— `streamController.ts` rehydrate 处核对。

### 4.2 `streamController.ts:rehydrateMessages`

读取 `LoadedMessage.speaker` → 写入 `ChatMessage.speaker`。**零业务逻辑**,只透传。

### 4.3 `createNewSession` 支持 group_chat

`app/src/stores/chat.ts:549` `createNewSession` 加分支:
```ts
type CreateSessionOpts = {
  // ... 既有
  sessionType?: "chat" | "group_chat";
  participants?: ParticipantConfig[];
};
if (opts.sessionType === "group_chat") {
  payload.session_type = "group_chat";
  payload.metadata = { participants: opts.participants };
}
```

后端 `commands/sessions.rs:106` `create_session` 加 `session_type` + `metadata` 两个 optional 参数(serde 直接透传,DB INSERT 已支持两列,Phase 1 落地)。

### 4.4 `update_session_metadata` IPC

`commands/sessions.rs` 新增 `update_session_metadata(session_id, metadata: serde_json::Value)` —— 写 `sessions.metadata` 列,允许群聊配置 modal 重新编辑。
**SQL**: `UPDATE sessions SET metadata = ?2 WHERE id = ?1`。
**路由**: `daemon/routes/sessions.rs` 镜像为 `PATCH /api/v1/sessions/:id/metadata`(REST 形态对得上)。
**前端 façade**: `chat.ts` `updateGroupChatConfig(sessionId, participants)` 调该 IPC。

### 4.5 `GroupChatConfigModal` 组件

新建 `app/src/components/chat/GroupChatConfigModal.vue`(~150-200 行,模板形态):
- 复用现有 reka-ui `Dialog` + 表单组件。
- 内部子组件 `GroupChatParticipantRow.vue`:
  - `name` 输入(text,校验唯一 + 非空)
  - `model` 下拉(从 `models` store 拉所有 model_id)
  - `persona_md` textarea(monospace,可选)
  - 删除按钮(至少 2 个时启用)
- 顶部"+"加 participant(上限 3 个,D5)
- 底部:取消 / 保存(创建时调 `createNewSession`;编辑时调 `updateGroupChatConfig`)
- 简化:order 字段前端不暴露,提交时按数组顺序写 `order: 0..n-1`(P2 stub)

### 4.6 `SessionList` / `NewSessionButton` 入口

`SessionList.vue` 加 "新建群聊" 按钮(旁边或下拉里),点击 → 打开 `GroupChatConfigModal` 创建态。
**编辑入口**: 群聊 session 的右键菜单 / header 按钮 → 打开同一 modal 编辑态(传 `sessionId` + 当前 participants)。

### 4.7 `MessageItem.vue` speaker chip 渲染

`components/chat/MessageItem.vue:1121` `msg--${message.role}` 旁边加 speaker 渲染:
```vue
<div v-if="message.speaker" class="msg-speaker-chip" :class="speakerAccent(message.speaker)">
  {{ speakerLabel(message.speaker) }}
</div>
```

`speakerLabel(s)`:
- `"moderator"` → "主持人"
- 其他 → 原名(participant name)

`speakerAccent(s)`:
- `"moderator"` → 固定 token 色(neutral 或主色)
- 其他 → `colorTagFor(s)`(复用 `utils/colorTag.ts`,按 name hash 选色)
- 普通 chat(无 speaker) → 不渲染 chip

**视觉一致性**: 复用现有 `msg--user` / `msg--assistant` 类的 layout 位置,在 role label 旁边加 chip,不破坏现有样式 token。

## 5. IPC / Daemon 路由矩阵

| 旧 IPC | 改动 | Daemon REST 镜像 |
|---|---|---|
| `create_session` | inputs 加 `session_type?` + `metadata?` | `POST /api/v1/sessions` 同步加 |
| `get_sessions` | 返回 SessionSummary 结构加新字段 | `GET /api/v1/sessions` 同步 |
| `load_session` | 返回消息带 speaker | `GET /api/v1/sessions/:id` 同步 |
| **新增** `update_session_metadata` | `(session_id, metadata)` | `PATCH /api/v1/sessions/:id/metadata` |
| `chat` | unchanged(走 `session_type` branching) | `POST /api/v1/chat` 不变 |
| `cancel_chat` | unchanged(创造 cancel 路径已就绪) | `POST /api/v1/cancel` 不变 |

## 6. 人类插话(D9)路径

UI(前端):
1. 群聊 session,ChatInput 始终 enabled(run_chat_loop 在跑 → input 上显示"取消并发送"文字变体,具体 UX 看现有 cancel UI 复用)。
2. 人类提交 → `transport.invoke("cancel_chat", { session_id })` → 等待 cancel 完成 (poll `event`)→ `send` 新消息。

后端(`run_group_chat_loop` 现状已支持):
- `commands/cancel.rs:6` cancel `rid` → `token.cancel()` → `run_group_chat_loop` 的 `for round in 0..MAX ... if token.is_cancelled() { break; }` 跳出 → 落库不完整 turn(messages 部分)留作既成历史。
- 重新 `chat` send → 该 session 仍 `group_chat` → `chat_inner` 走 `run_group_chat_loop` 新分支 → `reload_messages` 读 DB 最新(含人类插话)→ moderator 重新决策。

**重要:不写「cancel 等到 in-flight turn 干净完成」**,只要 token cancel + 退出 round 即可。中间落库 messages 的完整性由现有 `persist_turn` 保证(每轮 done 落一次)。

## 7. 边界与回归

### 7.1 兼容性

- `session_type = chat` 的存量 session:`sessions.session_type` migration default `"chat"`(Phase 1 落地,Phase 1 Acceptance 已过);普通 chat 路径 `current_speaker` 全传 `None`,落库 `messages.speaker = None`,读取侧 `MessageRow.speaker: None` → 透传 `None` → 前端 `ChatMessage.speaker` undefined → UI 不渲染 chip。**完全零回归**。
- `messages.speaker` 列迁移: Phase 1 落地,nullable,默认 NULL。**无新迁移**。
- `run_chat_loop` 签名加 1 参,16 个 callsite 全部填 `None`(grep 已在 5.1 列出)。**编译时一次性机械改动,不涉及逻辑**。

### 7.2 失败模式

| 失败 | 现状 | F4 改动 |
|---|---|---|
| participant 名字冲突 | participants JSON 允许同名 | UI 校验不重;后端 `participant_by_name` 返回第一个;不抛错 |
| 主持人 fire 工具权限弹窗被 cancel | ask_user_question 监听 cancel token | 不变;D9 验证时确认 |
| wire 层 OpenAI `name` 字段非法 | OpenAI provider 过滤 | 不变;前端不传 `speaker` 给 wire |
| 前端 metadata 解析失败 | JSON 解析 `serde_json::from_str` `ok()` | 现有 SessionRow 解析已经 permissive,加optional metadata 同形 |

### 7.3 性能

- 群聊 max_turns 上限 30 轮 → 单 session 最多 30 * (participants + 1) messages。MVP 范围 OK。
- compaction 复用现有 `compact_messages`(C3 压缩),群聊也共享。**零新机制**。

## 8. 测试设计

### 8.1 后端

**单测(新增)**:
- `db/tests_message_speaker.rs`(或加进现有 tests.rs):
  - `persist_turn(...speaker=Some("alice"))` → reload `load_session` → `MessageRow.speaker == Some("alice")`。
  - `persist_turn(...speaker=None)` → reload → `MessageRow.speaker == None`(回归)。
- `agent/tests_group_chat_loop.rs`(新建):
  - moderator turn 跑完,落库消息 `speaker == Some("moderator")`。
  - participant turn 跑完,落库消息 `speaker == Some(participant.name)`。<br>
  *注*:这俩测试需要 mock provider 或复用现有 `tests_subagent` 的 MockProvider 模式。
- `agent/tests_chat_loop_speaker_rs`(在现有 tests 子目录):
  - `current_speaker=Some("alice")` 跑 → 落库消息带 speaker。
  - `current_speaker=None` 跑 → 落库消息 `speaker=None`(回归)。

**回归**: `cargo test --lib`(1616 全绿)+ `cargo clippy --lib --tests` 通过。

### 8.2 前端

**vitest**:
- `chat.types.ts` 新增字段 round-trip(JSON.parse 序列化反序列化留字段)。
- `streamController.rehydrateMessages` 传 `speaker` → 落 `ChatMessage.speaker`。
- `GroupChatConfigModal` 单元测试: 加/删/重排 + 校验(name 唯一、上限 3、必填 model)。
- `MessageItem.vue` 渲染测试: 有 speaker 渲染 chip,无 speaker 不渲染。

### 8.3 端到端(手工)

按 PRD 「Phase 4 验证方法」:
- 手工建 group_chat session(DB 改 session_type + 写 metadata)。
- 前端发首条 → 主持人点名 → participant 发言 → 人类插话 → end_discussion。
- 截图留作 acceptance 证据。

## 9. 风险 + 回滚

| 风险 | 等级 | 缓解 |
|---|---|---|
| `run_chat_loop` 加参漏改某个 callsite | 中 | 编译期 16 处全部填 `None`,grep 验证 |
| 主持人权威性 — 真模型不调用 nominate | 中 | Phase 3 已加 round-robin fallback + MAX 30 轮 |
| Anthropic 前缀注入 `@alice:` 混淆 | 低 | 已有 phase 2 wire 形态;模型真验证在端到端做 |
| 主持人 persona 想加上 | 低 | 元数据加 `moderator_persona_md` 预留(D3 接受),MVP 不读 |
| 抢占时 in-flight turn 部分落库 | 中 | 现有 `persist_turn` 每 turn done 落一次;cancel 触发已落的部分保留,语义可接受 |
| `SessionSummary.metadata` JSON 解析失败 | 低 | permissive 解析,失败 → null,前端 multi-cast 兜底 |

**回滚**: Phase 4 是纯加参数 + 加字段 + 加 modal 组件,**无破坏性变更**。`git revert <commit>` 单点回退,旧 chat session 行为完全不变。

---

## 10. 待办清单(P0 = Step 1 必须做完)

- [ ] **P0 TODO-A**:`run_chat_loop` 加 `current_speaker` 参数;16 callsite 填 `None`;落库点(`chat_loop.rs:2118`)透传。
- [ ] **P0 TODO-B**:`MessageRow` 加 `speaker`;`load_session` SELECT + map 加列;`reload_messages` 透传。
- [ ] **P1 前端类型**:`SessionSummary` + `ChatMessage.speaker` 透传;`streamController.rehydrateMessages`。
- [ ] **P1 创建入口**:`createNewSession` 支持 group_chat + participants;`update_session_metadata` IPC + REST mirror;`GroupChatConfigModal` 组件。
- [ ] **P1 UI**:`SessionList` 加入口;`MessageItem` 加 speaker chip + colorTag hash 配色。
- [ ] **P2 集成**:端到端手工验证(主持人 → participant → 人类插话 → end)+ 1616 回归 + clippy。
