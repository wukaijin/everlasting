# B6 PR3 跨层 Wire Shape 契约（前端 mirror 依据）

> 来源：trellis-check 于 2026-06-20 从后端源码 verbatim 提取。
> 后端已验证绿（732 tests pass，0 new warning）。
> 前端 PR3b 必须逐字 mirror 以下 serde 序列化结果。

## 1. `SubagentRunSummary` — `list_subagent_runs_by_session(sessionId)` 返回数组元素

`app/src-tauri/src/db/subagent_runs.rs`（`#[serde(rename_all = "camelCase")]`）：

```ts
interface SubagentRunSummary {
  id: string;
  parentSessionId: string;
  parentRequestId: string;
  subagentName: string;
  status: "running" | "completed" | "cancelled" | "error"; // typed enum, lowercase
  startedAt: string;
  finishedAt: string | null;
  tokenUsageJson: string | null;
  summary: string | null;
}
```

`SubagentStatusDb` enum：`#[serde(rename_all = "lowercase")]` → `"running" | "completed" | "cancelled" | "error"`。

## 2. `SubagentRunRow` — `get_subagent_run(runId)` 返回

```ts
interface SubagentRunRow {
  id: string;
  parentSessionId: string;
  parentRequestId: string;
  subagentName: string;
  status: string;            // ⚠️ 原始 String，不是 typed enum（与 Summary 不对称！）
  startedAt: string;
  finishedAt: string | null;
  tokenUsageJson: string | null;
  summary: string | null;
  transcriptJson: string | null;     // camelCase
  transcriptTruncated: number;       // camelCase（i64 → number）
  createdAt: string;
}
```

**⚠️ Drift 陷阱 1**：`SubagentRunRow.status` 是原始 `String`，`SubagentRunSummary.status` 是 typed enum。前端需统一处理（建议都 coerce 成 `"running" | "completed" | "cancelled" | "error"` 联合类型）。

## 3. `subagent:event` IPC payload — SubagentBufferSink emit

`app/src-tauri/src/agent/subagent.rs:421` `build_subagent_event_payload`：

```ts
interface SubagentEventPayload {
  runId: string;          // camelCase
  sessionId: string;      // camelCase
  kind: "chat_event" | "tool_call" | "tool_result" | "permission_ask";  // ⚠️ snake_case 字符串
  payload: Record<string, unknown>;
  timestamp: string;      // RFC 3339
}
```

## 4. `TranscriptKind` enum 序列化值

`#[serde(rename_all = "snake_case")]`：

```ts
type TranscriptKind = "chat_event" | "tool_call" | "tool_result" | "permission_ask";
```

已被 `build_subagent_event_payload_kind_strings_match_enum` 测试锁定。

## 5. `TranscriptEntry` — `transcript_json` 数组元素

`app/src-tauri/src/agent/subagent.rs:382`（**结构体无 `rename_all`**）：

```ts
interface TranscriptEntry {
  kind: TranscriptKind;
  payload_json: Record<string, unknown>;   // ⚠️ 保持 snake_case，不是 payloadJson！
}
```

**⚠️ Drift 陷阱 2**：`TranscriptEntry` 存的是 DB 存储格式，字段保持 snake_case（`payload_json`）；而 `subagent:event` 的 IPC payload 是为前端重新封装的 camelCase（`payload`）。**两套 shape 不同**，前端解析 `transcript_json`（来自 `get_subagent_run`）时用 `payload_json`，解析 `subagent:event` 实时流时用 `payload`。

`transcript_json` 是 JSON 字符串（`Option<String>`），前端需 `JSON.parse` 得到 `TranscriptEntry[]`。

## 监听 channel

`listen<SubagentEventPayload>("subagent:event", ...)`，来自 `@tauri-apps/api/event`。

## drawer 数据来源优先级（prd R6）

```
store.liveTranscript.get(openRunId)        // 实时流（worker 运行中）
  ?? (store.getRunCache.get(openRunId)?.transcriptJson 解析出的 TranscriptEntry[])  // 完成后 DB cache
  ?? []
```
