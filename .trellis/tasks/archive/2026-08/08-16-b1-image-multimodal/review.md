# B1 图片支持(multimodal)— 任务评审

> 评审日期:2026-08-16。评审对象:prd.md / design.md / implement.md / task.json / check.jsonl / implement.jsonl。
> 评审方式:通读三件套 + 对 design 引用的全部代码锚点逐一取证核验(行号以评审当日 main 为准)。

## 结论

**设计质量高,可以进入实施。** 核心架构(DB 只存文本 + metadata 引用、Image 块每轮从磁盘即时物化)与既有模式对齐良好;PR 划分(6 个)每个独立可提交/可回退;顺手闭合"LLM 输出外链图今天就会加载"的安全缺口是加分项。

发现 **1 个 P0 口径问题(images_token 计算范围)、4 个 P1 实施前需明确的细节**,均不阻塞开题,但建议在 `task.py start` 前按本文 §4 修订 design/implement。

## 1. 锚点核验(抽查全部命中,无一失实)

design 引用的关键锚点逐项核对结果(偏移 1~6 行的行号以符号名为准,design 已声明"行号随 main 漂移"):

| 层 | 锚点 | 核验结果 |
|---|---|---|
| wire | `message.rs` ContentBlock 无 Image 变体(Text/Thinking/RedactedThinking/ToolUse/ToolResult) | ✅ `message.rs:82-88` |
| wire | `to_wire.rs:369` content_block_to_wire_block;`:504` 附近 block_supported Reasoning gate | ✅ 命中,`block_supported` 为 caps 过滤单点 |
| wire | `wire/types.rs` WireCapabilities 结构、"derive 矩阵补行" | ✅ `:29-40` 有 supports_thinking/signatures 先例 |
| wire | Anthropic / OpenAI adapter 文件存在 | ✅ `provider/anthropic.rs`、`provider/openai.rs` |
| db | `schema.rs:221` supports_thinking;`:992/997` add_turn_trace_column_if_missing(tools_token/memory_token) | ✅ 逐字命中 |
| db | `schema.rs:166` messages.metadata 建表即有未用 | ✅ |
| db | `types.rs:107` ModelRow.supports_thinking;`config.rs` seed catalog | ✅ gpt-4o `supports_thinking=false` 已标注 |
| agent | `at_file.rs:494-495` 占位文案"[image: … 纯文本通道,B1 计划]" | ✅;且 `:672` 有测试 `image_file_degrades_to_placeholder` 锁着该文案 |
| agent | `init.rs:747` inject_at_tokens;metadata update | ✅(实际 update 调用在 `:773`,`:632` 为注释位置) |
| agent | `group_chat_prompts.rs:114` user 行 verbatim 透传 | ✅ `Role::User => out.push(m.clone())` 逐字命中 ——"群聊零改动"前提成立 |
| agent | delete_session 级联清理链(worktree → StubRegistry → MemoryDigestRegistry) | ✅ `sessions.rs:280` delete_session_inner 内 stub_loaded.clear / digest.registry().clear 属实 |
| daemon | `routes/files.rs` 仅 list_files,无二进制 GET | ✅ |
| 前端 | `markdown.ts:71` USE_PROFILES `{html:true}` | ✅ 外链图缺口真实存在 |
| 前端 | `chatInputCodeMirror.ts:667` IME keymap、无 domEventHandlers | ✅ paste 捕获点选择正确 |
| 前端 | `ChatInput.vue:224` emit("send", text) | ✅ |
| 前端 | `DefaultTab.vue:67` / `ModelRow.vue:78` thinking tag | ✅ |
| 前端 | `models.ts:19/93/120`、`ModelsTab.vue:47/77/95/113/118`、`ModelForm.vue:55/209` supportsThinking 全链 | ✅ |
| 前端 | `streamRehydrate.ts` metadata verbatim 挂载 | ✅ |
| 前端 | `http.ts:203` daemonBase、`:252-254` SSE query-token、`:319-320` proxyPrefix | ✅ |
| 前端 | `transport/index.ts` 全 HTTP、PROD 同源 | ✅ |
| 前端 | vite.config.ts 无 /api proxy(需"顺带加上") | ✅ 确认无 proxy |

## 2. 需修正的问题(P0)

### P0-1 images_token 计算口径(design §4.3)

**问题**:历史图每轮从 metadata 重建重发(design §2.2/§4.2),**每一轮请求都含全部历史 Image 块**,Anthropic/OpenAI 每请求都按图计费。§4.3 写"当轮 user 消息 images 估算求和"若只算当轮新图,则 context_input 含历史图而 images_token 不含 → token 治理切片系统性低估,TurnCard 占比失真。

**建议**:改为"**请求内全部 Image 块(新图 + 历史重建)估算求和**";写点放在 wire 入口(此时请求内容已定),attach 时算好的单图估算值可存 metadata 供复用,但求和必须按每轮请求全量算。

## 3. 实施前需明确的细节(P1)

### P1-1 @ 注入图的 w/h 缺失(design §4.3)

尺寸由前端上传时读取(FileReader/Image 对象);@ 文件复制路径不经前端 → `(w×h)/750` 无输入,该路图片 images_token=0 或需另法。

**建议二选一**:
- 接受低估并在文档写明理由(at 图占比小);
- 后端读文件头尺寸(PNG IHDR / JPEG SOF / WebP VP8X,仅头部几十字节,不解析像素,与"后端不解析图片像素"不冲突)。**推荐后者,成本约一天。**

### P1-2 @ 引用超限图(>5MB)行为未定义

§3.1 说 @ 复制"同一写入 helper"(含 5MB/mime 校验),超限时是拒绝引用、回退 495 行占位、还是跳过?PRD/design 均未定义。

**建议**:超限回退占位文案(与 B2 兜底同模式),写进 at_file 测试;白名单外格式同理(ext 判定已在 classify 层)。

### P1-3 上传失败路径未定义(PR5)

"逐图 save_attachment → startRequest → 清暂存",若中途某图 upload 失败(网络/超限/daemon 重启),暂存是否清空?部分成功时请求带不带已成功的图?

**建议**:任一图失败 → toast + **整体 abort 且暂存保留**(用户删失败图可重试);写入 implement PR5 验收点。

### P1-4 10 张/轮闸只有前端

§3.1 服务端 save_attachment 只校验 mime + 5MB;直接调 API(或 @ 注入)可绕过张数闸。

**建议**:服务端在 init.rs 合并 manifest 处加 `>10 拒绝 + 报错`,一行成本。

## 4. 次要问题(P2)

| # | 问题 | 建议 |
|---|---|---|
| P2-1 | 既有测试 `at_file.rs:672 image_file_degrades_to_placeholder` 锁着 495 行占位文案,PR4 必改;implement 测试列表未点名 | implement PR4 补一行"改写该测试" |
| P2-2 | DOMPurify 收紧在 pwa-remote 模式的 allowlist:远程模式下 img src 是"远程 daemonBase 绝对 URL + token query",allowlist 须覆盖远程主机前缀,不只相对路径 | 按"相对 /api/v1/attachments/ + 主机 equals 当前 daemonBase 的绝对 URL"两态写正则;token 进 img src 的泄露面与 SSE query-token 同款,照先例接受 |
| P2-3 | 暂存 objectURL 泄漏:URL.createObjectURL 需在删除/清空时 revokeObjectURL | PR5 加一条销毁函数 |
| P2-4 | 流程:check.jsonl / implement.jsonl 仍是 `_example` 模板占位;task.json status=planning、dev_type/scope/package 为 null | `task.py start` 前填 jsonl(spec 引用)+ 补元数据 |

## 5. 确认无误的设计决策(评审背书)

- **resolve 时机(§2.1/2.3)**:base64 只在 wire 入口存在一轮,文件名让 C3 压缩估算、群聊 clone、SSE 序列化不背 MB 级负载。注意 resolve 函数签名须带 session_id(Image 块自身无 session 上下文,但请求构建处有——实施时勿让块脱离请求上下文传递)。
- **群聊零改动 + 回归锁**:`role_history` 逐字 clone 前提已实证,参与者 caps 逐人判定与既有"caps 消费在适配层"模式(to_wire Reasoning gate)一致。
- **R7 顺带闭合外链图加载缺口**:这是本任务最有价值的部分之一 —— 当前 `markdown.ts:71` 配置下 LLM 输出外链图即真实发请求,属安全修复。
- **回退面清晰**:`supports_images` 默认 0 = 全量占位降级 = 行为等价现状;PR2/PR5 可独立回退;pwa-remote GET binary 风险预留了降级路径。

## 6. 开题前置清单

- [ ] design §4.3 修订 images_token 口径(P0-1)
- [ ] P1-1~P1-4 四项在 design/implement 写明取舍
- [ ] implement PR4 补"改写 image_file_degrades_to_placeholder"
- [ ] check.jsonl / implement.jsonl 填 spec context 条目;task.json 元数据补齐后 `task.py start`
