# 重写 SubagentDrawer entry 为 tool-card 样式

## Goal

把 `app/src/components/chat/SubagentDrawer.vue` 内的每条 transcript entry 从当前的扁平 `<kind-badge> + <body>` 结构重写成跟主面板 `ToolCallCard.vue` 同款的卡片结构（3px 彩色左边框 + 单行 header + body），复用已抽好的 `<ToolInputBody>` / `<ToolOutputBody>` / `<PermissionAskBody>` / `<WorkerTextTimeline>` 共享 body 组件，达成 drawer 与主面板视觉一致、tool 类型 / 状态 / 时延一眼可读。

## Background

2026-06-21 review `docs/review/b6-subagent-assessment.md` 反馈：SubagentDrawer 当前"不直观"，每条 entry 视觉权重一样、没有彩色左边框、没有 status icon / duration 信息，跟主面板 `ToolCallCard` 的视觉语言差异大。本次改造不涉及后端，只重写 drawer 内的 entry 渲染层。

## What I already know

### 现状（来自代码 + 图片）

- **Drawer 宽度**：`width: min(640px, 90vw)`，`SubagentDrawer.vue:640`
- **Entry 结构**：`<li class="subagent-drawer__entry">` 包 `<span class="subagent-drawer__kind">` + `<div class="subagent-drawer__body-content">`，entry 之间用 `border-bottom: 1px solid` 分割（`SubagentDrawer.vue:889-898`）
- **Kind badge**：4 类 — `chat_event`（muted 灰）/ `tool_call`（accent 蓝）/ `tool_result`（write 绿）/ `permission_ask`（shell 琥珀），48px 宽方块标签
- **Body 内容**：每条 entry 路由到已抽好的共享组件
  - `tool_call` → `<ToolInputBody :name :input>`
  - `tool_result` → `<ToolOutputBody :content :is-error>`
  - `permission_ask` → `<PermissionAskBody mode="historical">`（worker 是 Tier 4 deny，无 interactive UI）
  - `chat_event` → `<WorkerTextTimeline :events>`
- **Filter**：默认隐藏 `chat_event`（PRD 决定 #2），有 `Show chat events` 复选框 + `+N chat hidden` 提示
- **Header 已有**：status badge + name + jump-to-latest + close + failure-reason banner (FT-F-005) + timestamps + summary

### ToolCallCard 的视觉语言（要复用的）

- `.tool-card` 容器：`background: --color-bg-surface` + `border: 1px solid --color-bg-border` + `border-left: 3px solid <accent>` + `border-radius: 6px` + `padding: 8px 12px`
- `.tool-card--error`：`border-color: --color-tool-error` + 背景 `--color-bg-elevated`
- `.tool-card--running`：`border-left-color: --color-tool-shell` + status icon 脉冲动画
- Header：单行 `icon + name + path | status-icon + status-text + duration + diff-btn`
- accent 色按 tool 名映射（`toolAccentVar(name)`）：read_file=cyan, write_file=emerald, shell=amber
- 输入默认 `<details>` 折叠，输出直接展示

### 已抽好的共享组件（FT-F-001 PR1）

- `<ToolInputBody>` — `<details>` + input `<pre>`，空 input 自动隐藏
- `<ToolOutputBody>` — output content + envelope-unwrapping + truncation（无 duration 显示）
- `<PermissionAskBody>` — `mode: "historical" | "interactive"`，historical mode 只显示不可操作
- `<WorkerTextTimeline>` — chat_event 文本流渲染

### Transcript 数据来源

- `liveTranscript.get(openRunId)`（live stream，subagent:event IPC 累积）
- `run.transcriptJson`（DB cache from `fetchRun`）
- entry 字段：`{ kind, payload_json }`，`payload_json` 形态：
  - `tool_call`：`{ name, input }`
  - `tool_result`：`{ content, is_error }`（无 duration_ms）
  - `permission_ask`：`{ toolName, toolInput, path, reason, risk, ... }`
  - `chat_event`：`{ type: "text|thought|...", text }`

### 后端 transcript 不携带 duration

- `agent/subagent.rs` 的 `record()` 方法（657-699）按 event 入库 payload_json，不含 durationMs
- 这是个**已知 gap**：drawer 改 tool-card 样式后，tool_result 卡片的 duration 字段无来源
- 决策待定：是否需要后端补字段（落入 `scope`），还是 drawer 不显示 duration？

## Assumptions (待用户确认)

1. **视觉风格一致性是首要目标**，不需要为 drawer 设计独立风格
2. **复用已有样式 token**（`tool-card` 相关 CSS class），不引入新视觉风格系统
3. **filter / jump-to-latest / auto-scroll / banner / status 计时等现有 drawer 行为保留**
4. **permission_ask 在 drawer 里仍然是 historical mode**（worker 无 Tier 4 UI sink，prompt 不会到 UI）

## Open Questions

### 已决策 ✅

- **Q1 = B**：call+result 合并为一张卡（跟 ToolCallCard 严格一致）
  - 前端建立"配对缓冲层"：tool_call event 到达时**不立即渲染**，进入 pending buffer；tool_result event 到达时按 `tool_use_id` 匹配，merge 为一张卡再渲染
  - 边界：worker cancel / error / 超时 → pending call 卡 fallback 为"未完成"（带超时清理 timer）
- **Q1.1 = A**：后端 `SubagentBufferSink.record()` 的 tool_call / tool_result 两个分支都补 `tool_use_id` 字段
- **Q2 = A**：后端补 `duration_ms` 到 tool_result payload_json（measure from tool_call event 到 tool_result event 的 wall-clock）
- **Q3 = A**：保留 chat_event toggle，默认折叠，+N chat hidden 提示保留
- **Q4 = A**：ToolInputBody 的 `<details>` 在 drawer 里默认折叠（跟主面板一致）

### 跨层决策点

- **DB schema**：`subagent_runs.transcript_json` 已是 TEXT 列（推测），新字段作为 JSON 内的 key 即可，无需 migration
- **IPC 协议**：`subagent:event` emit 的 transcript entry payload_json 增加新字段；前端 `TranscriptEntry` type 同步
- **Type 同步**：store `subagentRuns.ts` 的 `TranscriptEntry` 类型增加 `tool_use_id` 和 `duration_ms` 可选字段
- **共享样式**：drawer entry 复用 `ToolCallCard` 的 `.tool-card` 相关 class，不复制样式（避免双源）

## Expansion Sweep (DIVERGE)

### Future evolution（1-3 个月）

- **多 worker 并行**：`is_parallel_eligible()`（`chat_loop.rs:1907`）已存在，未来支持并行 dispatch 时 drawer 一次只能开一个 `openRunId` 成为瓶颈。**本期不修，列入 follow-up**。
- **Worker 模型覆盖**：researcher 用 Haiku / general-purpose 用 Sonnet（v2 OOS）—— drawer 不受影响（不展示 model）。
- **Worker retry**：cancel 后无 retry（v2 OOS）—— drawer 仅显示 cancelled 状态。

### Related scenarios（需保持一致）

- **主面板 dispatch_subagent 卡片**（`ToolCallCard.vue:558-590`）：已是 tool-card 样式。drawer 改造后视觉与主面板卡片严格一致，**反向不影响主面板**。
- **Audit 日志**（C4）：drawer 不记录 audit（不是 Tier 1-3 决策点），无需改动。

### Failure / edge cases

- **Worker cancel 时 pending call 卡**：缓冲层需要 timeout flush（建议 30s，超过则强制渲染为"未完成" + amber left-border）
- **tool_use_id 配对失败**（IPC 丢 event / DB 截断 / 后端 bug）：fallback 为独立渲染（call 卡显示"无 result" + result 卡显示"无对应 call"）
- **Transcript 4 MiB 截断**：pending buffer 可能在截断边界丢失配对 —— 截断边界 entry 强制 flush 即可
- **Drawer 打开时 worker 在跑**：缓冲层持续追加，新 event 配对后实时更新对应卡片（reactive 替换原卡 + scroll-to-latest）

## Requirements (final)

### Backend (Rust)

- `SubagentBufferSink::record()` 的 `tool_call` 分支 payload_json 增加 `tool_use_id: String`
- `SubagentBufferSink::record()` 的 `tool_result` 分支 payload_json 增加 `tool_use_id: String` + `duration_ms: u64`
  - `duration_ms` 测量方式：tool_call → tool_result 在 sink 内的 wall-clock 差（同一个 `SubagentBufferSink` 实例持有 `tool_call_received_at: HashMap<tool_use_id, Instant>`）
  - 边界：tool_result 到达但无对应 tool_call 时（IPC 丢 event），`duration_ms = 0` + 静默 log warn
- `format_dispatch_result` 等下游 consumers 不需要改动（payload_json 是开放 schema）

### Frontend (TypeScript / Vue)

- 新增 `BufferedTranscriptEntry` 类型，区分三种状态：
  - `pending_call` — tool_call 已收、等 result
  - `paired` — tool_call + tool_result 都已收、合并渲染
  - `standalone` — chat_event / permission_ask / orphan call or result（无对应配对）
- Drawer 新增"配对缓冲层" `useTranscriptPairing()` composable（或 store 内方法）：
  - 输入：原始 `TranscriptEntry[]`
  - 输出：缓冲后的 `BufferedTranscriptEntry[]`
  - 内部：HashMap<tool_use_id, PendingCall>，timeout 30s flush orphan call 为 `standalone`
- Drawer entry 渲染重写为 `.tool-card` 同款结构：
  - 容器：复用 `.tool-card` + `.tool-card--error` + `.tool-card--running` 样式 class
  - header：icon + tool name + path（tool_call）+ `toolAccentVar(name)` 左边框 | status-icon + status-text + duration + (result chars)
  - body：`<ToolInputBody :name :input>` 折叠 + `<ToolOutputBody :content :is-error :duration-ms>` 直展
- `permission_ask` entry：amber 左边框 + ask reason（historical mode，无 interactive UI）
- `chat_event` entry：muted 灰左边框（仍是 toggle 控制可见性）
- Drawer 现有行为保留：filter toggle、jump-to-latest、auto-scroll、failure banner、status 计时器、`+N chat hidden` 提示
- 共享样式策略：复用 `ToolCallCard.vue` 的 `.tool-card` 等 class，drawer 通过 `<style scoped>` import 或全局 class 复用（不复制样式）

### Type 同步

- `app/src/stores/subagentRuns.ts` 的 `TranscriptEntry` type 扩展：
  ```ts
  type TranscriptEntry =
    | { kind: 'tool_call'; payload_json: { name: string; input: Record<string, unknown>; tool_use_id: string } }
    | { kind: 'tool_result'; payload_json: { content: string; is_error: boolean; tool_use_id: string; duration_ms: number } }
    | { kind: 'permission_ask'; payload_json: { ... } }
    | { kind: 'chat_event'; payload_json: { ... } };
  ```

### 测试

- 后端单元测试（`app/src-tauri/src/agent/subagent.rs`）：tool_use_id 写入、duration_ms 计算、orphan tool_result fallback
- 前端 Vitest 单测（`app/src/components/chat/SubagentDrawer.test.ts` + 新增 `useTranscriptPairing.test.ts`）：
  - 配对正常路径
  - pending call timeout flush
  - orphan call / orphan result fallback
  - chat_event filter 行为不变
  - .tool-card 样式 class 应用
- 视觉验证（手动）：与主面板 dispatch_subagent 卡片截图对比

## Acceptance Criteria (final)

### Backend

- [ ] `SubagentBufferSink::record()` tool_call 分支 payload_json 包含 `tool_use_id` 字段
- [ ] `SubagentBufferSink::record()` tool_result 分支 payload_json 包含 `tool_use_id` + `duration_ms` 字段
- [ ] `duration_ms` 在同 sink 内 tool_call → tool_result 的 wall-clock 差（毫秒）
- [ ] Orphan tool_result（无对应 tool_call）`duration_ms = 0` + warn log，不 panic
- [ ] 现有 `cargo test` 全部通过

### Frontend

- [ ] Drawer 内每条 entry 的 DOM 结构跟 `ToolCallCard` 同款（同样 `.tool-card` 容器 class）
- [ ] tool_call + tool_result 配对合并为一张卡（按 `tool_use_id`）
- [ ] tool_call entry 的 3px 左边框按 tool 名映射（read_file=cyan / write_file=emerald / shell=amber）
- [ ] tool_result entry 显示 `duration_ms` 和 is_error 状态色
- [ ] 合并卡片 header 单行：icon + tool name + path | status-icon + status-text + duration
- [ ] error entry 整张卡片变红（`--color-tool-error` 边框 + 背景 elevated）
- [ ] permission_ask entry 用琥珀左边框 + ask 原因展示（historical mode）
- [ ] chat_event entry 用 muted 灰左边框，仍是 toggle 控制可见性
- [ ] Pending call 卡 30s timeout 后强制 flush 为"未完成"独立卡（amber 边框 + 提示文案）
- [ ] Orphan call / orphan result fallback 独立渲染（不卡 UI）
- [ ] Drawer 现有的 filter / jump-to-latest / banner / status 计时器功能不退化
- [ ] Drawer 在 running / completed / cancelled / error 四种状态下视觉过渡正常
- [ ] Vitest 单测覆盖：配对路径、timeout flush、orphan fallback、chat_event filter、样式 class 应用
- [ ] `vue-tsc --noEmit` 通过
- [ ] `pnpm build` 不报错

### 跨层一致性

- [ ] 后端 payload_json 新字段在前端 `TranscriptEntry` type 同步声明
- [ ] 共享 body 组件（`ToolInputBody` / `ToolOutputBody`）props 扩展有向后兼容（缺字段不崩）
- [ ] 现有 dispatch_subagent 主面板卡片视觉不变（drawer 反向不影响）

## Definition of Done

- 后端改动：`SubagentBufferSink::record()` 两分支扩字段 + 单元测试 + `cargo test` 全绿
- 前端改动：`useTranscriptPairing` 缓冲层 + `SubagentDrawer.vue` entry 重写 + Vitest 覆盖 + `vue-tsc --noEmit` 通过
- `pnpm build` 通过
- 视觉验证：截图对比主面板 dispatch_subagent 卡片和 drawer entry，确认视觉一致
- 在 PR 描述或 commit message 里链接本 prd.md
- 如发现新的视觉约定（drawer 复用 tool-card 样式 token 的方式），更新 `.trellis/spec/frontend/` 下的相关 spec

## Technical Approach

### 配对缓冲层设计（前端核心）

```ts
// stores/subagentRuns.ts 内的纯函数（无副作用、易测）
type PendingCall = {
  tool_use_id: string;
  call: TranscriptEntry; // kind === 'tool_call'
  received_at: number; // Date.now()，用于 timeout 计时
};

type BufferedTranscriptEntry =
  | { kind: 'paired'; tool_use_id: string; call: TranscriptEntry; result: TranscriptEntry }
  | { kind: 'pending_call'; tool_use_id: string; call: TranscriptEntry }
  | { kind: 'standalone'; entry: TranscriptEntry };

function pairTranscript(entries: TranscriptEntry[], now: number): BufferedTranscriptEntry[] {
  const pending = new Map<string, PendingCall>();
  const out: BufferedTranscriptEntry[] = [];
  for (const e of entries) {
    if (e.kind === 'tool_call') {
      pending.set(e.payload_json.tool_use_id, {
        tool_use_id: e.payload_json.tool_use_id,
        call: e,
        received_at: now,
      });
    } else if (e.kind === 'tool_result') {
      const id = e.payload_json.tool_use_id;
      const p = pending.get(id);
      if (p) {
        out.push({ kind: 'paired', tool_use_id: id, call: p.call, result: e });
        pending.delete(id);
      } else {
        // orphan result
        out.push({ kind: 'standalone', entry: e });
      }
    } else {
      out.push({ kind: 'standalone', entry: e });
    }
  }
  // flush remaining pending calls
  for (const p of pending.values()) {
    if (now - p.received_at >= PENDING_TIMEOUT_MS) {
      out.push({ kind: 'standalone', entry: p.call });
    } else {
      out.push({ kind: 'pending_call', tool_use_id: p.tool_use_id, call: p.call });
    }
  }
  return out;
}
```

`PENDING_TIMEOUT_MS = 30_000`。Drawer 用 `setInterval` 每 5s 重算 `pairTranscript(transcript, Date.now())` 让 pending 卡随时间推进（不能因为没新 event 就停在那里不更新）。

### 复用 ToolCard 样式的策略

Drawer 在 `<style scoped>` 里直接用 `.tool-card` / `.tool-card--error` / `.tool-card--running` class（不复制样式）。Vue 3.5 scoped 样式作用域下，`SubagentDrawer.vue` 可以通过 `:deep()` 或全局 class 引用其他组件的样式。**首选**：在 `app/src/components/chat/_shared-toolcard.scss` 抽出公共样式（如果项目没有 SCSS，则用 `<style>` 全局块）。**回退**：直接在 drawer 里 `:deep(.tool-card) { ... }` 复制最小子集。

最终选择 Phase 2 implement 时根据项目 SCSS / CSS 模块约定决定。

### 后端 duration_ms 测量

```rust
// SubagentBufferSink 新增字段
struct SubagentBufferSink {
    // ... 现有字段
    tool_call_received_at: HashMap<String, Instant>,
}

impl SubagentBufferSink {
    fn record_tool_call(&mut self, tool_use_id: String, ...) {
        self.tool_call_received_at.insert(tool_use_id.clone(), Instant::now());
        // ... 现有序列化
    }
    fn record_tool_result(&mut self, tool_use_id: String, ...) {
        let duration_ms = self.tool_call_received_at
            .remove(&tool_use_id)
            .map(|t| t.elapsed().as_millis() as u64)
            .unwrap_or_else(|| {
                tracing::warn!(tool_use_id, "tool_result without matching tool_call");
                0
            });
        // ... 现有序列化 + tool_use_id + duration_ms
    }
}
```

## Decision (ADR-lite)

**Context**: B6 review `docs/review/b6-subagent-assessment.md` 反馈 SubagentDrawer "不直观"——entry 视觉权重一样、没有彩色左边框、没有 status / duration 信息，跟主面板 ToolCallCard 视觉差异大。

**Decision**:
1. Drawer entry 重写为 `.tool-card` 同款结构，call+result 配对合并为一张卡（跟 ToolCallCard 严格一致）
2. 后端 `SubagentBufferSink` 同步补 `tool_use_id`（两分支）+ `duration_ms`（tool_result 分支）字段，让合并卡可显示 duration
3. 前端建立 `pairTranscript` 配对缓冲层 + 30s timeout flush orphan pending call
4. 复用 ToolCallCard 样式 token，不复制

**Consequences**:
- ✅ Drawer 与主面板视觉一致，user 在主面板和 drawer 间切换无"翻译"成本
- ✅ tool_result duration 一眼可见
- ✅ error entry 立即变红（跟主面板一致）
- ⚠️ 后端 payload_json schema 扩字段，下游 consumers 不变（开放 schema）
- ⚠️ Drawer 缓冲层是新组件，需要充分单测覆盖（pending / orphan / timeout 路径）
- ⚠️ 复用样式 token 的具体方式（`:deep()` vs 全局 SCSS）Phase 2 implement 时定

## Out of Scope (本期明确不做)

- drawer 整体移除 / 改内联展开模式
- worker transcript 摘要写回 parent LLM context（review defect B，独立任务）
- worker system_prompt dead code 修复（独立任务 `06-21-fix-worker-system-prompt-dead-code`）
- 多 worker 并行下 drawer 同时开多个（drawer 一次只能开一个 openRunId）—— **列入 follow-up**
- worker retry / 模型覆盖 / context_window 覆盖（v2 OOS）
- 改 SubagentBufferSink 的 IPC 协议其他部分
- 新增 design tokens / 重构全局样式系统

## Implementation Plan (small PRs)

### PR1 — Backend payload_json 扩字段
- 改动 `app/src-tauri/src/agent/subagent.rs`：`SubagentBufferSink` 加 `tool_call_received_at` HashMap；`record_tool_call` / `record_tool_result` 路径补字段
- 新增后端单元测试：正常配对 / orphan tool_result fallback
- `cargo test --lib` 通过
- **不依赖前端 PR，可独立 merge**

### PR2 — Frontend 配对缓冲层 + 类型同步
- 新增 `app/src/utils/transcriptPairing.ts`：`pairTranscript` 纯函数
- `app/src/stores/subagentRuns.ts` 的 `TranscriptEntry` type 扩展
- 新增 `app/src/utils/transcriptPairing.test.ts` 单元测试
- `vue-tsc --noEmit` 通过
- **不依赖 PR3，可独立 merge（属于准备 PR）**

### PR3 — Drawer entry 重写
- 改动 `app/src/components/chat/SubagentDrawer.vue`：
  - 引入 `pairTranscript`
  - 改 entry 渲染为 `.tool-card` 同款结构（容器 + header + body）
  - 加 30s pending timeout 触发重算
  - 保留所有现有 drawer 行为（filter / jump-to-latest / auto-scroll / banner / 计时器）
- 共享样式策略选择（`:deep()` vs 全局 SCSS）：根据项目约定定
- 调整 `app/src/components/chat/SubagentDrawer.test.ts` 测试断言（DOM 结构 / class 应用 / 配对行为）
- 视觉验证：截图对比主面板
- **依赖 PR1 + PR2**

### PR4 — 文档与 review 更新（可选）
- 在 `docs/review/b6-subagent-assessment.md` 追加"已修复"章节（drawer entry 视觉对齐 ToolCallCard）
- 在 `.trellis/spec/frontend/` 下如发现新视觉约定（drawer 复用 tool-card 样式 token），更新 spec
- `pnpm build` 最终验证

## Out of Scope (本期明确不做)

- 后端 `tool_result.durationMs` 字段补齐（除非 Q2 选了 B）
- drawer 整体移除 / 改内联展开模式（方案 B 已被否决）
- worker transcript 摘要写回 parent LLM context（这是 review defect B，独立任务）
- worker system_prompt dead code 修复（独立任务 `06-21-fix-worker-system-prompt-dead-code`）
- 改 SubagentBufferSink 的 IPC 协议
- 新增 design tokens / 重构全局样式系统

## Technical Notes

- 涉及文件：
  - `app/src/components/chat/SubagentDrawer.vue`（主改动）
  - 共享 body 组件可能小幅调整（如果 props 需要新增）：
    - `app/src/components/chat/ToolInputBody.vue`
    - `app/src/components/chat/ToolOutputBody.vue`
    - `app/src/components/chat/PermissionAskBody.vue`
    - `app/src/components/chat/WorkerTextTimeline.vue`
  - 复用样式 token（不复制样式）：
    - `app/src/components/chat/ToolCallCard.vue`（`.tool-card`, `.tool-card--error`, `.tool-card--running`）
    - `app/src/utils/messageFormat.ts`（`toolAccentVar`, `toolIcon`）
- 已有测试参考：
  - `app/src/components/chat/ToolCallCard.test.ts`（dispatch_subagent 分支的卡样式 / 交互测试）
  - `app/src/components/chat/SubagentDrawer.test.ts`（drawer 集成测试 — 需更新断言以匹配新样式）
- 关键约束：
  - reka-ui `DialogContent` portal 到 body，样式需要保留 Vue 3.5 `data-v-*` 选择器（见 `AuditLogModal.vue` 的 `:deep()` 模式）
  - `bodyEl` ref / auto-scroll 逻辑必须保留（drawer 的体验核心）
  - `WorkerTextTimeline` 已有 memoization，新 entry 类型不应破坏其性能