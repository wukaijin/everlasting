# p1-permission-asks-cleanup

> 关联 DEBT.md:RULE-B-001 (P1) + RULE-B-002 (P1)
> 状态:**open(已记账,待实施)**。本次未实施 —— 与 `06-16-p1-openai-o1-glob-spawn-blocking` 拆分(用户决策:B 工作量大、性质为清理隐性依赖,单独 task)。

## Goal

清理 permission ask 生命周期两处隐性依赖 + dead code:

- **RULE-B-002**:`cancel_session_asks(store, _session_id)` 用下划线前缀忽略 session_id,body 直接 `map.clear()` 全清 —— **latent bug**:当前安全只因未被调用。一旦接到 delete_session(B-001),会误清其他 session 的 pending ask。
- **RULE-B-001**:`delete_session` 只调 `cancel_inflight_for_session`,未调 `cancel_session_asks`。实际不泄漏(biased select! 间接清理),但隐性依赖 + `cancel_session_asks`/`cancel_pending_asks` 死代码误导维护者。

## What I already know (2026-06-16 已勘察)

### PermissionStore 结构 — `permissions/mod.rs:284-285`

```rust
pub type PermissionStore =
    Arc<Mutex<HashMap<String, oneshot::Sender<PermissionResponse>>>>;
```

key 是 rid(UUID),**无 session 绑定**。

### rid 生命周期

| 阶段 | 位置 | 说明 |
|---|---|---|
| 生成 | `mod.rs:913` | `uuid::Uuid::new_v4()`(在 `check()` 内,`ctx.session_id` 可达 `:258`) |
| register | `mod.rs:940` | `register_ask(store, rid.clone())`(**调用点仅此一处**) |
| resolve | `commands/permissions.rs:214` | `resolve_ask(store, rid, resp)` |
| cancel | `mod.rs:330` | `cancel_session_asks(store, _session_id)` clear() 全清(`#[allow(dead_code)]`) |

### 关键约束:key 不能改 `(session_id, rid)`

resolve 端(`permission_response` IPC,`commands/permissions.rs:214`)只传 rid,**不知道 session_id**。若 key 改 `(session_id, rid)`,resolve_ask 要 iterate 全表找 rid 匹配 → 退化。**所以必须走「value 存 session_id」方案**。

## Decisions (resolved, 实施时复核)

- **方案 A:value 改结构**(推荐)
  - `PermissionStore` value:`oneshot::Sender` → `PendingAsk { session_id: String, tx: oneshot::Sender<PermissionResponse> }`
  - `register_ask(store, session_id, rid)` 加 session_id 参数(调用点 `:940` 补 `&ctx.session_id`)
  - `resolve_ask` 不变(按 rid remove,value 拆出 tx 发送)
  - `cancel_session_asks(store, session_id)` 真过滤:iterate,session_id 匹配的 remove(tx drop → receiver `Err` → `check()` 走 Deny)
- **接入 delete_session**:`sessions.rs:126` `cancel_inflight_for_session` + `await_inflight_exit` 之后,补 `cancel_session_asks(&state.permission_asks, &session_id).await`。
- **`cancel_pending_asks`(`commands/permissions.rs:315`)**:当前 dead code 调 `cancel_session_asks(store, "")`。改为接 session_id 参数(供未来其他 destructive op 复用),或直接删除 —— 实施时定。
- **跨 session 隔离单测**:两个 session 各 register 一个 ask,cancel session A,断言 session B 的 ask 仍在(未误清)。

## Requirements (待实施锁定)

- [ ] store value 带 session_id(`PendingAsk`)
- [ ] `register_ask` 加 session_id 参数;`resolve_ask` 适配新 value
- [ ] `cancel_session_asks` 按 session 过滤(不全清)
- [ ] `delete_session` 接入 `cancel_session_asks`
- [ ] 跨 session 隔离单测(cancel A 不动 B)

## Out of Scope (explicit)

- rid 生成策略、permission_response IPC 协议(保持只传 rid)。
- ReadGuard / 其他 session 清理路径。
- `cancel_pending_asks` 是否保留(实施时最小化决定)。

## Technical Notes

- **改动文件**:`agent/permissions/mod.rs`(store 类型 + register/cancel/resolve 签名)+ `commands/sessions.rs`(接入)+ `commands/permissions.rs`(cancel_pending_asks)+ `agent/tests.rs`(mock store,若有 register 调用)。
- **工作量估算**:~50 行生产 + 测试(明显大于 D-002/E-004 的 ~15 行,故单独 task)。
- **回归保护**:RULE-A-006 的 9 个 `agent_loop_*` 集成测试覆盖 `run_chat_loop`,permission ask 路径若被触及会被测。
- **验证**:`cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib`。
