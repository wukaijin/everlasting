# Implement — group chat Phase 4

> 执行清单。需求见 `prd.md`;技术设计见 `design.md`。
> 顺序按依赖 DAG:后端落库/读取接通 → 前端类型 → 创建入口 → UI 渲染 → 插话验证 → 集成回归。

## 阶段 A:后端签名扩 1 参(Step 1 上半)

- [ ] **A1**. `app/src-tauri/src/agent/chat_loop.rs:175` `run_chat_loop` 签名在 `Some(turn_state)` 之后追加 `current_speaker: Option<String>`(加 doc 注释指向 PRD Q1 设计裁决)。
- [ ] **A2**. `chat_loop.rs:2118` 唯一落库点改成 `speaker: current_speaker.clone()`。
- [ ] **A3**. 全仓 4 个生产 callsite 填值:
  - `agent/chat.rs:422` → `None`
  - `agent/subagent/dispatch.rs:1159` → `None`
  - `agent/group_chat_loop.rs:181` (moderator) → `Some("moderator".to_string())`
  - `agent/group_chat_loop.rs:280` (participant) → `Some(participant.name.clone())`
- [ ] **A4**. 测试 callsites 共 58 处(主要 `tests_subagent.rs` ~15 + `tests_agent_loop.rs` ~4 + `tests_sse.rs:44` + `tests_c2plus.rs:90/800` + `tests_ask_user_question.rs:181` + 其他),全部填 `None`。**机械改动,无逻辑**。
- [ ] **A5**. 二次 grep `speaker: None` 在 chat_loop.rs 只剩 10 处(全为中间变量 / 只读),无落库点遗漏:
  ```bash
  grep -n "speaker: None" app/src-tauri/src/agent/chat_loop.rs
  # 期望 line 2118 不再出现
  ```
- **验证**:
  ```bash
  cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo build --lib
  # 期望: 0 error,grep 验证 A5
  ```

## 阶段 B:DB 读取接通(Step 1 下半)

- [ ] **B1**. `app/src-tauri/src/db/types.rs:419` `MessageRow` 加 `pub speaker: Option<String>`(位置: `metadata` 后,`ttfb_ms` 前——按列语义聚类)。
- [ ] **B2**. `app/src-tauri/src/db/sessions.rs:222` (messages SELECT) 末尾 `, speaker` 加列(核对 migrations.rs:213-225 已加列)。
- [ ] **B3**. `db/sessions.rs:284` (MessageRow map) 加 `speaker: r.try_get("speaker")?`。
- [ ] **B4**. `app/src-tauri/src/agent/group_chat_loop.rs:100` `reload_messages` 改 `speaker: m.speaker`(原 `speaker: None`)。
- [ ] **B5**. 同时核对 `db/sessions.rs` 其它 messages SELECT(120/963/1123 行的子查询)是否要同步加 speaker —— 这些是 `id`+`content`/`text` 子查询,不需要 speaker 列,无需改。
- [ ] **B6**. `persist_turn` 调用点(`chat_loop.rs:4270` 附近)透传 `current_speaker.as_deref()` 到 `speaker: Option<&str>` 参数(已存在)。
- **验证**:
  ```bash
  cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib
  # 期望: 1616 全绿,Speaker 字段读取 0 报错
  ```

## 阶段 C:后端测试

- [ ] **C1**. 新建 `app/src-tauri/src/agent/tests_group_chat_speaker.rs`(或在 `tests_subagent.rs` 追加):
  - 测试 `run_chat_loop(provider, ..., current_speaker=Some("alice"))` → 落库最新 message `speaker == Some("alice")`。
  - 测试 `current_speaker=None` → 落库 `speaker == None`(回归)。
- [ ] **C2**. 在 `db/tests.rs`(或 `tests_db_sessions.rs` 现有测试文件)加:
  - `persist_turn(...speaker=Some("alice"))` → `load_session` → 末条 message `speaker == Some("alice")`。
  - `persist_turn(...speaker=None)` → 末条 `speaker == None`(回归)。
- [ ] **C3**. `tests_group_chat_loop.rs`(若 Phase 3 已有)追加:moderator turn 落库 message `speaker == Some("moderator")`;participant turn 落库 `speaker == Some(participant.name)`。
- [ ] **C4**. `cargo clippy --lib --tests` 零新 warning。
- **验证**:
  ```bash
  cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib
  cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo clippy --lib --tests
  ```

## 阶段 D:前端类型 + rehydrate(Step 2)

- [ ] **D1**. `app/src/stores/chat.types.ts:310` `SessionSummary` 加:
  ```ts
  session_type: "chat" | "group_chat";
  metadata: Record<string, any> | null;
  ```
- [ ] **D2**. 同文件加 `ParticipantConfig` interface(`name: string; model: string; persona_md?: string; order?: number`)—— 放 `chat.types.ts` 末尾或新建 `groupChat.types.ts`。
- [ ] **D3**. `app/src/stores/streamController.ts:443` `rehydrateMessages` 读取 `LoadedMessage.speaker` → 写 `ChatMessage.speaker`。(核对 `LoadedMessage` 是否需要先加字段,通常一并加。)
- [ ] **D4**. `ChatMessage` 已有 `speaker?: string`(Phase 1),核对 `streamController` 增量更新事件(`chat-event` 等)是否漏字段;若有事件流组装 ChatMessage 处,补 `speaker: undefined` 默认。
- **验证**:
  ```bash
  cd app && pnpm vue-tsc --noEmit
  # 期望: 0 type error
  ```

## 阶段 E:创建入口 + 配置 UI(Step 3)

- [ ] **E1**. `app/src/stores/chat.ts:549` `createNewSession` 加分支:
  ```ts
  opts: { sessionType?: "chat" | "group_chat"; participants?: ParticipantConfig[] }
  if (opts.sessionType === "group_chat") {
    payload.session_type = "group_chat";
    payload.metadata = { participants: opts.participants };
  }
  ```
- [ ] **E2**. 后端 `commands/sessions.rs:106` `create_session` 加 `session_type?` + `metadata?` 两个 optional 参数(serde 透传,DB INSERT 已支持)。daemon 镜像:`daemon/routes/sessions.rs` 同步加 query 参数解析。
- [ ] **E3**. 新建 `commands/sessions.rs::update_session_metadata` 处理函数 + `#[tauri::command]` 注册,参数 `session_id: String, metadata: serde_json::Value`;SQL `UPDATE sessions SET metadata = ?2 WHERE id = ?1`。
- [ ] **E4**. `daemon/routes/sessions.rs` 镜像 `PATCH /api/v1/sessions/:id/metadata` 路由。
- [ ] **E5**. 前端 `app/src/stores/chat.ts` 加 `updateGroupChatConfig(sessionId, participants)` façade → `transport.invoke("update_session_metadata", ...)`。
- [ ] **E6**. 新建 `app/src/components/chat/GroupChatConfigModal.vue`(~150-200 行):
  - props: `mode: "create" | "edit"; initialParticipants?: ParticipantConfig[]; sessionId?: string`。
  - 子组件 `GroupChatParticipantRow.vue`(同一文件内或独立):
    - `v-model` 三字段(name/model/persona_md)
    - name 校验:session 内唯一
    - 删除按钮:≥2 时启用
  - 顶部"+"≥3 隐藏(D5 上限)
  - 底部:取消 / 保存。create 模式 → `createNewSession({ sessionType: "group_chat", participants: ... })`;edit 模式 → `updateGroupChatConfig(sessionId, ...)`。
  - 复用现有 reka-ui `Dialog` + 表单风格。
- [ ] **E7**. `app/src/components/SessionList.vue` 加 "新建群聊" 按钮 → 打开 `GroupChatConfigModal`(create 模式)。
- [ ] **E8**. 群聊 session 的 header(在 `ChatWindow.vue` 或 `AppHeader.vue` 附近)加 "编辑参与者" 按钮 → 打开 `GroupChatConfigModal`(edit 模式)。
- **验证**:
  ```bash
  cd app && pnpm vue-tsc --noEmit
  cd app && pnpm build  # 编译时 sanity check
  ```

## 阶段 F:speaker chip 渲染(Step 4)

- [ ] **F1**. `app/src/components/chat/MessageItem.vue:1121` 在 role label 旁边加 speaker chip(条件 `v-if="message.speaker"`):
  ```vue
  <div class="msg-speaker-chip" :class="`msg-speaker-chip--${speakerAccent(message.speaker)}`">
    {{ speakerLabel(message.speaker) }}
  </div>
  ```
- [ ] **F2**. `<script setup>` 加 `speakerLabel(s)`: `s === "moderator" ? "主持人" : s`。
- [ ] **F3**. 加 `speakerAccent(s)`: `s === "moderator" ? "neutral" : colorTagFor(s)`(复用 `utils/colorTag.ts` hash 选色)。
- [ ] **F4**. CSS: 复用现有 radius/border token,新增 `.msg-speaker-chip--{neutral,blue,green,...}` 调色板分桶(参照现有 `msg--user` / `msg--assistant` 风格,不引入新色 token)。
- [ ] **F5**. 群聊 session 在消息列表顶部加一条 "群聊 (N participants)" 小提示(可放 MessageList header 旁)。
- **验证**:
  ```bash
  cd app && pnpm vue-tsc --noEmit
  cd app && pnpm test
  ```

## 阶段 G:前端测试

- [ ] **G1**. `app/src/utils/__tests__/groupChatConfig.test.ts`(vitest):
  - `ParticipantConfig` 序列化 round-trip
  - name 唯一性校验
  - 数组顺序 → `order: 0..n-1` 转换
- [ ] **G2**. `app/src/components/chat/GroupChatConfigModal.test.ts`:
  - 加/删/重排交互
  - name 重复 → 保存失败
  - 满 3 参与者时 "+" 隐藏
- [ ] **G3**. `MessageItem.test.ts` 加:
  - speaker=Some("alice") → 渲染 chip
  - speaker=Some("moderator") → 渲染"主持人" + neutral 配色
  - speaker=None → 不渲染(回归)
- [ ] **G4**. `streamController.test.ts`(若存在)或新建:
  - `rehydrateMessages` 传 `speaker` → ChatMessage 保留
- **验证**:
  ```bash
  cd app && pnpm test
  cd app && pnpm vue-tsc --noEmit
  cd app && pnpm build
  ```

## 阶段 H:人类插话验证(Step 5)

- [ ] **H1**. 手工 `pnpm tauri dev` 或 `daemon.sh bg` 起 daemon。
- [ ] **H2**. 手工创建 group_chat session(走 UI 或 DB 改):
  - 2 participants,简单 model(快速测试)
  - 给个短引子 prompt
- [ ] **H3**. 观察首个 moderator turn → 调 nominate → participant 发言 → 验证 messages 表:
  ```sql
  sqlite3 ~/.config/everlasting/everlasting.db "SELECT seq, role, speaker FROM messages WHERE session_id='...' ORDER BY seq"
  # 期望: ... | moderator | alice | participantUser | alice
  ```
- [ ] **H4**. 触发人类插话:在 participant 跑时立刻发自己的消息 → 验证:
  - 旧发言 turn 的 partial persist 不损坏 DB
  - 人类消息 user-role 落库
  - 重新进入 moderator turn,moderator 能看到人类消息
- [ ] **H5**. 触发 `end_discussion` → moderator 结束 → 群聊编排退出。
- [ ] **H6**. 触发权限弹窗场景:让 moderator 走 `ask_user_question` tool 时人类插话 → 验证 QuestionStore oneshot 被 cancel 唤醒(收到的 tool_result `is_error=true` 含 `cancelled_by_session:true`)。
- [ ] **H7**. 截图存档(`/tmp/group-chat-phase4-acceptance/`):
  - 创建 modal
  - 群聊消息流(多 speaker 染色)
  - 人类插话打断
  - end_discussion 收尾

## 阶段 I:全量回归 + 收尾(Step 6)

- [ ] **I1**. 后端全套:
  ```bash
  cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib
  cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo clippy --lib --tests
  # 期望: 1616 + 新增 ≥3 全绿,clippy 0 warning
  ```
- [ ] **I2**. 前端全套:
  ```bash
  cd app && pnpm test
  cd app && pnpm vue-tsc --noEmit
  cd app && pnpm build
  ```
- [ ] **I3**. 1216 回归:不起 daemon,只跑 unit/integration(已含在 I1/I2)。
- [ ] **I4**. `trellis-check`(Agent 形式)验证 spec 合规 + 跨层一致性 + 1616 全绿 + design.md Acceptance 逐项。
- [ ] **I5**. `trellis-update-spec`:把「group chat session type + speaker 维度 + 主持人/参与者区分」沉淀进 spec(后端 `db/models.md` + 前端 `chat-store.md`)。
- [ ] **I6**. commit(Phase 3.4)+ 归档 (`task.py archive 07-29-group-chat`):
  - commit 1: 后端 (A + B + C)
  - commit 2: 前端类型 + UI (D + E + F + G)
  - commit 3: H 阶段验证截图(可选)/doc 增量
  - commit message 沿用 `feat(group-chat): ...` 风格

## Review Gates

- 阶段 A → B: `cargo build --lib` 干净通过 = 签名契约立住。
- 阶段 B → C: SELECT/map 全通 + `persist_turn` ↔ `load_session` round-trip 单测绿 = DB 闭环。
- 阶段 C → D: 后端 1616 + clippy 0 warning = 后端可锁定。
- 阶段 D → E: 前端类型编译过 = wire 契约立住,再写组件。
- 阶段 E → F: UI 渲染基础先做,验证后再加 chip。
- 阶段 G → H: 全套测试绿 = 可进手工验证。
- 阶段 H → I: 端到端三场景(基础 / 插话 / 权限弹窗)全过 = acceptance 落定。

## Rollback Points

| 阶段 | 回滚策略 |
|---|---|
| A–B | `git revert <commit>` 单点回退,旧 chat session 行为完全不变 |
| C | 仅测试,无需回滚 |
| D–E | `git revert <commit>`;前端类型可独立回退,后端 RPC 加 optional 参数不破坏旧调用 |
| F–G | `git revert <commit>`;UI 组件 import 失败可级联到 D,合并 revert |
| H–I | 已提交后的 doc/截图归档,无破坏性 |

## Validation Commands 速查

```bash
# 后端 (WSL 注意 PKG_CONFIG_PATH)
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo build --lib
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo clippy --lib --tests

# 前端
cd app && pnpm vue-tsc --noEmit
cd app && pnpm test
cd app && pnpm build

# 端到端
./scripts/daemon.sh start  # 起 daemon
# 手工 UI 验证

# DB 直接查
sqlite3 ~/.config/everlasting/everlasting.db "SELECT seq, role, speaker FROM messages WHERE session_id='...' ORDER BY seq"
```
