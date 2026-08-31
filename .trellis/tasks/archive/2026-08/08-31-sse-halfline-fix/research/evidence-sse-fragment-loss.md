# 取证:SSE 丢分片事故(2026-08-31)

## 数据面(everlasting.db)

- 全库 275 条 `is_error` tool_result 中,**30 条 + 1 条权限超时壳 = SSE 家族**:
  - 24 条直接 `Missing required parameter: path/command/pattern`,配对检查 tool_use **input 100% 为 `{}`**;
  - 6 条被记忆召回前缀(`⚠️ Memory: …edit_file fails with 'has changed on disk'…`)包裹,尾部仍是 `Missing required parameter: path`(session `5df29977` seq 66/82/102/104/106/108);
  - 时间跨度 **08-06 → 08-31(当天仍在发生)**,涉及 8 个 session;provider 均为 `api.wukaijin.com`(OpenAI 兼容聚合)。
- 决定性反证「模型能力不足」:session `d6728b3a` seq 91 同一 assistant 消息**并行两个 edit_file,一个 input:{}、一个参数完整**——同轮同模型,丢失与参数内容无关,与流式分片时机有关。
- 静默变体实证(同日):
  - 4 条 `sh: Syntax error: Unterminated quoted string`:命令里出现孤立单引号——`app/src-tauri' | grep`(seq 67)、`grep -v ' | head -20`(seq 129)、`grep -a-z_]+::…`(seq 23)、`2>/dev/null; 29:'; grep -c`(seq 415),均为中段文本连同闭合引号被丢;
  - 12 条 `old_string not found`:session `b374ac02` seq 209 与 213 是**同一处编辑的两次重试,第二次丢了 `tools/tool_output.rs`(C6` 一段**(重试丢的位置不同,丢哪由 TCP 时机决定);`d6728b3a` seq 219 old_string `- 118 个command]` handler` 缺 `原 \`#[tauri::` 段。

## 日志面(~/.local/state/dev.everlasting.app/daemon.log,08-31)

- `openai: failed to parse SSE data JSON` **255 次**,错误均为 `EOF while parsing a string at column N`——SSE data 行被拦腰截断的直证(被丢 chunk 带 `router-*` id);
- `openai: failed to parse tool_call arguments JSON, using empty object` **17 次**,args_buf 呈三种残缺形态:开头缺 `{`/key(`key must be a string at column 2`)、字符串值中途断句直接跳 `, "path"`(`expected , or }`)、结尾截断(`EOF while parsing a string`)。

## 代码链路

1. [sse.rs:37-77 `SseParser::feed`](../../../../app/src-tauri/src/llm/sse.rs):`chunk.split('\n')` 逐行处理,**无行缓冲**——chunk 尾部不完整行被当完整行:
   - 残余以 `data:` 开头 → 内容提前入 data_buf;下一 chunk 的后续片段不以 `data:` 开头 → 落入 "Anything else is malformed; drop silently"(sse.rs:74);
   - 现有测试 `buffers_across_chunks` 只在**行边界**切分,未覆盖半行场景——bug 因此长期潜伏。
2. [openai.rs:786-795](../../../../app/src-tauri/src/llm/provider/openai.rs):截断 event 的 `serde_json::from_str` 失败 → `continue` **静默丢弃整个 chunk**(仅 DEBUG 日志),`function.arguments` 分片永久丢失。
3. [streaming.rs:82-93 `build_tool_call_event`](../../../../app/src-tauri/src/llm/provider/streaming.rs):幸存分片拼出的 args_buf 非法 → 兜底 `json!({})` → 工具层报 `Missing required parameter`。

排除项:RULE-D-007「缺 index 的 tool_call delta 跳过」日志 0 次命中,与本事故无关;`tool_call buffer has no name`、`non-utf8 chunk` 均 0 次(utf8 跨 chunk carry 已有修复)。

## 修复方向

`feed()` 只消费完整行,半行残余进实例缓冲下次拼接(参考官方 OpenAI SDK 的 eventsource 行为)。一个点修复同时消掉显性(input={})与静默(参数/正文缺段)两类症状。
