# 拆分 subagentRuns.ts — 抽 types + RunAccumulator

## Goal

`stores/subagentRuns.ts` 当前 1416 行,包含 3 个关注点:(1) ~324 行类型/接口/常量声明,(2) RunAccumulator 类 + 其内部实现(buildSectionsFromRaw / chat_event 解析族 / parseTranscriptJson)~497 行,(3) Pinia store 主体 ~447 行 + coerceStatus helper。把它们物理拆成 3 个文件,实现"类型层 / 累积器层 / store 层"隔离,降低单文件阅读负担,零运行时行为变更。

## What I already know

* `subagentRuns.ts` 当前结构(`grep -nE "^(export )?(type|interface|class|function|const|enum) "` 实测):

  | 行号 | 符号 | 归属(本次决策) |
  |---|---|---|
  | 69–103 | `SubagentStatus` / `TranscriptKind` | → types |
  | 104–223 | `SubagentRunSummary` / `SubagentRunRow` / `SubagentEventPayload` / `SubagentFinishedPayload` / `TranscriptEntry` | → types |
  | 224 | `export const SUBAGENT_EVENT_DEBOUNCE_MS` | → types(常量,与事件契约强绑定) |
  | 241–368 | `TranscriptSectionKind` + 6 个 Section interface + `PermissionAskOutcome` + `TranscriptSection` union | → types |
  | 370–392 | `ChatEventInnerKind` | → types |
  | 393–404 | `export function coerceStatus` | **留 store**(独立,只依赖 SubagentStatus type) |
  | 410–447 | `export function parseTranscriptJson` | **→ runAccumulator**(见 ADR:打破循环依赖) |
  | 481–499 | `function chatEventInnerKind`(未导出) | → runAccumulator(RunAccumulator 内部) |
  | 506–509 | `function chatEventText`(未导出) | → runAccumulator(RunAccumulator 内部) |
  | 518–523 | `export function chatEventSignature` | → runAccumulator(chat_event 解析族,本文件未用但与兄弟函数同族) |
  | 552–869 | `export class RunAccumulator` | → runAccumulator |
  | 871–964 | `function buildSectionsFromRaw`(未导出) | → runAccumulator(RunAccumulator.rebuildFromCache 的辅助) |
  | 970–1416 | `export const useSubagentRunsStore` | **留 store** |

* **依赖图(决定拆分边界的核心约束)**:
  - `RunAccumulator.rebuildFromCache`(行 798)调用 `parseTranscriptJson`(行 410)
  - `RunAccumulator` 内部调用 `chatEventInnerKind`(690) / `chatEventText`(694, 745)
  - `buildSectionsFromRaw`(871)调用 `chatEventInnerKind`(879) / `chatEventText`(883, 920) → 它是 RunAccumulator 的私有辅助
  - `chatEventInnerKind` / `chatEventText` / `buildSectionsFromRaw` **全部只被 RunAccumulator 家族调用**,无外部消费者
  - `coerceStatus` 在 store 内**无真实调用**(grep 仅命中注释),纯 export 给 SubagentDrawer + 测试
  - **若 parseTranscriptJson 留 store**:store → runAccumulator(import RunAccumulator)+ runAccumulator → store(import parseTranscriptJson)= **ES module 循环依赖**。故 parseTranscriptJson 必须跟 RunAccumulator 走。
  - 最终依赖方向(单向无循环):`subagentRuns.ts`(store) → `runAccumulator.ts` → `subagentRuns.types.ts`

* **外部消费者清单**(grep `from ".*stores/subagentRuns"`):

  | 文件 | 导入符号 | 改动 |
  |---|---|---|
  | `ChatWindow.vue` | `useSubagentRunsStore` | 不变 |
  | `WorkerAskBanner.vue` / `.test.ts` | `useSubagentRunsStore` | 不变 |
  | `ToolCallCard.vue` | `useSubagentRunsStore`, `type SubagentRunSummary` | type → `.types` |
  | `ToolCallCard.test.ts` | `useSubagentRunsStore` | 不变 |
  | `DrawerThinkingBlock.vue` / `.test.ts` | `type ThinkingSection` | → `.types` |
  | `WorkerTextTimeline.vue` | `type TranscriptEntry` | → `.types` |
  | `transcriptPairing.ts` | `type TranscriptEntry, TranscriptSection` + 行 38 `export type { TranscriptEntry } from "../stores/subagentRuns"` re-export | → `.types`(re-export 源也改) |
  | `transcriptPairing.test.ts` | `type TranscriptSection` | → `.types` |
  | **`SubagentDrawer.vue`** | `useSubagentRunsStore, coerceStatus, parseTranscriptJson, type SubagentStatus, type TranscriptSection, type ThinkingSection` | **拆 3 源**:store+coerceStatus ← subagentRuns;`parseTranscriptJson` ← runAccumulator;3 type ← .types |
  | **`SubagentDrawer.test.ts`** | `useSubagentRunsStore, type SubagentRunRow, type TranscriptSection` | store ← subagentRuns;2 type ← .types |
  | **`subagentRuns.test.ts`** | `useSubagentRunsStore, coerceStatus, parseTranscriptJson, SUBAGENT_EVENT_DEBOUNCE_MS, RunAccumulator` + 5 type | **拆 3 源**:store+coerceStatus ← subagentRuns;`RunAccumulator`+`parseTranscriptJson` ← runAccumulator;`SUBAGENT_EVENT_DEBOUNCE_MS` + 5 type ← .types |

* **先例(锁定惯例)**:`stores/chat.types.ts`(split-chat-types 任务,2026-06-23 落地)已确立 types 抽离惯例:
  - types 文件纯类型 + 与类型强绑定的 const(如 `MODE_CYCLE` → 对应本次 `SUBAGENT_EVENT_DEBOUNCE_MS`)
  - **外部消费者 import 路径全改成 `stores/xxx.types`**(不用 `export *` re-export 保 public API)—— 14 处 chat 消费者已全部改路径
  - store.ts 顶部 `import { ... } from "./xxx.types"` 引入自用类型
  - 本次 subagentRuns 拆分**严格遵循该先例**

## Requirements

### 必含(明确要抽)

**→ `subagentRuns.types.ts`**(新文件,~354 行):
* 行 69–392 全部 `export type/interface` + `export const SUBAGENT_EVENT_DEBOUNCE_MS`(224)
* 无运行时依赖(纯 TS 声明),可独立编译

**→ `runAccumulator.ts`**(新文件,~537 行):
* `parseTranscriptJson`(410–447)
* `chatEventInnerKind`(481–499,未导出,改不导出保持不变)
* `chatEventText`(506–509,未导出)
* `chatEventSignature`(518–523)
* `RunAccumulator` class(552–869)
* `buildSectionsFromRaw`(871–964,未导出)
* 顶部 `import { ... } from "./subagentRuns.types"` + vue 的 `shallowRef` / `markRaw` / `ShallowRef`

### 留 `subagentRuns.ts`(~547 行)

* `coerceStatus`(393–404)
* `useSubagentRunsStore`(970–1416)
* 顶部 `import { ... } from "./subagentRuns.types"` + `import { RunAccumulator, parseTranscriptJson } from "./runAccumulator"`(store 行 1104 用 parseTranscriptJson)

### 不变

* 所有对外 export 名称保持不变(仅 import 路径变)
* 零运行时行为变更

## Acceptance Criteria

* [ ] 新文件 `stores/subagentRuns.types.ts` 存在,只含类型/接口/类型相关常量
* [ ] 新文件 `stores/runAccumulator.ts` 存在,含 RunAccumulator + 其全部内部实现 + parseTranscriptJson
* [ ] `stores/subagentRuns.ts` 顶部正确 import types 与 runAccumulator
* [ ] 11 个外部消费者 import 路径正确更新(type → `.types`;SubagentDrawer.vue/test + subagentRuns.test.ts 的运行时符号 → runAccumulator)
* [ ] `vue-tsc --noEmit` 全绿
* [ ] `vitest run` 全绿(尤其 `stores/subagentRuns.test.ts` + 5 个 subagent/chat 组件测试 + `utils/transcriptPairing.test.ts`)
* [ ] `pnpm build` 全绿
* [ ] `subagentRuns.ts` 行数降到 ~547 行(离 1000 行目标仍超,但本次已抽走 types + 累积器两个独立单元)
* [ ] 无循环 import(`runAccumulator.ts` 不反向 import `subagentRuns.ts`)

## Definition of Done

* 3 文件物理隔离完成,无运行时行为变化
* 类型检查 / vitest / build 全绿
* 无循环 import(依赖单向:store → runAccumulator → types)
* commit message: `refactor(stores): split subagentRuns.ts into types + runAccumulator`

## Technical Approach

* **零行为变更**:纯文件物理拆分 + import 路径调整
* **不改 API**:所有对外 export 名称保持不变
* **遵循 chat.types.ts 先例**:type 从 `.types` import;`SUBAGENT_EVENT_DEBOUNCE_MS` 常量归 types(与 MODE_CYCLE 同理)
* **打破循环的关键**:`parseTranscriptJson` 不留 store(否则 store ↔ runAccumulator 循环),跟着 RunAccumulator 走 runAccumulator.ts
* **不改 store 行为 / 状态 / 方法**

## Out of Scope

* 改 RunAccumulator / store 的任何逻辑
* 拆 store body 内部(fetchRun / feed / pairing 层等留待后续任务)
* 拆其他 store(chat-input / message-item / db-tests / subagent-drawer 是平行的其他任务)
* 把 `coerceStatus` 抽到独立 utils 文件(它留 store 即可,避免过度拆分)

## Decision (ADR-lite)

**Context**:`subagentRuns.ts` 1416 行,3 个关注点混在一起。task description 原说"留 store + parsers",但依赖分析表明 `parseTranscriptJson` 不能留 store。

**Decision**(2026-06-23,由代码依赖唯一确定):

1. **parseTranscriptJson → runAccumulator.ts**(修正 description 的"留 store")
   - ✅ 理由:`RunAccumulator.rebuildFromCache`(798)依赖它;若留 store 则 store ↔ runAccumulator 形成 ES module 循环依赖。跟着 RunAccumulator 走是唯一不引入循环的归属。
   - ⚠️ 副作用:SubagentDrawer.vue(行 62)原本从 store import 它,改后要从 runAccumulator import。可接受(chat 先例里多源 import 是常态)。

2. **coerceStatus → 留 subagentRuns.ts**
   - ✅ 理由:它在 store 内无真实调用(纯 export 给 Drawer 显示层),不依赖 RunAccumulator,留 store 不产生循环;且语义上是 store 的"显示层 wire→enum 辅助"。
   - ❌ 不去 runAccumulator:与 RunAccumulator 无关;不去独立 utils:避免过度拆分(本次只 3 文件)。

3. **chatEventSignature → runAccumulator.ts**(而非留 store)
   - ✅ 理由:与 `chatEventInnerKind` / `chatEventText` 同族(chat_event payload 字段提取),放一起语义最连贯;本文件未用它,但兄弟函数都在 runAccumulator。

4. **import 路径惯例 → 遵循 chat.types.ts 先例**
   - 外部 type import 全改 `stores/subagentRuns.types`(不用 `export *` re-export 保 public API)
   - 运行时符号(`RunAccumulator` / `parseTranscriptJson`)从 `stores/runAccumulator` import

**Consequences**:
- ✅ 依赖单向无循环;3 文件边界清晰(类型 / 累积器 / store)
- ✅ 与 chat.types.ts 先例完全一致
- ⚠️ SubagentDrawer.vue 改动最大(单文件拆 3 个 import 源);subagentRuns.test.ts 同理

## Open Questions

*(已全部由依赖分析 + chat 先例解决)*

## Technical Notes

* 当前 store 文件:`app/src/stores/subagentRuns.ts`(1416 行)
* 类型检查:`pnpm --filter app exec vue-tsc --noEmit`
* 测试:`pnpm --filter app exec vitest run`
* 先例文件:`app/src/stores/chat.types.ts`(split-chat-types 任务产物,~310 行,本任务 import 惯例的直接来源)
* Trellis 优先级:P2

## Research References

* [`.trellis/spec/frontend/state-management.md`](../../spec/frontend/state-management.md) — Pinia store facade 模式 + chat.types.ts 既有拆分先例
* [`.trellis/spec/guides/cross-layer-thinking-guide.md`](../../spec/guides/cross-layer-thinking-guide.md) — 跨层/跨文件依赖验证清单(本次用于验证循环依赖)
* [`.trellis/spec/frontend/index.md`](../../spec/frontend/index.md) — frontend spec 索引
