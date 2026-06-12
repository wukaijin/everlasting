# A2 + B7 权限系统 + 多模式 — 计划审查报告

> **审查日期**: 2026-06-13
> **审查范围**: `.trellis/tasks/06-12-a2-b7-permission-and-mode/` 全量文档
> **审查类型**: 设计评审 (pre-implementation review)
> **审查基线**: PRD 8 决策 + 3 PR 计划 + 30 条 AC + ADR + PermissionModal UX Spec + 4 份调研

---

## 0. 总体评价

**整体评分: ★★★★ (4/5) — 计划扎实，有 4 个需在实施前解决的关键问题**

这是一份组织出色的设计文档包。4 份调研覆盖了 Claude Code / OpenHands / Aider / Cursor / Continue.dev 共 5 个主流 coding agent 的权限系统和模式切换设计，横向对比深度足够。PRD 的 3 PR 拆分合理，AC 覆盖全面，PermissionModal UX Spec（PR3 子 spec）达到可直接实施的质量。现有代码模板（`model_id` per-session binding、`ConfirmDialog` modal 规范、`SettingsModal` reka-ui 使用先例）被充分识别和复用。

**但以下 4 个问题需要在 PR1 开始前解决**，否则会导致实施阶段的返工或安全漏洞：

---

## 1. 致命问题：⑨ 关 5 道 check 顺序不一致 (BLOCKER)

**严重程度: P0 — 必须在实施前统一**

PRD 内部存在 **三套不同的 ⑨ 关检查顺序**：

| 来源 | 顺序 |
|---|---|
| PRD §Assumptions (L42) | 静态白名单 → 路径 → 参数schema → 危险模式 → 用户确认 |
| PRD §Requirements (L64-71) | Hooks → **Deny** → **Ask** → **Mode** → Allow |
| yolo-safety research §6 | Static allowlist → Hard kill list → Path → Schema → Mode check → Permission policy(deny → ask → mode=yolo → first-time → allow) → Audit |

**具体矛盾**:

1. **Deny 的位置**: PRD Requirements 把 Deny（硬 kill list）放在 Tier 2（Ask 之前），但 Assumptions 把"危险模式"放在第 4 道（用户确认之前）。哪个在前？如果 Deny 在 Ask 之前，那 Yolo 模式下 `rm -rf /` 是直接拒绝（不弹窗）还是弹"危险操作仍要确认"的 modal？

2. **Mode 的位置**: PRD Requirements 把 Mode 放在 Ask 之后（Tier 4），但 yolo-safety research 把 Mode check 放在 Permission policy 之前（作为独立步骤 5）。如果 Plan 模式在 Ask 之前拦截（runtime return text），那根本不会触发 Ask——这跟 PRD 的 5 道顺序冲突。

3. **6 道 vs 5 道 vs 7 道**: 不同文档的检查步骤数量不一致，因为有些步骤被合并/展开。

4. **Yolo 下 deny 的行为**: PRD AC 说 "Yolo 模式 session, LLM 调 `shell` 跑 `rm -rf /tmp/foo` → **仍然被拒**"，但 yolo-safety research §5.4 说 "denylist 命中 → 弹'危险操作在 Yolo 模式下仍要确认'"。是**静默拒绝**还是**弹窗确认**？这两个行为对 UX 影响巨大。

**建议**: 在 PRD 顶部追加一份 **唯一的 source-of-truth 检查顺序**，建议采用 Claude Code 风格的评估顺序（permissions.md 原文: Hooks → Deny rules → Ask rules → Permission mode → Allow rules → canUseTool），并明确每一层的行为：

```
1. Hooks (pre-call 接口,留空给后期扩展)
2. Deny rules — 硬拒绝列表,命中直接返回 is_error:true,不弹窗
   Yolo 模式下也走这步,不弹窗,静默拒绝
3. Ask rules — 检查 session_tool_permissions,有"始终允许" → 跳过弹窗;
   无 → emit permission:ask,等用户响应
4. Mode check — Plan/Review 模式拦截:不在此模式的 tool → 返回 text 错误
5. Allow rules — 默认所有 tool 在白名单(后期可收缩)
6. Audit hook — 记录决策
```

---

## 2. 重要遗漏：缺少 Per-Mode Tool List 过滤 (P1)

**严重程度: P1 — 浪费 LLM token + 用户体验差**

调研文档 `mode-state-machine.md` Q3 明确推荐 **Design B 混合方案**（system prompt + tool list 过滤 + ⑧a runtime），但 PRD §Requirements 只描述了 ⑧a runtime intercept：

> "如果 `mode ∈ {Plan, Review}` 且 LLM 返回 `tool_use`,改返回 text 'I cannot execute in this mode'"

**问题**: 如果不在 Plan 模式下从 tool list 中移除 `write_file` / `edit_file` / `shell`，LLM 会：
- 在第一个 turn 尝试调用 write_file
- 收到 "I cannot execute in plan mode" 错误
- 浪费一个 turn（对大模型而言成本高昂）
- Claude Code / Cursor / Continue.dev **全都**做 tool list 过滤，这是行业标准

**建议**: 在 PRD Requirements 中补充：

```
⑧a Mode 检查 = 三层防御:
  a. Per-turn 构造 system prompt 时注入 mode 前缀
     (Plan: "你处于 Plan 模式,只能分析并提出方案,不能执行")
  b. Per-turn 构造 tool list 时过滤:
     Plan/Review → 移除 write_file/edit_file/shell
     Chat/Background/Yolo → 全量 tool list
  c. ⑧a runtime: Plan/Review 下 LLM 仍发 tool_use → 返回 text error(兜底)
```

**实现影响**: 这主要影响 `agent/chat.rs` 的 `build_instructions_blocks()` 和 tool list 构造逻辑，工作量约 +30 行，不需要新模块。

---

## 3. 中等问题 (P2)

### 3.1 Background Mode 规格空缺

Background 出现在 5 个 Mode 中，但在 PRD 里没有任何具体行为描述：
- ⑧a 行为未定义（research 说 "同 Chat"）
- ⑨ 行为未定义
- "完成时通知" 机制未定义
- UI badge 样式未定义

**建议**: 要么在 MVP 中移除 Background（留后续 PR），要么补充完整的 mode 行为矩阵。当前状态下 implement agent 会不知道该做什么。

### 3.2 IPC 超时与异常路径

`permission:ask` → `permission_response` 的 IPC 流缺少以下异常处理：

| 异常场景 | 当前处理 | 风险 |
|---|---|---|
| 用户从不响应 | 无超时 | Agent loop 永久阻塞 |
| 重复 `permission_response` | 无去重 | 可能唤醒错误的 future |
| Session 在等待时被删除 | 无清理 | 僵尸 future |
| `rid` 过期/无效 | 无校验 | 未定义行为 |

**建议**: 在 PRD §Technical Notes 补充：
- `permission:ask` 超时: 120s 后自动 deny（等同超时 = 安全失败）
- `rid` 去重: 后端用 `HashMap<Rid, oneshot::Sender>` 存 pending，收到 response 后 remove并校验 key 存在
- Session 删除时: 给该 session 的所有 pending permission future 发 `Cancel`(与 C1 cancel 复用同一 CancellationToken)

### 3.3 Yolo Root Check 的时机不明确

PRD 说 "启动时检测 root"，但 Yolo 可以 mid-session 切换。应该在 `set_session_mode('yolo')` 时做检查，而不是启动时。此外：

- `nix` crate 依赖 vs `unsafe libc::geteuid()`：为单一检查引入 nix crate 过度。建议用 `#[cfg(target_family = "unix")]` + `unsafe { libc::geteuid() }`（3 行代码，已在 Rust std 生态广泛使用）
- Windows 上 root 检查语义不同，需要 `#[cfg]` 分支

### 3.4 审计日志表 Schema 不完整

`session_audit_events(session_id, ts, kind, payload_json)` — `kind` 枚举值未定义。至少需要：

```
kind ∈ {
  "tool_denied",       // ⑨ 关拒绝
  "tool_allowed",      // ⑨ 关放行
  "tool_permission_ask", // 弹窗询问
  "permission_granted",  // 用户选"始终允许"
  "mode_changed",      // Mode 切换
  "yolo_entered",      // 进入 Yolo
  "yolo_exited",       // 退出 Yolo
  "request_cancelled"  // C1 cancel
}
```

### 3.5 Test Strategy 的 Tauri IPC 测试缺口

PRD 说 "每条 AC 至少 1 个端到端 case(cargo test + vitest)"，但 `permission:ask` → `permission_response` IPC 的端到端测试需要 Tauri 运行环境。当前 vitest 配置无法 mock Tauri event system。

**建议**: 
- `permissions.rs` 模块的 5 道 check 纯函数单元测试 — `cargo test`（无问题）
- IPC 集成测试: 手动 smoke test checklist 或使用 `tauri::test` mock（如果 Tauri 2 支持）

---

## 4. 轻微问题 (P3)

### 4.1 两个颜色 Token 细节冲突

- PermissionModal spec 说 `border-left: 3px solid var(--color-tool-error)` 但 design-tokens.md 约定 "Border width is always 1px"
- Icon 容器: research Q3 说 24x24，PRD UX Spec 最终说 56x56px — 后者是最终决定但调研文档未更新（无伤大雅）
- `var(--color-tool-error)` 等 token 需确认在 design-tokens.md 中已定义

### 4.2 ChatInput Flex 布局重构风险

PRD 要求 `ChatInput.vue` 改 flex 布局：左侧 `<ModeSelect>`,右侧保留输入框 + 发送按钮。但当前 ChatInput 可能还有其他元素（worktree badge / model badge / project badge）。需确认布局不会太拥挤。

### 4.3 Session Deletion Cascade

PRD AC 说 "删除 session 时 cascade 清 `session_tool_permissions` + `session_audit_events`"。SQLite 默认不启用外键约束（需要 `PRAGMA foreign_keys = ON`）。需确认 migration 或连接初始化时已启用。

### 4.4 Shift+Tab 快捷键的浏览器冲突

Shift+Tab 是浏览器的默认"反向 focus"行为。PRD 提到 "需要在 capture phase preventDefault"，但 `useKeyboard` 模块的实现细节（如何接入 Vue lifecycle、如何与 reka-ui Dialog 的 focus trap 共存）需要明确。

### 4.5 Research 文档中的陈旧信息

`agent-permission-best-practice.md` §5.3 持久化方案建议 `allowed_tools TEXT NOT NULL DEFAULT '[]'` 作为 sessions 表的 JSON 列，但 PRD 最终决定用独立的 `session_tool_permissions` 表。这个不一致不会影响实施（implement agent 应以 PRD 为准），但会在阅读时造成困惑。

---

## 5. 3 PR 拆分评估

| PR | 范围 | 评估 | 风险 |
|---|---|---|---|
| **PR1**: 后端基础设施 + Mode 持久化 + ⑨ 关 5 道 + ⑧a | 合理 | **中** — ⑨ 关 5 道顺序需先统一 | 如果 5 道顺序返工，会影响 `permissions.rs` 模块结构 |
| **PR2**: 前端 ModeSelect + ChatInput + useKeyboard + YoloConfirmModal | 合理 | **低** — 纯前端，依赖 PR1 的 DB + IPC | 如果 ChatInput 布局改坏，影响所有对话 |
| **PR3**: 前端 PermissionModal + IPC 联通 + 端到端测试 + spec 同步 | 合理 | **中** — IPC 联通是首次"前端等后端"的模式 | IPC 超时 + 异常路径需在 PR1 就预留接口，不能在 PR3 补 |

**额外建议**: 在 PR1 和 PR2 之间插入一个 **PR1.5: 端到端 smoke test**（手动）验证：
1. Mode 切换 → DB 持久化 → 重启后 mode 保留
2. Plan 模式下 LLM 确实不能执行 write_file
3. Chat 模式下 shell 执行正常

这能尽早暴露 IPC 问题和 mode 行为 bug，避免 PR3 时积压。

---

## 6. PermissionModal UX Spec 专项审查

PR3 的子 spec（已嵌入 PRD §PermissionModal UX Spec）质量很高。以下为针对性反馈：

### 6.1 做对的事 ✅

- Center modal 选择正确（vs inline/底部卡片）
- 3-button 布局 ("拒绝 / 仅一次 / 始终允许") 符合 Claude Code 行业标准
- `max-height: 240px` + `overflow: auto` 处理长命令
- Copy icon + terminal icon 的添加是好的 UX 细节
- `:key="pendingPermission.rid"` 触发重新 mount 处理 multi-tool_use
- reka-ui `:deep()` 警告到位
- Enter → "仅一次" (默认 focus) 是最合理的键盘默认
- Esc/X/遮罩 = Deny 符合 Claude Desktop / Cursor / Continue.dev 事实标准
- 移除 "本次会话记住此选择" checkbox 的决策正确（与"始终允许"按钮功能重复）

### 6.2 可改进的细节

1. **Risk 标签中英文**: `low` → "低" 的映射在第 227 行定义为 "低/中/高/极高"，但紧接着第 193 行的 Title 文本用的是中文自然语言。建议统一 risk 标签只用中文或只用英文，避免混用。

2. **`critical` risk 时的 "拒绝" 默认暗示**: spec 说 "视觉上要暗示'拒绝'是默认"，但当前 3 按钮等宽 33%、"仅一次" 是 Enter 默认 focus。这两者矛盾——如果 Enter 触发 "仅一次" 但视觉暗示 "拒绝"，用户会困惑。建议 critical 时改 Enter 默认 focus 到 "拒绝"。

3. **Copy toast "已复制" 2s**: 实现上可以用 `<Transition>` 或 CSS animation。需确认 toast 不跟 modal 的 z-index 冲突（toast 应在 modal 之上 = z-index 10000）。

---

## 7. 跟现有代码的对齐检查

| PRD 假设 | 实际代码状态 | 对齐? |
|---|---|---|
| `execute_tool` 在 `tools/mod.rs:95` | ✅ 准确 — `mod.rs:95` 是 `pub async fn execute_tool` | ✅ |
| `agent/chat.rs:971` 调用 `execute_tool`，无 ⑨ 关 | ✅ 准确 — `chat.rs:971` 直接调 `execute_tool` | ✅ |
| `model_id` 作为 per-session 绑定模板 | ✅ `db/sessions.rs:304-329` 有 `update_session_model_id` | ✅ |
| `ModelSelect.vue` 作为 popover 模板 | ✅ 已确认存在 | ✅ |
| C1 cancel 用 `tokio::select!` 包 `execute_tool` | ✅ `tools/mod.rs:107` 有 `tokio::select!` | ✅ |
| boundary check 在 `chat.rs:279` | 需确认 — 路径 279 可能不是 `assert_within_root` 调用 | ⚠️ 轻微偏差 |
| `nix crate` 用于 root check | 当前 Cargo.toml **无** nix 依赖 | ⚠️ 需确认是否引入 |
| design-tokens 有 `--color-tool-*` token | 需确认 token 是否完整定义 | ⚠️ 需在实施时验证 |

---

## 8. 建议行动清单

### 实施前必须完成 (P0)

- [ ] **统一 ⑨ 关 5 道 check 顺序** — 在 PRD 顶部追加唯一的 source-of-truth 顺序定义（建议: Hooks → Deny → Ask → Mode → Allow）
- [ ] **明确 Yolo 下 deny 命中行为** — 是静默拒绝还是弹窗确认？建议: 静默拒绝（`is_error: true`），保持 Yolo 的 "无弹窗" 承诺但拒绝灾难性命令

### 实施前强烈建议完成 (P1)

- [ ] **补充 Per-Mode Tool List 过滤** 到 PRD Requirements（Plan/Review 模式移除 write 类 tool）
- [ ] **决定 Background Mode 去留** — 移除或定义完整行为
- [ ] **定义 IPC 异常路径** — 超时(120s)、重复 rid、session 删除、无效 rid

### 实施前建议完成 (P2)

- [ ] **定义审计日志 `kind` 枚举** — 至少 7 个事件类型
- [ ] **确认 root check 实现方式** — `unsafe libc::geteuid()` vs `nix` crate
- [ ] **确认 design-tokens.md 中 `--color-tool-*` token 完整性**
- [ ] **验证 `chat.rs:279` 的 boundary check 实际位置**

### 实施中注意 (P3)

- [ ] SQLite `PRAGMA foreign_keys = ON` 需在连接初始化时确认
- [ ] `Shift+Tab` 的 preventDefault 需在 capture phase 处理
- [ ] `useKeyboard` 模块的 Vue lifecycle 集成方式需在 PR2 开始时明确
- [ ] PermissionModal critical risk 时的 Enter 默认 focus 建议改为 "拒绝"

---

## 9. 文档质量

| 文档 | 行数(约) | 质量 |
|---|---|---|
| prd.md | 395 | ★★★★½ — 结构完整，AC 覆盖全面，ADR 决策清晰。建议追加统一的 ⑨ 关顺序定义 |
| research/agent-permission-best-practice.md | 506 | ★★★★★ — 横向对比深度足够，take-away 可操作。§5.3 持久化建议略陈旧 |
| research/mode-state-machine.md | 218 | ★★★★★ — 复用 `model_id` 模板的论证最精彩，Q2/Q3/Q4 决策扎实 |
| research/yolo-safety-design.md | 219 | ★★★★★ — 4 件套论证 + 外部来源引用详尽，安全审计级别 |
| research/permission-modal-ux.md | 759 | ★★★★★ — 可直接实施的 spec，8 个 UX 决策 + 13 条 AC，reka-ui 警告到位 |

---

## 10. 结论

**计划可以进入实施阶段**，前提是 **§1 的 ⑨ 关顺序不一致问题先解决**。这是唯一可能引发返工的 blocker。

其余问题（§2 tool list 过滤遗漏、§3 IPC 异常路径）可以在 PR1 过程中增量补充，但建议在 PR1 开始前在 PRD 中明确，避免 implement agent 做假设。

方案的整体安全设计（Deny 优先于 Yolo、root 拒绝、audit hook、二次确认）对标 Claude Code 标准，对于自研 harness 项目而言是合适的严格程度。
