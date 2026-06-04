# Everlasting — App (Step 1)

> Tauri 2 + Vue 3 + Pinia 的最小聊天 app,直连 Anthropic Messages API 兼容端点。

## 跑起来

需要的环境变量(参考 `~/sse-spike/Cargo.toml` 同样的协议):

```bash
export ANTHROPIC_BASE_URL=https://api.wukaijin.com   # 可选,默认 api.anthropic.com
export ANTHROPIC_API_KEY=sk-...                      # 必填(或 ANTHROPIC_AUTH_TOKEN)
export LLM_MODEL=GLM-4.7                             # 可选,默认 GLM-4.7
export LLM_MAX_TOKENS=1024                           # 可选,默认 1024
```

启动:
```bash
cd /usr/local/code/github/everlasting/app
pnpm tauri dev
```

第一次跑会下载 Tauri 2 CLI + 编译 Rust 端(可能 3-5 分钟),之后增量编译很快。

## 当前能力(MVP Step 1)

- ✅ 输入中文 → 流式显示响应
- ✅ 错 API key → 友好错误(中文提示,不崩)
- ✅ 多次连续对话(前端持有消息历史,每次重发全量)
- ✅ 未知 SSE 事件不崩(per HACKING-llm "额外观察")
- ✅ 错误归一化 5 类:Auth / RateLimit / InvalidRequest / Server / Network
- ✅ 嵌套 JSON 错误体容错(GLM 双层 wrapper)

## 目录结构

```
app/
├── package.json          # Vue 3 + Vite + Pinia + reka-ui + @tauri-apps/api
├── vite.config.ts        # port 1420, strict
├── tsconfig.json
├── index.html
├── src/
│   ├── main.ts           # createApp + Pinia
│   ├── App.vue
│   ├── style.css         # 全局样式 + CJK 字体栈
│   ├── stores/
│   │   └── chat.ts       # Pinia 状态:消息 + 流式监听
│   └── components/
│       └── ChatWindow.vue
└── src-tauri/
    ├── Cargo.toml
    ├── tauri.conf.json   # identifier=com.wukaijin.everlasting, productName=Everlasting
    ├── build.rs
    ├── icons/
    ├── capabilities/
    │   └── default.json
    └── src/
        ├── main.rs       # entry
        ├── lib.rs        # Tauri builder + chat command
        └── llm/
            ├── mod.rs
            ├── client.rs # chat_stream + LlmConfig::from_env
            ├── sse.rs    # SseParser(state machine)
            ├── error.rs  # LlmError + classify_error_response
            └── types.rs  # ChatMessage / ChatRequest / ChatEvent
```

## IPC 协议

**前端 → Rust**:`invoke("chat", { requestId, messages })`
- `requestId: string` — UUID-like,前端生成,关联本次流的所有事件
- `messages: Array<{ role, content }>` — 完整历史(包含刚发的用户消息)

**Rust → 前端**:事件 `chat-event`,payload 形如:
```json
{ "request_id": "abc", "kind": "start" }
{ "request_id": "abc", "kind": "delta", "text": "你" }
{ "request_id": "abc", "kind": "delta", "text": "好" }
{ "request_id": "abc", "kind": "done", "stop_reason": null }
{ "request_id": "abc", "kind": "error", "message": "...", "category": "auth" }
```

前端 `listen("chat-event", ...)` 接收,根据 `request_id` 过滤,根据 `kind` dispatch。

## 不做(Step 2+ 才上)

- 工具调用(`read_file` / `write_file` / `shell`)— Step 2
- 多 session / SQLite 持久化 — Step 3
- 中断 / 取消生成 — Step 2(按需)
- 重试(5xx / Network)— Step 2

## 测试

```bash
# Rust 单元测试(SSE parser + error classification)
cd src-tauri && cargo test --lib

# 前端类型检查 + 打包
pnpm build
```
