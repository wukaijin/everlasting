# Implement: TaskStatus accommodate custom plugin state (C0)

> 父任务：`07-26-workflow-review-plugin`
> 配套：`prd.md` + `design.md`
> 本文件是 ordered execution checklist + 验证命令 + 风险点。

## 执行顺序

enum 改动 → 序列化 → parse → transition 校验 → Copy 移除 audit → 测试。先改核心类型，再修编译错误，最后验证。

---

## Phase 1：TaskStatus enum + 序列化（核心改动）

### 步骤 1.1 — enum 加 Custom + 去 Copy + 手动 Serialize

**文件**：`app/src-tauri/src/agent/workflow/task.rs:120`

**改动**（design §1.1）：
1. 去掉 `Copy`（derive 行删 Copy）
2. 加 `Custom(String)` 变体
3. 去掉 derive 的 `Serialize`，改手动 `impl Serialize for TaskStatus`（输出 `serializer.serialize_str(self.as_str())`）

```rust
#[derive(Debug, Clone, PartialEq, Eq)]  // 去 Copy,去 Serialize
pub enum TaskStatus {
    Planning, InProgress, Done, Completed,
    Custom(String),
}

impl serde::Serialize for TaskStatus {
    fn serialize<S>(&self, s: S) -> Result<S::Ok, S::Error>
    where S: serde::Serializer {
        s.serialize_str(self.as_str())
    }
}
```

**验证**：`cargo build --lib`（会有 Copy 相关编译错误，Phase 5 处理）

### 步骤 1.2 — from_str_opt + as_str

**文件**：`task.rs:130 + 143`

**改动**（design §1.2/§1.3）：
- `from_str_opt`：`_ => Self::Planning` 改 `other => Self::Custom(other.to_string())`
- `as_str`：签名 `&'static str` → `&str`，加 `Custom(s) => s` 分支

**验证**：单测（Phase 6）
```rust
assert_eq!(TaskStatus::from_str_opt("reviewing"), TaskStatus::Custom("reviewing".to_string()));
assert_eq!(TaskStatus::Custom("reviewing".to_string()).as_str(), "reviewing");
```

---

## Phase 2：parse_target_state（transition 入口放行）

### 步骤 2.1 — 未知值返回 Custom

**文件**：`app/src-tauri/src/agent/workflow/state.rs:139`

**改动**（design §2）：
```rust
other => Ok(TaskStatus::Custom(other.to_string())),  // 原: Err(InvalidTargetState(other))
```

**验证**：单测
```rust
assert!(matches!(parse_target_state("reviewing"), Ok(TaskStatus::Custom(_))));
```

---

## Phase 3：transition 合法性校验（can_transition 驱动）

### 步骤 3.1 — request_task_state_transition 校验改用 can_transition

**文件**：`app/src-tauri/src/tools/request_task_state_transition.rs`

**改动**（design §3）：
- 在 execute_blocking 拿到 from/to 后，加 transition 合法性校验：
  ```rust
  // workflow session: 由 plugin def 的 can_transition 校验
  if let Some(def) = workflow_def {
      if !can_transition(def, from.as_str(), to.as_str()) {
          return Err(InvalidTransition { from, to });
      }
  } else {
      // 非 workflow session: 回退旧 enum 校验（from/to 必须是已知 TaskStatus 非 Custom）
      if matches!(to, TaskStatus::Custom(_)) || matches!(from, TaskStatus::Custom(_)) {
          return Err(...);  // 非 workflow session 不允许 Custom state
      }
  }
  ```
- validate 阶段不再因 target_state 非已知而拒（parse_target_state 已放行）。

**关键**：`can_transition`（def.rs:306）已存在，按字符串查 transitions，零新增逻辑。

**验证**：单测
- workflow session + review def：intake→reviewing 合法，planning→done 非法（无此 transition）
- 非 workflow session：Custom state 被拒

---

## Phase 4：Copy 移除的编译错误 audit

### 步骤 4.1 — 按编译器报错改 clone/借用

`cargo build --lib` 会报所有 Copy 使用点。按 design §4 表处理：

| 模式 | 处理 |
|---|---|
| `status.as_str()` | 无影响（借用） |
| `match status`（按值消费） | 改 `match &status` 或 `match status.clone()` |
| `Some(t.status)`（拷贝进 tuple） | `Some(t.status.clone())` |
| 函数参数 `from: TaskStatus`（按值） | 改 `from: &TaskStatus` 或调用方 clone |

**audit 原则**：优先 `&TaskStatus` 借用（零成本），无法借用时 clone。

**关键文件**（grep 已知高频）：
- `dispatch.rs:1630`（role gate，`status.as_str()`，应无影响）
- `chat_loop.rs:3569`（`Some(t.status)` → clone）
- `state.rs`（dispatch_hook 参数，可能改 `&TaskStatus`）
- `update_checklist.rs`（match status）
- `inject.rs`（`task.status.as_str()`，应无影响）

**验证**：`cargo build --lib` 编译通过，无 Copy 相关错误。

---

## Phase 5：as_str 签名变化的 'static 依赖 audit

### 步骤 5.1 — 确认无 'static 依赖

**read 点**：grep `as_str()` 所有调用，确认无赋值给 `&'static str` 字段或常量上下文。

```bash
grep -rn "\.as_str()" app/src-tauri/src/ --include="*.rs" | grep -i "status\|TaskStatus"
```

**预期**：现有调用都是临时使用（赋值给 String、format!、参数传递），无 'static 依赖。若发现（如赋值给 struct 的 `&'static str` 字段），改字段类型为 `String` 或 `Cow<'static, str>`。

**验证**：编译通过即说明无 'static 依赖问题。

---

## Phase 6：单测 + 全量验证

### 步骤 6.1 — task.rs 单测

```rust
#[test]
fn custom_status_round_trip() {
    let t = TaskStatus::Custom("reviewing".to_string());
    let json = serde_json::to_string(&t).unwrap();
    assert_eq!(json, r#""reviewing""#);  // 纯字符串,非 {"Custom":...}
    let parsed: TaskStatus = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, t);
}

#[test]
fn from_str_opt_custom_not_planning() {
    assert_eq!(TaskStatus::from_str_opt("reviewing"), TaskStatus::Custom("reviewing".to_string()));
    // 已知值仍正确
    assert_eq!(TaskStatus::from_str_opt("planning"), TaskStatus::Planning);
}

#[test]
fn custom_as_str() {
    assert_eq!(TaskStatus::Custom("reviewing".to_string()).as_str(), "reviewing");
}
```

### 步骤 6.2 — state.rs 单测

```rust
#[test]
fn parse_target_state_custom_ok() {
    assert!(matches!(parse_target_state("reviewing"), Ok(TaskStatus::Custom(_))));
}
```

### 步骤 6.3 — transition + role gate 集成单测

- review def + can_transition(intake→reviewing) → true
- role gate: `roles_by_state["reviewing"]` 查到 `["reviewer"]`（用 Custom state）

### 步骤 6.4 — 回归 + 全量

```bash
cd app/src-tauri
cargo test --lib 2>&1 | tail -20
cargo clippy --lib --tests -- -D warnings 2>&1 | tail -10
```

**回归重点**：
- dev planning→in_progress→done 全流程正常（现有 task 状态机测试全绿）
- spec_distillation hook 仍触发（dev InProgress→Done）
- 旧 task.json（"planning"/"done" 字符串）读回正确变体
- 现有 subagent role gate（dev 的 planning/in_progress state）不受影响

---

## 风险点

### R1（中）：Copy 移除的连锁编译错误
**问题**：去掉 Copy 后所有按值传递 TaskStatus 的地方要改,可能涉及多个文件。
**缓解**：编译器逐一报错,按 Phase 4 处理。优先 `&TaskStatus` 借用。audit 完编译通过即解决。

### R2（低）：as_str 签名 'static → &str
**问题**：若有调用点依赖 'static（赋值给常量上下文）,签名变化会破坏。
**缓解**：Phase 5 grep audit。现有调用都是临时使用,预期无问题。

### R3（低）：非 workflow session 的 Custom 拒绝
**问题**：非 workflow session 用 Custom state 会被拒（design §3）。若有非 workflow 场景需要自定义 state,会受限。
**缓解**：当前只有 workflow plugin 用 Custom state,非 workflow session 不该有 Custom。语义正确。

---

## Follow-up（C0 范围外）

- review 自己的钩子（reported 时自动归档等）：C3 落地时在 dispatch_hook match 加 review 专属分支。
- TaskStatus 的 Display trait（若日志/调试需要更友好的输出）：按需加。

---

## 验证命令汇总

```bash
cd app/src-tauri
cargo build --lib                    # Phase 1-5 编译通过
cargo test --lib task               # Phase 6.1
cargo test --lib state              # Phase 6.2
cargo test --lib transition         # Phase 6.3
cargo test --lib                    # Phase 6.4 全量
cargo clippy --lib --tests -- -D warnings
```
