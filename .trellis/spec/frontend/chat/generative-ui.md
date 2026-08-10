# B9 生成式 UI — use_ui primitive registry (2026-07-02)

> **Source**: extracted from `frontend/chat.md` §"B9 生成式 UI — use_ui primitive registry (2026-07-02) / B9+ D3/D4 (2026-07-13)" (2026-08-10 doc-split task).

## B9 生成式 UI — `use_ui` primitive registry (2026-07-02)

`use_ui` tool 的 primitive 由前端 `uiPrimitiveRegistry.ts` 的 `type → Component` Map 派发。`<UiCard>` 读 `call.input.primitives` 遍历，按 `primitive.type` 从 registry 取组件，未知 type 走 fallback（不崩）。

### 注册条目

| type | 组件 | child |
|---|---|---|
| `diff` | `DiffPrimitive.vue`（`parsePatch` 拆多文件 → 复用 `DiffView` 只读 + 复制） | C |
| `code_block` | `CodeBlockPrimitive.vue`（hljs 高亮 + 复制） | B |
| (unknown) | `MockPrimitive`（fallback，渲染 type + JSON dump） | A |

### MessageItem dispatch（`tool_name` 路由，仿 ask_user_question 对称结构）

`<UiCard>` 作为 `<ToolCallCard>` 的 sibling 挂在 `visibleToolCalls` v-for 内（同 `AskUserQuestionCard` 模式，不 portal）：
```vue
<template v-for="tc in visibleToolCalls" :key="tc.id">
  <ToolCallCard :call="tc" :result="..." />
  <AskUserQuestionCard v-if="askCardPropsFor(tc) !== undefined" v-bind="askCardPropsFor(tc)!" />
  <UiCard v-if="tc.name === USE_UI_TOOL_NAME" :call="tc" />
</template>
```

### 加新 primitive 的契约（registry 可扩展性）

加新 type = 改 `uiPrimitiveRegistry.ts` 一行条目 + 后端 `use_ui.rs` 的 `KNOWN_TYPES` + schema `enum` + description 字段说明。`<UiCard>` / MessageItem dispatch 零改动（Child B/C 实证：各只改 registry 一行）。后端 `definition_schema_type_enum_matches_known_types` 测试锁定 schema `enum` 与 `KNOWN_TYPES` 同步。

### 数据源

`<UiCard>` 直接读 `call.input.primitives`（tool_use 输入）—— non-blocking tool 无需独立 IPC 事件（不像 `ask_user_question` 的 `tool:question` channel）。`ToolCallInfo.input: Record<string, unknown>`，`primitives` 用 `Array.isArray` narrow，非数组/缺 → 不渲染（防御 stale 消息）。

### hljs 共享（D6）

`utils/highlight.ts` 的 `renderCodeHtml(code, language)` 两个入口共用：markdown 管线（`marked-highlight` 接 marked 18，现有 markdown 代码块顺带高亮）+ `<CodeBlockPrimitive>`。语言集 `highlight.js/lib/common`（非 full ~900KB）。**注意**：hljs 改变代码块 HTML 输出（`<span class="hljs-*">`），含代码块 substring 的测试断言要适配（见 markdown.test.ts / MarkdownDetailModal.test.ts）。

### Tests required

- `UiCard.test.ts`：registry dispatch（type→组件）+ 未知 type fallback + 空/缺 primitives 守卫
- `CodeBlockPrimitive.test.ts` / `DiffPrimitive.test.ts`：各自渲染 + 复制 + 边界
- 后端 `use_ui::tests`：definition schema + execute 校验

### DiffPrimitive / DiffView raw fallback contract (RULE-FrontDiff-001, 2026-07-02)

`use_ui` 的 `diff` primitive 的 LLM 输出有**两种合法形态**,渲染器必须都能兜住(LLM 风格 +/- 片段是默认行为,标准 unified-diff 是升级形态):

| 形态 | 例子 | 渲染路径 |
|---|---|---|
| 标准 unified-diff(首选) | `--- a/foo\n+++ b/foo\n@@ ... @@\n-x\n+y` | `jsdiff parsePatch` 拆出 hunks → `<DiffView>` 走 `diff-file__hunks` 分支(行级 +/- 染色 + 双 gutter 行号 + 折叠) |
| LLM 风格 +/- 片段(无 `---`/`+++` 头) | ` foo\n-x\n+y\n bar` | **raw fallback** — `DiffPrimitive.files` 按"原 patchToText round-trip 后无内容"路径返回原始 `diff_text` → `<DiffView>` 走 `diff-file__raw` 分支(每行 div 按首字符分类 add/del/ctx/other,绿/红背景,无双 gutter) |

#### 关键 invariant(b00dde2 + b5073ea 落地)

1. **`parsePatch` 空 hunks 检测**:`jsdiff` 对"无 `---`/`+++` 头但有 `+`/`-` 行"的输入返回 **`[{ hunks: [] }]`(`length === 1` 但 hunks 空)**,**不是** `[]`。`patches.length === 0` 守卫会失效。`DiffPrimitive.files` computed 必须额外检查 `patches.length > 0 && patches.every(p => p.hunks.length === 0)` 才触发 raw fallback。
2. **raw fallback 必须保留原文**:`DiffPrimitive` 走 raw fallback 时,`FileDiff.diff_text` 字段填**原始文本**(不是 `patchToText(p)` 重新打包)。DiffView re-parse 这段文本得到同一 `[{hunks:[]}]` 形态,触发自身的 raw fallback `<div>` 分支。round-trip 会丢内容(`patchToText({hunks:[]}) === "--- a\n+++ b"`),永远不要走。
3. **DiffView 防御性兜底**:`out.parsed` 仅在 `out.hunks.length > 0` 时置 true。原 `out.parsed = true` 隐含 `parsed-but-empty` 静默空白根因(模板 `v-for pf.hunks` 走 0 迭代)。如果上游某天绕过 `DiffPrimitive` 直接传 `FileDiff[]` 进 DiffView,这一守卫保证不出现空 body。
4. **计数从原文派生**:raw fallback 的 `added`/`removed` 字段必须按 `text.split("\n")` 重数 `+`/`-` 行(不能从空 hunks 派生,否则永远 0),保证文件 header 的 `+N / −M` badge 显示真实值。
5. **行级染色复用既有 token**:`.diff-raw-line--add / --del` 用 `rgba(16, 185, 129, 0.12)` / `rgba(239, 68, 68, 0.12)`(与 `diff-line--add / --del` 同色,不引入新 token);`--ctx` 与 `--other` 走 `--color-text-secondary` 普通色。

#### Wrong vs Correct

**Wrong**:`DiffPrimitive.files` 只用 `patches.length === 0` 守卫,空 hunks 形态漏判:

```ts
const patches = parsePatch(text);
if (patches.length === 0) return [rawFallback(text)];  // ❌ 漏 catch [{hunks:[]}]
return patches.map(p => ({ diff_text: patchToText(p), ... }));  // 进去后 round-trip 空
// 后果: DiffView 拿 "--- a\n+++ b" → parsePatch 又得 0 hunks
//       → out.parsed = true + hunks = [] → 模板静默空白
```

**Correct**:双守卫 + 原文保留 + DiffView 防御:

```ts
// DiffPrimitive.vue
const allHunksEmpty =
  patches.length > 0 && patches.every(p => p.hunks.length === 0);
if (patches.length === 0 || allHunksEmpty) {
  let added = 0, removed = 0;
  for (const line of text.split("\n")) {
    if (line.startsWith("+") && !line.startsWith("+++")) added++;
    else if (line.startsWith("-") && !line.startsWith("---")) removed++;
  }
  return [{ path: "diff", status: "modified", added, removed, diff_text: text }];
}
return patches.map(p => ({ ... patchToText(p) }));
```

```ts
// DiffView.vue
out.hunks = patch.hunks.map(hunk => /* lines */);
out.parsed = out.hunks.length > 0;  // ❌→✅ 防御 parsed-but-empty
```

#### Common Mistakes / Gotchas

- **jsdiff 空 hunks 不是空数组**:`patches.length === 0` 看着像合理守卫,实际只 catch 0 patch 的输入;LLM-style 不齐的 +/- 进去都是 1 个 patch + 0 hunks。**Lock**:测试两种空 fallback 输入("just some prose" → `[]`、LLM-style → `[{hunks:[]}]`)都得触发 raw 路径。
- **`patchToText({hunks:[]})` 是字符串陷阱**:返回的 `"--- a\n+++ b"` 看似合法(unified-diff 头),但下游 re-parse 后得到 0 行,body 空白。永远不要从空 hunks round-trip。
- **不在 frontend 强制 LLM 格式**:Schema 仅校验 `type`,不校验 `diff_text` 字段是不是真 unified-diff(避免误拒 + 与 `additionalProperties: true` 一致);渲染器承担兜底责任,LLM 契约写在 tool description 里(给 LLM 看,见 `tool-contract/09-use-ui.md` §use_ui + `use_ui.rs` definition 的 `description`)。

#### Tests required(RULE-FrontDiff-001 锁定)

- `DiffPrimitive.test.ts`:"falls back to raw text for LLM-style +/- fragments" — 断言(1)`+N / −M` header 计数正确,(2)`add`/`del`/`ctx` 行级 class 各自分配正确次数,(3)`(1..=n).product()` 与 `match n {` 等关键文本可见。
- `DiffPrimitive.test.ts`:"does not crash on non-diff text" 保留(`parsePatch returns []` 单字串分支,断言 wrapper 存在)。
- 防回归锚点:session `1b469d93-84d3-49b0-a4c5-eefc34b1bf58` — prompt "调 use_ui 输出一个 code_block(rust 代码)和一个 diff(两段对比)",该次 LLM 输出是 LLM-style `+/-` 片段,该 prompt 下必须不再出现"打开 diff 卡片是空白"。

### B9+ D3/D4 — `use_ui` 可交互升级(button primitive + diff 应用) (2026-07-13)

> 完整任务 PRD 走 `.trellis/tasks/07-13-b9plus-generative-ui-followup/`;后端 IPC 契约见 [tool-contract/13-use-ui-button-apply-ui-diff.md §Scenario: `use_ui` `button` + `apply_ui_diff`](../../backend/tool-contract/13-use-ui-button-apply-ui-diff.md)。本节锁定前端契约 + cross-ref 锚点。

B9 (07-02) ship 时 `use_ui` 是纯展示(silent Allow Tier 5,零副作用)。B9+ 把"LLM 提议 → 用户拍板"的最后一公里闭环,核心命门 = "应用动作"的权限归属 — 既不能破坏 plan 模式语义,也不能与 `edit_file` 形成"两种修改模型"冲突。**三角色分离**是本批的根设计:

| 角色 | 动作 | 权限形态 |
|---|---|---|
| **LLM**(`use_ui` tool) | 只展示(提议 diff / 渲染 button),**不执行任何动作** | Silent Allow Tier 5(不变) |
| **用户**(点击应用) | 动作触发权威 = 显式意图 = 授权 | 不走 LLM tool 链,不弹 modal |
| **后端 `apply_ui_diff` IPC** | 用户触发的写路径 | 不进 Tier/PermissionStore;做 boundary 校验 + 审计 |

#### DiffPrimitive — Apply / Reject 按钮(D4)

`<DiffPrimitive>` header 加两个按钮(放在已有的「复制」按钮左边):

| 按钮 | 状态机 | 触发 | 反馈 |
|---|---|---|---|
| **应用** | `idle → applying → applied` | `applyUiDiff(sid, primitive.diff_text)` | 成功:toast「已应用 N 个文件」+ card 标记「已应用」(`<span class="ui-prim__applied-tag">`) + 按钮 disable;失败:inline `<div class="ui-prim__error">` 展示 `APPLY_UI_DIFF_ERROR_TEXT[errorKind]`(中文文案见 `uiDiffApply.ts`) |
| **拒绝** | `idle → rejected`(本地隐藏) | 无 IPC,纯前端 | `v-if="applyState !== 'rejected'"` 控制 card 显隐 |

**Raw fallback 禁用门**(D-Q8):`hasUnifiedHeaders` computed
(`/^--- /m.test(text) && /^\+\+\+ /m.test(text)`)— false 时 Apply 按钮
`disabled` + tooltip「该 diff 格式不可应用(需带 ---/+++ 路径头的标准 unified diff)」。
Reject 按钮在 raw fallback 时仍可用(用户可关闭噪音卡)。

**无活跃会话禁用门**:`!chatStore.currentSessionId` 时 Apply `disabled` + tooltip「无活跃会话」(后端 `apply_ui_diff` 需要 `sessionId` 解析 write target)。

**State machine**:
```
idle ─click apply→ applying ─success→ applied ──→ (buttons hide, tag shows)
                          └─failure→ idle + errorKind ≠ null + inline error
idle ─click reject→ rejected ──→ (card hidden via v-if)
```

#### ButtonPrimitive — 通用 button primitive(D3)

新组件 `app/src/components/chat/primitives/ButtonPrimitive.vue`,在 `uiPrimitiveRegistry` 注册 `button → ButtonPrimitive`。接收 `UiButtonPrimitive` shape(`type: "button"` + `action` + `label?` + `payload?`)。**3 action 分发**:

| `action` | 触发 | 实现 |
|---|---|---|
| `apply_diff` | `applyUiDiff(sid, payload.diff_text)` | 复用 DiffPrimitive 的 IPC;成功 toast「已应用 N 个文件」;失败 inline error |
| `copy` | `navigator.clipboard.writeText(payload.text)` | 成功 toast「已复制到剪贴板」(1500ms);前端兜底:无 `payload.text` → `kind="io"` |
| `dismiss` | 本地隐藏 | 无 IPC / 无 toast;`v-if="state !== 'done'"` |

**Default labels**(`DEFAULT_LABELS` const):
- `apply_diff` → "应用"
- `copy` → "复制"
- `dismiss` → "关闭"

LLM 显式传 `label` 时覆盖默认值。

**Action-specific 配色**(复用 `--color-tool-*` token,无新 token):
- `apply_diff` → `--color-tool-write`(emerald)
- `copy` → `--color-text-primary`(neutral)
- `dismiss` → `--color-text-muted`(灰色,hover 变 `--color-tool-error`)

**Disable 条件**:
- `apply_diff` 无活跃会话 → disable + tooltip「无活跃会话」
- 任意 action 在 `state === "working"` 时 disable + tooltip「处理中...」

**Defensive layer**(未知 action):Rust validator 已拒绝,前端渲染器仍
mount 一个 no-op 按钮(stale 消息保护)。`applyUiDiff` 异常(`ApplyUiDiffError` 之外的网络 / 序列化错误)归 `kind="io"` + `console.error`。

#### IPC 包装 — `utils/uiDiffApply.ts`

单文件,thin wrapper over `@tauri-apps/api/core::invoke`:
- `APPLY_UI_DIFF_CMD = "apply_ui_diff"`(单源 command name)
- `ApplyUiDiffResult` tagged union(`{ok:true, files} | {ok:false, kind, error}`)
- `ApplyUiDiffFile { path, added, removed }`(成功 path 的每个文件)
- `ApplyUiDiffFailureKind = "boundary" | "parse" | "conflict" | "io" | "empty"`
- `applyUiDiff(sessionId, diffText): Promise<ApplyUiDiffFile[]>` — throws `ApplyUiDiffError { kind, message }` on backend failure
- `APPLY_UI_DIFF_ERROR_TEXT` — kind → 中文文案表(单源,frontend chat.md 是设计合约,`uiDiffApply.ts` 是实现权威)

Tauri 自动把 JS `sessionId`/`diffText` 转 Rust `session_id`/`diff_text`,前端组件不需要 snake/camel 翻译。

#### Audit 前端分发 — `AuditLogModal` 加 `ui_diff_applied`

`utils/audit.ts` 增:
- `UI_DIFF_APPLIED_KIND = "ui_diff_applied"`(wire 锁定 `AuditKind::UiDiffApplied.as_str()`)
- `UiDiffAppliedAuditPayload { files?: Array<{path?, added?, removed?}>, total_files? }`
- `AUDIT_KIND_OPTIONS` 加 `{ value: "ui_diff_applied", label: "应用 diff" }`
- `AuditIconFamily` 加 `"ui-diff-applied"` variant
- `iconFamilyForKind("ui_diff_applied")` → `"ui-diff-applied"`
- `parseAuditPayload` 新 case 返回 `{ kind: "ui_diff_applied", payload: ... }`

`AuditLogItem.vue` 增:
- `meta("ui-diff-applied")` → `{ iconName: "file-check", colorVar: "var(--color-tool-write)" }`(lucide `FileCheck` icon,需 `Icon.vue` 注册 `"file-check": FileCheck`)
- `uiDiffAppliedSummary` computed — 格式 `应用 diff · N 个文件 (+A / -B) · path1, path2, path3 [+N more]`(取 `files` 前 3 个 path 展示 + truncated 计数)

#### 测试矩阵

- **`DiffPrimitive.test.ts`**(B9+ D4 段,17 测试):IPC invoke spy / 成功 toast + 「已应用」 tag / kind → inline error / raw fallback disabled / 无 session disabled / 异常错误归 `io` / reject 隐藏卡
- **`ButtonPrimitive.test.ts`**(11 测试):3 action 默认 label / 自定义 label override / `apply_diff` IPC 调用 / `copy` clipboard / `dismiss` 本地 / 无 session disabled / 未知 action defensive
- **`UiCard.test.ts`**(增量):registry `button` entry 间接通过 DiffPrimitive / ButtonPrimitive 子组件覆盖;Pinia setup 必要(DiffPrimitive 用 `useChatStore`)
- **后端 cross-ref**:24 `diff_apply::tests` + 7 `use_ui::tests::execute_button_*` + 4 `commands::ui::tests`(详见 [tool-contract/13-use-ui-button-apply-ui-diff.md §B9+ D3/D4](../../backend/tool-contract/13-use-ui-button-apply-ui-diff.md) §6)

#### 关键决策(前端契约视角)

- **应用按钮是用户 IPC,不是 LLM tool 触发**:用户点 Apply = 显式授权,无 Tier / 无 PermissionStore。plan 模式天然可用(`filter_tools_for_mode` 看不到 `apply_ui_diff`,因为它不在 `builtin_tools()`)。
- **raw fallback 禁用是 UX 优化,不是安全门**:后端 `parse_unified_diff` 是兜底(无头 → `kind="parse"`);前端禁用避免无意义 round-trip + 给用户直观反馈。
- **state machine 不进 messages / 不进 audit 失败路径**:`apply_ui_diff` 失败**不**落 `UiDiffApplied` 审计(只有成功才 audit)。前端 inline error 反馈即可;若未来需要失败可观测性,新增 `AuditKind::UiDiffFailed` 变体,**不**复用 `UiDiffApplied`。
- **错误文案是 frontend 单源**:`APPLY_UI_DIFF_ERROR_TEXT` 是 `uiDiffApply.ts` 的 `Record<Kind, string>`,`DiffPrimitive.vue` 直接读;kind 字符串 wire 与后端 `ApplyUiDiffResult.kind` 1:1 锁定(后端测试 `parse_error_kind_label_is_parse` 守护字符串字面量)。

---
