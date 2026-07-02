# Implement — B9-A use_ui 基础设施

## 执行清单（有序）

1. **后端 `tools/use_ui.rs`**
   - `definition()`（ToolDef + input_schema，见 design §1.1）
   - `execute()`（non-blocking，校验 primitives 非空 + type ∈ {diff, code_block}，返回"已渲染 N 个 primitive"）
   - `#[cfg(test)]` 单测：合法 input / 空 primitives / 未知 type / 超 maxItems
2. **后端注册（`tools/mod.rs`）**
   - `pub mod use_ui;`
   - `builtin_tools()` 加 `use_ui::definition()`（带注释：non-blocking 展示型，走 execute_tool match）
   - `execute_tool_inner` match 加 `"use_ui"` 分支（仿 remember）
3. **前端 types + registry**
   - `components/chat/uiCard.types.ts`：`USE_UI_TOOL_NAME = "use_ui"` + `UiPrimitive` interface（`{ type: string; title?: string; [k: string]: unknown }`）
   - `components/chat/uiPrimitiveRegistry.ts`：registry Map（MVP mock 占位）
4. **前端 `<MockPrimitive>` + `<UiCard>`**
   - `components/chat/primitives/MockPrimitive.vue`：渲染 `primitive.type` + JSON dump
   - `components/chat/UiCard.vue`：遍历 `call.input.primitives` + registry dispatch + 未知 type 降级
5. **MessageItem dispatch（`MessageItem.vue`）**
   - import `USE_UI_TOOL_NAME` + `UiCard`
   - 加 `tc.name === USE_UI_TOOL_NAME` 分支（sibling `<UiCard>`，仿 :702 + :333 的 ask_user_question 挂法）
6. **测试**
   - 后端：`cargo test`（use_ui 单测 + execute_tool `"use_ui"` dispatch 集成）
   - 前端：`UiCard.test.ts`（registry dispatch + 未知 type 降级）+ MessageItem dispatch 测试（use_ui 分支路由 UiCard）

## 验证命令

```bash
# 后端
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test
# 前端
cd app && pnpm vitest run
cd app && pnpm exec vue-tsc --noEmit
# 端到端(mock 验证)
cd app && pnpm tauri dev   # 让 LLM 调 use_ui(可用临时 prompt 引导),确认 UiCard 渲染 mock
```

## 风险点 / 回滚

- **风险**：`execute_tool_inner` match 加分支位置/格式不对 → unknown-tool fallback。→ 单测覆盖 `"use_ui"` dispatch 命中。
- **风险**：MessageItem dispatch 分支误伤其他 tool。→ 仅 `===` 精确匹配 + dispatch 测试。
- **风险**：`call.input.primitives` 字段名前后端不一致（snake_case 约定，见 BACKLOG §5.2）。→ 前端按 snake_case 读 `primitives`（与 Rust struct 一致）。
- **回滚**：纯新增（use_ui.rs / UiCard.vue / registry / MockPrimitive），现有文件仅 2 处增量注册（mod.rs + MessageItem.vue），回滚 = 删新增 + 撤两处注册。

## Review gates

1. 后端 `cargo test` 绿 → 进前端
2. 前端 vitest + `vue-tsc --noEmit` 0 err → 进端到端 mock 验证
3. 端到端（LLM 调 use_ui → UiCard 渲染 mock primitive）→ **Child A done，解锁 B/C**
