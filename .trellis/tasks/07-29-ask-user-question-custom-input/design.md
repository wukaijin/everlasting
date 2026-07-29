# Design — ask_user_question custom input + skip-semantics

> 技术设计。需求见 `prd.md`。本文聚焦跨层契约 + 数据流 + 互斥状态机 + 边界。

## 1. 数据契约（wire shape，全 snake_case）

### 1.1 LLM → 后端（Question 输入，新增 `allow_custom`）

`definition()` 的 `input_schema`，question 对象加：
```json
"allow_custom": {
  "type": "boolean",
  "default": false,
  "description": "If true, render a free-text input so the user can type their own answer instead of picking an option. Selecting an option and typing are mutually exclusive; when the user types, options becomes empty and custom carries the text."
}
```
放 `multi_select` 之后，与它语义对称（都是「这道题怎么答」的开关）。

### 1.2 后端 `Question` 结构（`question_store.rs`）

```rust
pub struct Question {
    pub question: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub header: Option<String>,
    pub options: Vec<QuestionOption>,
    #[serde(default)]
    pub multi_select: bool,
    #[serde(default)]                      // ← 新增，与 multi_select 同形
    pub allow_custom: bool,
}
```
缺字段 → default false。无需 `skip_serializing_if`（bool 的小开销可接受，且 `multi_select` 也是全 emit，保持对称）。

### 1.3 后端 `QuestionAnswer` 结构（`question_store.rs`）

```rust
pub struct QuestionAnswer {
    pub question: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub header: Option<String>,
    pub options: Vec<String>,
    pub multi_select: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]   // ← 新增，与 header 同形
    pub custom: Option<String>,
}
```
**互斥不变量（D1）**：`custom.is_some()` ⇔ `options.is_empty()`。前端 `buildAnswer` 保证；后端不做强制校验（信任前端，与现有「backend trusts the shape」惯例一致 —— `question_store.rs:144` doc）。

### 1.4 用户 → 后端（answer 提交，`custom` 透传）

`commands/question.rs:resolve_response_from_args` 接 `Option<Vec<QuestionAnswer>>`，serde 自动反序列化新 optional `custom` 字段。**零改动**。

### 1.5 后端 → LLM（answer 回传，`Cancelled` 语义改）

`execute_blocking` 两 arm（skip-semantics，D2）：

| arm | 现在 | 改后 |
|---|---|---|
| `Answered`（line 391） | stringify raw value, `is_error=false` | **不变**（custom 字段自动随 JSON 流） |
| `Cancelled`（line 416） | `{"cancelled":true}`, `is_error=**true**` | `{"cancelled":true,"reason":"user_skipped","hint":"..."}`, `is_error=**false**` |
| `cancel.cancelled()`（line 374） | `{"cancelled_by_session":true}`, `is_error=true` | **不变** |
| `RecvError`（line 422） | `{"cancelled_by_session":true}`, `is_error=true` | **不变** |

**互斥区分**：模型按 content 区分「用户跳过」（`reason:user_skipped`，可继续）vs「session 中断」（`cancelled_by_session`，真停止）。这正是 C4 方案 A 的核心。

### 1.6 前端类型（`stores/questionCards.types.ts`）

```ts
export interface Question {
  question: string;
  header?: string;
  options: QuestionOption[];
  multi_select: boolean;
  allow_custom?: boolean;        // ← 新增，镜像 Rust default false
}

export interface ToolQuestionAnswer {
  question: string;
  header?: string;
  options: string[];
  multi_select: boolean;
  custom?: string;               // ← 新增
}
```
snake_case（文件头 line 23-45 明确警告不要 camelCase）。

## 2. 卡片交互状态机（`AskUserQuestionCard.vue`）

### 2.1 本地状态（新增对齐 `selectedByQuestion`）

```ts
const customByQuestion = ref<string[]>(props.questions.map(() => ""));
```
（用 `string[]` 而非 `Set`，文本天然单值。）

### 2.2 互斥规则（D1，一条）

- `toggleSelection(qIndex, label)`（现有）：选选项时 **清 `customByQuestion[qIndex] = ""`**。
- 新增 `onCustomInput(qIndex, value)`：打字时 **清 `selectedByQuestion[qIndex] = new Set()`**，写 `customByQuestion[qIndex] = value`。

无论单选/多选，规则相同：动了选项就清文本，动了文本就清选项。

### 2.3 提交门 `allAnswered`（放宽）

```ts
const allAnswered = computed(() =>
  props.questions.every((q, i) => {
    const sel = selectedByQuestion.value[i];
    const custom = customByQuestion.value[i]?.trim() ?? "";
    const hasOption = sel && (q.multi_select ? sel.size >= 1 : sel.size === 1);
    const hasCustom = !!q.allow_custom && custom.length > 0;
    return hasOption || hasCustom;
  }),
);
```
- 不允许同时满足（互斥已保证 hasOption 时 hasCustom 不可能，反之亦然）。
- `allow_custom=false` 的题退化为原逻辑（hasCustom 恒 false）。

### 2.4 `buildAnswer`（互斥 wire）

```ts
function buildAnswer(): ToolQuestionAnswer[] {
  return props.questions.map((q, i) => {
    const set = selectedByQuestion.value[i] ?? new Set<string>();
    const custom = customByQuestion.value[i]?.trim() ?? "";
    const useCustom = !!q.allow_custom && custom.length > 0;
    const answer: ToolQuestionAnswer = {
      question: q.question,
      options: useCustom
        ? []
        : q.options.map((o) => o.label).filter((l) => set.has(l)),
      multi_select: !!q.multi_select,
    };
    if (q.header !== undefined) answer.header = q.header;
    if (useCustom) answer.custom = custom;   // 镜像 header 的条件 attach
    return answer;
  });
}
```
注意：`options` 始终存在（类型 required），用 custom 时为 `[]`。`custom` 条件 attach（未用时 omit，与 `header` 一致）。

### 2.5 模板（新增输入行）

在每个 question 的 `<ul class="ask-card__options">` 后、`allow_custom` 为真时：
```html
<div v-if="q.allow_custom" class="ask-card__custom" :data-testid="`ask-card-custom-${qIndex}`">
  <input
    type="text"
    class="ask-card__custom-input"
    :value="customByQuestion[qIndex]"
    :disabled="localState !== 'pending'"
    :placeholder="`或自行输入…`"
    :data-testid="`ask-card-custom-input-${qIndex}`"
    @input="onCustomInput(qIndex, ($event.target as HTMLInputElement).value)"
  />
</div>
```
- disabled 随 `localState`（与 option `--disabled` 同步）。
- 复用现有 border/radius token（`.ask-card__option` 风格），不引入新色 token。

### 2.6 summary 渲染（answered 状态）

`labelsForAnswer` / summary 区：custom 非空时额外渲染 pill：
```html
<span class="ask-card__summary-label ask-card__summary-label--custom">自定义: {{ answer.custom }}</span>
```
`answeredSummary` 已透传 `ToolQuestionAnswer[]`，custom 自动带在 answer 上，无需额外处理。

## 3. 历史回放（`MessageItem.vue`）

### 3.1 `parseAnswerEnvelope` validator（放宽）

现 validator（line 186-193）要求 `Array.isArray(a.options)`。custom 场景 `options=[]` 仍是 array，**过 validator**。但为稳妥，把校验改为 `options` 可选 + 至少有 question + multi_select：
```ts
typeof a.question === "string" &&
Array.isArray(a.options) &&            // custom 场景是 []，仍过
typeof a.multi_select === "boolean"
```
**实际无需改**（`[]` 是合法 array）—— 留意即可。

### 3.2 `synthQuestions`（custom 回放）

answered 回放时，原逻辑把 `a.options.map(label => ({label}))` 当合成 options 渲染。custom 场景 `options=[]`，需把 custom 文本合成进去，否则 summary 空：
```ts
options: [
  ...a.options.map((label) => ({ label })),
  ...(a.custom ? [{ label: `自定义: ${a.custom}` }] : []),
],
```
保持「回放 summary 与提交时一致」。

## 4. 向后兼容

- **旧 task.json**：缺 `allow_custom` → serde default false（R1）；缺 `custom` → Option None（R1）。
- **旧 tool_result（历史消息）**：无 `custom` 字段的 answer → `parseAnswerEnvelope` permissive，透传；`synthQuestions` `a.custom` 为 undefined → 不合成额外 label。
- **老前端缓存**：questionCards store 不缓存 answer（确认无 `selectedAnswer` 字段），custom 无需持久化处理。

## 5. 测试设计

### 5.1 后端（`ask_user_question.rs` + `question_store.rs`）

- `allow_custom=true` 的 Question 通过 `validate()`（无新边界）。
- `QuestionAnswer` 带 `custom` round-trip：serialize → deserialize 字段保留；不带 custom 的旧 answer 反序列化 custom=None。
- skip-semantics：`cancelled_path_returns_user_cancel_marker` 改 `assert!(!is_error)` + content 含 `reason`/`hint`。session-cancel 测试保持 `is_error=true`。
- **不动** schema-validation 测试（507/547/573/601）—— PRD §纠正已说明。

### 5.2 前端（`AskUserQuestionCard.test.ts`）

- **修 load-bearing toEqual**（~line 368-388）：现有用例（选 Vue + Routing/State）不触发 custom，buildAnswer 不 attach custom → wire 无 custom 字段。toEqual 保持通过（条件 attach 的好处）。
- 新增：
  1. `allow_custom=false`（默认）不渲染 custom input。
  2. `allow_custom=true` 渲染 custom input（testid 存在）。
  3. 打字填 custom → 清选项；选项点击 → 清 custom（互斥）。
  4. 提交门：custom 非空（无选项）时 submit enabled。
  5. 提交 wire：custom 用例 → `options=[]` + `custom="<文本>"`。
  6. answered summary：custom 非空时渲染「自定义: …」pill。

### 5.3 前端（`MessageItem.test.ts`）

- 加 1 个 answer-envelope 带 custom 的回放用例：parse 正常 + synthQuestions 含 custom label。

## 6. 风险与回滚

- **风险**：互斥逻辑在多选 + allow_custom 组合下，用户先勾多项再打字，会清空所有勾选。这是 D1 互斥的既定行为（预期），非 bug。
- **风险**：skip-semantics 改 is_error=false 后，若有下游代码假设 cancelled==error。已查：消费点仅 `execute_blocking` 透传 content，C2+（chat_loop.rs:2455）只看 `Answered`，不读 cancelled arm 的 is_error。**无下游假设**。
- **回滚**：本任务纯加字段 + 一处 arm 改值，git revert 单 commit 即可全量回退。
