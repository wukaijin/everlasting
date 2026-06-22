# 拆分 chat.ts — 类型抽离到 chat.types.ts

## Goal

`stores/chat.ts` 当前 1640 行,包含 ~245 行类型/接口定义 + ~1395 行 store 主体 + 少量 helper。把纯类型/接口声明抽到独立文件 `stores/chat.types.ts`,store 主体继续留在 `chat.ts`,实现"类型层"与"实现层"的物理隔离,降低单文件阅读负担且不影响任何对外 API 契约。

## What I already know

* `chat.ts` 当前结构(`grep -nE "^(export )?(type|interface|function|const|enum) "` 实测):
  - 行 40:`type Role = "user" | "assistant"` —— 未导出(内部用)
  - 行 41–301:14 个 `export type/interface` + 1 个 `export const MODE_CYCLE`
  - 行 327:`type ContentBlockPayload = ...` —— 未导出(仅 `send` 内部用)
  - 行 344:`interface ChatMessagePayload` —— 未导出(仅 `send` 内部用)
  - 行 349:`const genId` —— 内部 helper
  - 行 355:`export function thinkingBlocksToText(...)` —— 导出的 helper 函数
  - 行 360–1640:`export const useChatStore = defineStore("chat", () => {...})` —— Pinia store
* **外部消费者清单**(grep `import.*from.*chat['"]`):27 个文件 import,核心导入形式:
  - `useChatStore`(主)
  - `type SessionMode` / `type SessionSummary` / `type ToolCallInfo`
  - `MODE_CYCLE`(常量)
  - `useChatStore, MODE_CYCLE, type SessionMode` 等多导入组合
* **命名约定**:代码库没有 `*.types.ts` 前例(全 `stores/` 目录 18 个文件,无一例外是平铺命名)。引入 `chat.types.ts` 是新约定,但属于"一次性领域约定",无明显代价。
* **改动风险**:
  - 跨文件 import 路径调整:27 个文件需要从 `import { ... } from "./chat"` → `import { ... } from "./chat.types"`
  - `chat.ts` 内部需要 `import { ... } from "./chat.types"` 来引用类型
  - vitest / vue-tsc 应能自动识别,无额外配置改动

## Requirements

### 必含(明确要抽)

* 14 个 `export type` / `export interface` 声明(行 41–295)
* `export const MODE_CYCLE`(行 301)—— 虽非类型,但与 `SessionMode` 强绑定,语义上属于类型模块
* 4 个未导出类型(`Role` / `ContentBlockPayload` / `ChatMessagePayload` / `genId`)—— **待用户确认边界**

### 不变

* `useChatStore` 主 store 留 `chat.ts`
* `thinkingBlocksToText` helper 留 `chat.ts`(纯函数 + 无类型依赖,放 store 文件合理)—— **待用户确认边界**

## Acceptance Criteria

* [ ] 新文件 `stores/chat.types.ts` 存在,只包含类型/接口/类型相关常量
* [ ] `stores/chat.ts` 顶部新增 `import { ... } from "./chat.types"` 引入需要的类型
* [ ] 所有 27 个外部 import 文件的 import 路径正确更新(可通过 `vue-tsc --noEmit` + `vitest run` 验证)
* [ ] `vue-tsc --noEmit` 全绿
* [ ] `vitest run` 全绿(尤其 `stores/chatMode.test.ts` + `stores/streamController.test.ts` + 9 个 chat 组件测试)
* [ ] `pnpm build` 全绿
* [ ] `chat.ts` 行数减少 ~250 行,降到 ~1390 行(离 1000 行目标仍有距离,但本次只动类型)

## Definition of Done

* 类型物理隔离完成,无运行时行为变化
* 类型测试 / 类型检查全绿
* 没有引入循环 import(若 store.ts 反向 import types.ts 出现循环,需重新考虑边界)
* commit message: `refactor(stores): extract chat.ts types to chat.types.ts`

## Technical Approach

* **零行为变更**:纯文件物理拆分 + import 路径调整
* **不改 API**:所有对外 export 名称保持不变
* **TypeScript import 类型擦除**:类型/接口在编译时消失,运行时无影响
* **不需要 `verbatimModuleSyntax`**:`tsconfig.json` 当前已支持 `import { type X }` 语法分离(项目已用),无需配置调整

## Out of Scope

* 改任何 store 行为 / 状态 / 方法
* 拆分 `thinkingBlocksToText` 或其他函数
* 拆分 `streamController.ts` 或其他 store(那是 #1 #5 等其他任务)
* 引入 `*.types.ts` 命名约定到其他 store(本次只对 chat)

## Decision (ADR-lite)

**Context**:`chat.ts` 1640 行,顶部 14 个 `export type/interface` + `MODE_CYCLE` 常量(~245 行)混在 store 文件里。需要决定物理拆分时哪些"看起来像类型"的东西一起搬。

**Decision**:**Option A — 只搬导出的**(2026-06-23 用户确认)
- ✅ 搬到 `chat.types.ts`:14 个 `export type/interface` + `export const MODE_CYCLE`
- ❌ 不搬:`type Role` (行 40)、`type ContentBlockPayload` (行 327)、`interface ChatMessagePayload` (行 344)、`const genId` (行 349)、`export function thinkingBlocksToText` (行 355)
- ❌ 不动:`useChatStore`(留 chat.ts)
- 理由:内部类型是 `chat.ts` 私有实现细节,放 types 模块会让"公开契约 vs 私有细节"的边界模糊;`MODE_CYCLE` 因为是 `SessionMode` 的具象化数组,与类型强绑定所以一起搬;`thinkingBlocksToText` 是运行时函数不是类型,且无外部 consumer(只 chat.ts 内部用)。

**Consequences**:
- ✅ 改动最小,验证最快;types 文件语义最纯粹("对外契约")
- ✅ `chat.ts` 仍保留 ~20 行内部类型 + ~30 行 helper + ~1340 行 store
- ⚠️ 距"1000 行目标"仍有 ~340 行差距 —— 但本次只动 P3 边界,后续可拆 helpers / store body(那是 #1 #5 #7 等其他任务)

## Open Questions

*(已全部解决)*

## Technical Notes

* 当前 store 文件:`app/src/stores/chat.ts`(1640 行)
* 外部消费者:27 个文件(11 个 .vue + 16 个 .ts),主要分布在 `components/chat/`、`components/layout/`、`stores/`、`utils/`、`components/audit/`
* 类型检查命令:`pnpm --filter app exec vue-tsc --noEmit`(项目内运行)
* 测试命令:`pnpm --filter app exec vitest run`(项目内运行)
* Trellis 优先级:P3(轻量拆分的预热任务)

## Research References

* [`.trellis/spec/frontend/state-management.md`](../../spec/frontend/state-management.md) — Pinia store facade 模式 + chat.ts/streamController.ts 既有拆分先例
* [`.trellis/spec/frontend/chat.md`](../../spec/frontend/chat.md) — chat store 公开 API 契约(14 个类型 + MODE_CYCLE)
* [`.trellis/spec/frontend/index.md`](../../spec/frontend/index.md) — frontend spec 索引
* [`.trellis/spec/guides/cross-layer-thinking-guide.md`](../../spec/guides/cross-layer-thinking-guide.md) — 跨层数据流验证清单