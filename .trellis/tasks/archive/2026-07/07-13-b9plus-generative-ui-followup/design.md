# B9+ design.md — 技术设计

> 配套 `prd.md`(产品决策 D-Q1~Q4 已定)。本文讲架构、契约、核心权衡、子任务结构。

## 1. 架构与边界(核心:三角色分离)

B9+ 的命门是"让 primitive 可交互"但不破坏既有安全模型。解法 = **三角色分离**:

| 角色 | 动作 | 权限形态 |
|---|---|---|
| **LLM**(`use_ui` tool) | 只展示(提议 diff / 渲染 button),**不执行任何动作** | Silent Allow 不变(展示无副作用) |
| **用户**(点击应用/按钮) | 动作触发权威 = 显式意图 = 授权 | 不走 LLM tool 链,不弹 modal |
| **后端 `apply_ui_diff` IPC** | 用户触发的写路径 | 不进 Tier/PermissionStore;做 boundary 校验 + 审计 |

```
LLM → use_ui({diff / button}) → [展示 only, silent Allow]
                                       │ 用户点「应用」
前端 ──invoke apply_ui_diff──→ 后端:解析 diff → assert_within_root → 读+应用 hunk → 写 → 审计
                                       │
                                  返回 ok/err → 前端反馈
```

**边界不变量**:
- `use_ui` 保持 Tier 5 Silent Allow + `Risk::Low`(展示无副作用,与 `remember` 同档)—— **不动**。
- `apply_ui_diff` **不注册为 tool**(不在 `builtin_tools()`),故不进 `filter_tools_for_mode` → **plan 模式天然可用**(plan 只过滤 LLM 的 tool,不过滤用户 IPC)。
- `apply_ui_diff` 复用 `projects::boundary::assert_within_root`(同 `edit_file:116`)做路径校验。
- `apply_ui_diff` **不复用 `ReadGuard`** —— ReadGuard 是 LLM-edit 的安全网(必须读过 + 磁盘未变);用户 apply 是显式意图,且 diff 可能含本 session 未读文件。boundary 校验已足够防越权。
- `apply_ui_diff` 写入目标 = `db::load_session(session_id).worktree_path`;**None 时 fallback project root**(与 edit_file / chat_loop 一致,见 `chat.rs:369` 注释"session worktree, or the project root if no worktree")。boundary 校验针对该路径。**无 worktree 不拒绝**(写到 project 原目录,与既有文件操作一致)。

## 2. 数据流与契约

### 2.1 `use_ui` schema 扩展(button type)
primitives items 加 `button`:
```jsonc
{ "type": "button", "action": "apply_diff" | "copy" | "dismiss", "label"?: string, "payload"?: object }
```
- `apply_diff`:`payload = { diff_text }`(标准 unified diff,带 `---`/`+++` 路径头)
- `copy`:`payload = { text }`(纯前端剪贴板)
- `dismiss`:无 payload(纯前端隐藏 card)

`KNOWN_TYPES` + schema `enum` 加 `"button"`(同步,`definition_schema_type_enum_matches_known_types` 测试守卫)。

### 2.2 `use_ui::execute`(button 不执行 action)
execute 仍 non-blocking:校验 `type ∈ 枚举` + button 的 `action ∈ {apply_diff,copy,dismiss}`,返回"已渲染 N 个 primitive"。**action 由前端按 type 分发,execute 不执行**。use_ui 永远 silent Allow。

### 2.3 `apply_ui_diff` IPC 契约(新 Tauri command)
```ts
invoke("apply_ui_diff", { sessionId, diffText })
→ { ok: true,  files: [{ path, added, removed }] }
→ { ok: false, error: string, kind: "boundary" | "parse" | "conflict" | "io" | "empty" }
```
后端步骤:
1. 解析 `diffText`(unified diff → `Vec<FilePatch>`,每个含 path + hunks)。
2. **只接受标准 unified diff**(带 `--- a/path` / `+++ b/path` 头)。无路径头的 LLM 式 +/- 片段 → `kind=parse` 拒绝(DiffPrimitive 的 raw fallback 形式不可应用)。
3. 逐 FilePatch:
   - `assert_within_root(worktree_path, file_path)` → 越界 `kind=boundary` fail-fast。
   - 读当前文件;按 hunk(`@@ -oldStart,oldLines +newStart,newLines @@`)定位 + context 行校验。
   - context 不匹配 → `kind=conflict`,**全失败不部分应用**(避免半应用状态)。
   - 应用 +/- 行 → 写文件。
4. 审计 `AuditKind::UiDiffApplied` `{ session_id, files: [{path,added,removed}] }`。
5. 返回结果(成功才审计;失败不审计,前端反馈 error)。

### 2.4 `<DiffPrimitive>` 应用按钮(D4)
DiffView 下方加"应用"/"拒绝"按钮。应用 → `invoke apply_ui_diff({ sessionId, diffText: primitive.diff_text })`。
- 成功:toast + card 标记"已应用"(禁用按钮)。
- 失败:inline 错误文案(`kind` 映射:boundary="路径越界"/parse="diff 格式无法应用(需标准 unified diff)"/conflict="context 不匹配,文件已变"/io="写入失败")。
- **raw fallback 形式(无路径头)禁用应用按钮**(无可应用目标)。

### 2.5 `<ButtonPrimitive>`(D3)
渲染按钮(`label`)。点击按 `action` 分发:
- `apply_diff` → `invoke apply_ui_diff({ sessionId, diffText: payload.diff_text })`(复用 2.3)。
- `copy` → `navigator.clipboard.writeText(payload.text)` + "已复制"反馈。
- `dismiss` → 本地 `ref` 隐藏 card(无后端)。

### 2.6 审计
- `AuditKind` 加 `UiDiffApplied` 变体(`agent/permissions/audit.rs:34`)。
- 新 `record_ui_diff_applied` 函数(复用 `record_tool_executed_audit:258` 模式)。
- 前端 `useAuditStore` + `<AuditLogModal>` 加 `UiDiffApplied` kind 分发(payload 渲染受影响文件列表)。

## 3. 核心技术权衡:diff → 文件写入怎么实现

unified diff 应用到文件,三选项:

| 选项 | 说明 | 代价 |
|---|---|---|
| A. 引入 `diffy` crate | `Patch::apply` 成熟,零风险 | **新增依赖**,违反 TECH §1.4"零新增依赖"卖点 |
| B. **手写 hunk apply** | 按 `@@` 行号定位 + context 校验 + +/- 应用;冲突 fail-fast | 零依赖,~150 行 + 测试 |
| C. 前端 parsePatch 传结构化 patch | 后端只做行号应用 | 后端逻辑仍要写 + 契约变大 |

**推荐 B(手写)**:零依赖,与项目自研调性一致;hunk apply 是确定性算法,单测可覆盖。

**MVP 能力边界**:
- ✅ 标准 unified diff,多文件多 hunk
- ✅ 按 `@@` 行号 + context 行校验定位
- ✅ 冲突 fail-fast(context 不匹配 = 全失败,不部分应用)
- ❌ 二进制 diff / 新建空文件 / rename / mode change(后续,非本批)
- ❌ LLM 式无路径 +/- 片段(应用按钮禁用)

## 4. 兼容性 / migration
- `use_ui` schema 加 `button` type = **增量**;旧 `diff`/`code_block` primitive 零影响。
- `apply_ui_diff` = 新 IPC,无 migration。
- `AuditKind::UiDiffApplied` = enum 增量;审计 payload 走既有 JSON 列,**无 DB migration**。
- 持久化:`use_ui` tool_use `input.primitives`(含新 `button`)由 `persist_turn` 天然落库;apply 结果**不进 messages**(用户动作,非 LLM turn,只进审计)。

## 5. 运维 / 回滚
- `apply_ui_diff` IPC 可独立禁用(注释 `generate_handler!` 注册行)回滚到"只读展示"。
- 手写 hunk apply 是纯函数,单测覆盖(conflict / boundary / 多 hunk / 行号偏移)。
- 审计 append-only,无回滚风险。

## 6. 任务结构(单 task 分阶段,不拆 child)

D3/D4 有依赖(D3 button 的 `apply_diff` 复用 D4 的 `apply_ui_diff` IPC),拆 child 收益低于 B9(当初 3 primitive 独立)。**parent 单 task 内分 3 阶段**(`implement.md` 落实):

- **阶段 1 — 后端写闭环(D4 核心)**:`apply_ui_diff` IPC + 手写 hunk apply + `assert_within_root` + `AuditKind::UiDiffApplied` + 审计落表。纯后端,可单测。
- **阶段 2 — 前端 diff 应用(D4 闭环)**:`<DiffPrimitive>` 应用/拒绝按钮 + IPC 调用 + 反馈 + raw fallback 禁用。
- **阶段 3 — D3 通用 button**:`use_ui` button type(后端 schema/execute 校验)+ `<ButtonPrimitive>` + action 分发(apply_diff 复用阶段 1 IPC / copy / dismiss 纯前端)。

每阶段独立可验证 → 回滚单元清晰。spec 更新随每阶段尾部。

## 7. LLM 行为边界(教 LLM 何时用 button vs edit_file)

D-Q1"不与 edit_file 冲突"靠 **LLM-facing description** 落实,不是靠代码强制。两类工具的适用场景必须在 `use_ui::definition().description` 讲清:

| 场景 | 用什么 | 理由 |
|---|---|---|
| edit/yolo 模式,该改就改 | `edit_file` | LLM 已授权,直接改,别让用户多点一下 |
| 想让用户确认才写 / plan 模式想提议改 | `use_ui` diff + 应用按钮 | 提议权在 LLM,应用权在用户 |
| 展示多个备选方案让用户对比 | `use_ui` 多个 diff primitive | 用户对比后选一个应用 |

`button` primitive 的 LLM-facing description(写到 `definition().description`):
- `button` = **用户可点的动作按钮**,用于**提议修改 + 交用户拍板**(human-in-the-loop),**不是**让 LLM 自己改。
- `apply_diff`:写 `payload.diff_text` 到文件(**用户点击触发,非 LLM 执行**)。
- `copy`:复制 `payload.text` 到剪贴板(展示便利,纯前端)。
- `dismiss`:用户确认 / 关闭 card(纯前端)。

**反模式(description 要明示避免)**:
- edit 模式下 LLM 该改却用 button 让用户点 → 增加摩擦,错。
- plan 模式下想改但 `edit_file` 被过滤 → 此时 use_ui diff 提议是**正确**用法(plan 唯一的"提议修改"出口)。

**关键**:LLM 在 edit 模式默认用 `edit_file` 直接改;只有当它判断"这个改动用户应该过目确认"或"在 plan 模式无法直接改"时,才用 use_ui diff/button 提议。这条行为准则写进 description,让 LLM 自分流。
