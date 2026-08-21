# Scenario: read_file 读图(08-21-b1-image-followups,2026-08-21)

## Contract

`read_file` 对 png/jpg/jpeg/webp(扩展名白名单 + 魔数校验,helper 共享自 at_file)不再走
UTF-8 文本路径:

- ≤5MiB:字节复制进 `attachments/<session_id>/`(副本语义同 @图),`imagesize::blob_size`
  头探测算 `tokens_est=(w×h)/750`,返回 `(文本行 "[image: <path> (w×h) — 已作为图片块发送]",
  false, Some(vec![AttachmentRef{source:"read_file",…}]))`。caps 判断不在 tool 层——降级统一
  在 wire 层(与 B1「caps 消费在适配层」一致)。
- \>5MiB:`("[image: … — 超过 5MB 上限未读取；请压缩或转换后重试]", false, None)` 非 error。
- 魔数不符(文本改名 .png):is_error=true "magic mismatch"(不产图,防毒化 provider 请求)。
- 无 session_id / save 失败:非 error 占位。

`execute` 返回 `(String, bool, Option<Vec<AttachmentRef>>)`;`execute_tool` 5 元组第 5 位
透传(其余工具一律 None)。图片块经 ToolResult 全链上 wire(llm-contract.md §Tool-Result
Image Blocks)。description 已向 LLM 披露该能力(vision 模型可直接看截图)。

## Tests

`cargo test --lib read_file` 五态:命中(副本+tokens_est+文本行)/ 魔数不符 / 超限占位 /
非图扩展老路 / 无 session 占位。
