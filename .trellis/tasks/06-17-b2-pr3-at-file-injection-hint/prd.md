# B2 PR3 @文件注入提示（前端）

## Goal

user message 下方显示 **@文件注入清单提示行**，让用户确认 LLM 实际看到了什么（注入内容 / 降级 / 跳过），解决"LLM 瞎编看了"的疑虑。PR2 后端已注入内容（commit `a00adbc`），但 `inject_at_tokens` 返回 `()`、注入清单丢失，且 DB 存原始 `@relpath`、前端 MessageItem 走 markdown 纯文本显示——用户无法确认注入结果。

## Context

- **PR2**（`.trellis/tasks/06-17-b2-pr2-at-file-injection/`，commit `a00adbc`）：后端 `agent/at_file.rs::inject_at_tokens` 就地把 @relpath 注入文件内容（text 复用 `read_file` 截断）/ 占位降级（图片PDF/Office/二进制）/ 无效路径保留原 token。**返回 `()`，清单丢失**。
- **当前前端**：MessageItem 走 `renderMarkdown`（v-html），@token 纯文本；PR1.5 着色只在 ChatInput 输入框。
- 6 家调研（`docs/research/at-file-injection-coding-agents-survey.md`）：CC/Cursor 等 @file 有 chip/提示展示。
- 用户痛点：看不到 @文件被注入了什么 → 疑似 LLM 瞎编。

## Decision（已决 2026-06-17）

| 决策 | 定稿 |
|---|---|
| **展示形态** | **简洁提示行**（user message 下方一行，列注入清单），不喧宾夺主 |
| **数据流** | 后端实时推 `ChatEvent` + 存 `messages.metadata`（reload 用） |
| **复用** | ToolCallCard「message 下挂」模式（`message.injections` 平行 `toolCalls`）；`ChatEvent` 事件流；`messages.metadata` JSON 列 |

**提示行文案**：
```
📎 已引用文件:
   · src/foo.ts   ✓ 注入 48 行
   · bar.png      ⊘ 图片·未注入(B1)
   · spec.docx    ⊘ 文档·未注入(可 pandoc 转换)
   · missing.txt  ⊘ 跳过(不存在)
```
（只列被 @ 引用的文件；无效路径也列出来标"跳过"，让用户知道引用被识别但未注入）

## Research 关键结论（Explore 调研，架构通道现成）

- **持久化通道现成**：`messages.metadata TEXT` JSON 列（`db/migrations.rs:202`）；`MessageRow.metadata: Option<serde_json::Value>`（`db/types.rs:291`）。`persist_turn` 已有 metadata 参数（`chat_loop.rs:267` 传 `None`）——**PR3 直接传清单 JSON，无需新 db 函数**。
- **事件通道现成**：`ChatEvent` enum（`llm/types.rs:326`），加 `FileInjections` variant，复用 tool:call 事件流（`streamController.ts::handleToolCall` 模式）。
- **展示复用**：`MessageItem.vue:176` `.msg__tools` + `toolCalls` 数组模式，`injections` 平行加一套（同类容器 + 子渲染）。
- **ChatMessage interface**（`stores/chat.ts:114`）有 `toolCalls/toolResults/thinkingBlocks`，加 `injections?: InjectionRecord[]` 容易。

## Requirements

### 后端（Rust）
- `inject_at_tokens` 改返回**注入清单**（保留就地展开 content 的行为 + 额外返回清单）。清单结构：
  - `InjectionRecord { path: String, action: InjectionAction }`
  - `InjectionAction`: `Injected { lines: usize }` | `Degraded(FileKind)` | `Skipped(SkipReason)`（SkipReason: OutOfRoot/Missing/Unreadable）
- **持久化**：调整 `chat_loop` 顺序——快照最后一条 user message 的**原始 content**（@relpath）→ `inject_at_tokens`（产生清单 + 就地展开 content）→ `persist_turn` 用**原始 content + metadata=清单 JSON**（保持 PR2"DB 存原始 source of truth"+ 加 metadata）。其余 user message 的清单按需（历史 message 重载时从 metadata 读，不重新注入）。
- **实时推送**：inject 后发 `ChatEvent::FileInjections { request_id, message_seq, injections }`，前端更新对应 user message。
- 注入清单序列化进 `metadata`（serde_json::Value），reload 时前端从 `MessageRow.metadata` 读。

### 前端（Vue 3 / TS）
- `ChatMessage` interface 加 `injections?: InjectionRecord[]`（TS type 镜像后端 enum）。
- `streamController`：handle `FileInjections` 事件 → 更新对应 user message（按 request_id + message_seq 定位）的 `injections`。
- session reload：从 DB `message.metadata` 反序列化 `injections` 填充。
- `MessageItem.vue`：user message（`msg--user`）下方，`injections` 非空时渲染提示行（见 Decision 文案）；轻量 secondary 色 + monospace path + ✓/⊘ 状态符。
- 提示行用独立子组件或内联（参考 `.msg__tools` 容器模式）。

### 单测
- 后端：`inject_at_tokens` 返回清单正确（text 注入行数 / 各 Degraded kind / 各 Skipped reason / 多 token / 无 token 空清单）。
- 前端：vue-tsc 0 错误（无测试框架）。

## Acceptance Criteria

- [ ] @text 文件 → user message 下方提示 `· src/foo.ts ✓ 注入 N 行`（N=文件行数）。
- [ ] @图片/pdf/office/binary → 提示对应 `⊘ <类型>·未注入(...)`。
- [ ] @无效路径（越界/不存在/不可读）→ 提示 `· xxx ⊘ 跳过(<原因>)`。
- [ ] 多 @token → 提示行列全部。
- [ ] session reload 后提示仍显示（从 metadata 读）。
- [ ] 实时：send 后提示行尽快出现（inject 后事件推）。
- [ ] 后端单测清单正确；`cargo check` 0 warning；`vue-tsc` 0 错误。
- [ ] 不破坏现有 tool call 卡片渲染 + PR2 注入行为。

## Out of Scope

- 注入内容预览（卡片展开看文件内容）——当前用简洁提示行，卡片形态留后续。
- 内联 chip（@token in text → chip）——改 markdown 管线，留后续。
- 图片实际注入（multimodal）——B1（第三档）。
- 历史 message 重新注入（reload 只显示当时 metadata 清单，不重新读文件）。

## References

- **PR2 后端注入**: `.trellis/tasks/06-17-b2-pr2-at-file-injection/`（commit `a00adbc`），`app/src-tauri/src/agent/at_file.rs`
- **6 家调研**: `docs/research/at-file-injection-coding-agents-survey.md`
- **持久化**: `db/migrations.rs:202`（messages.metadata）, `db/types.rs:291`（MessageRow.metadata）, `chat_loop.rs:267`（persist_turn metadata 参数）
- **事件**: `llm/types.rs:326`（ChatEvent enum）, `stores/streamController.ts`（handleToolCall 模式）
- **展示复用**: `components/chat/MessageItem.vue:176`（.msg__tools + toolCalls）, `ToolCallCard.vue`
- **前端消息结构**: `stores/chat.ts:114`（ChatMessage interface）
