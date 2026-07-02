# Design — B9-A use_ui 基础设施

> 技术 design。需求见 `prd.md`，父决策见 parent `prd.md` D1-D6。

## 1. 后端 use_ui tool

### 1.1 input_schema（discriminated union by `type`）

```jsonc
{
  "type": "object",
  "properties": {
    "primitives": {
      "type": "array",
      "minItems": 1,
      "maxItems": 8,
      "items": {
        "type": "object",
        "properties": {
          "type": { "enum": ["diff", "code_block"] },
          "title": { "type": "string", "description": "可选卡片标题" }
        },
        "required": ["type"],
        "additionalProperties": true   // type-specific 字段(diff_text / code / language)由 Child B/C 定义,本 child 放行
      }
    }
  },
  "required": ["primitives"]
}
```

- `maxItems: 8` 防滥用（一次渲染过多卡片）。
- 本 child 只校验 `type`，**不**校验 type-specific 字段（B/C 各自加）。

### 1.2 execute（non-blocking，仿 `remember::execute`）

```rustc
pub async fn execute(input: serde_json::Value, _ctx: &ToolContext, _session_id: &str) -> (String, bool) {
    // 解析 primitives 数组,校验非空 + 每个 type ∈ {diff, code_block}
    // OK  → ("已渲染 N 个 primitive", false)
    // 非法 → (中文错误, true)
}
```

- **不**走 `execute_blocking`（非 ask_user_question 的 blocking oneshot）。
- **不**注册 Tier 4 权限 ask：展示型无副作用，Tier 5 silent Allow（同 `remember`，走 `_` 默认分支 + `Risk::Low`）。

### 1.3 注册（`tools/mod.rs`）

- `pub mod use_ui;`
- `builtin_tools()` 末尾加 `use_ui::definition()`（带注释：non-blocking 展示型，仿 remember，走 execute_tool match 非 blocking 拦截）。
- `execute_tool_inner` match 加：
  ```rustc
  "use_ui" => {
      let (out, is_err) = use_ui::execute(input, ctx, session_id).await;
      (out, is_err, ToolContextUpdate::default(), None)
  }
  ```

## 2. 前端 component registry

### 2.1 registry（`components/chat/uiPrimitiveRegistry.ts`）

```ts
import type { Component } from "vue";
import MockPrimitive from "./primitives/MockPrimitive.vue";

// MVP: mock 占位。Child B/C 各自把条目换成真实组件。
export const UI_PRIMITIVE_REGISTRY: Record<string, Component> = {
  diff: MockPrimitive,
  code_block: MockPrimitive,
};
```

### 2.2 `<UiCard>` 容器（`components/chat/UiCard.vue`）

- props: `call: ToolCallInfo`（读 `call.input.primitives`）。
- 遍历 primitives → `<component :is="UI_PRIMITIVE_REGISTRY[primitive.type]" :primitive="primitive" />`。
- 未知 type → 降级渲染（"未知 primitive 类型: X"，不崩）。

### 2.3 MessageItem dispatch（仿 ask_user_question 对称结构）

- 新增 `stores`/types 常量 `USE_UI_TOOL_NAME = "use_ui"`（放 `components/chat/uiCard.types.ts` 或就近，对齐 `ASK_USER_QUESTION_TOOL_NAME` 模式）。
- MessageItem template：`<ToolCallCard>` 之后加 sibling `<UiCard v-if="tc.name === USE_UI_TOOL_NAME" :call="tc" />`（与 AskUserQuestionCard 的 sibling 挂法一致，见 MessageItem.vue:702 + :333 dispatch 判断）。

## 3. 关键设计决策

| 决策 | 选择 | 理由 | 否决的备选 |
|---|---|---|---|
| 卡片形态 | ToolCallCard + sibling UiCard（仿 ask_user_question） | 一致性 + input 折叠可见(debug) + primitives 在 sibling 渲染,数据源清晰(读 `call.input`) | ToolCallCard output 区内嵌：primitives 读 input 而非 result，output 语义错位 |
| registry 实现 | `Record<type, Component>` Map | 加新 type 只改 Map | v-if/v-else 链：type 多了臃肿 |
| 数据源 | `call.input.primitives`（tool_use 输入） | primitives 是 LLM 输出的展示数据，在 input 里；non-blocking 无需独立 IPC 事件 | 独立 `ui:render` 事件：过度设计（ask_user_question 才需，因 blocking） |
| 权限 | Tier 5 silent Allow | 展示型无副作用，同 remember | Tier 4 ask：无必要 |
| 持久化 | 复用 persist_turn（tool_result 落库） | 不引入新 DB 表 / 通道 | 独立 UI 事件表：YAGNI |

## 4. 兼容性 / 回归

- 不改现有 `ContentBlock` / Provider wire（D1）。
- 不改 ask_user_question 链路（selector 复用，零改动）。
- MessageItem dispatch 仅**新增** `tool_name === use_ui` 分支，不影响其他 tool。
- ToolCallCard 不改（use_ui 复用现有框架，input 折叠默认）。

## 5. Mock primitive 验证策略

Child A 不实现真实 primitive，用 `<MockPrimitive>` 渲染 `primitive.type` + JSON dump，端到端验证：
- LLM 调 use_ui → 前端收到 tool:call → UiCard 渲染 → mock 显示 type。
- Child B/C 各自把 registry 条目替换为真实组件（MockPrimitive 保留为 fallback / 测试用）。
