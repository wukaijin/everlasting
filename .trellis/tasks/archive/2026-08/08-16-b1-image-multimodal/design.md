# B1 图片支持(multimodal)— 技术设计

> 对应 PRD:`prd.md`(R1-R8,2026-08-16 议注定案)。本文档只讲怎么做;PRD 讲做什么/为什么。
> 取证锚点均为 2026-08-16 现状,行号随 main 漂移以符号名为准。

## 0. 总体架构(一张图)

```
[前端]                          [daemon]                        [provider]
ChatInput 粘贴 ──暂存(内存态)──┐
  │ 发送时:逐图 upload          ├→ save_attachment ─→ attachments/<sid>/<uuid>.png
  ▼                            │
chat invoke ───────────────────┼→ init.rs: user 行追加 Image 块(内存,provider 侧)
  {messages(+attachments), …}  │   metadata.attachments 落 DB(与 injections 同通道)
                               │
                               ▼
                          to_wire.rs: caps.supports_images?
                               ├─ true  → WireBlock::Image → Anthropic base64 / OpenAI data URL
                               └─ false → Text 占位块(模型知道没收到图)

@文件路径(不经过前端上传):expand_at_tokens 遇 FileKind::Image
  → 服务端复制副本进 attachments/<sid>/ → 记录进当轮 attachments manifest → 同上
```

设计核心:**DB 永远只存文本 content + metadata 引用;Image 块只在每轮请求构建时从磁盘引用即时生成**。好处:① 对齐 B2 "DB 持久化原始 content" 原则;② C3 压缩/stub/memory-digest 等既有内存侧管线无需感知磁盘;③ 群聊 `role_history` 对 user 行 verbatim clone,Image 块天然透传。

## 1. 数据层(R1:models.supports_images)

- **migration**:`schema.rs` `models` 建表语句加 `supports_images INTEGER NOT NULL DEFAULT 0`(新库)+ 对齐 `supports_thinking` 的既有幂等列添加调用(参照 `add_turn_trace_column_if_missing` 的 helper 模式,models 表若已有同类 `add_models_column_if_missing` 则复用,没有则新增同构 helper)。
- **ModelRow**(`db/types.rs:100`)加 `pub supports_images: bool`;`db/models.rs` 全部 SELECT/INSERT/UPDATE 语句列表同步。
- **IPC**:`commands/providers.rs` `add_model_inner` / `update_model` 参数结构加 `supports_images: bool`(Tauri camelCase 特例无需处理——单词字段)。
- **seed**:`db/config.rs` 初始 provider catalog 按真实能力标注(如 `gpt-4o` true / `gpt-4.1` 视配置)。既有库迁移后全部默认 0,用户在 Settings 手动勾选——**不做模型名启发式猜能力**(显式配置是 R1 的语义)。

## 2. wire 层(R3/R5:Image 块 + caps + 两 adapter)

### 2.1 内部表示

```rust
// llm/types/message.rs — ContentBlock 加变体
Image {
    /// attachments 目录内的相对文件名(如 "a1b2c3d4.png")——不是任意路径。
    file: String,
    media_type: String,        // "image/png" | "image/jpeg" | "image/webp"
},
```

- **为什么存文件名而非 base64**:base64 留在内存会让 C3 压缩的 token 估算、群聊 role_history clone、SSE 事件序列化全部背着 MB 级负载跑;文件名让 Image 块保持轻量可复制,真正取 bytes 推迟到 to_wire 前的一次 resolve。
- **resolve 时机**:请求构建完成、`provider.send` 之前,在 wire 入口把 `Image{file}` 替换为携带 base64 的内部临时形态(见 2.3),同轮只读一次盘。**resolve 签名显式带 session 上下文**(attachments 目录按 session 分域,Image 块自身只有文件名)——勿让块脱离请求上下文独立 resolve(评审 §5 提醒,采纳)。

### 2.2 ChatMessage 附件字段(历史图回传通道)

```rust
// llm/types/chat.rs ChatMessage 加:
#[serde(default, skip_serializing_if = "Option::is_none")]
pub attachments: Option<Vec<AttachmentRef>>,
// AttachmentRef { file: String, media_type: String, source: "paste" | "at_file" }
```

- 前端 rehydrate 后 `msg.metadata.attachments` 已免费透出(Explore 取证:`streamRehydrate.ts:284` verbatim 挂 `msg.metadata`);发送时 `toPayloadContent` 把它映射回 `attachments` 字段——**历史图的回传走前端 history,与 @注入 manifest 同模式**。
- 经典 chat 每轮 init 从 history 的 `attachments` 重建 Image 块;当前轮新图走 ChatRequest 扩参(见 §4)。

### 2.3 WireCapabilities + 占位降级 + adapter 发射

- `wire/types.rs` `WireCapabilities` 加 `supports_images: bool`(derive 矩阵补一行 ← `model_row.supports_images`,注释风格对齐 `supports_thinking`)。
- `to_wire.rs`:
  - `ContentBlock::Image{..}` → `WireBlock::Image{media_type, data}`(resolve 后);**caps=false 时替换为 `WireBlock::Text{"[image: {file} — 当前模型不支持图片,未发送]"}`**——对齐 PRD R3 占位降级,模式抄 `to_wire.rs:504` Reasoning 过滤的 gate 写法。
- **Anthropic adapter**:emit `{"type":"image","source":{"type":"base64","media_type":…,"data":…}}`。
- **OpenAI adapter**:emit `{"type":"image_url","image_url":{"url":"data:{mime};base64,{data}"}}`。
- Anthropic `cache_control` 断点不受影响:instructions 首块 Ephemeral 照旧;Image 块追加在最后 user 消息,不与首块耦合(design 定点 5 关闭)。

## 3. 附件存储 + 路由(R4/R7:目录、上传、GET、清理、防护)

### 3.1 目录与生命周期

- 路径:`{app_data_dir}/attachments/{session_id}/{uuid}.{ext}`;`uuid` = 随机 hex(不信任客户端文件名)。
- 写入:`save_attachment(session_id, filename, media_type, data_base64)` 新 IPC(daemon route `POST /api/v1/attachments/save`)——前端粘贴暂存图发送前逐图调用;返回 `{file}`。校验:mime 白名单(png/jpeg/webp)+ 解码后字节 ≤5MB。
- @文件复制:`at_file.rs` `expand_single` 遇 `FileKind::Image` → 读项目文件 → 同一写入 helper 复制进 attachments → 返回文本标记 `[image: {relpath}]`(进 content,模型可读)+ 附件 ref 进 manifest(不再走 495 行占位文案)。**超限/白名单外回退占位文案**(评审 P1-2,与 B2 兜底同模式:图 >5MB 或非 png/jpg/webp → 维持 495 行占位 + `Degraded` manifest record,既有测试语义保留给降级分支)。
- **服务端两级张数闸**(评审 P1-4 修正版,驳回原"init 合并处 >10 拒绝"一行修法——历史图每轮重建进请求,多轮图片对话会累积误伤):① 新图/轮 ≤10(save_attachment 侧 + init.rs 合并 manifest 处复核,前端暂存闸同值);② **请求总量**(当轮新图 + 历史重建)≤20(对齐两家 API 限制取严),超限 → 明确报错提示"历史图片累积过多,建议新建 session"(MVP 报错即止,历史图 eviction 留 follow-up)。
- 清理:`delete_session_inner` 级联删 `attachments/{session_id}/` 整目录(挂点紧邻 StubRegistry / MemoryDigestRegistry 清理调用)。
- **content-hash 去重不做**(PRD Notes 定点 2 关闭:收益小,uuid 直写最简)。

### 3.2 GET 路由 + 防护

- `daemon/routes/attachments.rs`:`GET /api/v1/attachments/:session_id/:file`,返回文件 bytes + 正确 `Content-Type`。
- 严格校验:`session_id` 格式(既有 session id 字符集)、`file` 必须匹配 `^[0-9a-f]{12,}\.(png|jpg|jpeg|webp)$`,拼接后二次 `canonicalize` 确认仍在 `attachments/` 下——双保险防 traversal。
- 内存缓存 `Cache-Control: private, max-age=86400`(附件不可变,uuid 唯一)。

### 3.3 前端取图 URL(PRD 未显式覆盖的实现决策)

新 util `attachmentUrl(sessionId, file)`:

- **本地模式(PROD)**:`/api/v1/attachments/…` 相对路径——Tauri thin 与浏览器模式均与 daemon 同源(transport/index.ts:5-7 注释,PROD `location.origin` 即 daemon)。
- **DEV 浏览器模式**:vite 1420 无 `/api` 代理 → util 里用 `daemonBase()`(`http.ts:203`,DEV 返回 `http://localhost:7456`)拼绝对 URL。
- **pwa-remote 模式**:拼 `${daemonBase()}${proxyPrefix}/api/v1/attachments/…?token=${deviceToken}`(proxyPrefix 逻辑抄 `http.ts:314-329` invoke 的拼法;query-token 模式抄 SSE `http.ts:252-254`)。**实施时需验证 remote 反代转发 GET+binary**(决策⑤称"HTTP 原文透传",理论支持;若实测不通,降级:remote 模式图片渲染为可点击链接不内联,记 DEBT)。
- DEV vite proxy(`server.proxy['/api'] → 7456`)顺带加上,让 DEV 相对路径也通(消除 daemonBase 分支)。

## 4. chat 链路(R2/R6:发送、注入、持久化、token 度量)

### 4.1 ChatRequest 扩参

`daemon/routes/agent.rs` `ChatRequest` + Tauri command 同步加:

```rust
#[serde(default)]
pub attachments: Vec<AttachmentRef>,   // 当轮新图(已 upload 拿到 file)
```

### 4.2 init.rs 挂载点(与 at_file 同区)

1. 持久化 user 行后、`inject_at_tokens`(`init.rs:747`)附近:当轮 `attachments` + @注入产生的 image refs 合并为 manifest,**与 injections manifest 同写 `messages.metadata`**(merge 进同一 JSON,不动既有 `injections` 键;`init.rs:632` 的 metadata update 调用扩一个字段)。
2. 请求内存侧:为最后 user 消息追加 `ContentBlock::Image` 列表(当前轮新图);历史 user 消息按 `ChatMessage.attachments` 同样重建(每轮)。
3. group chat:`role_history`(`group_chat_prompts.rs:114`)user 行 `m.clone()` verbatim——Image 块与 attachments 字段天然透传给所有参与者,无代码改动;参与者模型 caps 在各自 provider 调用处生效(占位降级逐人判定)。

### 4.3 token 度量(R6,评审 P0-1/P1-1 修订)

- 估算公式:`tokens ≈ (w×h)/750`(Anthropic 官方口径,OpenAI 近似同量级)。
- **w/h 读取**:前端粘贴路径 FileReader/Image 对象在上传时读;**@文件路径由后端读图片文件头**(PNG IHDR / JPEG SOF / WebP VP8X,用 `imagesize` crate——纯 Rust 零系统依赖,非像素解析)。
- **`turn_trace.images_token` 口径 = 请求内全部 Image 块求和**(当轮新图 + 历史重建——历史图每轮重发、每请求按图计费,只算当轮新图会系统性低估、TurnCard 占比失真)。**写点在请求构建完成处**(init.rs 重建完历史 Image 块之后、provider.send 之前),attach 时算好的单图估算存 metadata 供复用,求和按每轮请求全量。
- 列添加:`add_turn_trace_column_if_missing(pool, "images_token", "INTEGER")`(照抄 `schema.rs:992/997` 两行先例);落库挂 `LoopInit` → `drive_turn` 路径(与 tools_token/memory_token 同 Done 写点契约)。
- `<TracePanel>` TurnCard 加 `img` cell(占比 = images_token/context_input,同 mem cell 模式)。
- `turn-smoke.sh`:无图轮 images_token=0,报告列加上即可(不强行做带图冒测)。

## 5. 前端(R2a/R3/R6/R7)

### 5.1 粘贴 + 暂存列表(R2a)

- **paste 捕获**:CM composable `chatInputCodeMirror.ts` 加 `EditorView.domEventHandlers({ paste })`(现只有 keymap,:667-712);`e.clipboardData.files` 里有图片 → `preventDefault` + 逐张校验(mime 白名单 / ≤5MB)→ 进暂存;**纯文本粘贴零改动**(files 为空直接放行)。
- **暂存态**:`ChatInput.vue` 本地 `ref<StagedImage[]>`(内存态,组件卸载即清——切 session 时 ChatInput 不卸载?**实施时确认**:若组件常驻则用 `watch(sessionId)` 清空;`{url: objectURL, file, w, h, tokensEst}`)。UI:输入框 row 上方一条横向缩略图列(48px,右上 X 删除),零新依赖。
- **数量闸**:暂存 + 当轮 ≤10 张,超限 toast。
- **发送签名**:`emit("send", text)` → `emit("send", text, staged)`;`chatSendActions.send(text, staged?)`——乐观插入的 userMsg 挂 `metadata.attachments`(乐观渲染缩略图,发送成功后被 DB metadata 对齐)。**发送流程**:逐图 `save_attachment` → 拿 file refs → `startRequest({..., attachments})` → 清暂存。**上传失败路径**(评审 P1-3):任一图 upload 失败 → toast + **整体 abort 且暂存保留**(用户可删失败图重试),部分成功不部分发送。**objectURL 卫生**(评审 P2-3):删除暂存图/清空时 `revokeObjectURL`,封装一个销毁函数随组件卸载与 session 切换调用。纯图发送:空文本守卫放行条件改为 `text.trim() || staged.length`。
- **轻提示(R3)**:发送时当前 session model `supports_images === false && staged.length > 0` → toast"当前模型不支持图片,将以占位符发送"。

### 5.2 渲染

- **MessageItem 缩略图行**:user 气泡后、`FileInjectionsHint`(MessageItem.vue:631)同位阶,`v-if="message.role === 'user' && attachments.length"`,横向缩略图列(点击新 tab 打开原图 `window.open(attachmentUrl(...))`)。
- **DOMPurify 收紧(R7)**:`markdown.ts` `PURIFY_CONFIG` 收紧 img 为**两态放行**(评审 P2-2):① `src` 以 `/api/v1/attachments/` 开头的相对路径;② `src` 为绝对 URL 且 host 等于当前 `daemonBase()` 的 attachments 前缀(pwa-remote 模式带 token query);外链 `<img>` 整体替换为 `<a href>原 URL</a>` 链接文本。token 进 img src 的泄露面与 SSE query-token 同款,照先例接受。单测锁:`![](http://evil.com/x.png)` 不产生 img 加载。

### 5.3 Settings + picker(R1/R3)

- `models.ts`(:19 类型 + :90/:117 opts)→ `ModelsTab.vue`(:41/:77/:95/:113)→ `ModelForm.vue`(:55 + CheckboxRoot 仿 :206)全链加 `supportsImages`。
- `DefaultTab.vue` 仿 :67 加 `vision` tag;`ModelRow.vue` 同。

## 6. 边界与豁免(R8)

- **worker/subagent**:零改动——dispatch 只传文本 task;worker 请求构建路径不产 Image 块(attachments 只在主 loop init.rs 挂);`READONLY_TOOL_ALLOWLIST` 不涉及。
- **read_file 读图**:维持二进制降级(不动 `read_file.rs`)。
- **群聊**:见 §4.2.3,预期零改动 + 回归测试锁(参与者请求含 Image 块的 user 行透传)。**实施时确认两点**(评审漏项):① turn_trace `images_token` 在群聊的语义——一轮编排含 N 个参与者请求(各自图片集合经 role_history 透传基本相同),对齐 `tools_token` 在群聊的既有记账行为(记首个请求/求和/只记 moderator,以既有实现为准),不另造口径;② D3 resend / edit_user_message 从 DB 重建历史时 `attachments` 随 `ChatMessage` 回传,加回归测试。
- **C3 压缩**:Image 块计入请求体积的估算方式与现状一致(blocks 序列化长度),不做特殊处理;压缩裁剪若命中 user Image 块——**接受现状**(压缩只动尾部工具结果区,图片在 user 行,实测确认即可)。
- **stub/memory-digest gate**:图片与两者无交集,不加 gate。

## 7. 测试策略

- **Rust 单测**:ContentBlock::Image serde round-trip;to_wire 降级占位(两 protocol 各锁 payload 形状);save_attachment 白名单/5MB/traversal 拒绝;GET 路由 200/400/404;@图 expand 复制 + manifest;init.rs attachments→Image 块 + metadata merge;delete_session 清目录;群聊 role_history 含图透传;既有全量回归(`cargo test -p everlasting --lib`)。
- **前端 vitest**:paste handler(图片/纯文本/白名单外);暂存列表增删/数量闸;纯图发送守卫;attachmentUrl 三模式;DOMPurify 外链图降级;ModelsTab/Form supportsImages 接线;MessageItem 缩略图行渲染。
- **live 验证**:支持图片的模型(实施时从 provider catalog 确认可用视觉模型)贴截图问图;`supports_images=false` 模型复验占位 + toast;群聊贴图全参与者可见;浏览器模式(非 Tauri)缩略图 + `<img>` 直载。

## 8. 风险与回退

| 风险 | 缓解 |
|---|---|
| pwa-remote GET binary 转发不通 | §3.3 降级(链接不内联)+ DEBT 记录 |
| Anthropic 对 image 块 + cache_control 组合的行为差异 | live 验证双轮 cache 率(照 memory-gov 验证法) |
| 粘贴大图拖慢 daemon 请求(5MB×10) | 5MB/10 张硬闸已设;base64 只在 wire 入口存在一轮 |
| 群聊弱模型收到 Image 块报错 | caps 占位降级逐人生效;实测参与者模型无视觉能力场景 |

回退单元:PR 分 6 个(见 implement.md),wire 层(PR2)与前端(PR5)可独立回退不影响存量纯文本行为;`supports_images` 列默认 0 = 全量占位降级 = 行为等价现状。
