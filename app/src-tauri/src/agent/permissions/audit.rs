//! `AuditKind` enum + audit-row writers. Split out of `mod.rs`
//! on 2026-06-23.
//!
//! `AuditKind` is intentionally a **single enum** (18 variants),
//! NOT split into per-domain enums: `record_audit`'s signature,
//! the serde tag landing in `session_audit_events.kind`, and the
//! frontend C4 audit-log UI all key off the flat lowercase wire
//! strings. The variants are grouped below by domain
//! (Tool / Permission / Mode / Message / Loop / Worker) using
//! section comments for readability — the grouping is cosmetic.

use sqlx::SqlitePool;

use super::types::PermissionContext;

// ---------------------------------------------------------------------------
// AuditKind enum (serialized into `session_audit_events.kind`)
// ---------------------------------------------------------------------------

/// Audit event kinds. Serialized lowercase (matches DB column).
/// 18 variants — see the module-level docstring above (variant count
/// grouped by domain) + PRD `## A2 后端` "审计 `kind` 枚举" section.
///
/// `ModeChanged` / `YoloEntered` / `YoloExited` are written
/// directly by the `set_session_mode` Tauri command via
/// `db::record_audit_event(.., "mode_changed", ..)` (the
/// command path uses string literals for the kind, not this
/// enum, to keep the cross-module call graph tight). The
/// variants are kept here as the typed single source of truth
/// for the audit log schema — PR3's C4 audit-log UI will
/// match on these.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditKind {
    // === Tool 域 ===
    /// ⑨ 关拒绝 (Tier 2 hit, Tier 3 timeout, Tier 3 user deny)
    ToolDenied,
    /// ⑨ 关放行 (Tier 5 默认 OR Tier 3 "始终允许" 命中 OR Tier 3 user "仅一次")
    ToolAllowed,
    /// ⑨ 关弹窗询问 (Tier 3 emit permission:ask)
    ToolPermissionAsk,
    /// ⑩ tool 执行完成 (C4 任务 PR1, 2026-06-14): payload 携带
    /// `tool_name` / `tool_input` / `duration_ms` / `exit_code`,
    /// 用于"哪步最慢 / 哪步报错"的事后回看。落表点在 agent
    /// loop 拿到 `execute_tool` 返回值之后 (duration + exit_code
    /// 已知), 见 `agent/chat.rs` 的 tool 执行循环。
    ToolExecuted,
    /// P3b (08-31-a2-p3b-sandbox-executor, D2): ReadOnly 档 shell
    /// 命令在 Landlock+seccomp 沙盒下执行完成(spawn 成功)。payload
    /// 携带 `command_sha256_12`(命令哈希前缀——**不存全命令**,全文
    /// 已由 `tool_executed` 行承载)+ `ruleset`(ruleset 摘要) +
    /// `tool_name`。落表点在 `tools/shell.rs` spawn 成功后
    /// (best-effort,同其余 helper)。PR2 起 `run_background_shell`
    /// 沙盒路径共用本 kind。
    SandboxedShellExecution,
    /// Yolo 模式下仍被 Tier 2 deny 拦截 (硬墙)
    ToolDeniedYolo,

    // === Permission 域 ===
    /// 用户选"始终允许"(后端写了 session_tool_permissions)
    PermissionGranted,
    /// Tier 3 120s 超时 (user 没响应)
    PermissionTimeout,
    /// C1 cancel 触发的请求终止 (与 Tier 3 deny 区分)
    RequestCancelled,

    // === Mode 域 ===
    /// Mode 切换 (set_session_mode 触发)
    ModeChanged,
    /// 进入 Yolo (mode → Yolo)
    YoloEntered,
    /// 退出 Yolo (mode != Yolo 且之前是 Yolo)
    YoloExited,
    /// 2026-07-07 (`request_mode_change` tool): LLM 调 tool
    /// 触发(无论最后是 allow / deny / noop,每次 LLM 申请都落)。
    /// payload 携带 `target_mode` / `reason` / `noop: bool`
    /// (`noop=true` 表示 LLM 申请切到当前 mode,tool 立即返
    /// `{"noop": true}`,无 IPC、无 card)。落表点在
    /// `tools::request_mode_change::execute_blocking` 入口。
    ModeChangeRequested,
    /// 2026-07-07 (`request_mode_change` tool): user 在 card 上
    /// 点"允许"+ DB UPDATE 成功。`set_session_mode` IPC
    /// 路径会写 `mode_changed` (自动);`resolve_mode_change`
    /// IPC handler 额外落本行(`mode_change_allowed` +
    /// `mode_changed` 同时存在,职责:marker 表明这次 mode
    /// 切换由 LLM 申请触发,而非 user 主动)。payload 携带
    /// `prev_mode` / `new_mode` / `target_mode`。
    ModeChangeAllowed,
    /// 2026-07-07 (`request_mode_change` tool): user 在 card 上
    /// 点"拒绝" / Yolo 二次 modal 取消 / Yolo root guard 触发
    /// 拒绝。三种场景统一记 `mode_change_denied` 便于审计
    /// 过滤;payload 的 `reason` 字段区分
    /// `user denied` / `yolo_cancelled_confirm` /
    /// `yolo_root_guard` / `db_error`。落表点在
    /// `commands::question::resolve_mode_change` 三种
    /// deny 路径(user 点拒绝 / apply 失败 / 二次 modal 取消)。
    ModeChangeDenied,

    // === Message 域 ===
    /// D3 PR1 (2026-06-17): user 在 session 内编辑了一条 user 消息
    /// (in-place update + 级联删后续 message + 重新 send 前的
    /// edit 落点)。payload 携带 `message_seq` /
    /// `new_text_preview` / `edited_at`。落表点在
    /// `db::sessions::edit_user_message` 的事务尾部,与 cascade
    /// delete 同一个事务,失败回滚不入审计。
    EditMessage,
    /// D3 PR3 (2026-06-17): user 在 session 内点 Resend 重发
    /// 了一条已存在的 user message(不修改 content,只 cancel
    /// 旧 stream + 重新 send 同一条 prompt)。payload 携带
    /// `message_seq` / `content_text_preview`。落表点在
    /// agent loop 接收 user message 路径,识别 metadata flag
    /// `{ kind: "resend", message_seq }` 后,通过
    /// `record_message_resend_audit` helper 异步落表
    /// (best-effort,非事务内,因为 audit 缺失不影响 chat
    /// 主流程)。
    ResendMessage,

    // === Loop 域 (2026-07-05, C2+ 循环检测主动干预) ===
    /// C2+ (2026-07-05): harness 主动干预循环检测。当 C2 软提示
    /// 连续命中 N=3 次仍未能让 LLM 自我纠正时,harness 通过
    /// `QuestionStore` 主动询问用户「是否终止本次 agent loop」。
    /// payload 携带 `hit_count` / `verdict_kind` ("hard"|"soft") /
    /// `action` ("asked"|"terminated"|"continued")。落表点在
    /// chat_loop.rs 的 C2+ 状态机三个分支:register 成功后立即
    /// 落 `asked`;用户选「终止」落 `terminated`;用户选「继续」
    /// 落 `continued`。worker 触发不落本表(worker 无独立审计
    /// surface,worker run 自有 transcript 记录)。
    LoopIntervention,
    /// MAX_TURNS softcap (08-18-max-turns-softcap, 2026-08-19):
    /// 单聊主 loop 撞线(默认 200 turn)不再硬终断,改为经
    /// QuestionStore 弹浮动询问卡(继续 +200 / 压缩后续跑 / 停止)。
    /// payload 携带 `turn`(撞线时的 turn 号,= budget+1)/
    /// `budget`(撞线时的预算)/ `action`(见
    /// [`record_turn_limit_softcap_audit`])。落表点在
    /// chat_loop.rs 的软卡询问 helper 各分支:register 成功后
    /// 立即落 `asked`;三分支/超时/取消各落对应 action。worker
    /// 与群聊路径不经软卡(硬卡原样),不落本表。
    TurnLimitSoftcap,
    /// unified-context-budget WP2 (08-19-unified-context-budget,
    /// 2026-08-19): 关卡⑤硬卡静默裁剪发生时落表。payload 携带
    /// `arms`(裁剪臂明细,`[{kind, count, tokens_freed}]`,kind ∈
    /// `at_file`|`image`|`memory_section`)/ `over_by` / `pre_total`
    /// / `post_total` / `window`。落表点在 drive.rs send 前
    /// budget gate(仅裁剪实际发生时;gate 关/未超线不落)。
    /// 裁尽仍超线的 fail-fast 不落本表(那是 Error turn,非裁剪)。
    /// worker / 群聊不经硬卡,不落本表。
    ContextBudgetTrim,

    // === Worker 域 (2026-06-22, RULE-FrontSubagent-003 fix) ===
    /// worker subagent 在 Tier 4 交互式 ask 后,user 选了"Allow"
    /// / "仅一次"。payload 携带 `worker_run_id` / `tool_name` /
    /// `tool_input` — 与 `ToolAllowed` 形状对齐。落表点是 worker
    /// 路径 `ask_path` 三臂 resolve 后(oneshot 收到
    /// `PermissionResponse::AllowOnce` / `AllowAlways`)。
    /// 与 parent `ToolAllowed` 区分:`session_id` 共享(worker 复用
    /// parent_session_id,见 RULE-A-014),但前端 C4 audit log UI
    /// 看到 worker-ask-allowed 时应知道这是 worker 决策。
    WorkerAskAllowed,
    /// worker subagent Tier 4 ask 收到 user
    /// `PermissionResponse::Deny`。落表点是 worker 路径
    /// `ask_path` oneshot 收到 Deny。reason 字段携带 user 可选
    /// feedback("拒绝并说明")。
    WorkerAskDenied,
    /// worker subagent Tier 4 ask 在 120s 内无 user 响应,自动
    /// Deny。落表点是 `tokio::select!` 的 timeout 臂命中。
    WorkerAskTimedOut,
    /// worker subagent Tier 4 ask 在 user 主动 cancel parent
    /// session 时 resolve 为 Deny。落表点是
    /// `tokio::select!` 的 cancel 臂命中 (parent_token 取消 →
    /// worker_token child 取消)。
    WorkerAskCancelled,
    // 2026-07-08 (`07-08-workflow-integration` Phase 3 Step 3.1):
    // workflow task-state-transition request lifecycle. Tool
    // (`request_task_state_transition::execute_blocking`) writes
    // `*_requested`; the IPC handler
    // (`commands::question::resolve_task_state_transition`)
    // writes `*_allowed` / `*_denied` post-apply. Same shape as
    // `ModeChangeRequested` / `ModeChangeAllowed` /
    // `ModeChangeDenied` (the request_mode_change task on
    // 2026-07-07). The payload shape carries
    // `target_state` / `current_state` / `slug` instead of
    // `target_mode` / `current_mode`.
    TaskStateTransitionRequested,
    TaskStateTransitionAllowed,
    TaskStateTransitionDenied,
    // === UI 域 (2026-07-13, B9+ D4) ===
    /// B9+ D4 (2026-07-13): user 在 DiffPrimitive / ButtonPrimitive
    /// 点「应用」按钮,后端 `apply_ui_diff` IPC 解析 unified diff 并
    /// 写文件成功后落本行。payload 携带
    /// `files: [{path, added, removed}]` + `total_files`。
    /// **失败** 路径(boundary / parse / conflict / io / empty)**不**落
    /// 审计 — 前端 inline error 反馈即可,失败不是"apply 行为"。
    /// 落表点是 `commands::ui::apply_ui_diff` 全文件写完后;
    /// 任意 file 写失败时前面已写的文件也 rollback(io 出错时不能
    /// 半落表,本 audit 整批撤)。
    /// 与 `ToolExecuted` 区分:本 IPC 由 **user 点击** 触发(非 LLM tool);
    /// `session_id` 共享,C4 audit log UI 据此 disambiguate
    /// "用户主动写" vs "LLM 工具调用"。
    UiDiffApplied,
    // === Scheduler 域 (F2 定时任务, 2026-08-28, `08-28-f2-scheduled-tasks`) ===
    /// 定时任务调度事件。daemon 调度循环的每个判定分支都落本行
    /// (挂**目标 session**),payload 统一携带 `task_id` / `task_name` /
    /// `action`,action 枚举(prd R3):
    /// - `fired` — 到点触发(宽限 60s = 2×tick 内)
    /// - `catchup` — 停机跨过到期点后的补偿触发(due 距 now > 60s)
    /// - `skipped_dedup` — 上次注入条目仍滞留队列,本轮跳过去重
    /// - `skipped_queue_disabled` — `message_queue_enabled=false`,fire
    ///   会走 legacy 取消语义,跳过
    /// - `lost` — 账已落但注入条目被 Stop 语义清队丢弃(best-effort
    ///   兜底;仅覆盖驱动器 cancel break + Stop 命令两处清队点)
    /// - `error` — fire 失败(`reason` 附错误类别,如 `queue_full`)
    ScheduledTaskFired,
}

impl AuditKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ToolDenied => "tool_denied",
            Self::ToolAllowed => "tool_allowed",
            Self::ToolPermissionAsk => "tool_permission_ask",
            Self::PermissionGranted => "permission_granted",
            Self::ModeChanged => "mode_changed",
            Self::YoloEntered => "yolo_entered",
            Self::YoloExited => "yolo_exited",
            // 2026-07-07 (`request_mode_change` tool)
            Self::ModeChangeRequested => "mode_change_requested",
            Self::ModeChangeAllowed => "mode_change_allowed",
            Self::ModeChangeDenied => "mode_change_denied",
            Self::ToolDeniedYolo => "tool_denied_yolo",
            Self::PermissionTimeout => "permission_timeout",
            Self::RequestCancelled => "request_cancelled",
            Self::ToolExecuted => "tool_executed",
            Self::SandboxedShellExecution => "sandboxed_shell_execution",
            Self::EditMessage => "edit_message",
            Self::ResendMessage => "resend_message",
            Self::LoopIntervention => "loop_intervention",
            // 08-18-max-turns-softcap (2026-08-19): wire shape
            // snake_case lowercase, mirrors LoopIntervention.
            Self::TurnLimitSoftcap => "turn_limit_softcap",
            Self::ContextBudgetTrim => "context_budget_trim",
            Self::WorkerAskAllowed => "worker_ask_allowed",
            Self::WorkerAskDenied => "worker_ask_denied",
            Self::WorkerAskTimedOut => "worker_ask_timed_out",
            Self::WorkerAskCancelled => "worker_ask_cancelled",
            // 2026-07-08 (Phase 3 Step 3.1, see enum variant
            // doc). Wire shape: snake_case lowercase, mirrors
            // ModeChange* counterparts.
            Self::TaskStateTransitionRequested => "task_state_transition_requested",
            Self::TaskStateTransitionAllowed => "task_state_transition_allowed",
            Self::TaskStateTransitionDenied => "task_state_transition_denied",
            // 2026-07-13 (B9+ D4): user-triggered diff apply.
            // Wire shape: snake_case lowercase, mirrors
            // ToolExecuted counterparts (payload carries `files`
            // instead of `tool_name`/`tool_input`/`duration_ms`/
            // `exit_code`).
            Self::UiDiffApplied => "ui_diff_applied",
            // F2 定时任务 (2026-08-28): scheduler lifecycle events.
            // Wire shape: snake_case lowercase; payload carries
            // `task_id`/`task_name`/`action` (+ optional `reason`),
            // see the variant doc + `record_scheduled_task_audit`.
            Self::ScheduledTaskFired => "scheduled_task_fired",
        }
    }
}

// ---------------------------------------------------------------------------
// Audit helpers
// ---------------------------------------------------------------------------

/// Build the payload JSON for an audit row and write it. Errors
/// are logged at `warn!` but never propagated — the audit log
/// is best-effort (a write failure must not break the agent
/// loop).
///
/// The `critical` field is included in the payload so the
/// PermissionModal (PR3) / C4 audit-log UI can render the
/// 3px red border + shield-x icon styling on critical-risk
/// denials. The flag is `true` only for Tier 2 hard-kill-list
/// denials (where the kill list is intrinsically
/// catastrophic); Tier 4 mode denials are `false` (the LLM
/// is "just" in a read-only mode, not a catastrophic
/// operation). Tier 3 user-deny / timeout / cancel paths
/// are also `false` (the user opted out, nothing catastrophic).
pub(super) async fn record_audit(
    db: &SqlitePool,
    ctx: &PermissionContext,
    kind: AuditKind,
    tool_name: &str,
    tool_input: &serde_json::Value,
    reason: Option<&str>,
) -> Result<(), sqlx::Error> {
    // Map audit kind to critical flag: only Tier 2 hard-kill
    // denials are critical. Everything else is a "normal" path.
    let critical = matches!(kind, AuditKind::ToolDenied | AuditKind::ToolDeniedYolo);
    let payload = serde_json::json!({
        "tool_name": tool_name,
        "tool_input": tool_input,
        "reason": reason,
        "mode": ctx.mode.as_str(),
        "critical": critical,
    });
    let payload_str = payload.to_string();
    crate::db::record_audit_event(
        db,
        &ctx.session_id,
        kind.as_str(),
        Some(&payload_str),
        ctx.turn_seq,
    )
    .await
}

/// C4 PR1 (2026-06-14): record a `tool_executed` audit row. Unlike
/// [`record_audit`], this row carries **duration + exit_code**
/// instead of the ⑨ 关 payload shape (`reason` / `mode` /
/// `critical`). The agent loop calls this from the tool-execution
/// loop right after `execute_tool` returns, with the wall-clock
/// delta measured in the loop and the exit code the tool reported.
///
/// **Best-effort** (same contract as `record_audit`): a DB write
/// failure is logged at `warn!` and swallowed — the agent loop
/// never sees the error and continues normally.
///
/// `duration_ms` is `u128` from `Duration::as_millis()`; JSON has
/// no problem serializing the wider type and the value in practice
/// is well under `u64::MAX` (a single tool call rarely exceeds
/// MAX_TIMEOUT_MS = 600_000ms).
///
/// `exit_code` is `None` for tools that don't produce one
/// (`read_file` / `write_file` / `edit_file` / `grep` / `glob` /
/// `list_dir` / `web_fetch`); `Some(code)` for `shell`. The C4
/// audit-log UI uses `Some(0)` vs `Some(non-zero)` to color the
/// icon, and `None` for "N/A" — don't hardcode 0 to represent
/// "no exit code", that would conflate "succeeded" with "N/A".
pub async fn record_tool_executed_audit(
    db: &SqlitePool,
    session_id: &str,
    tool_name: &str,
    tool_input: &serde_json::Value,
    duration_ms: u128,
    exit_code: Option<i32>,
    turn_seq: Option<i64>,
) -> Result<(), sqlx::Error> {
    let payload = serde_json::json!({
        "tool_name": tool_name,
        "tool_input": tool_input,
        "duration_ms": duration_ms,
        "exit_code": exit_code,
    });
    let payload_str = payload.to_string();
    crate::db::record_audit_event(
        db,
        session_id,
        AuditKind::ToolExecuted.as_str(),
        Some(&payload_str),
        turn_seq,
    )
    .await
}

/// D3 PR3 (2026-06-17): record a `resend_message` audit row.
/// Mirrors [`record_tool_executed_audit`] but for the user-
/// initiated "重发" path: the user clicks Resend on an
/// existing user message, the frontend cancels any in-flight
/// stream and re-fires the `chat` IPC with a metadata flag
/// `{ kind: "resend", message_seq }`. The agent loop's user
/// message persist site detects the flag and fires this
/// helper, best-effort.
///
/// **Best-effort** (same contract as `record_audit` /
/// `record_tool_executed_audit`): a DB write failure is logged
/// at `warn!` and swallowed — the chat loop never sees the
/// error and continues normally. Audit loss is acceptable
/// here because the user has already seen the visual
/// confirmation (the new assistant turn is streaming); the
/// audit row is only for after-the-fact review.
///
/// The payload mirrors the edit audit shape (`message_seq`)
/// but uses `content_text_preview` instead of `new_text_preview`
/// (no content mutation — the resend path re-uses the existing
/// message text). Truncated to 80 chars to match the edit
/// audit's preview budget.
///
/// Distinct from `EditMessage`: that path is *destructive*
/// (in-place update + cascade delete + audit inside one
/// transaction); Resend is *additive* (re-fires the same
/// prompt, no content change, no cascade). The two audit
/// kinds let the user tell "you edited this prompt at X" from
/// "you re-ran this prompt at Y" when reviewing history.
pub async fn record_message_resend_audit(
    db: &SqlitePool,
    session_id: &str,
    message_seq: i64,
    content_text_preview: &str,
    turn_seq: Option<i64>,
) -> Result<(), sqlx::Error> {
    let payload = serde_json::json!({
        "message_seq": message_seq,
        "content_text_preview": content_text_preview.chars().take(80).collect::<String>(),
    });
    let payload_str = payload.to_string();
    crate::db::record_audit_event(
        db,
        session_id,
        AuditKind::ResendMessage.as_str(),
        Some(&payload_str),
        turn_seq,
    )
    .await
}

/// P3b (2026-08-31, D2): record a `sandboxed_shell_execution` audit
/// row after a ReadOnly-tier shell command spawned successfully under
/// the Landlock+seccomp sandbox. Mirrors the other best-effort
/// helpers (warn + swallow on DB error).
///
/// The payload carries a **command hash prefix + ruleset summary**,
/// not the command text (design §2.6: the full command is already in
/// the sibling `tool_executed` row; this row exists to answer "which
/// commands ran sandboxed and with what rule shape").
///
/// `turn_seq` is `None` from the tool layer (the shell tool has no
/// turn context); kept as a parameter so a future caller inside the
/// agent loop can thread it.
pub async fn record_sandboxed_shell_audit(
    db: &SqlitePool,
    session_id: &str,
    tool_name: &str,
    command_sha256_12: &str,
    ruleset: &str,
    turn_seq: Option<i64>,
) -> Result<(), sqlx::Error> {
    let payload = serde_json::json!({
        "tool_name": tool_name,
        "command_sha256_12": command_sha256_12,
        "ruleset": ruleset,
    });
    let payload_str = payload.to_string();
    crate::db::record_audit_event(
        db,
        session_id,
        AuditKind::SandboxedShellExecution.as_str(),
        Some(&payload_str),
        turn_seq,
    )
    .await
}

/// C2+ (2026-07-05): record a `loop_intervention` audit row.
/// Mirrors [`record_message_resend_audit`] / [`record_tool_executed_audit`]
/// but for the harness-driven 循环检测主动干预 path: when C2 软提示
/// 连续命中 N=3 次仍未能让 LLM 自我纠正,harness 通过 `QuestionStore`
/// 主动询问用户「是否终止本次 agent loop」。This helper is fired at
/// the three C2+ state-machine branches in `chat_loop.rs`:
///
/// - `action = "asked"` —— `QuestionStore::register` 成功后立即落
/// - `action = "terminated"` —— 用户选「终止 loop」分支
/// - `action = "continued"` —— 用户选「继续」分支
///
/// **Best-effort** (same contract as `record_audit` /
/// `record_tool_executed_audit` / `record_message_resend_audit`):
/// a DB write failure is logged at `warn!` and swallowed — the
/// agent loop never sees the error and continues normally.
/// Audit loss is acceptable because the user has already seen
/// the visual confirmation (the question card / the loop break);
/// the audit row is only for after-the-fact review.
///
/// `run_id: Option<&str>` is the worker run id when the audit is
/// fired from a worker subagent path (future-proofing — C2+ PR1
/// ships before the worker path lands the audit; main loop passes
/// `None`). The frontend C4 audit-log UI can use this to attribute
/// the intervention to a specific worker run when present.
///
/// `verdict_kind` is `"hard"` for `LoopVerdict::HardLoop` /
/// `"soft"` for `LoopVerdict::SoftLoop` (caller-side match —
/// `loop_detection.rs` is unchanged by C2+).
///
/// `action` is one of `"asked"` / `"terminated"` / `"continued"`
/// (see the state-machine docstring above).
pub async fn record_loop_intervention_audit(
    db: &SqlitePool,
    session_id: &str,
    run_id: Option<&str>,
    hit_count: u32,
    verdict_kind: &str,
    action: &str,
    turn_seq: Option<i64>,
) -> Result<(), sqlx::Error> {
    let payload = serde_json::json!({
        "hit_count": hit_count,
        "verdict_kind": verdict_kind,
        "action": action,
        "run_id": run_id,
    });
    let payload_str = payload.to_string();
    crate::db::record_audit_event(
        db,
        session_id,
        AuditKind::LoopIntervention.as_str(),
        Some(&payload_str),
        turn_seq,
    )
    .await
}

/// MAX_TURNS softcap (08-18-max-turns-softcap, 2026-08-19):
/// record a `turn_limit_softcap` audit row. Mirrors
/// [`record_loop_intervention_audit`] (best-effort, warn+swallow on
/// DB error — a failed audit write must not break the agent loop).
///
/// Fired at the softcap ask helper's branches in `chat_loop.rs`:
///
/// - `action = "asked"` —— `QuestionStore::register` 成功后立即落
///   (同 C2+ 先例);
/// - `action = "continued"` —— 用户选「继续(+200 轮)」;
/// - `action = "compacted_continued"` —— 用户选「压缩后续跑」;
/// - `action = "stopped"` —— 用户选「停止」/ 未匹配畸形载荷 /
///   用户跳过卡片 / register `AlreadyPending` 降级(降级为今日
///   max_turns 硬停行为);
/// - `action = "timeout_stopped"` —— 询问超时(`softcap_ask_timeout`,
///   缺省 10min)无响应,按决议停止;
/// - `action = "cancelled"` —— pending 期间用户 Stop(取消令牌)
///   或 oneshot dropped。
///
/// `turn` 是撞线时的 turn 号(budget+1);`budget` 是撞线时的
/// turn 预算(缺省 `MAX_TURNS` = 200,每次「继续」+200)。
pub async fn record_turn_limit_softcap_audit(
    db: &SqlitePool,
    session_id: &str,
    turn: usize,
    budget: usize,
    action: &str,
    turn_seq: Option<i64>,
) -> Result<(), sqlx::Error> {
    let payload = serde_json::json!({
        "turn": turn,
        "budget": budget,
        "action": action,
    });
    let payload_str = payload.to_string();
    crate::db::record_audit_event(
        db,
        session_id,
        AuditKind::TurnLimitSoftcap.as_str(),
        Some(&payload_str),
        turn_seq,
    )
    .await
}

/// unified-context-budget WP2 (08-19-unified-context-budget,
/// 2026-08-19): record a `context_budget_trim` audit row when the
/// 关卡⑤ hard gate actually trimmed something. Best-effort semantics
/// mirror the softcap helper (warn + swallow on DB error — a failed
/// audit write must not break the turn).
///
/// `arms_json` is the serialized `[{kind, count, tokens_freed}]`
/// list from `budget::BudgetTrimReport`; `over_by` / `pre_total` /
/// `post_total` / `window` carry the unified-estimate accounting
/// (prd R8/D9 — trace 列记实发值,预裁值只活在 audit payload)。
#[allow(clippy::too_many_arguments)]
pub async fn record_context_budget_trim_audit(
    db: &SqlitePool,
    session_id: &str,
    arms_json: &str,
    over_by: u32,
    pre_total: u32,
    post_total: u32,
    window: u32,
    turn_seq: Option<i64>,
) -> Result<(), sqlx::Error> {
    let payload = serde_json::json!({
        "arms": serde_json::from_str::<serde_json::Value>(arms_json)
            .unwrap_or(serde_json::json!([])),
        "over_by": over_by,
        "pre_total": pre_total,
        "post_total": post_total,
        "window": window,
    });
    let payload_str = payload.to_string();
    crate::db::record_audit_event(
        db,
        session_id,
        AuditKind::ContextBudgetTrim.as_str(),
        Some(&payload_str),
        turn_seq,
    )
    .await
}

/// B9+ D4 (2026-07-13): record a `ui_diff_applied` audit row.
///
/// Mirrors [`record_tool_executed_audit`] / [`record_message_resend_audit`]
/// but for the **user-triggered** "应用 diff" path: user clicks Apply on a
/// `DiffPrimitive` / `ButtonPrimitive`, the frontend invokes
/// `apply_ui_diff`, the backend parses the unified diff and writes one or
/// more files. After all writes succeed, this helper records the success
/// audit row with the affected file list.
///
/// **Best-effort** (same contract as the other audit helpers): DB write
/// failures are `warn!`-logged and never propagated. The user has
/// already seen the visual confirmation (toast / card "已应用"); audit
/// loss is acceptable.
///
/// **Distinct from `ToolExecuted`**: the source is **user click**, not
/// LLM `tool_use`. `session_id` is the same, but the C4 audit log UI
/// can disambiguate via the `kind` discriminator + the `files` payload
/// (no `tool_name` / `tool_input` / `duration_ms` / `exit_code`).
///
/// `files` is the success summary from the apply handler:
/// `&[(path, added, removed)]`. Each entry's `path` is the
/// canonical-form path the boundary check accepted (post-canonicalize).
/// `added` / `removed` are `usize` from `apply_to_file`.
///
/// Files are capped at 32 entries in the audit row to match the
/// audit-log UI's row-size budget (a pathological 100-file diff is
/// still readable; the audit row just doesn't list every entry).
/// `total_files` records the true count so the UI can show "+N more"
/// when truncated.
pub async fn record_ui_diff_applied_audit(
    db: &SqlitePool,
    session_id: &str,
    files: &[(String, usize, usize)],
    turn_seq: Option<i64>,
) -> Result<(), sqlx::Error> {
    let files_summary: Vec<serde_json::Value> = files
        .iter()
        .take(32)
        .map(|(path, added, removed)| {
            serde_json::json!({
                "path": path,
                "added": added,
                "removed": removed,
            })
        })
        .collect();
    let payload = serde_json::json!({
        "files": files_summary,
        "total_files": files.len(),
    });
    let payload_str = payload.to_string();
    crate::db::record_audit_event(
        db,
        session_id,
        AuditKind::UiDiffApplied.as_str(),
        Some(&payload_str),
        turn_seq,
    )
    .await
}

/// F2 定时任务 (2026-08-28): record a `scheduled_task_fired` audit row
/// for the daemon scheduler's decision branches. Mirrors the other
/// best-effort helpers (warn + swallow on DB error).
///
/// Fired from `scheduler/mod.rs` (tick branches: `fired` / `catchup` /
/// `skipped_dedup` / `skipped_queue_disabled` / `error`) and from the
/// two Stop-语义 queue-clear sites (`agent/chat.rs` driver cancel break +
/// `commands/cancel.rs`) with `action = "lost"`. The row hangs off the
/// **target session** (best-effort, non-transactional — same convention
/// as `record_message_resend_audit`).
///
/// `turn_seq` is `None`: scheduler fires happen outside any turn loop
/// (the injected turn's own seq is owned by the agent loop, not the
/// scheduler) — the audit schema allows NULL `turn_seq`.
///
/// `reason` is `Some` only for the `error` action (e.g. `queue_full`).
pub async fn record_scheduled_task_audit(
    db: &SqlitePool,
    session_id: &str,
    task_id: &str,
    task_name: &str,
    action: &str,
    reason: Option<&str>,
) -> Result<(), sqlx::Error> {
    let mut payload = serde_json::json!({
        "task_id": task_id,
        "task_name": task_name,
        "action": action,
    });
    if let Some(r) = reason {
        payload["reason"] = serde_json::json!(r);
    }
    let payload_str = payload.to_string();
    crate::db::record_audit_event(
        db,
        session_id,
        AuditKind::ScheduledTaskFired.as_str(),
        Some(&payload_str),
        // 调度器 fire 在任何 turn loop 之外,无 turn_seq 可挂(签名
        // 的零值 = None,不留 TODO,design §4)。
        None,
    )
    .await
}
