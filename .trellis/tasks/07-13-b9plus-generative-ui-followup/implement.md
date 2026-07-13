# B9+ implement.md — 执行计划

> 配套 `prd.md`(产品决策)+ `design.md`(技术设计)。**单 task 分 3 阶段**(design §6,不拆 child)。每阶段独立可验证,尾部跑 validation 才进下一阶段。

## 前置(D-Q 决策摘要,执行时不再重议)
- D-Q1 定位 = 用户确认 UI(LLM 提议,用户应用,不走 LLM tool 链)
- D-Q3 scope = D4 + D3(不做 D5)
- D-Q2a/b action = 预定义枚举;首批 = `apply_diff`(后端写+审计)+ `copy`/`dismiss`(纯前端)
- D-Q4 apply 权限 = 不弹 modal + `assert_within_root` + 审计落表

## 阶段 1 — 后端写闭环(D4 核心,命门)

手写 hunk apply 是本批最难、最高风险部分,单测覆盖前不进阶段 2。

- [ ] **1.1** `agent/permissions/audit.rs`:加 `AuditKind::UiDiffApplied` 变体(新 "UI 域" 段,或归 Tool 域)+ `as_str()` 返 `"ui_diff_applied"` + `record_ui_diff_applied(db, session_id, files)`(复用 `record_tool_executed_audit:258` 模式,payload `{ files: [{path, added, removed}] }`)。
- [ ] **1.2** 新 `src/diff_apply.rs`(手写 hunk apply,零依赖):
  - `parse_unified_diff(text) -> Result<Vec<FilePatch>, ParseError>`:解析 `--- a/` / `+++ b/` / `@@ -o,ol +n,nl @@` 头 + hunk 行;**无路径头 → ParseError**(raw fallback 不可应用)。
  - `apply_to_file(patch: &FilePatch, current: &str) -> Result<ApplyStats, ApplyError>`:按 `@@` 行号定位 + context 行校验;context 不匹配 → `ApplyError::Conflict`(**fail-fast,不部分应用**);返回 `{added, removed}`。
  - 单测:多 hunk / 多文件 / context 匹配 / context 冲突 / 行号偏移 / 空 diff / 仅新增文件(若支持)。
- [ ] **1.3** 新 `src/commands/ui.rs::apply_ui_diff(state, session_id, diff_text)`:
  - 解析 → `Vec<FilePatch>`;空/无路径头 → `{ok:false, kind:"parse"|"empty"}`。
  - 逐 patch:`assert_within_root(worktree_path, path)`(同 `edit_file:116`)→ 越界 `kind:"boundary"` 全失败;读文件 → `apply_to_file` → 写(`tokio::fs::write`)。任一冲突 → 全失败不写(`kind:"conflict"`)。
  - 成功 → `record_ui_diff_applied` 审计 → 返 `{ok:true, files}`。
  - 返回结构见 design §2.3。
- [ ] **1.4** `src/commands/mod.rs` 加 `pub mod ui;` + `lib.rs` `generate_handler!` 注册 `commands::ui::apply_ui_diff`(带注释,仿 `merge_worker_run` L247)。
- [ ] **1.5 验证**:
  ```bash
  cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib diff_apply
  cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib
  cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo fmt --check
  ```

## 阶段 2 — 前端 diff 应用(D4 闭环)

- [ ] **2.1** IPC 调用 helper:`utils/uiDiffApply.ts`(或加到既有 invoke utils)—— `applyUiDiff(sessionId, diffText) → {ok, files?, error?, kind?}`。
- [ ] **2.2** `components/chat/primitives/DiffPrimitive.vue`:DiffView 下方加"应用"/"拒绝"按钮。
  - 应用 → `applyUiDiff` → 成功 toast + card 标记"已应用"(按钮禁用);失败 inline 错误(`kind` → 中文文案:boundary=路径越界 / parse=需标准 unified diff / conflict=文件已变 context 不匹配 / io=写入失败)。
  - **raw fallback 形式(无 `---`/`+++` 头)禁用应用按钮** + 提示"该 diff 格式不可应用"。
- [ ] **2.3** 审计前端分发:`utils/audit.ts`(或 `AuditLogItem.vue`)的 `iconFamilyForKind` + `parseAuditPayload` 加 `"ui_diff_applied"` case(图标族 + payload 渲染受影响文件列表)。
- [ ] **2.4 验证**:
  ```bash
  cd app && pnpm test    # vitest(DiffPrimitive.test.ts 加 apply 用例)
  cd app && pnpm build   # vue-tsc --noEmit + vite build
  ```

## 阶段 3 — D3 通用 button

- [ ] **3.1** `tools/use_ui.rs`:`KNOWN_TYPES` + schema `enum` 加 `"button"`;`execute` 校验 button 的 `action ∈ {apply_diff,copy,dismiss}`(未知 action → 错误)。更新 `definition()` description 教 LLM 何时用 button + action 语义(`apply_diff` = 提议修改交用户应用;不要用于直接改文件)。
- [ ] **3.2** `uiCard.types.ts`:加 `ButtonPrimitive` 接口(`{type:"button", action, label?, payload?}`)。
- [ ] **3.3** 新 `components/chat/primitives/ButtonPrimitive.vue`:渲染按钮(`label`),点击按 `action` 分发 —— `apply_diff` → `applyUiDiff(sessionId, payload.diff_text)`(复用 2.1);`copy` → `navigator.clipboard.writeText(payload.text)` + 反馈;`dismiss` → 本地 `ref` 隐藏。
- [ ] **3.4** `uiPrimitiveRegistry.ts`:`button` → `ButtonPrimitive`。
- [ ] **3.5 验证**:
  ```bash
  cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib use_ui
  cd app && pnpm test && pnpm build
  ```

## 阶段 4 — spec / 文档(收尾)

- [ ] **4.1** `.trellis/spec/backend/tool-contract.md`:`use_ui` 段补 button type + 新 `apply_ui_diff` IPC 契约(design §2.3)+ UiDiffApplied 审计。
- [ ] **4.2** `.trellis/spec/frontend/chat.md`:UiCard 段补 ButtonPrimitive + DiffPrimitive apply + raw fallback 禁用门。
- [ ] **4.3** ROADMAP §1.2 加 B9+ 条目 + §2 第三档 B9+ 标 ✅;IMPLEMENTATION §4 决策日志加 B9+ ADR(D-Q1~Q4 + 手写 hunk apply 决策)。

## 测试矩阵 + UX 细节(补充)

### 后端单测
**`diff_apply.rs`(阶段 1.2,命门)**:
- parse:标准 unified diff 多文件多 hunk / 无路径头 raw 片段 → `ParseError` / 空 diff → empty / 损坏 `@@` 头
- apply:context 精确匹配成功 / context 任一行不匹配 → `Conflict` fail-fast / 多 hunk 累积行号偏移正确 / 纯新增行 / 纯删除行 / hunk 起始行号定位

**`commands::ui::apply_ui_diff`(阶段 1.3)**:
- boundary 越界(`/etc/...`)→ `kind=boundary`,不写
- 无路径头 diff → `kind=parse`
- context 冲突 → `kind=conflict`,全失败不部分写
- 成功 → 文件写入 + `record_ui_diff_applied` 落表(查 DB 验)+ 返 files 列表
- 无 worktree session → fallback project root 仍可写

**`tools::use_ui`(阶段 3.1)**:
- `KNOWN_TYPES` + schema enum 含 `button`(`definition_schema_type_enum_matches_known_types` 守卫)
- button + `action ∈ {apply_diff,copy,dismiss}` 通过;未知 action → 错误
- description 含 button 行为边界(design §7 反模式提示)

### 前端 vitest
**`DiffPrimitive`(阶段 2.2)**:
- raw fallback(无 `---`/`+++` 头)→ 应用按钮 disabled + tooltip
- 标准 unified diff → 应用按钮可点;点应用 → 调 `applyUiDiff`;成功 → "已应用"禁用;失败 → inline kind 文案
**`ButtonPrimitive`(阶段 3.3)**:
- `apply_diff` → 调 `applyUiDiff`;`copy` → `clipboard.writeText`;`dismiss` → card 隐藏

### UX 细节
- **应用成功**:toast「已应用 N 个文件」+ card 按钮变「已应用」(disabled)。多文件 diff 一次应用全部(返 files 列表)。
- **失败**:inline 错误留在 card(不 toast,便于查看),不自动重试;用户改 diff 后可重点。
- **raw fallback 禁用门**:应用按钮 `disabled` + tooltip「该 diff 格式不可应用(需带路径头的标准 unified diff)」。
- **kind 中文文案**:boundary=路径越界 / parse=需标准 unified diff / conflict=文件已变,context 不匹配 / io=写入失败 / empty=空 diff。

## 全局验收(design AC)
```bash
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo fmt --check
cd app && pnpm test && pnpm build
```
对照 prd.md Acceptance Criteria 逐条勾。

## 风险文件 / 回滚点
- **`src/diff_apply.rs`(新,阶段 1.2)= 命门**:手写 hunk apply 必须单测覆盖 conflict/boundary/多 hunk 再进阶段 2。回滚 = 删模块 + 注释 IPC 注册。
- `use_ui.rs` schema 加 button 是**增量**,旧 primitive 零影响,无回滚风险。
- `apply_ui_diff` IPC 独立,注释 `generate_handler!` 注册行即回滚到"只读展示"。
- 审计 enum 增量,无 DB migration,无回滚风险。

## task.py start 前的 follow-up
- [ ] 用户审查 prd.md / design.md / implement.md 通过
- [ ] **inline workflow**(单 task 分阶段,不 dispatch subagent)→ 跳过 implement.jsonl / check.jsonl gate(Phase 2 走 `trellis-before-dev` 加载 spec)
- [ ] 确认 child 不拆(design §6);如改主意拆 child,回到 prd Subtask Map 调整
