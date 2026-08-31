# SseParser 半行缓冲:修复 TCP chunk 边界丢 SSE data 行

## Goal

`SseParser::feed` 只消费到最后一个 `\n` 为止的完整行,尾部残余半行留到下一次 feed 拼接;消除 TCP chunk 边界切断 `data:` 行时分片被静默丢弃的问题。

## 问题与影响(取证见 `research/evidence-sse-fragment-loss.md`)

- 现状:`feed()` 用 `chunk.split('\n')` 逐行处理,chunk 尾部的**不完整行**被当完整行消费:
  - 若残余以 `data:` 开头,其内容提前入 `data_buf`;下一 chunk 的后续片段不以任何已知前缀开头,落入 "malformed; drop silently" 分支([sse.rs:74](../../../app/src-tauri/src/llm/sse.rs))被整段丢弃;
  - 截断的 event JSON 在 [openai.rs:786-795](../../../app/src-tauri/src/llm/provider/openai.rs) 解析失败后 `continue`,整个 SSE chunk(含 `function.arguments` / `delta.content` 分片)丢失,仅 DEBUG 级日志。
- 用户可见症状(08-31 实测):
  - **显性**:tool_use `input={}` → `Missing required parameter: path/command/pattern`(全库 30 例、8 个 session、08-06 起持续);
  - **静默**:JSON 仍合法但字符串值中段缺一块 → shell 命令引号不闭合(`Unterminated quoted string`)、edit_file old_string 缺段反复 `old_string not found`(同一编辑重试两次丢不同位置);
  - 长中文参数(文档编辑场景)单 data 行多 KB、跨 chunk 概率高,症状集中。

## Requirements

- `SseParser` 增加行缓冲:`feed()` 处理完所有完整行后,把最后一个 `\n` 之后的残余文本保留到实例状态,下一次 feed 先拼接再解析。
- `reset()` 清空行缓冲(连接中断复用语义不变)。
- `MAX_DATA_BYTES`(RULE-D-003 1MiB cap)语义不变:作用于 event 级 data_buf,不受行缓冲影响。
- 行为兼容:`buffers_across_chunks` 等现有测试全部保持通过(行边界切的用例行为不变)。
- 顺带可观测性:`openai.rs` 的 `failed to parse SSE data JSON` 由 DEBUG 升为 WARN(本次事故该日志 255 次全在 DEBUG,若为 WARN 可更早暴露;修复后正常路径不触发,不会造成日志噪音)。

## Acceptance Criteria

- [ ] 新增回归测试:`data: {...}` 行在任意字节位置切成两次 feed(至少覆盖:行中切断、切断点落在多字节 CJK 字符中间、`data:` 前缀本身被切断、连续多个 event 交错切断),事件完整产出、内容逐字节等于不切断时的结果。
- [ ] 新增回归测试:半行残余 + `reset()` 丢弃后,后续 feed 从干净状态开始。
- [ ] 现有 `sse.rs` 测试全部通过;`cargo test -p everlasting --lib` 全绿。
- [ ] `scripts/turn-smoke.sh` 跑通,daemon 日志无 `failed to parse SSE data JSON` / `failed to parse tool_call arguments JSON`。

## 约束

- 不改 SSE 事件语义(多 data 行 join `\n`、`data:` 无空格容忍、注释/id/retry 忽略等 RULE-D-003 行为全部保留)。
- 不动 Anthropic 路径(同一 parser,修复对其同样受益,无需分叉)。
