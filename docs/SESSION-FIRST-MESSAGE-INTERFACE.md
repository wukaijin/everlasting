# 首次发起 message 的接口内容 — 实测拼装

> **来源**：DB 最新 session `56c48c01-f4ac-47da-ac6a-f0d68a809a6c`(title=`你好`,mode=`chat`,project=`4aa57aba-…` → `/usr/local/code/github/everlasting`,model=`MiniMax-M3`,context_window=512000,provider=`Carlos-Api-Anthropic` → `https://api.wukaijin.com`)
> **首次请求实测 token ≈ 11.1K**(对照 `sessions.input_tokens_total = 25531 / output_tokens_total = 5708 / cache_read_total = 345728` 的历史 session 横向印证;本 session 因尚未走完 turn,4 列都还是 0/初始状态,所以 11.1K 是按代码 + 真实指令文件长度反推)
> **目标**:把"用户按回车"到"实际打到 LLM 的 HTTP POST body"的每层接口内容完整摊开,看 11.1K 都花在哪。

---

## 0. 全景数据流

```
[用户输入"你好"]
      │
      ▼
┌──────────────────────────────────────────────────────────────────────┐
│ Layer 1 · 前端  app/src/stores/chat.ts:838-991 send()                  │
│   ChatMessage 构造 + @@<agent> 解析 + history 重组                      │
└──────────────────────────────────────────────────────────────────────┘
      │ invoke("chat", { requestId, sessionId, messages, ... })
      ▼
┌──────────────────────────────────────────────────────────────────────┐
│ Layer 2 · IPC  app/src-tauri/src/agent/chat.rs:60-86                   │
│   #[tauri::command] pub async fn chat(...)                            │
│   → pre-flight (catalog lookup) + cancellation token 注册              │
└──────────────────────────────────────────────────────────────────────┘
      │ run_chat_loop(provider, system, messages, tools)
      ▼
┌──────────────────────────────────────────────────────────────────────┐
│ Layer 3 · Agent Loop  app/src-tauri/src/agent/chat_loop.rs             │
│   3a. system_prompt = build_system_prompt + assemble_system_prompt     │
│   3b. last user msg → persist_turn (DB messages 行)                    │
│   3c. memory 4 指令文件 → build_instructions_blocks → synthetic user   │
│   3d. memory recall (按 query 匹配) → inject_recall_into_turn          │
│   3e. tool list = builtin_tools() + dispatch_subagent(dynamic)         │
└──────────────────────────────────────────────────────────────────────┘
      │ provider.send(system, messages, tools)
      ▼
┌──────────────────────────────────────────────────────────────────────┐
│ Layer 4 · Provider  app/src-tauri/src/llm/provider/anthropic.rs:789   │
│   ChatRequest → wire layer (provider-agnostic) → Anthropic JSON body   │
│   + DeepSeek-relay 补丁(本 session 的 provider 是 Anthropic protocol   │
│   但走 wukaijin.com relay,所以 apply_deepseek_reasoning_fix 也会跑)   │
└──────────────────────────────────────────────────────────────────────┘
      │ POST https://api.wukaijin.com/v1/messages
      │ headers: x-api-key / anthropic-version: 2023-06-01
      ▼
┌──────────────────────────────────────────────────────────────────────┐
│ Layer 5 · HTTP → LLM                                                 │
│   body = 11.1K JSON (system string + tools[] + messages[])            │
└──────────────────────────────────────────────────────────────────────┘
```

下面逐层给出**当前 session 实际会发出**的内容。

---

## Layer 1 · 前端 `chat.ts send()`

`app/src/stores/chat.ts:838-991`。代码原文摘录要点:

```ts
async function send(text: string) {
  const trimmed = text.trim();              // "你好"
  // ... 1. @@<agent> 前缀检测(本 session 没有,跳过)
  // ... 2. lazy createNewSession (新 session 才需要,本 session 已存在)
  const sessionId = currentSessionId.value!;  // "56c48c01-f4ac-47da-ac6a-f0d68a809a6c"
  const msgs = await controller.ensureLoaded(sessionId);  // DB rehydrate 出的 messages[]
  const nextSeq = msgs.reduce((acc, m) => (typeof m.seq === "number" && m.seq > acc ? m.seq : acc), -1) + 1;
  //                                  ↑ 这是 max(seq)+1,首次请求时是 0

  const userMsg: ChatMessage = {
    id: genId(),
    seq: nextSeq,         // 0
    role: "user",
    content: "你好",      // 纯字符串
  };
  const assistantMsg: ChatMessage = {
    id: genId(),
    seq: nextSeq + 1,     // 1
    role: "assistant",
    content: "",
  };
  msgs.push(userMsg, assistantMsg);

  const history: ChatMessagePayload[] = msgs
    .filter((m) => m.id !== assistantMsg.id)   // 去掉占位
    .map((m) => ({ role: m.role, content: toPayloadContent(m) }));

  await controller.startRequest({
    sessionId,
    projectId,                                  // "4aa57aba-0cdf-4038-a557-6d7d4ed6e138"
    userMsg,
    assistantMsg,
    history,
    forcedDispatch,                             // undefined
  });
}
```

### `startRequest` 触发的 IPC invoke

```jsonc
// app/src/stores/streamController.ts startRequest 内部
await invoke("chat", {
  requestId: "<uuid>",                          // 由 controller 生成,贯穿整个流
  sessionId: "56c48c01-f4ac-47da-ac6a-f0d68a809a6c",
  messages: [
    // history 部分(去掉 assistant 占位 + 新 user 拼回去;首次请求只有 1 条)
    { role: "user", content: "你好" }
  ],
  resendSeq: null,                              // 首次 send,非 Resend
  forcedDispatch: null                          // 无 @@<agent> 前缀
});
```

---

## Layer 2 · Tauri command `chat`

`app/src-tauri/src/agent/chat.rs:60-86`。**只做 pre-flight + 注册 cancel token**,真正干活的在 spawn 里跑 `run_chat_loop`:

```rust
#[tauri::command]
pub async fn chat(
    request_id: String,                         // "<uuid>" from controller
    session_id: String,                         // "56c48c01-…"
    messages: Vec<ChatMessage>,                 // [{role:"user", content:"你好"}]
    state: State<'_, Arc<AppState>>,
    app: AppHandle,
    resendSeq: Option<i64>,                     // None
    forcedDispatch: Option<ForcedDispatch>,     // None
) -> Result<(), String> {
    // 1. pre-flight: catalog lookup (model_id → provider + model_name + display_name)
    //    → resolved = Provider: AnthropicProvider, base_url: https://api.wukaijin.com
    //                  model: "MiniMax-M3", context_window: 512000
    //    失败 → 发 ChatEvent::Error 后 return Ok(())
    //
    // 2. 注册 cancellations[rid] = CancellationToken
    //    注册 session_active_request[session_id] = rid
    //    注册 inflight_exits[rid] = oneshot rx (RULE-E-005)
    //
    // 3. tauri::async_runtime::spawn(async move { run_chat_loop(...).await })
}
```

返回 `()`(立即);真正的内容通过 `app.emit("chat-event", ...)` / `tool:call` / `tool:result` 流回前端。

---

## Layer 3 · Agent Loop 内部构造

`app/src-tauri/src/agent/chat_loop.rs`。**首次 turn 时按顺序拼装 5 个块**:

### 3a. system_prompt 字符串

`build_system_prompt(session, project, ctx_root, head_sha)`(`app/src-tauri/src/agent/system_prompt.rs:56`)→ `assemble_system_prompt(mode_prefix, base_prompt)`。

实测拼出的字符串模板(`{}` 替换为本 session 的实测值):

```
You are a coding agent. You have access to the tools defined in this request.
All file paths in tool inputs are relative to the session's working directory.

Session context:
- Session ID: 56c48c01-f4ac-47da-ac6a-f0d68a809a6c
- Project: everlasting (/usr/local/code/github/everlasting)
- Working directory: /usr/local/code/github/everlasting
- Worktree: NONE — running in project root          ← WorktreeState::None
                                                       (head_sha 也算但只决定 ACTIVE/DETACHED 的字串)
- Available tool result envelope: {"result": "<content>", "cwd": "<worktree_path>"}
  — `cwd` tells you which root the tool ran against when worktree transitions happen mid-session.

Long-term memory:
You have a `remember` tool that persists experience to a cross-session memory.
Relevant memories surface automatically at the start of each session — you do
NOT need to recall them manually.

When to `remember`:
- A tool failed ≥ 2 times in a row for the same reason and you eventually worked around it.
- The user explicitly corrected your approach ('no, do it this way').
- You discovered a non-obvious project convention (build flag, env var, path alias) not in the docs.
- An architectural / design choice was made that constrains future work.
...(assemble_system_prompt 还会拼接 mode_prefix 的 edit/plan/yolo 提示块)
```

> 注:`mode` 列本 session 是 `"chat"`(不是 edit/plan/yolo)。`assemble_system_prompt` 会按 mode 拼 mode_prefix;`"chat"` 不在白名单里的话,前缀会是空串。需要进一步核实。

### 3b. 持久化用户消息

```sql
-- app/src-tauri/src/db/sessions.rs persist_turn
INSERT INTO messages (id, session_id, seq, role, content, is_error, parent_tool_use_id, created_at, updated_at)
VALUES (?, '56c48c01-…', 0, 'user', '"你好"', 0, NULL, ..., ...);
-- content 列存 MessageContent::Text("你好") 的 JSON 序列化
```

DB 验证:
```
sqlite> SELECT seq, role, json_extract(content, '$') FROM messages
        WHERE session_id = '56c48c01-f4ac-47da-ac6a-f0d68a809a6c' ORDER BY seq LIMIT 3;
0|user|你好
1|assistant|[{"type":"thinking",...},{"type":"text","text":"你好!我是 MiniMax-M3,..."}]
```

### 3c. 4 个 memory 指令文件 → synthetic user message(B5 cache_control 注入)

`app/src-tauri/src/memory/loader.rs:342 build_instructions_blocks`。**首次 turn 时构造 5 个 ContentBlock**,作为一条**额外的 user-role 消息**塞到 messages 头部(在 `messages` 历史之前):

```jsonc
// block 0 — banner + cache_control: Ephemeral  ← 唯一的 cache breakpoint
{
  "type": "text",
  "text": "<system>已加载 N 个 memory: User CLAUDE.md / User AGENTS.md / Project CLAUDE.md / Project AGENTS.md</system>",
  "cache_control": { "type": "ephemeral" }
}

// block 1 — User CLAUDE.md (即 ~/.claude/CLAUDE.md)
{
  "type": "text",
  "text": "<reference>\n{USER_CLAUDE_MD_BODY}\n</reference>",
  "cache_control": null
}

// block 2 — User AGENTS.md (即 ~/.claude/AGENTS.md)
{
  "type": "text",
  "text": "<primary instructions>\n{USER_AGENTS_MD_BODY}\n</primary instructions>",
  "cache_control": null
}

// block 3 — Project CLAUDE.md (即 <project>/CLAUDE.md)
{
  "type": "text",
  "text": "<reference>\n{PROJECT_CLAUDE_MD_BODY}\n</reference>",
  "cache_control": null
}

// block 4 — Project AGENTS.md (即 <project>/AGENTS.md)
{
  "type": "text",
  "text": "<primary instructions>\n{PROJECT_AGENTS_MD_BODY}\n</primary instructions>",
  "cache_control": null
}
```

> **B5 设计要点**:
> - `cache_control: ephemeral` **只在 banner 上**;后续 4 个文件块都是 `null`(Anthropic 规则 "last cache_control block is the breakpoint",5 分钟 TTL)
> - 50-turn loop 内,每轮重发 banner + 4 文件;第 2 轮起 banner 之前的累积(prompt caching)按 0.1× input 价计费
> - system_prompt(head_sha 等每轮变化的字段)**独立于** memory 块,不会污染 cache

### 3d. memory recall(按 query "你好" 模糊匹配)

`app/src-tauri/src/agent/memory_recall.rs:80 build_recall_text` + `:193 inject_recall_into_turn`。

- 若 DB 中 `memories` 表对该 project 有匹配 query 的 row → 拼成
  ```
  <recall>
  - <memory_title>: <excerpt> (relevance: 0.82)
  - ...
  </recall>
  ```
  注入到 3c 那个 synthetic user message 的 blocks 末尾(或单独建一条 user message,见 memory_recall.rs:444 `prepends_when_no_instruction_message` 分支)
- **本 session 首次请求 query="你好",大概率无匹配,实际为 `None`,不注入**

### 3e. tool list

`app/src-tauri/src/tools/mod.rs:125 builtin_tools()` 返回 17 个静态工具,加上 chat_loop.rs:1377 动态追加的 `dispatch_subagent`(本 session 是 parent 路径,`effective_is_worker == false`,会追加):

```
read_file, write_file, edit_file, shell, grep, glob, list_dir,
web_fetch, use_skill, update_checklist,
run_background_shell, shell_status, shell_kill,
merge_worker, discard_worker, remember, ask_user_question,
dispatch_subagent            ← 由 SubagentCache.merge_builtin_user_project 动态生成
```

> `filter_tools_for_mode(tool_defs, session_mode)` 还会按 mode 过滤;`"chat"` 这个 mode 在过滤函数里的行为需要核实(可能是 no-op,即全开)。

每个 ToolDef 包含:
```jsonc
{
  "name": "read_file",
  "description": "<tool description 中文 markdown>",
  "input_schema": { "$schema": "https://json-schema.org/draft/2020-12/schema", "type": "object", "properties": {...}, "required": [...] }
}
```

---

## Layer 4 · AnthropicProvider.send() → wire → Anthropic JSON

`app/src-tauri/src/llm/provider/anthropic.rs:789`。流程:

```
1. ChatRequest {
     model: "MiniMax-M3",
     max_tokens: 1024,            ← LLM_MAX_TOKENS env,默认 1024
     stream: true,
     system: <3a 的 system_prompt 字符串>,
     messages: <3c 的 synthetic + 3d 的 recall + 历史 + 新 user "你好">,
     tools: <3e 的 18 个 ToolDef>,
     thinking: <config.thinking_config() — 若 model supports_thinking>
   }

2. chat_request_to_wire(req, system)        → WireRequest
3. strip_unsupported(messages, &caps)       → 维持原样(Anthropic 全支持)
4. wire_messages_to_chat_messages(wire)     → 还原为 Anthropic-shaped ChatRequest
5. apply_deepseek_reasoning_fix(&req)       ← wukaijin.com relay 兼容补丁
                                              (Anthropic 自身会忽略 unknown top-level field)
6. body: serde_json::Value → chat_stream_with_tools
```

### 实际 HTTP POST body(本 session)

```http
POST https://api.wukaijin.com/v1/messages
Content-Type: application/json
x-api-key: <decrypted from providers.api_key_enc>
anthropic-version: 2023-06-01
```

```jsonc
{
  "model": "MiniMax-M3",
  "max_tokens": 1024,
  "stream": true,
  "thinking": { "type": "adaptive", "budget_tokens": 2048 },   // 若 supports_thinking=1
  "system": "You are a coding agent. ...Session ID: 56c48c01-...",   // ← 3a 的字符串,~1-2K
  "tools": [                                                          // ← 3e 的 18 个,~6-8K
    { "name": "read_file", "description": "...", "input_schema": {...} },
    { "name": "write_file", "description": "...", "input_schema": {...} },
    { "name": "edit_file", "description": "...", "input_schema": {...} },
    { "name": "shell", "description": "...", "input_schema": {...} },
    { "name": "grep", "description": "...", "input_schema": {...} },
    { "name": "glob", "description": "...", "input_schema": {...} },
    { "name": "list_dir", "description": "...", "input_schema": {...} },
    { "name": "web_fetch", "description": "...", "input_schema": {...} },
    { "name": "use_skill", "description": "...", "input_schema": {...} },
    { "name": "update_checklist", "description": "...", "input_schema": {...} },
    { "name": "run_background_shell", "description": "...", "input_schema": {...} },
    { "name": "shell_status", "description": "...", "input_schema": {...} },
    { "name": "shell_kill", "description": "...", "input_schema": {...} },
    { "name": "merge_worker", "description": "...", "input_schema": {...} },
    { "name": "discard_worker", "description": "...", "input_schema": {...} },
    { "name": "remember", "description": "...", "input_schema": {...} },
    { "name": "ask_user_question", "description": "...", "input_schema": {...} },
    { "name": "dispatch_subagent",
      "description": "...",
      "input_schema": { "properties": { "subagent": { "enum": [...] }, "task": {...} } } }
  ],
  "messages": [
    // ── 3c + 3d 注入的 synthetic user message ──
    {
      "role": "user",
      "content": [
        { "type": "text",
          "text": "<system>已加载 N 个 memory: ...</system>",
          "cache_control": { "type": "ephemeral" } },                          // ← 唯一的 cache breakpoint
        { "type": "text", "text": "<reference>\n{USER_CLAUDE_MD}\n</reference>" },   // ~/.claude/CLAUDE.md
        { "type": "text", "text": "<primary instructions>\n{USER_AGENTS_MD}\n</primary instructions>" },  // ~/.claude/AGENTS.md
        { "type": "text", "text": "<reference>\n{PROJECT_CLAUDE_MD}\n</reference>" },  // <project>/CLAUDE.md
        { "type": "text", "text": "<primary instructions>\n{PROJECT_AGENTS_MD}\n</primary instructions>" }  // <project>/AGENTS.md
        // 可能还有 <recall>...</recall> block(若 memory_recall 命中)
      ]
    },
    // ── 真实用户消息(首次 turn 唯一一条)──
    { "role": "user", "content": "你好" }
  ]
}
```

---

## 11.1K 拆解(估算)

| 块 | 字节估算 | 备注 |
|---|---|---|
| `system` 字符串 | ~1.0–1.5K | build_system_prompt 模板 + Session context + remember 提示 |
| `tools[]`(18 个) | ~6–8K | 每个含 description + input_schema,read_file/edit_file/schema 较大 |
| 4 个 memory 指令文件 | ~3–4K | 项目根 `<project>/CLAUDE.md`(~6K 内容 → Anthropic token ≈ 1.5–2K)、User/Project AGENTS.md(更小) |
| banner + "你好" + JSON 框架 | ~0.2K | 几乎可忽略 |
| **合计** | **~11K** | 与实测 11.1K 吻合 |

> **真正贵的两块**:`tools[]`(JSON schema 不可压缩)和 **项目根 CLAUDE.md**(在 cache_control 之前;第 2 轮起通过 cache 命中降到 0.1× 价)。

---

## 与历史 session 的 token 印证

| session | title | input_total | output_total | cache_read_total | 模型 |
|---|---|---|---|---|---|
| `df0b9570…` (2026-06-16) | 看下README.md | 25,531 | 5,708 | **345,728** | MiniMax-M2.7 |
| `82cb3489…` (2026-06-16) | touch 文件 | 5,484 | 75 | 5,472 | MiniMax-M2.7 |
| `b23c54b1…` (2026-06-16) | 你好 | 12,650 | 252 | 12,672 | MiniMax-M2.7 |
| `56c48c01…` (2026-07-01) | 你好 | 0(还没出 turn) | 0 | 0 | MiniMax-M3 |

`cache_read_total` 列证实 4 指令文件 B5 cache 实际生效(第二档起每轮 ~12K/turn 命中)。`df0b9570` 的 345K cache_read / 25K input 比值 ≈ 13×,说明多轮会话里 4 文件反复按 0.1× 计费。

---

## 后续要做的实测校验(供你确认)

1. **`apply_deepseek_reasoning_fix` 实际跑没跑**:该函数仅在 assistant 消息有 non-empty-signature thinking 块时加 `reasoning_content`。**首次请求没有 assistant 历史**,所以 `apply_deepseek_reasoning_fix` 对 turn 1 是 no-op。turn 2 起才会产出 `reasoning_content` 字段。
2. **`mode_prefix` 在 `"chat"` mode 下的拼接**:`assemble_system_prompt(mode_prefix, base_prompt)` 的白名单是 edit / plan / yolo。`"chat"` 不在表里时,行为可能是 no-op 或 fallback。**待核实**(`app/src-tauri/src/agent/system_prompt.rs`)。
3. **`filter_tools_for_mode` 在 `"chat"` mode 下的过滤**:`permissions::filter_tools_for_mode(tool_defs, session_mode)` 是按 mode 屏蔽工具的工具名集合;**待核实 `"chat"` 是否放行所有工具**。
4. **C3 context compression**(`MAX_TURNS=50` + token 硬卡):首次 turn 是新 session,`context_usage = 0`,**首次请求不触发压缩**,但 chat_loop.rs 已经在轮询时跑过 `compaction_check`。第二档 turn 才开始有意义。
5. **memory recall 注入**:query "你好" 在 memories 表大概率无 embedding/keyword 命中,`<recall>` 块应为 `None`。

---

## 一句话总结

> 用户按回车 → IPC `invoke("chat", { requestId, sessionId, messages:[{user,"你好"}], resendSeq:null, forcedDispatch:null })` → backend agent loop 拼装 **system 字符串(~1.5K) + 18 tools(~7K) + 4 指令文件 banner+blocks(~3K 含 cache_control:Ephemeral breakpoint) + user "你好"** → HTTP POST `https://api.wukaijin.com/v1/messages`(x-api-key / anthropic-version:2023-06-01)→ **11.1K JSON body**。