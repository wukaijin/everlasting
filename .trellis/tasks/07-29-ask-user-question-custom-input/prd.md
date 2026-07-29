# ask_user_question custom input + skip-semantics

> 合并任务（两件都动同一个工具/卡片/测试，一次改完）：
> 1. **自由输入**：`ask_user_question` 加「用户自行输入」能力（LLM 按需开启）。
> 2. **skip-semantics**：修掉 review epic C4 标记的 follow-up「跳过被误判为致命错误」。

## 来源

- 自由输入：用户提出（2026-07-29），需求是让 `ask_user_question` 在多选之外支持自由文本输入。
- skip-semantics：`07-27-review-plugin-intake-usability`（C4）§R4 明确标记的 follow-up `ask-user-question-skip-semantics`。C4 PRD 已拍板**方案 A**（`is_error` true→false + content 加 reason/hint），但属跨 plugin agent 行为改动，让出 C4 范围。

## Background（代码事实）

### 当前 `ask_user_question` 形态

纯多选工具。LLM 出 2–4 个选项（`QuestionOption`），用户单选/多选或点「跳过」。

- **LLM schema**（`tools/ask_user_question.rs:definition()`）：`Question` = `{question, header?, options[2..=4], multi_select}`，无自由输入通道。
- **答案结构** `QuestionAnswer`（`question_store.rs:148`）= `{question, header?, options: Vec<String>, multi_select}`，只回传选中 label。
- **前端卡片** `AskUserQuestionCard.vue`：radio/checkbox + 提交/跳过，无输入框。

### skip-semantics 当前死法（C4 E2E 实证）

`tools/ask_user_question.rs:416-421` 的 `InteractionResponse::Cancelled` arm：
```rust
let content = serde_json::json!({"cancelled": true}).to_string();
(content, true, ...)  // is_error: true
```
用户点「跳过」本意是「换方式继续」，但 `is_error=true` + 模型训练里的强关联 → MiniMax-M3 把它当致命错误，输出 `[已停止]` 罢工（session 99866757）。

**两种 cancelled 必须区分**（C4 PRD 已理清，本任务遵守）：

| 返回值 | isErr | 语义 | arm |
|---|---|---|---|
| `{"cancelled": true}` | **本任务改 false** | 用户点「跳过」 | `InteractionResponse::Cancelled`（line 416） |
| `{"cancelled_by_session": true}` | 保持 true | session 级 cancel（Stop / 关 app） | `cancel.cancelled()`（line 374）或 `RecvError`（line 422） |

### ⚠️ C4 PRD 的过期引用（本任务纠正）

C4 PRD 说 skip-semantics 方案 A「需同步改 3 处测试断言（line 507/547/564）」。**实测**那 3 处是 **schema-validation 短路测试**（`assert!(is_error)` 验证空 questions / 超量 / options 越界），是真正的错误，**必须保持 true**。真正要改的是 `cancelled_path_returns_user_cancel_marker`（~line 717，断言在 ~752）+ content 检查（~753-757）。session-cancel 测试（`cancel_arm_returns_session_cancelled_marker`，~line 709）**不动**。

### resolve 路径已确认零改动

- `commands/question.rs:resolve_response_from_args`（line 133）+ `daemon/routes/question.rs`：透传 `Vec<QuestionAnswer>`，serde 自动处理新 optional 字段。
- `execute_blocking` 的 `Answered` arm（line 391）：原样 stringify raw `serde_json::Value`，`custom` 字段自动随 JSON 流给 LLM。
- `chat_loop.rs:2455` C2+ 消费点 re-parse `Vec<QuestionAnswer>`：serde 忽略未知字段，透明。

## Decisions（已拍板）

### D1. 自由输入 = LLM 按需开启 + 互斥（「其他」选项）

- `Question` 加可选 `allow_custom: boolean`（默认 false，与 `multi_select` 同形）。
- 开启时卡片多渲染一行文本输入。
- **自定义文本与选项互斥**：单选打字清选择、选择清文本；多选同理。状态机一条规则，提交门和测试最简。
- 用自定义提交时 `options=[]`、`custom="<文本>"`；选项和自定义**不同时有值**。
- 选互斥而非叠加（备注）：配合「LLM 按需开启」——模型要么想要结构化选择、要么想要自由输入。互斥 wire 语义最干净（`options=[]` ⇔ `custom` 有值），状态机/门控/测试都最简。叠加会让 `allAnswered` 门、互斥清空逻辑、wire schema（两字段可并存）都变复杂。

### D2. skip-semantics = C4 方案 A

- `InteractionResponse::Cancelled` 的 `is_error` 从 `true` → `false`。
- content 从 `{"cancelled": true}` → `{"cancelled": true, "reason": "user_skipped", "hint": "用户主动跳过此问题，请用其他方式继续或直接做决定"}`。
- **session-cancel 两 arm 保持 `is_error: true` 不变**。

## Requirements

### 自由输入

- **R1（后端类型）**：`Question` 加 `#[serde(default)] pub allow_custom: bool`；`QuestionAnswer` 加 `#[serde(default, skip_serializing_if = "Option::is_none")] pub custom: Option<String>`。向后兼容旧 task（缺字段时 default）。
- **R2（LLM schema）**：`definition()` 的 `input_schema` 在 question 对象加 `allow_custom`（boolean, default false, 带描述说明互斥语义）。
- **R3（前端类型）**：`stores/questionCards.types.ts` 的 `Question` 加 `allow_custom?: boolean`、`ToolQuestionAnswer` 加 `custom?: string`（snake_case，遵守文件头 cross-layer 警告）。
- **R4（卡片交互）**：`allow_custom` 为真时渲染文本输入；互斥逻辑（打字清选项 / 选择清文本）；提交门 `allAnswered` 放宽为「选项已选 **或** custom 非空」；`buildAnswer` 用 custom 时 `options=[]` + `custom`。
- **R5（summary 渲染）**：answered 状态 custom 非空时额外渲染 pill（「自定义：<文本>」）。
- **R6（历史回放）**：`MessageItem.vue` 的 `parseAnswerEnvelope` validator 放行 custom 场景下 `options` 可选；`synthQuestions` 回放时 custom 非空则把 custom 文本作为合成 label 渲染。

### skip-semantics

- **R7（后端）**：改 `Cancelled` arm（line 416-421）`is_error` false + content 加 reason/hint；同步 doc bullets（line 272/276-277/279-280）+ `description()`（line 118-119）。
- **R8（后端测试）**：改 `cancelled_path_returns_user_cancel_marker` 断言（`assert!(!is_error)` + content 含 reason/hint）；**不动** schema-validation 测试（507/547/573/601）和 session-cancel 测试（709）。

## Acceptance Criteria

- [ ] **自由输入（后端）**：`Question` 带 `allow_custom=true` 通过 validate；`QuestionAnswer` 带 `custom` round-trip serde 单测通过；`cargo test --lib` 全绿。
- [ ] **自由输入（前端）**：`allow_custom` 为真时渲染输入框；打字与选项互斥；提交门放宽；custom 提交时 wire 含 `custom` 且 `options=[]`；answered summary 渲染 custom。`AskUserQuestionCard.test.ts` 覆盖以上 + 修过 toEqual。
- [ ] **历史回放**：带 custom 的 answer-envelope 在 `MessageItem` 回放正常；`MessageItem.test.ts` 加回放用例。
- [ ] **skip-semantics**：用户跳过返回 `is_error=false` + content 含 reason/hint；session-cancel 仍 `is_error=true`；断言修正。
- [ ] **回归**：`cargo test --lib` + `cargo clippy --lib --tests` + `pnpm test` + `pnpm vue-tsc --noEmit` + `pnpm build` 全绿。
- [ ] **wire 向后兼容**：旧 task（无 custom / allow_custom 字段）的 task.json + 历史 tool_result 仍能正常反序列化 + 渲染。

## Out of Scope

- **叠加语义（备注）**：custom 与 options 并存。本任务做互斥（D1）。
- **强制 JSON 输出约束**：reviewer/subagent 的结构化输出强制（层次 3），引擎改动过大，另立 task。
- **skip-semantics 方案 B**（保留 isErr 强化 prompt）：本任务做方案 A（D2）。
- **换主 LLM 模型**：MiniMax-M3 是配置选择，不针对单一模型优化。

## Open Questions

（brainstorm + plan 已收敛，无遗留。D1 互斥 vs 叠加曾问过用户未答，按互斥判断执行——理由见 D1。）
