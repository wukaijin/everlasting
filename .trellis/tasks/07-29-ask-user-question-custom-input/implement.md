# Implement — ask_user_question custom input + skip-semantics

> 执行清单。需求见 `prd.md`，契约见 `design.md`。顺序按「先契约层、后消费层、最后测试」。

## 阶段 A：后端类型层（契约源头）

- [ ] **A1**. `app/src-tauri/src/agent/question_store.rs` `Question` 加 `#[serde(default)] pub allow_custom: bool`（放 `multi_select` 后）。
- [ ] **A2**. 同文件 `QuestionAnswer` 加 `#[serde(default, skip_serializing_if = "Option::is_none")] pub custom: Option<String>`（放 `multi_select` 后）。
- [ ] **A3**. 检查同文件 + `tools/ask_user_question.rs` 测试里所有 `Question { ... }` / `QuestionAnswer { ... }` struct literal：因新字段无 `Default` impl，literal 必须显式给 `allow_custom` / `custom`（或决定加 `..Default::default()`）。**决策**：先加字段看编译错，按编译错指引逐个补 —— 字段少，比维护 Default impl 更直接。
- **验证**：`cd app/src-tauri && cargo build --lib`（编译过即可，字段补齐）。

## 阶段 B：后端工具层（schema + skip-semantics）

- [ ] **B1**. `app/src-tauri/src/tools/ask_user_question.rs` `definition()` 的 `input_schema`：question 对象加 `allow_custom`（见 design §1.1）。
- [ ] **B2**. `validate()`：`allow_custom` 无新边界，确认不需要加 arm。
- [ ] **B3**. **skip-semantics**：改 `Cancelled` arm（~line 416-421）→ content 加 `reason`/`hint`，`is_error` true→**false**。
- [ ] **B4**. 同步改 doc bullets（~line 272/276-277/279-280）反映新 content shape + is_error=false（session-cancel 两 bullet 保持 true）。
- [ ] **B5**. 改 `description()`（~line 118-119）：从 "returns is_error: true" 改为描述「跳过返回 `{"cancelled":true,"reason":"user_skipped"}` 非 error，继续即可」。
- **验证**：`cd app/src-tauri && cargo build --lib`。

## 阶段 C：后端测试

- [ ] **C1**. `ask_user_question.rs`：改 `cancelled_path_returns_user_cancel_marker`（~line 717）断言 → `assert!(!is_error, ...)` + content 含 `"reason":"user_skipped"` 和 `"hint"`。
- [ ] **C2**. 加 `allow_custom=true` 的 Question 通过 `validate()` 单测。
- [ ] **C3**. 加 `QuestionAnswer` 带 `custom` 的 round-trip serde 测试（serialize→deserialize 字段保留 + 不带 custom 的旧 answer → None）。
- [ ] **C4**. 确认 **不动** schema-validation 测试（507/547/573/601）和 session-cancel 测试（709）。
- **验证**：`cd app/src-tauri && cargo test --lib`（全绿）+ `cargo clippy --lib --tests`（零新 warning）。

## 阶段 D：前端类型层

- [ ] **D1**. `app/src/stores/questionCards.types.ts` `Question` 加 `allow_custom?: boolean`。
- [ ] **D2**. 同文件 `ToolQuestionAnswer` 加 `custom?: string`。
- [ ] **D3**. 确认 snake_case（文件头警告）。
- **验证**：`cd app && pnpm vue-tsc --noEmit`（类型过）。

## 阶段 E：卡片交互（核心前端）

- [ ] **E1**. `AskUserQuestionCard.vue` 加 `customByQuestion = ref<string[]>(...)`。
- [ ] **E2**. `toggleSelection`：选选项时清 `customByQuestion[qIndex] = ""`（互斥）。
- [ ] **E3**. 新增 `onCustomInput(qIndex, value)`：清 `selectedByQuestion[qIndex]` + 写 custom（互斥）。
- [ ] **E4**. `allAnswered` 放宽（见 design §2.3）。
- [ ] **E5**. `buildAnswer` 互斥 wire（见 design §2.4）。
- [ ] **E6**. 模板：`allow_custom` 为真时渲染 custom input（testid + disabled 随 localState）。
- [ ] **E7**. summary 区：custom 非空时渲染「自定义: …」pill（见 design §2.6）。
- [ ] **E8**. CSS：`.ask-card__custom*` 复用现有 border/radius token，无新色 token。
- **验证**：`cd app && pnpm vue-tsc --noEmit`。

## 阶段 F：历史回放

- [ ] **F1**. `MessageItem.vue` `synthQuestions`（~line 274）：custom 非空时合成 label（见 design §3.2）。
- [ ] **F2**. 确认 `parseAnswerEnvelope` validator 无需改（`[]` 仍是合法 array）。
- **验证**：`cd app && pnpm vue-tsc --noEmit`。

## 阶段 G：前端测试

- [ ] **G1**. `AskUserQuestionCard.test.ts` 确认 load-bearing toEqual（~368-388）无需改（现有用例不触发 custom，条件 attach）。
- [ ] **G2**. 新增 custom 测试（见 design §5.2 六项）。
- [ ] **G3**. `MessageItem.test.ts` 加 custom 回放用例（design §5.3）。
- **验证**：`cd app && pnpm test`（全绿）+ `pnpm vue-tsc --noEmit` + `pnpm build`。

## 阶段 H：全量回归 + 收尾

- [ ] **H1**. `cd app/src-tauri && cargo test --lib && cargo clippy --lib --tests`。
- [ ] **H2**. `cd app && pnpm test && pnpm vue-tsc --noEmit && pnpm build`。
- [ ] **H3**. 手测（可选，GUI）：`pnpm tauri dev`，构造一个 `allow_custom:true` 的 ask_user_question，验证输入框渲染 + 互斥 + 提交 + summary。
- [ ] **H4**. `trellis-check`（Agent 形式）验证 spec 合规 + 跨层一致性。
- [ ] **H5**. `trellis-update-spec`：把「ask_user_question 自由输入互斥语义 + skip-semantics is_error 区分」沉淀进 spec（如有合适层）。
- [ ] **H6**. commit（Phase 3.4）+ 归档。

## Review Gates

- 阶段 A→B 之间：类型编译过 = 契约立住，再改消费层。
- 阶段 C 后：后端全绿才动前端。
- 阶段 G 后：前端全绿才进 H 全量回归。

## Validation Commands 速查

```bash
# 后端
cd app/src-tauri && cargo build --lib
cd app/src-tauri && cargo test --lib
cd app/src-tauri && cargo clippy --lib --tests

# 前端
cd app && pnpm vue-tsc --noEmit
cd app && pnpm test
cd app && pnpm build
```
