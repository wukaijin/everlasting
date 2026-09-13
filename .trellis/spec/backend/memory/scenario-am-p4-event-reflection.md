<!-- Moved from memory/scenario-autonomous-memories.md 2026-09-13 (doc-split) -->

#### Event-driven bypass reflection contract (P4, write side of the loop) — 2026-06-29, 06-29-am-p4-event-reflect

> **P4 是 P3 的写入对偶**。spike-007 §3 路径2(spike-007 §6 接入点 C)定义的"连续 ≥2 次同名工具失败后成功 → 旁路 LLM reflection → 自动产出 pitfall(active)"。P3 是"读"(工具执行前召回已有 pitfall),P4 是"写"(事件驱动把新 pitfall 写库)。P3 + P4 闭合完整自动闭环:踩坑 → 记住 → 下次规避。
>
> P4 **不**改 `permissions::check()` 内部,**不**改 P3 的 pre-execute seam,**不**改 `ToolResultPayload` shape — 它在 chat_loop 的 **post-execute seam**(与 P3 的 pre-execute seam 互补)读 `ToolResultPayload.is_error` 信号。

**Signatures**(在 `app/src-tauri/src/agent/auto_reflect.rs`):

```rust
// agent::auto_reflect::FailureTracker — per-session 状态机
pub struct FailureTracker {
    // (tool_name) -> TrackerEntry { consecutive_failures, last_failure_input,
    //                                last_failure_content, last_failure_path }
    inner: Mutex<HashMap<String, TrackerEntry>>,
}

pub const REFLECTION_FAILURE_THRESHOLD: usize = 2;

impl FailureTracker {
    pub fn new() -> Self;
    pub fn try_record_outcome(
        &self,
        tool_name: &str,
        tool_input: &serde_json::Value,
        content: &str,
        is_error: bool,
    ) -> Option<ReflectionTrigger>;
    // Some(_)= 触发 reflection(call site 调 tokio::spawn 跑 reflect_to_pitfall);
    // None  = 不触发(失败计数 0/1,或 success 但无前置 ≥2 失败)
}

// agent::auto_reflect::try_record_outcome (public entry,chat_loop 调用)
pub fn try_record_outcome(
    tracker: &Arc<Mutex<FailureTracker>>,
    request_id: &str,
    session_id: &str,
    project_id: &str,
    tool_name: &str,
    tool_input: &serde_json::Value,
    content: &str,
    is_error: bool,
);

// agent::auto_reflect::reflect_to_pitfall (private,fire-and-forget 内核)
async fn reflect_to_pitfall(
    request_id: &str,
    session_id: &str,
    project_id: &str,
    tool_name: &str,
    tool_input: &serde_json::Value,
    failure_content: &str,
    success_content: &str,
    provider: Arc<dyn Provider>,
    pool: SqlitePool,
);
```

**Contracts**:

| 项 | 值 | 说明 |
|---|---|---|
| 触发时机 | `chat_loop` 的 `execute_tool()` 返回之后、audit 写之前 | `!token.is_cancelled()` 守卫(与 RULE-A-004 audit-skip 对齐) |
| 触发信号 | 同一 `tool_name` 连续 `REFLECTION_FAILURE_THRESHOLD = 2` 次 `is_error=true` **之后**的 `is_error=false` | 单次失败不触发(PRD AC #3);计数器在成功或触发后重置 |
| 状态机存储 | `Arc<Mutex<HashMap<tool_name, TrackerEntry>>>` 内嵌于 `run_chat_loop` 局部,per-session 内存 | **不**跨 session 持久化(v1 接受 session 边界重置,v2 扩展位 spike-007 §10) |
| 调用点 | `chat_loop.rs` parallel-batch L2 path + serial path,两处(seam 与 P3 镜像) | 共享同一 `failure_tracker` 句柄 |
| Reflection LLM 调 | 走主 provider 同一实例(不另起);独立 `REFLECT_SYSTEM_PROMPT` + `REFLECT_USER_TEMPLATE`;**不**消费主 system prompt / 不消费消息历史 | 1 个 user message 含"失败+成功 transcript 片段";空 `tools` 数组;`max_tokens=512` |
| Reflection 期望产出 | JSON `{title, content, trigger_key: {tool, command_pattern, path_globs}}` | markdown 代码围栏剥离;JSON parse 失败 → `warn!` + 丢弃 |
| 写库参数 | `kind=Pitfall, status=Active, scope=Project, source_session_id, source_ref=<request_id>:<tool_name>` | 走 P1 `insert_memory` 复用安全网(敏感过滤 / 长度 / 敏感路径 / frequency cap 50/session)|
| 触发阈值 | `consecutive_failures >= 2`(常量 `REFLECTION_FAILURE_THRESHOLD`) | PRD AC #3:单次失败不触发 |
| Fire-and-forget | `tokio::spawn` 整段 reflection | **不** await 主 loop;失败一律 `tracing::warn!` + 静默吞;**不** panic / `unwrap()` / `expect()` |
| Decision 语义 | **不参与** `permissions::check()` 决策链 | P4 不在 P3 的 pre-execute seam,也不在 5-tier 内部 |
| ToolResultPayload 污染 | **无** — P4 是 read-only consumer,只读 `is_error` / `content` / `tool_input` | 协议 `tool_use_id` 配对 / `is_error` 语义 / envelope `{result, cwd}` 全部不变 |

**为什么 P4 写在 post-execute 而非 pre-execute(P3 seam)**:
- P3 是"工具执行前查已知 pitfall" — 写发生在 pre-execute 之前;**读**则在 P3 的 pre-execute seam
- P4 是"工具执行完记录新经验" — 需要看到 `is_error` 真实结果(成功/失败)才能决策,**写**发生在 post-execute 之后
- 两个 seam 是 sibling,不互相依赖(顺序独立:同一个 tool_use_id P3 在前 P4 在后,中间夹 `execute_tool` + audit)

**为什么 P4 走 P1 `insert_memory` 而非自写 INSERT**:
- 写入安全网(sensitive regex / 长度 cap / 敏感路径 deny-list / frequency cap 50/session)单源;旁路绕过 P1 安全网会引入敏感泄漏 / 库膨胀 / 路径泄漏
- 状态机字段(`hit_count` / `last_used_at` / `demoted_reason`)由 P1 维护,P5 消费;旁路 INSERT 会破坏 P5 状态机读取
- 复用 `MemoryKind::Pitfall` 枚举 + `MemoryStatus::Active` 强类型,P4 写时直接用 `MemoryInput { kind, status, ... }`

**P3 ↔ P4 闭环**(P4 单元测试 `reflected_pitfall_is_recallable_by_p3_helper` 锁定):
1. session A:同 `tool_name='shell'` 连续 2 次 `cargo test --no-default-features` 失败,后 1 次 `cargo test` 成功
2. P4 状态机触发 → `tokio::spawn` reflection → 调 LLM 提炼 `{title: "WSL cargo test 需显式 features", content: "...", trigger_key: {tool: "shell", command_pattern: "cargo test", path_globs: null}}` → 写 `insert_memory(kind=pitfall, status=active)`
3. session B:agent 跑 `cargo test` → P3 pre-execute seam 调 `find_pitfalls_by_trigger('shell', Some("cargo test"), None)` → 命中 session A 写的 pitfall → 注脚 prepend 到 tool_result → agent 看到 "⚠️ Memory: ..." 提示 → 第一次执行就规避

**Reflection prompt 模板**(独立常量,`auto_reflect.rs` 内部):

```text
// REFLECT_SYSTEM_PROMPT
"你是一个经验提炼助手。给定一个工具调用连续失败的 transcript + 后续成功的 transcript,
提炼成一句 200 字内的'可复用经验'。输出严格 JSON,字段:
  title: 短标题(≤30 字符)
  content: 一句可复用的踩坑经验(≤200 字符,imperative 语气)
  trigger_key: 结构化触发键,字段:
    tool: 工具名(shell/edit_file/grep/read_file/...)
    command_pattern: 触发命令模式(可空)
    path_globs: 触发路径 glob 列表,可空(null = 不限路径)
**只输出 JSON**,不要 markdown 包装,不要解释。"

// REFLECT_USER_TEMPLATE
"<failure>
  tool: {tool_name}
  input: {tool_input_json}
  error: {failure_content_truncated_2kib}
</failure>
<success>
  tool: {tool_name}
  input: {tool_input_json}
  output: {success_content_truncated_2kib}
</success>
请提炼上述失败→成功经验。"
```
