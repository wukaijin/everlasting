# TaskStatus 容纳 plugin 自定义 state — 方案分析

> 触发：C2/C3 评审期间发现 review 的 4 state（intake/reviewing/revising/reported）存不进 task.json.status（TaskStatus enum 硬编码 4 值，未知值 fallback Planning）。
> 这是 review epic 的架构级前置，比 C1（resume）更优先。
> 本文档调研事实 + 三方案对比，供拍板。

## 0. 调研事实

### TaskStatus enum 现状（task.rs:120-149）

```rust
pub enum TaskStatus { Planning, InProgress, Done, Completed }  // 4 硬编码值

impl TaskStatus {
    fn from_str_opt(s: &str) -> Self {
        match s {
            "in_progress" => InProgress,
            "implement" | "check" => InProgress,   // legacy
            "done" => Done,
            "completed" => Completed,
            _ => Planning,                          // ← review 的 intake/reviewing/... 全部落到这
        }
    }
    fn as_str(self) -> &'static str { ... }
}
```

### 关键发现：dev 的 state == task.json.status

- `task.rs:87-94` 注释明说：「WorkflowDef::states (planning/in_progress/done)」即 task.json 的 status 字段。
- 即 dev plugin 的 3 state 与 TaskStatus 的 3 值（Planning/InProgress/Done）**一一对应，是同一个东西**。
- task.json 没有独立的「workflow session state」字段——status 字段身兼两职（task 生命周期 + workflow state）。

### 消费点分布（grep 统计）

| 文件 | TaskStatus 用法数 | 性质 |
|---|---|---|
| state.rs | 39 | `set_task_state` 钩子按 `(from, to)` 穷举 match（InProgress→Done 触发 spec_distillation 等） |
| task.rs | 37 | enum 定义 + 序列化 + 边界 |
| request_task_state_transition.rs | 11 | transition 工具，校验 from/to 合法性 |
| update_checklist.rs | 10 | 按 status 决定能否改 items |
| inject.rs | 9 | workflow ctx 的 current_task.status + breadcrumb 注入 |
| dispatch.rs | 6 | **role gate：`status.as_str()` 查 `roles_by_state[state]`**（line 1630） |
| mod.rs | 4 | 测试 |
| create_task.rs | 1 | 新建 task 默认 Planning |

### 关键消费点的耦合方式

1. **role gate（dispatch.rs:1630）**：`let state = status.as_str(); allowed_roles(workflow_def, state)` —— **靠字符串查 map**，只要 status 能存任意字符串并 as_str() 回来就工作。review 的 `roles_by_state["reviewing"]` 能命中。
2. **set_task_state 钩子（state.rs:266）**：`match (from, to) { (InProgress, Done) => spec_distillation, (Planning, InProgress) => preflight, ... }` —— **穷举 match**，未知组合走默认（不触发钩子）。review 的 (intake→reviewing) 等不在枚举里会编译失败或 match 不中。
3. **transition 校验（request_task_state_transition.rs）**：校验 from/to 是合法 TaskStatus —— review 的 state 会被拒。

## 1. 方案对比

### 方案 X：TaskStatus 改 String（或 enum + Custom(String)）

**做法**：`task.json.status` 改成纯字符串，TaskStatus enum 降级为「已知 dev 状态的常量」+ 兜底 Custom(String)。

```rust
pub enum TaskStatus {
    Planning, InProgress, Done, Completed,    // dev 用
    Custom(String),                            // review 等 plugin 用
}
// from_str_opt: 已知值 match，未知 → Custom(s)
// as_str: Custom(s) → s
```

| 维度 | 评估 |
|---|---|
| role gate | ✅ 完美工作（as_str 返回任意字符串，查 map 命中） |
| set_task_state 钩子 | ⚠️ match 要加 `_ => {}` 兜底（review 的 transition 不触发 spec_distillation，正确）；但失去穷举安全性 |
| transition 校验 | ✅ 改成校验「state ∈ workflow_def.states」而非「是已知 TaskStatus」——这其实更对（transition 合法性应由 plugin 定义） |
| 改动面 | 🟡 中。TaskStatus enum + from_str/as_str + set_task_state match + transition 校验 + 所有 match 点加 Custom 分支 |
| 风险 | dev 的现有行为完全保留（Custom 只对 review 等新 plugin 触发）；但 set_task_state 的穷举 match 失去编译期完整性检查 |
| 类型安全 | 🟡 降。Custom(String) 是黑盒，编译期无法穷举 review 的 state |

### 方案 Y：task.json 加 workflow_state 字段（与 status 并存）

**做法**：task.json 加 `workflow_state: String` 字段。`status` 仍走 dev 的 4 值（task 生命周期），`workflow_state` 存 plugin 当前 state。

- dev plugin：status 和 workflow_state 同步（planning↔planning, in_progress↔in_progress, done↔done）—— 两个字段冗余但一致。
- review plugin：status 保持 Planning（review 不改 task 生命周期），workflow_state 存 intake/reviewing/revising/reported。

| 维度 | 评估 |
|---|---|
| role gate | ⚠️ 要改：`state = workflow_state.or(status.as_str())`（优先 workflow_state，回退 status）—— 或 role gate 统一改读 workflow_state |
| set_task_state 钩子 | ✅ status 钩子完全不动（仍按 dev 4 值 match）；workflow_state 的 transition 另走逻辑 |
| transition 校验 | 🟡 要分两套：status transition（dev 4 值）+ workflow_state transition（plugin states） |
| 改动面 | 🟡 中。task.json schema 加字段 + role gate 读哪个 + transition 工具改 + WorkflowCtx 暴露 workflow_state |
| 风险 | dev 行为保留（status 不动）；但两个字段的同步/优先级容易混乱（dev 时两者要同步，review 时 status 冻结） |
| 语义清晰度 | 🟡 中。status 和 workflow_state 的职责区分要文档化，否则后人困惑 |

### 方案 Z（补充）：方案 X 的变体 —— status 改 String，dev 状态做约定

**做法**：`task.json.status` 直接改 `String`（不要 enum）。dev 的 planning/in_progress/done 成为「约定字符串」，review 用自己的字符串。

| 维度 | 评估 |
|---|---|
| role gate | ✅ 完美（status 本就是 String，查 map） |
| set_task_state 钩子 | 🔴 破坏大：match (from, to) 全改成字符串 match，失去类型安全；spec_distillation 的 (InProgress→Done) 触发要改成 `("in_progress", "done")` 字符串 match |
| 改动面 | 🔴 大。所有 TaskStatus::X 用法改成字符串；类型安全全面丧失 |
| 风险 | 高。打字错误编译期不报 |

## 2. 我的推荐：方案 X（enum + Custom(String)）

理由：

1. **role gate 零改动**。review 的 role gate 靠 `status.as_str()` 查 map（dispatch.rs:1630），Custom(s).as_str() 返回 s，`roles_by_state["reviewing"]` 直接命中。这是 review 能工作的最关键链路，方案 X 对它零侵入。

2. **set_task_state 钩子语义正确**。review 的 transition（intake→reviewing 等）**本就不该触发** dev 的 spec_distillation（那是 dev InProgress→Done 的专属）。方案 X 的 `_ => {}` 兜底恰好实现「review transition 不触发 dev 钩子」，语义正确。如果未来 review 要自己的钩子，在 match 加 review 专属分支即可。

3. **transition 校验更合理**。现状校验「from/to 是已知 TaskStatus」——这其实不对，transition 合法性应由 plugin 的 workflow.json 定义（`can_transition(def, from, to)`，def.rs:306 已有此函数）。方案 X 把校验从「全局 enum」改成「plugin def」，更符合 plugin 架构。

4. **dev 零回归**。Custom 只对非 dev 状态触发，dev 的 Planning/InProgress/Done/Completed 行为完全不变。

5. **比 Y 简单**。Y 要维护两个字段的同步和优先级（dev 时同步、review 时 status 冻结），语义复杂；X 是单字段扩展，语义统一。

**方案 X 的唯一代价**：set_task_state 的 match 失去穷举安全性（要加 `_` 分支）。但这个代价可接受——review 的钩子本就该和 dev 不同，穷举 match 反而是过度耦合。

## 3. 方案 X 的实施轮廓（若采纳，作为 C0 子任务）

### 改动文件
- `task.rs`：TaskStatus enum 加 `Custom(String)` 变体；`from_str_opt` 未知值 → Custom(s)；`as_str` Custom(s) → s；序列化 Custom(s) → 字符串原值。
- `state.rs`：set_task_state 的 `match (from, to)` 加 `_ => {}` 兜底（review transition 不触发钩子）；或显式列 review 分支（若 review 需要钩子）。
- `request_task_state_transition.rs`：transition 合法性校验从「是已知 TaskStatus」改成「`can_transition(workflow_def, from, to)`」（用 def.rs:306 现有函数）。
- 其余 match 点（update_checklist 等）：加 Custom 分支或 `_` 兜底。

### 单测
- Custom("reviewing") round-trip（写进 task.json 读回）
- role gate 用 Custom state 查 roles_by_state 命中
- set_task_state 的 Custom transition 不触发 dev 钩子
- transition 校验用 can_transition（review 的 intake→reviewing 合法）

### 风险
- TaskStatus 序列化兼容：旧 task.json 的 "planning" 等仍能读回（from_str_opt 已知值优先 match）；新增 Custom 不影响旧数据。
- set_task_state 钩子的穷举 match 改兜底：要 audit 所有 match 点确保 `_` 分支语义正确（不静默吞 dev 的合法 transition）。

## 4. 待拍板

**选方案 X（推荐）/ Y / Z？** 选定后我新建 C0 子任务（TaskStatus 容纳自定义 state），调整 epic 依赖为 C0 → C1 → C3 → C2。
