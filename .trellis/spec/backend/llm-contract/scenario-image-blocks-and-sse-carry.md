<!-- Moved from llm-contract.md 2026-09-19 (doc-split) -->

## Scenario: Image Blocks — dual-form lifecycle (B1, 2026-08-17)

`ContentBlock` 两个图片变体(`08-16-b1-image-multimodal` PR2):

- **`ImageRef { file, media_type }`** — 稳定引用形态(serde tag `image_ref`,内部专用,永不进 provider wire)。存在于历史/role_history clone/C3 估算;`drive.rs` 在 retry_open 前调 `attachments::resolve_image_refs`(每轮每图一次读盘)换成 resolved 形态。**resolve 带不上 session 上下文就会降级占位**——签名显式收 `app_data_dir + session_id`,勿让块脱离请求上下文独立 resolve。
- **`Image { source: ImageSource }`** — resolved 预发形态(serde 即 Anthropic 原生 `{"type":"image","source":{"type":"base64",…}}`,Anthropic adapter serde 直发零转换;OpenAI adapter 映射 `image_url` data URL)。

**Pair Atomicity 不变**:图片只进 user 消息;assistant 消息里的图(防御路径)在 to_wire 降级为文本占位。

**caps 降级(R3)**:`WireCapabilities.supports_images=false` 时 `strip_unsupported` 对 UserBlocks 内 Image **替换**(非丢弃)为 `[image: {label} — 当前模型不支持图片,未发送]` 文本——模型必须知道有图未发,防幻觉。live 实证(08-17,MiniMax-M3):模型读到占位后明确拒答"图片没有送达"。

**When this bites**:① user 消息含图强制 UserBlocks 路径(图不能进 `User{content: String}`);② OpenAI 侧含图 content 必须是数组(text + image_url 混排),无图时保持历史字符串形状(回归锁测试);③ C3 估算对图块用固定 ~1600 tok 垫板(base64 字符串会百倍高估);④ Anthropic cache 断点在首块 text,Image 追加在 user 消息尾部不耦合。


## Scenario: SSE chunk-boundary UTF-8 carry (RULE, 2026-08-18)

**Bug (incident `3qnzktvosvxmsycoz46` turn=25)**:流式 LLM 生成中断,日志
`WARN ... chat: LLM stream errored ... error=network error: non-utf8 chunk:
incomplete utf-8 byte sequence from index 4082`。根因:两个 provider 对每个
网络 chunk 单独 `std::str::from_utf8`,而 TCP/HTTP chunking 会把一个多字节
UTF-8 字符(如 CJK 的 3 字节字)切开分到两个 chunk——chunk 尾部截断属于
"incomplete"(`error_len() == None`)而非损坏,应跨 chunk 拼接,但代码直接
`yield Err(Network)` 杀掉了整轮健康的生成。中文会话高频触发(3 字节/字,
单轮几千 token,25 轮里迟早抽中边界)。

**Rule**:流式 SSE 解码必须跨 chunk carry 残留字节;`error_len().is_none()`
(尾部截断)时缓冲等下一 chunk,只有 `error_len().is_some()`(流内真无效字节)
才报 Network 错。共享 helper 在 `llm/sse.rs` 的 `utf8_chunk_text(&mut carry,
&bytes)`:`Ok(Some(text))` / `Ok(None)`(等待)/ `Err(Utf8Error)`(硬错误)。
两个 provider(`anthropic.rs` / `openai.rs` 的 stream 循环)都必须走它,禁止
对 `bytes_stream()` 的裸 chunk 直接 `from_utf8`。

**When this bites**:① 任何 UTF-8 多字节内容(中文/emoji/数学符号)长输出;
② 代理/网关分包偏小或乱切时概率上升;③ 换 provider 重写流式读取时照抄了
"每 chunk 独立解码"的旧模式。回归测试在 `llm/sse.rs`(CJK 切 3 段、逐字节
喂、ASCII 前缀 + 截断尾、无效字节仍报错)。

## Scenario: Tool-Result Image Blocks — ToolResultData dual-form serde (08-21-b1-image-followups, 2026-08-21)

`read_file` on a whitelisted image (png/jpg/jpeg/webp, magic-checked, ≤5MiB) returns an
`AttachmentRef` (`source: "read_file"`) riding the tool result. `ContentBlock::ToolResult`
now carries TWO optional image fields with mutually-exclusive-by-construction lifecycles:

- `images: Option<Vec<AttachmentRef>>` — **persisted form** (DB rows, frontend rehydrate,
  wire history from the client). File refs only; never base64 on disk.
- `resolved: Option<Vec<ImageSource>>` — **request-copy-only** base64, set by the pre-send
  resolve pass (same lifecycle as user-image `ImageRef → Image`). DB rows never carry it.

**Serde is MANUAL on `ContentBlock`** (derive removed; fixture tests lock byte-compat):
- `resolved: Some` → `content` serializes as the Anthropic-documented **block array**
  (`[{type:"image",source},…,{type:"text",text}]`); the `images` refs field is NOT emitted
  (unknown fields rejected by the API).
- otherwise → string content (+ `images` refs when present for DB rows).
- both `None` → byte-identical to the historical derive output.

Degradation (wire layer, same principle as user images — caps consumed at the adapter):
- `strip_unsupported` Tool arm: images + `!supports_images` → clear + prepend per-image
  placeholder line to content.
- OpenAI adapter Tool arm: protocol is string-only → same placeholder degradation
  (vision models on OpenAI-protocol providers also degrade here).

Invariants:
1. **No-image path is byte-identical** to pre-R4 output (fixture tests in `tests_types.rs`).
2. Tool-result rows are user-role but have NO message-level `attachments` manifest —
   `estimate_images_token` reads `ToolResult.images` inline `tokens_est` instead (no 1600 pad).
3. The resolve pass REBUILDS `images` to the successfully-loaded subset (unreadable refs
   degrade to a notice line inside content) so estimate counts exactly what ships.
4. The frontend wire history MUST carry `images` back (`ContentBlockPayload.tool_result.images`,
   `toPayloadContentPure`) — dropping them silently loses the images on every later turn.
5. Budget trim arm 2 covers old-turn tool images (resolved+images double-clear + placeholder).

Gotcha: worker read_file images land in the PARENT session's attachments dir (worker shares
the parent session id), so drawer thumbnails build URLs from `run.parentSessionId`.
