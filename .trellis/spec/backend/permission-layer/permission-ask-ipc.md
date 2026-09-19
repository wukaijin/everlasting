<!-- Moved from permission-layer.md 2026-09-19 (doc-split) -->

### 5. ⑨ 关 ↔ `permission:ask` IPC 协议

**Server → Client**:后端 `agent::permissions::check` Tier 3 发:

```rust
app.emit("permission:ask", &PermissionAskPayload {
    rid: String,                // UUID
    tool_name: String,
    tool_input: serde_json::Value,
    risk: Risk,                  // Low | Medium | High | Critical (lowercase)
    reason: Option<String>,      // 人类可读原因
});
```

`PermissionAskPayload` uses `#[serde(rename_all = "camelCase")]`,
producing the wire shape:

```jsonc
{
  "rid": "uuid",
  "toolName": "shell",
  "toolInput": { "command": "ls -la" },
  "risk": "high",
  "reason": "The tool shell requires your confirmation (risk: 高)."
}
```

**Client → Server**:前端 `usePermissionsStore.respond(rid, decision)`:

```typescript
invoke("permission_response", { rid: "uuid", decision: "allow_once" | "allow_always" | "deny" })
```

后端 `commands::permissions::permission_response` (Tauri command)
查 `PermissionStore: Arc<Mutex<HashMap<rid, oneshot::Sender>>>`,
发响应到 `check()` 正在 await 的 oneshot,唤醒后续逻辑。

**Wire invariant**:frontend `respond(rid)` 必须用后端 emit
`permission:ask` 时附带的 `rid` — 后端 `HashMap<rid, Sender>`
的 key 是 emit 时刻的 UUID。客户端不能自己生成 rid;只能转发
后端给的 rid + decision。

