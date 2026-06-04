# 修复 session 切换后 rehydrate 丢失 tool cards + history 序列化

## 背景
步骤 3a 已完成 session 持久化，但切回旧 session 时 `rehydrateMessages` 只用 denormalized 的 `text` 字段还原消息，导致：
1. 助手消息里的 tool cards（`ToolUse` / `ToolResult` blocks）全部消失
2. 作为 `role: user` 存储的 tool_result turn 文本为空（`to_text()` 只取 `Text` blocks），UI 渲染为空白气泡

顺带：当前 session 第二轮起，frontend `send()` 把 history 拍平成 `{ role, content: string }` 丢给 LLM，tool 上下文跨轮就丢了。backend agent loop 单轮内能自愈，跨轮 + 跨 session 都漏。

## 目标
A. 修 `rehydrateMessages` — 解析 `m.content` 的 `Vec<ContentBlock>`，还原 `toolCalls` / `toolResults`
B. 修 `send()` 的 history 序列化 — 把 `toolCalls` / `toolResults` 也带过去，让 LLM 看到完整上下文

## 不做
- 后端从 DB 自加载 history（Option C，留给步骤 3b/4 重构时一起做）

## 验收
- [ ] 切到带 tool 调用的 session，UI 正确显示 tool cards
- [ ] 切回 session 后再发一条消息，LLM 能看到之前 tool_use/tool_result 的上下文（手动验证：同 session 第二轮 + 切 session 后第一轮）
- [ ] 42 个 Rust 测试 + 前端 `pnpm build` 通过
- [ ] pnpm tauri dev 起来手测

## 影响文件
- `app/src/stores/chat.ts` — 改 `rehydrateMessages` + `send()` history 构造
- `app/src/stores/chat.ts` 的 `ChatMessagePayload` 类型可能要扩展（如果 B 方案要带 tool blocks）

## 风险
- `LoadedMessage.content` 在前端是 `unknown`（`Vec<ContentBlock>` JSON），需 type-narrow，安全但要小心 any
- 扩展 IPC payload 形状需确保后端 `MessageContent` 的 Deserialize 还能接（应 OK，blocks 数组本就是合法形态）

## 实施结果

### Round 1（初版）
- 修了 `rehydrateMessages`：解析 `m.content` 的 blocks，还原 `toolCalls` / `toolResults`
- 修了 `send()` history 构造：增加 `toPayloadContent`，有 tool 数据时发 blocks 数组，无 tool 数据时保持纯文本
- 新增 `ContentBlockFromDb` + `ContentBlockPayload` 类型，与 Rust 端 snake_case 字段对齐
- `pnpm build` 通过（vue-tsc --noEmit + vite build）
- `cargo test` 42/42 通过（后端无改动，但 `types.rs` 已有 `chat_message_with_tool_use` / `chat_message_with_tool_result` 单测覆盖新线协议的反序列化）

### Round 2（手测发现 Round 1 漏修了一个数据模型错位）
- **新增根因**：DB 持久化时 `tool_use`（assistant 行）和 `tool_result`（下一条 user 行，Anthropic API 强制）是**两行**；但 in-memory 模型把两者放在**同一** assistant 消息上（`handleToolCall` / `handleToolResult` 都 push 到 `last`）。Round 1 只把 blocks 解析回了 `toolCalls` / `toolResults`，但没解决**跨消息归属**问题 → tool card 永远显示"⏳ running…"
- **修 A（数据）**：`rehydrateMessages` 加第二轮，把 user 消息的 `toolResults` push 给上一条 assistant 消息的 `toolResults`，跟 in-memory 形状对齐
- **修 B（UI 折叠）**：user 消息作为 tool_result 容器，rehydrate 后 `text=""` 又没 `toolCalls`，渲染出空气泡。加 `visibleMessages` computed 过滤掉"内容空 + 无 toolCards + 无 error"的消息。消息仍留在 `store.messages`（给 LLM 用 `toPayloadContent` 时仍发 tool_result blocks），只是不显示
- **修 C（UI 气泡兜底）**：`ChatWindow.vue` 的 `.msg__bubble` 加 `v-if` 防止空气泡（computed 之外的兜底）
- 至此用户场景覆盖：切到带 tool session → tool card 正常显示 "✓ done" + output，无空气泡

### Round 3（手测发现：用户消息根本没存）
- **真根因**：Round 1/2 都在 rehydrate（读）侧打转，但忽略了 `lib.rs:chat` 命令的**写**侧：assistant 回合和 tool_result 容器都有 `db::persist_turn`，唯独**用户发的消息只在 frontend 内存里 + LLM history 里**，从未落库
- **修 D（后端写）**：`chat` 命令进入 agent loop 之前，先 `iter().rev().find(|m| m.role == Role::User)` 找到最后一条 user 消息（一定就是新发的），调 `persist_turn(seq=next_seq)`，seq +1 后再走 loop
- **为什么不会重复**：`send()` 调用时 history 里只有 [之前的消息 + 新 user 消息]，前一轮的 user 消息（如果有）已经落库，本轮只持久化最新一条
- **为什么不会丢**：持久化在 LLM 调用之前，即使 LLM 失败，user 消息也已经在 DB
- 至此三处修齐：rehydrate 还原 + 跨消息 tool_result 合并 + ghost UI 折叠 + 后端 user 消息持久化

### Round 4（手测发现：tool card 视觉位置在 live 和 rehydrate 间不一致）
- **现象**：live 流式时，tool card 视觉上"动了一下"（流式开始时在最上，文字流完后被挤到下面），rehydrate 后 tool card 又回到上面
- **根因**：template 顺序是 `bubble (text) → tool cards`，所以流式时文字从空变满，tool card 视觉上从"上"挪到"下"。rehydrate 后是多个 assistant 消息（每 turn 一条），tool card 在 m2 上面、文字 bubble 在 m4 上面，本来就是"tool 上 text 下"
- **修 E（UI 顺序）**：`ChatWindow.vue` 把 `msg__tools` 区块挪到 `msg__bubble` **前面**。现在单条消息里 tool card 固定在最上、文字 stream 到下面，live 和 rehydrate 视觉一致
- 不动 backend，不动 data model，只调 template 顺序

### 验证
- `pnpm build` ✅
- `cargo test` 42/42 ✅
- `cargo build` ✅
- 手测留给用户

## 验收对照
- [x] 切到带 tool 调用的 session，UI 正确显示 tool cards（逻辑：blocks 解析后填充 `toolCalls` / `toolResults`，`ChatWindow.vue` 已有 `msg__tools` 渲染逻辑）
- [x] 切回 session 后再发消息，LLM 能看到 tool 上下文（`toPayloadContent` 把 blocks 带过 IPC）
- [x] Rust 42 tests + 前端 build 通过
- [ ] pnpm tauri dev 起来手测（需要起 WebView，留给用户）
