# Design: TaskStatus accommodate custom plugin state (C0)

> 父任务：`07-26-workflow-review-plugin`
> 配套：`prd.md` + `../research/taskstatus-custom-state.md`（X/Y/Z 方案对比，采纳 X）
> 本 design 落定 PRD Open Questions（Custom 序列化 / 钩子兜底 / transition 校验 / 非 workflow session fallback）。

## 0. 调研事实（决策依据）

| 事实 | 位置 | 对方案 X 的影响 |
|---|---|---|
| `TaskStatus` 只 derive `Serialize`，**无 Deserialize derive** | `task.rs:120` | Custom 的序列化只需自定义 Serialize；反序列化复用 from_str_opt |
| 手动 `Deserialize` impl 调 `from_str_opt`（未知值 → Planning） | `task.rs:166` | 改 from_str_opt 即可，Deserialize impl 不动 |
| `read_task` 用 `serde_json::from_slice`（走手动 Deserialize） | `task.rs:365` | 自动复用新 from_str_opt |
| `from_str_opt` 未知值 fallback Planning | `task.rs:139` | 改成 Custom(s) 即解决塌陷 |
| `parse_target_state` 未知值返回 Err(InvalidTargetState) | `state.rs:150` | transition 校验拒绝 review state 的根因，R3 要改 |
| `dispatch_hook` 的 match 已有 `_ => {}` 兜底 + debug 日志 | `state.rs:273` | **R2 几乎不改**，Custom transition 落兜底正好语义正确 |
| `can_transition(def, from, to)` 按字符串查 transitions | `def.rs:306` | R3 复用，transition 合法性由 plugin def 决定 |
| `TaskStatus` derive `Copy` | `task.rs:120` | **Custom(String) 含 String 不能 Copy，derive Copy 要去掉** |
| role gate 用 `status.as_str()` 查 `roles_by_state` | `dispatch.rs:1630` | Custom(s).as_str()=s 直接命中，零改动 |

## 1. 方案 X 落地：Custom(String) 变体

### 1.1 enum 改动（task.rs:120）

```rust
// 去掉 Copy（Custom 含 String 不能 Copy）
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Planning,
    InProgress,
    Done,
    Completed,
    /// Plugin 自定义 state（如 review 的 intake/reviewing/revising/reported）。
    /// 非已知值经 from_str_opt 落到这里,不再塌成 Planning。
    /// as_str 返回原字符串;Serialize 输出原字符串。
    Custom(String),
}
```

**Serialize**：现有 `#[derive(Serialize)]` + `rename_all = snake_case` 对前 4 个变体生效；Custom(String) 需自定义（derive 的 Serialize 对带数据的变体输出 `{"Custom": "reviewing"}` 形态，不是我们要的纯字符串 `"reviewing"`）。

解决：去掉 derive Serialize，手动 impl Serialize（前 4 个输出 snake_case 字符串，Custom(s) 输出 s）：
```rust
impl serde::Serialize for TaskStatus {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where S: serde::Serializer {
        serializer.serialize_str(self.as_str())
    }
}
```
（与现有手动 Deserialize 对称,且 `as_str` 已是单一真相源。）

### 1.2 from_str_opt（task.rs:130）— 核心修复

```rust
pub fn from_str_opt(s: &str) -> Self {
    match s.trim().to_ascii_lowercase().as_str() {
        "planning" => Self::Planning,
        "in_progress" => Self::InProgress,
        "implement" | "check" => Self::InProgress,   // legacy
        "done" => Self::Done,
        "completed" => Self::Completed,
        other => Self::Custom(other.to_string()),    // 不再 fallback Planning
    }
}
```

**关键**：`other` 用 trim/lowercase 后的值（保持与已知值一致的大小写处理）。若 plugin 用大写 state（如 `Reviewing`），存进去是 `reviewing`（小写化）。review 的 workflow.json 用小写 state（intake/reviewing/...），无影响。

### 1.3 as_str（task.rs:143）

```rust
pub fn as_str(&self) -> &str {  // 返回 &str 不再是 &'static str（Custom 借用内部 String）
    match self {
        Self::Planning => "planning",
        Self::InProgress => "in_progress",
        Self::Done => "done",
        Self::Completed => "completed",
        Self::Custom(s) => s,
    }
}
```

**签名变化**：`&'static str` → `&str`（Custom 借用内部 String）。所有 `as_str()` 调用点编译器会提示,但语义不变（拿到字符串切片）。需 audit 调用点是否有依赖 `'static` 的（如赋值给 `&'static str` 字段）—— grep 确认现有调用都是临时使用,无 'static 依赖。

## 2. parse_target_state 改动（state.rs:139，R3）

```rust
pub fn parse_target_state(s: &str) -> StateResult<TaskStatus> {
    match s.trim().to_ascii_lowercase().as_str() {
        "planning" => Ok(TaskStatus::Planning),
        "in_progress" => Ok(TaskStatus::InProgress),
        "implement" | "check" => Ok(TaskStatus::InProgress),  // legacy
        "done" => Ok(TaskStatus::Done),
        other => Ok(TaskStatus::Custom(other.to_string())),  // 不再 Err
    }
}
```

**变化**：未知值返回 `Ok(Custom(s))` 而非 `Err(InvalidTargetState)`。transition 合法性改由 `can_transition(workflow_def, from, to)` 校验（见 §3）。

## 3. transition 合法性校验（request_task_state_transition.rs，R3）

现状：`parse_target_state` 报错拒绝未知 state。改后：parse 不报错,合法性由 plugin def 的 `can_transition` 校验。

**改动点**（request_task_state_transition.rs，validate + execute_blocking）：
- validate 阶段：不再因 target_state 非已知 TaskStatus 而拒（parse_target_state 已放行）。
- execute_blocking 阶段：拿到 from/to 后,若在 workflow session,用 `can_transition(workflow_def, from_str, to_str)` 校验 transition 是否在 plugin 的 transitions 里；不合法返回错误。

### R3 的 Open Question 3（非 workflow session fallback）

PRD OQ3：非 workflow session（无 workflow_def）的 transition 怎么校验？

**决策**：非 workflow session 时,回退到旧逻辑——校验 from/to 是已知 TaskStatus（Planning/InProgress/Done/Completed）。理由：
- 非 workflow session 意味着没有 plugin 定义 transitions,Custom state 无依据。
- 旧逻辑（task 生命周期 planning→in_progress→done）对非 workflow session 仍合理。
- 即:`if workflow_def.is_some() { can_transition } else { 旧 enum 校验 }`。

## 4. Copy 移除的影响 audit

去掉 `Copy` 后,所有按值传递 TaskStatus 的地方要改 `.clone()` 或借用。grep 调用点：

| 文件 | 用法 | 处理 |
|---|---|---|
| dispatch.rs:1630 | `status.as_str()`（借用,不 copy） | 无影响 |
| inject.rs | `t.status.as_str()` / `task.status.as_str()` | 无影响 |
| state.rs | `match (from, to)`（dispatch_hook 参数） | 参数改 `&TaskStatus` 或调用方 clone |
| chat_loop.rs:3569 | `Some(t.status)`（拷贝进 tuple） | 改 `Some(t.status.clone())` |
| request_task_state_transition.rs | `(current_state, current_slug)` 传值 | clone |
| update_checklist.rs | match status | 按需 clone 或借用 |

**audit 原则**：编译器会逐一报错,按报错改即可。优先 `&TaskStatus` 借用（零成本）,无法借用时 clone（TaskStatus 小,clone 便宜）。

## 5. dispatch_hook 兜底（R2，几乎不改）

state.rs:273 已有 `_ => { tracing::debug!(...) }` 兜底。Custom transition（intake→reviewing 等）落这里,正好「不触发 dev 钩子」——语义正确。

**唯一补充**：debug 日志已记录 from/to（`from = %from.as_str()`）,对 Custom 也工作。无需改。

**若未来 review 要自己的钩子**（如 reported 时自动归档）：在 match 加 review 专属分支,如 `(TaskStatus::Custom(a), TaskStatus::Custom(b)) if a=="revising" && b=="reported" => { ... }`。C0 不做,留 C3。

## 6. 影响面 + 回归风险

### 改动文件
- `task.rs`：TaskStatus enum 加 Custom + 去 Copy + 改 derive Serialize 为手动 impl + from_str_opt/as_str 改
- `state.rs`：parse_target_state 未知值返回 Custom
- `request_task_state_transition.rs`：transition 校验改用 can_transition（workflow session）+ 旧 enum 校验（非 workflow session）
- 其余 Copy 使用点：按编译器报错改 clone/借用

### 回归风险
- **dev 零回归**：dev 的 4 个已知值行为完全不变（from_str_opt 已知值优先 match,as_str/Serialize 输出不变,dispatch_hook 对 dev transition 仍命中）。
- **旧 task.json 兼容**：旧数据的 "planning"/"in_progress"/"done"/"completed" 读回正确变体（from_str_opt 已知值 match）。
- **Copy 移除**：编译器强制 audit 所有使用点,不会静默漏改。
- **as_str 签名变化**（`'static` → `&str`）：需 grep 确认无 `'static` 依赖（如赋值给常量上下文）。

### 单测
- `Custom("reviewing")` round-trip（写 task.json 读回）
- `from_str_opt("reviewing")` → `Custom("reviewing")`（不再 Planning）
- `Custom("reviewing").as_str()` → `"reviewing"`
- `Custom("reviewing").serialize()` → `"reviewing"`（纯字符串,非 `{"Custom":...}`)
- role gate 用 Custom state 查 `roles_by_state["reviewing"]` 命中
- `parse_target_state("reviewing")` → `Ok(Custom("reviewing"))`（不再 Err）
- `can_transition` 对 review 的 intake→reviewing 返回 true（workflow.json 定义了）
- **回归**：dev planning→in_progress→done 全流程,spec_distillation 仍触发；旧 "planning" 字符串读回 Planning

## 7. Open Questions 落定

1. ✅ **Custom 序列化**（PRD OQ1）：手动 impl Serialize（输出 `as_str()` 纯字符串）,与手动 Deserialize 对称。去掉 derive Serialize。
2. ✅ **钩子兜底**（PRD OQ2）：state.rs:273 现有 `_ => {}` + debug 日志已足够,不改。Custom transition 落兜底语义正确。
3. ✅ **非 workflow session fallback**（PRD OQ3）：回退旧 enum 校验（§3）。
