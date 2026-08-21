# B1 图片收尾:自动压缩 + 拖拽 + read_file 工具读图

## Goal

收口 B1(08-16/17 图片 multimodal)留下的全部三个 follow-up:

1. **自动压缩**:超大/高分辨率图片自动降采样 + 重编码后放行,把 `images_token`(口径 `(w×h)/750`)砍一个量级,替代"5MB 硬拒"与"全尺寸原样发送"两个极端。
2. **工具读图**:`read_file` 读到图片文件时返回 image 内容块,vision 模型直接"看见"磁盘上的图(agent 自主复核截图/UI 变得可行)。今天 read_file 对图片是裸 UTF-8 IO error。
3. **拖拽入框**:文件拖进聊天区走暂存管线,与粘贴同路。

用户价值:贴 4K 截图不再烧 ~11k token/轮;agent 能"看"磁盘图片(ui-review 类任务的自主复核闭环);第三条输入入口补齐。

## Background(代码事实,2026-08-21 探索锚点)

- 粘贴管线全在前端:`chatInputCodeMirror.ts:715-732` paste → `chatSendActions.ts:117-141 addStagedImages`(mime 白名单 png/jpeg/webp、单图 5MB、单轮 10 张);尺寸读取 `readImageDimensions`(`chatSendActions.ts:78-90`);`tokensEst = (w×h)/750` stage 时算好存 `StagedImage` → `AttachmentRef.tokens_est`。前端**无任何 canvas/降采样代码**。
- @文件图片走后端:`at_file.rs:574-620 try_inject_image`(魔数、5MiB 闸、`imagesize::blob_size` 估算、复制进 `attachments/<session_id>/`);后端无图片编码能力。
- `read_file`(`tools/read_file.rs:91-133`)**无二进制检测**,图片 → `read_to_string` UTF-8 IO error;三层二进制嗅探只在 `at_file.rs:320-333`(@ 路径)。
- tool_result 纯文本三层到底:`ContentBlock::ToolResult { content: String }`(`llm/types/message.rs:88-93`,即 DB serde 形状)→ `WireMessage::Tool { content: String }`(`wire/types.rs:129-137`)→ Anthropic adapter 经 from_wire 重建后原样 serde(**未用 tool_result content block array 形态**);OpenAI tool 消息协议本身 string-only。
- `estimate_images_token`(`attachments.rs:425-457`)只扫 user-role 消息块,1600/图垫底 + `m.attachments` 清单精确替换(不变量:Image 块被 attachments 1:1 覆盖,注释 L449-453);tool_result 行是 user-role 但 `attachments: None`。
- 预算裁剪 `budget.rs:170-203` arm 2 降级旧轮 Image 块并计 `images_freed`;caps 降级 `to_wire.rs:498-557 strip_unsupported` 只在 UserBlocks 臂有钩子,`WireMessage::Tool` 原样放行。
- `READONLY_TOOL_ALLOWLIST` 含 read_file(`subagent/tools_filter.rs:128-135`),只读 worker 可调用。
- **resolve/persist 时序**(B1 不变量):`resolve_image_refs`(drive.rs:916-918)只改请求副本 `turn_messages`(ImageRef→base64 Image);`persist_turn` 落库的是引用形态消息,DB 永不存 base64。
- catalog 已有 vision 模型 `MiniMax-M3`(provider Carlos-API,**anthropic protocol**)——能承载 tool_result 内嵌 image 块的协议,live 验证条件满足。

## 已决议(brainstorm 2026-08-21)

- **D1**:拖拽纳入本任务(与粘贴同管线,压缩天然覆盖两入口)。
- **D2**:压缩仅做前端入口(粘贴/拖拽)canvas 零依赖;@文件后端路径维持现状,不引入 Rust 图片编码库。
- **D3**:压缩策略——触发「长边>1568px 或 bytes>1MB」;长边超限→降采样至 1568;无透明通道且 bytes 超限→重编码 JPEG q0.85(有透明保持原格式仅降采样);压缩产物更大→保留原件;5MB 硬闸改**压后判定**;暂存条「已压缩」标注;tokensEst 按压缩后尺寸重算。
- **D4**:拖拽只收图片(png/jpg/webp);非图片文件 toast「请使用 @ 引用文件」。
- **D5**:工具读图前端显示缩略图(tool result 卡,主消息流 + SubagentDrawer)。

## Requirements

### R1 前端自动压缩(D2/D3)

粘贴与拖拽入口在 stage 前统一过压缩:触发条件、目标参数、守卫与闸序按 D3;压缩信息(已压缩标注、原始→结果尺寸)在暂存条可见;`StagedImage.tokensEst`/`w`/`h` 用压缩后值。

### R2 拖拽入框(D1/D4)

聊天区接收文件拖放:图片 → `addStagedImages`(压缩生效);非图片 → toast 引导 @ 引用;混合批次图片入暂存、非图片单次提示。

### R3 read_file 读图

`read_file` 命中 png/jpg/jpeg/webp(扩展名+魔数,复用 at_file 判定):≤5MiB → 复制进 `attachments/<session_id>/`(副本语义同 @图)、`imagesize::blob_size` 估算 tokens_est、返回文本行 + `AttachmentRef` 引用;>5MiB → 占位提示(非 error,引导压缩/转换);文件不存在等既有错误路径不变。

### R4 ToolResult 带图全链

- `ContentBlock::ToolResult` 加可选 `images: Option<Vec<AttachmentRef>>`(serde default + skip-none,旧 DB 行兼容;落库只有文件引用无 base64,B1 不变量延续)。
- resolve:请求构建时(`resolve_image_refs` 同点)把 ToolResult.images 解析为请求内 base64 形态,仅存于请求副本。
- Anthropic wire:带图 tool_result 以 **content block array** 形态发出(image 块 + text 块,Anthropic 文档形态);无图行为逐字节不变。
- 降级:OpenAI-compat provider 与 `supports_images=false` 模型 → 工具图降级为占位文本(占位降级原则,同 B1 caps 路径)。
- 前端事件 `ToolResultPayload` 带可选 images(引用),live 与 DB 回看两路同构。

### R5 记账与预算

`estimate_images_token` 精确计入 ToolResult.images(用内联 tokens_est,不落 1600 垫底);budget arm 2 覆盖旧轮工具图降级并计入 `images_freed`;`turn_trace.images_token` 口径注释同步(工具图含入)。

### R6 前端呈现(D5)

tool result 卡(主消息流 + SubagentDrawer 的 ToolCallCard)在文本行下渲染缩略图,经既有 `attachmentUrl` 路由;worker 读取的图经 SubagentDrawer transcript 同样可见。

### R7 顺手修

`at_file.rs:694-697` 非白名单/超限图片占位文案过时("当前为纯文本通道…B1 计划")→ 更新为现状描述。

## Acceptance Criteria

- [ ] **AC1(压缩)**:4K 无透明 PNG 粘贴 → 暂存条「已压缩」+ tokensEst 按 1568 长边口径;小图(≤1568 且 ≤1MB)零重编码原样放行;透明 PNG 只降采样不换格式;压缩产物更大时保留原件(以上逻辑单测锁;canvas 实际编码 live 验证)。
- [ ] **AC2(压缩闸序)**:压后仍 >5MB 才拒(几乎不可达路径);拒与放行的 toast 语义不变。
- [ ] **AC3(拖拽)**:拖 png 入聊天区 → 暂存(压缩生效);拖 .md → toast「请使用 @ 引用文件」;混合批次一次提示。
- [ ] **AC4(读图 live)**:MiniMax-M3 session 内 agent `read_file` 一张含文字截图 → 模型能描述图中内容(正向视觉路径首次 live 实证);`turn_trace.images_token` 含工具图。
- [ ] **AC5(兼容)**:旧 DB 行(ToolResult 无 images 字段)反序列化通过;新行落库 JSON 只含文件引用无 base64(单测锁)。
- [ ] **AC6(降级)**:OpenAI-compat 与 caps=false 两路,工具图降级占位文本、`is_error` 语义不变(单测锁)。
- [ ] **AC7(预算)**:构造超预算历史,旧轮工具图被降级且 `images_freed` 计入(单测锁)。
- [ ] **AC8(回看)**:重载 session 后 tool result 缩略图从 attachments 路由渲染;SubagentDrawer 同(live 验证)。
- [ ] **AC9(常规)**:后端 `cargo test -p everlasting --lib` 全绿 + 前端 `pnpm test` 全绿 + vue-tsc 0 err;`turn-smoke.sh` 不回归。

## Out of Scope

- @文件路径的压缩/重编码(后端,需 Rust 图片编码库——D2 明确不做)
- worker/subagent dispatch 传图(独立 follow-up)
- PDF/Office 多模态(B2 占位降级兜底)
- 图片编辑/裁剪 UI
- grep/glob/list_dir 等其他工具读图(只做 read_file)
