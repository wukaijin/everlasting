# Design — B1 图片收尾:自动压缩 + 拖拽 + read_file 工具读图

对应 `prd.md` R1-R7。三个交付面按 PR 切分:PR1 前端(压缩+拖拽,纯前端零后端改动)/ PR2 后端(read_file 读图 + ToolResult 带图全链)/ PR3 前端呈现 + 顺手修 + live 验证。PR1 与 PR2 无依赖可并行;PR3 依赖 PR2 的 payload 字段。

## 1. PR1 — 前端压缩 + 拖拽

### 1.1 压缩工具 `app/src/utils/imageCompress.ts`(新)

```ts
export interface CompressResult {
  file: File;          // 压缩后的新 File,或原文件(未触发/守卫保留)
  w: number; h: number; // 最终尺寸(tokensEst 由此算)
  compressed: boolean;
  origW: number; origH: number; origBytes: number; // 标注用
}
export async function compressImage(file: File): Promise<CompressResult>
```

- **解码**:`createImageBitmap(file, { imageOrientation: 'fromImage' })`(EXIF 归一);失败 fallback objectURL + `Image`(复用 `readImageDimensions` 模式);再失败返回原文件(压缩 fail-open,行为退化现状)。
- **触发**(D3):`max(w,h) > 1568 || file.size > 1MB`,两者皆否 → 原样返回。
- **降采样**:长边超限时按比例缩至长边 1568(canvas `drawImage` 高质量 `imageSmoothingQuality: 'high'`)。
- **透明检测**:PNG/WebP 输入时 `getImageData` 扫 alpha(步进采样 + 早退;1568² 一次性可接受);JPEG 输入视为无透明。
- **重编码**:无透明 且 `file.size > 1MB` → `toBlob('image/jpeg', 0.85)`(文件名扩展改 .jpg);有透明 → 保持原 mime 仅降采样(`toBlob(mime)`,PNG 重编码尺寸缩小后通常更小,仍走守卫)。
- **守卫**:`blob.size >= file.size` → 保留原文件(`compressed: false`)。
- **可测性**:阈值判定/alpha 判定/守卫拆纯函数;canvas 依赖注入或集中在薄壳里(vitest happy-dom 无 toBlob 实现,纯逻辑单测 + live 验证补)。

### 1.2 接线 `chatSendActions.ts::addStagedImages`

闸序调整(D3「压后判定」):

```
mime 白名单 → compressImage(新增,await)→ 5MB 闸(对压缩后 file)→ 10 张闸 → stage
```

`StagedImage`(`chat.types.ts:224-231`)加 `compressed?: boolean` + `origBytes?: number`;`tokensEst`/`w`/`h` 用 CompressResult 最终值。上传走既有 `save_attachment`(后端 5MiB 复检天然通过——压缩后产物远小)。

### 1.3 暂存条标注 `ChatInput.vue:704-725`

暂存缩略图角标「已压缩」(小 chip,title 显示 `origW×origH origBytes → w×h`);未压缩不加标。

### 1.4 拖拽 `ChatPanel.vue`

聊天区根容器 `@dragover.prevent` + `@drop.prevent`;handler 取 `e.dataTransfer?.files`:

- image/* 且白名单内 → `chatStore.addStagedImages(files)`(压缩生效,R1 覆盖);
- 存在非图片文件 → `showToast('请使用 @ 引用文件', 'warn')`(混合批次只提示一次,图片照常入暂存);
- 空文件列表/纯文本拖放 → 不拦截。

**风险**:CodeMirror 自带 drop(文本)——面板级 handler 只在 `files.length > 0` 时 `preventDefault`,文本拖放零影响;绑定放 ChatPanel 而非 ChatInput,覆盖消息区+输入区整片。

## 2. PR2 — read_file 读图 + ToolResult 带图全链(后端)

### 2.1 图片判定共享 helper

`at_file.rs:595-599` 的魔数判定(PNG/JPEG/WebP)+ 扩展名映射提取为 `attachments.rs` 共享 fn(`is_image_bytes(&[u8]) -> Option<&'static str>` 返回 media_type),at_file 与 read_file 共用;at_file 行为零变化。

### 2.2 `read_file.rs` 图片臂

`execute`(`read_file.rs:91-133`)在读文件前加图片分支:

- 扩展名 ∈ {png,jpg,jpeg,webp} → 读 bytes → 魔数判定(非图片魔数 → 走既有 UTF-8 路径,行为不变);
- ≤ 5MiB(常量对齐 at_file)→ `attachments::save_image(data_dir, session_id, ...)` 复制副本 + `imagesize::blob_size` 算 `tokens_est = (w*h)/750` → 返回 `(content_text, false, Some(vec![AttachmentRef{source: "read_file", ..}]))`(返回通道见 §2.4):
  - `content_text = "[image: <path> (w×h) — 已作为图片块发送]"`;
- \> 5MiB → `[image: <path> — 超过 5MB 上限未读取,可用压缩工具缩小后重试]`,`is_error: false`;
- **caps 不在 tool 层判断**——降级统一在 wire 层(与 B1「caps 消费在适配层」原则一致,`review.md §5` 背书)。

**ToolContext 需有 data_dir**:at_file 路径已用 `ctx.data_dir`(`at_file.rs:606-608` 先例),read_file 同源可用,零新穿参。

### 2.3 数据形状:`ContentBlock::ToolResult`(钉死)

现状(`llm/types/message.rs:88-93`):internally-tagged derive enum(`#[serde(tag = "type")]`),变体字段 `tool_use_id / content: String / is_error`。改造:

```rust
ToolResult {
    #[serde(flatten)]
    data: ToolResultData,
},

/// 独立结构体 + 自定义 Serialize/Deserialize(flatten 在 internally-tagged
/// enum 内合法;serde 缓冲整 map,无性能敏感点——每轮 tool result 数量小)。
pub struct ToolResultData {
    pub tool_use_id: String,
    pub content: String,
    pub is_error: bool,
    /// 持久化引用形态(DB 行 / 前端回看)。resolve 前的稳定形态。
    pub images: Option<Vec<AttachmentRef>>,
    /// 请求副本 resolve 产物(base64)。DB 永不落此字段(构造上互斥:
    /// 落库实例从不 set,请求副本 resolve 时 set)。
    pub resolved: Option<Vec<ImageSource>>,
}
```

**自定义 Serialize 分支**(互斥语义,`ToolResultData` 上):

- `resolved: Some(imgs)`(HTTP 出口形态)→ `content` 渲染为 **block array**:`[{type:"image",source:{…}} × N, {type:"text",text:content}]`,**不输出 `images`/`resolved` 字段**(Anthropic 不认未知字段);
- `resolved: None && images: Some(refs)`(DB 持久化形态)→ `content` 纯字符串 + 输出 `images: [refs]`(skip-if-none);
- 两者皆 None(绝大多数现状行)→ 与今天 derive 输出**逐字节一致**(`is_error` 沿用 `skip_serializing_if = "is_false"`)——fixture 对拍单测做硬闸。

**自定义 Deserialize**:`content` 接受 string(常规)或 array(把 image 块还原进 `resolved`、text 块拼回 `content`,对称卫养 + 测试用);`images` default None。

**AttachmentRef**(`llm/types/chat.rs:54-66` 现有)`source` 字段加新值 `"read_file"`(现有 `"paste" | "at_file"`)。

**构造点**:`agent/chat_loop/tools.rs` 五处(并行 ~L421-435 + serial ~L503/662/769/850)从 `execute_tool` 返回值挂 `images`;`build_synthetic_tool_result_message`(cancel 合成结果)不带图。**消费点波及**:`content` 字段类型不变(仍 String,存在 `ToolResultData` 里)——`compaction.rs:640` 渲染、`helpers.rs::tool_result_envelope`、sink payload、既有测试的 `ContentBlock::ToolResult { .. }` 模式匹配改 `ToolResult { data: ToolResultData { .. } }` 或加访问 helper(机械替换)。

### 2.4 execute 返回通道(read_file 局部)

`read_file::execute`(`read_file.rs:91-133`,现返回 `(String, bool)`)→ `(String, bool, Option<Vec<AttachmentRef>>)`(图片臂 Some,其余 None)。`tools/mod.rs::execute_tool`(现 4 元组 `(String, bool, ToolContextUpdate, Option<i32>)`)→ 加第 5 元素 `Option<Vec<AttachmentRef>>`(其他工具 arm 一律 `None`)。波及面:`mod.rs` dispatch + `chat_loop/tools.rs` 五个消费点,纯机械。

### 2.5 resolve:请求构建期解析(钉死)

`resolve_image_refs`(`attachments.rs:374-418`,调用点 drive.rs:916-918)扩展:扫到 `ToolResult.data.images = Some(refs)` → 读盘转 base64 写入 `data.resolved`,**`images` 保留不清**(estimate 靠它的 tokens_est 精确计费,见 2.7);读盘失败 → `images = None` + `content` 追加降级文案(对齐既有 ImageRef 失败路径 `attachments.rs:408-411`)。

### 2.6 wire 全链(钉死)

- `chat_request_to_wire` ToolResult lift(`to_wire.rs:313-327 block-preserving / 349-363 concatenating`)→ `WireMessage::Tool` 扩为 `{ tool_call_id, content: String, images: Vec<ImageSource> }`(从 `data.resolved` 取;to_wire 在 provider.send 内、resolve 之后,必为已解析形态;concatenating 路径多 ToolResult 合并时 images 拼接)。
- `strip_unsupported`(`to_wire.rs:498-557`)加 Tool 臂:images 非空 && `!caps.supports_images` → 清空 + `content` 前插 `"[image: 历史图片 — 当前模型不支持图片,未发送]"` 占位行(复用 `image_placeholder_wire_block` 文案模式)。
- Anthropic `from_wire` Tool 臂(`from_wire.rs:122-133`)重建 `ToolResultData { content, resolved: images, .. }` → 整体 serde 出 block array 形态(§2.3 分支 1)。
- OpenAI adapter Tool 臂(`openai.rs:334-344`):协议 string-only → images 非空时清空 + 前插同款占位(与 caps 降级同文案,语义一致)。
- **无图路径逐字节不变**:lift/strip/两 adapter 在 images 为空时输出与今天完全一致(单测对拍)。

### 2.6 降级(两路)

- **caps=false**:`strip_unsupported`(`to_wire.rs:498-557`)加 Tool 臂钩子——wire images 非空且 `!target_caps.supports_images` → 清空 images + content 前插占位行 `"[image: {file} — 当前模型不支持图片,未发送]"`(复用 `image_placeholder_wire_block` 文案模式)。
- **OpenAI-compat**:adapter Tool 臂(`openai.rs:334-344`)协议 string-only——images 非空 → 清空 + content 前插同款占位。**(是否需要区分提示语留实现期定,推荐同一句,降级语义一致。)**

### 2.7 记账与预算(钉死)

- `estimate_images_token`(`attachments.rs:425-457`,调用点 drive.rs:924 在 resolve **之后**):块扫描加 ToolResult 臂 → `data.images` 按 `tokens_est` 精确累加(None 时 1600 垫底,与 user 图同策略)。resolve 保留 `images` 不清(§2.5)正为此——post-resolve 仍可精确计费。不变量注释同步:"user Image 块由 `m.attachments` 清单覆盖;ToolResult.data.images 自带 tokens_est"。
- budget arm 2(`budget.rs:170-203`,post-resolve 运行):降级对象扩到旧轮 ToolResult → `data.resolved = None && data.images = None` + `content` 前插 `"[image: 历史图片 — 预算裁剪,未发送]"`;`images_freed` 经 before/after estimate 差值自然计入(既有机制)。
- `turn_trace.images_token` 口径不变("请求内全部图"),`token-usage-tracking.md` spec 补工具图来源一句。

### 2.8 worker 路径

`READONLY_TOOL_ALLOWLIST` 含 read_file——worker 读图零特殊门:同一 ToolResult 形状进 worker 请求,worker 模型 caps 走同一降级;SubagentBufferSink 的 ToolResultPayload 序列化天然带 images(serde 可选字段)。worker per-turn `images_token` 经同一 estimate 计入。

### 2.9 IPC / 事件

- `ToolResultPayload`(sink 事件)加 `#[serde(default)] images: Option<Vec<AttachmentWireRef>>`(前端 PR3 消费)。
- 零新 IPC:缩略图走既有 GET `/api/v1/attachments/:session_id/:file`(pwa-remote proxy 形态天然继承,B1 已铺)。

## 3. PR3 — 前端呈现 + 顺手修

- **类型**:`ToolResultPayload` TS 类型 + DB 回看解析(消息 content JSON 反序列化路径)加可选 `images`。
- **渲染**:tool result 卡(MessageItem 工具区内 + SubagentDrawer ToolCallCard)在文本行下方渲染缩略图行(复用 `MessageImages.vue` / `attachmentUrl.ts:32-42` 模式,56px cell + 点击放大若有既有 lightbox 则复用,无则 hover title)。
- **顺手修**:`at_file.rs:694-697` 占位文案 → "图片格式不在白名单或超过 5MB,未注入(支持 png/jpg/webp,≤5MB)"。

## 4. 测试策略

**Rust 单测**(全部 `cargo test -p everlasting --lib`):
- read_file 图片臂:白名单命中/魔数不符走 UTF-8 老路/≤5MiB 副本+tokens_est/>5MiB 占位非 error/文件不存在老路;
- ToolResult serde:旧行(无字段)反序列化 / 新行 refs-only 序列化(无 base64)/ 自定义 Serialize 的 array 形态(带 resolved)/ Deserialize 双形态;
- to_wire:Tool lift 带 images / strip_unsupported caps 降级 / openai 降级占位;
- estimate_images_token:工具图精确计费;
- budget arm2:旧轮工具图降级 + images_freed;
- at_file 回归:提取共享 helper 后零行为变化(既有 L949-994 测试组)。

**前端 vitest**:
- imageCompress 纯函数:触发判定(长边/bytes/组合)、alpha 判定(伪造 ImageData)、守卫(产物更大保留原件)、目标尺寸计算;
- addStagedImages 闸序:压缩后才判 5MB(mock compressImage)、compressed 标注入 StagedImage;
- ChatPanel drop:图片入暂存 / 非图片 toast / 混合批次单次 toast / 纯文本拖放不拦截;
- tool result 缩略图渲染(payload 带 images / 不带零变化)。

**Live(MiniMax-M3,anthropic protocol)**:
- AC4:session 内要求 agent `read_file` 含文字截图 → 模型描述内容(正向视觉首证);
- AC1:贴 4K PNG → 暂存条「已压缩」+ tokensEst 下降 → 发送 → TracePanel img cell 反映压缩后口径;
- AC8:重载 session 缩略图回看。

## 5. 回滚与风险

- PR1 纯前端,回滚 = revert 单 commit;压缩 fail-open(解码失败原样放行)保证最坏退化 = 现状。
- PR2 的 serde 改动是最敏感面:自定义 Serialize 必须保证**无图路径逐字节不变**(单测用既有 JSON fixture 对拍);DB 无 migration(可选字段,旧行天然兼容)。
- PR3 依赖 PR2 字段,先行合并 PR2 后无耦合。
- 风险点:Chromium `toBlob('image/png')` 重编码质量、CodeMirror drop 冲突(设计已规避)、Anthropic array 形态字段序(文档无字段序要求)。
