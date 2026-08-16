# B1 图片支持 — 实施计划

> 前置:PRD + design.md 已定案。6 个 PR,每个独立可提交/可回退;PR1-4 后端(顺序执行),PR5 前端(依赖 PR3 的 save_attachment、PR4 的 attachments 参数),PR6 收口。
> 命令速查(AGENTS.md):后端 `cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib`;前端 `cd app && pnpm test && pnpm vue-tsc --noEmit`;live 烟测 `scripts/turn-smoke.sh`。

## PR1 — 数据层:models.supports_images(R1)

- [ ] `db/migrations/schema.rs`:models 建表语句加列 + 幂等 helper(复用/新增 `add_models_column_if_missing`,对齐 `add_turn_trace_column_if_missing` 先例 `schema.rs:992`)
- [ ] `db/types.rs` `ModelRow.supports_images` + `db/models.rs` 全部 SQL 列表同步
- [ ] `commands/providers.rs` `add_model_inner` / `update_model` 参数 + `db/config.rs` seed 标注(gpt-4o 等真实能力)
- [ ] 测试:migration 幂等重跑;默认 0 既有行为零变化
- 验证:`cargo test -p everlasting --lib "models"`

## PR2 — wire 层:Image 块 + caps + 两 adapter(R3/R5)

- [ ] `llm/types/message.rs` `ContentBlock::Image { file, media_type }` + serde round-trip 单测
- [ ] `llm/provider/wire/types.rs` `WireCapabilities.supports_images`(derive 矩阵补行)+ `WireBlock::Image { media_type, data }`
- [ ] `to_wire.rs`:resolve(Image{file}→base64,只读一次盘)+ caps=false → Text 占位块(写法抄 `to_wire.rs:504` Reasoning gate)
- [ ] Anthropic adapter `image`/base64 source;OpenAI adapter `image_url` data URL;各自 payload 单测锁形状
- [ ] 群聊 `group_chat_tool_defs` 不涉及;`role_history` user 行 clone 天然透传(仅加回归测试,PR6 一起)
- 验证:`cargo test -p everlasting --lib "wire"` / `"to_wire"`

## PR3 — 附件存储 + GET 路由 + 清理(R4/R7)

- [ ] `agent/attachments.rs`(或 `daemon/attachments.rs`):`save_attachment`(uuid 文件名、mime 白名单、5MB、新图/轮 ≤10 服务端复核、返回 `{file}`)+ `delete_session` 级联删目录(挂 `delete_session_inner`,紧邻 StubRegistry 清理)
- [ ] `daemon/routes/attachments.rs`:`POST /api/v1/attachments/save` + `GET /api/v1/attachments/:session_id/:file`(严格格式正则 + canonicalize 双保险;Content-Type + immutable 缓存头)+ `routes/mod.rs` 挂载
- [ ] Tauri command 镜像(transport parity,参照既有双入口模式)
- [ ] 测试:白名单外/超限/traversal 拒绝;200/400/404;删除级联
- 验证:`cargo test -p everlasting --lib "attachment"`

## PR4 — chat 链路:发送/注入/持久化/token(R2 后端半 + R6 后端半)

- [ ] `llm/types/chat.rs` `ChatMessage.attachments: Option<Vec<AttachmentRef>>`(serde default/skip)
- [ ] `ChatRequest`(daemon + Tauri command)加 `attachments: Vec<AttachmentRef>`
- [ ] `agent/chat_loop/init.rs`:当轮 attachments + @图 refs merge 进 `messages.metadata`(与 injections 同 JSON,扩 `init.rs:632` 的 update);user 行内存侧追加 Image 块;历史行按 `ChatMessage.attachments` 每轮重建
- [ ] `agent/at_file.rs`:`FileKind::Image` 分支 495 行占位 → 复制副本进 attachments + `[image: {relpath}]` 文本标记 + manifest record;**超限/白名单外回退占位文案**(B2 兜底同模式);**改写既有测试 `image_file_degrades_to_placeholder`(`at_file.rs:670`,锁旧占位文案)+ 新增真注入/超限回退两测**(评审 P2-1/P1-2)
- [ ] **请求总量闸**:init.rs 重建历史后 Image 块总数 >20 → 明确报错(两级闸之二,评审 P1-4 修正版)
- [ ] `turn_trace.images_token` 列(`schema.rs` 加 `add_turn_trace_column_if_missing` 一行)+ `db/trace.rs` upsert 扩参 + **口径 = 请求内全部 Image 块求和(含历史重建,写点在请求构建完成后)** + `LoopInit`→`drive_turn` 落库 + `<TracePanel>` TurnCard `img` cell;@图 w/h 用 `imagesize` crate 读文件头(评审 P0-1/P1-1)
- [ ] `turn-smoke.sh` 报告列加 images_token
- [ ] 测试:@图复制与 manifest;超限回退占位;metadata merge 不覆盖 injections;init 重建历史 Image 块;images_token 历史图计入(第 2 轮请求含 2 轮图);群聊含图 user 行透传(role_history 回归);**resend/edit 从 DB 重建历史时 attachments 回传**
- 验证:`cargo test -p everlasting --lib "chat_loop" "at_file" "trace"`;`scripts/turn-smoke.sh` 无图轮不回归

## PR5 — 前端全套(R2a/R3/R6 前端半/R7)

- [ ] `chatInputCodeMirror.ts`:`EditorView.domEventHandlers({ paste })`(图片拦截/纯文本放行);ChatInput 暂存列表 UI(缩略图 + X 删除 + 数量闸 ≤10)
- [ ] 发送签名:`emit("send", text, staged?)` → `chatSendActions.send(text, staged?)`:逐图 `save_attachment` → `startRequest({..., attachments})` → 清暂存;**任一图 upload 失败 → toast + 整体 abort + 暂存保留**(评审 P1-3);**objectURL 销毁函数**(删除/清空/切 session/卸载时 revokeObjectURL,评审 P2-3);纯图发送守卫;乐观 userMsg 挂 `metadata.attachments`
- [ ] `attachmentUrl` util(三模式:PROD 相对 / DEV daemonBase / pwa-remote proxyPrefix+query token)+ `vite.config.ts` 加 `/api` → 7456 proxy
- [ ] MessageItem 缩略图行(FileInjectionsHint 同位阶;点击 `window.open` 原图);session 切换清暂存(确认 ChatInput 生命周期后选 watch 或 onUnmounted)
- [ ] `markdown.ts` DOMPurify 收紧(img 两态放行:相对 `/api/v1/attachments/` 前缀 OR host==daemonBase 绝对 URL;外链图 → `<a>` 链接)+ 单测锁
- [ ] toast 轻提示(当前 model `supportsImages===false` 且有暂存图)
- [ ] Settings:`models.ts` / `ModelsTab.vue` / `ModelForm.vue` / `ModelRow.vue` / `DefaultTab.vue` vision tag + checkbox 全链接线
- [ ] **实施时验证 pwa-remote GET binary 转发**;不通则按 design §3.3 降级(链接不内联)+ 记 DEBT.md
- 验证:`pnpm test` + `pnpm vue-tsc --noEmit`;浏览器模式(daemon serve dist)手测缩略图/`<img>` 直载

## PR6 — live 验证 + 收口

- [ ] 视觉模型 live:贴截图问图(Anthropic + OpenAI 兼容各一轮);`supports_images=false` 复验占位 + toast;双轮 cache 率对照(照 memory-gov 验证法)
- [ ] 群聊 live:贴图后各参与者可见/无视觉参与者收占位
- [ ] `cargo test -p everlasting --lib` + `pnpm test` + `vue-tsc` 全绿;clippy 干净
- [ ] ROADMAP §1.2 加行 + BACKLOG §3.1 图片切片标记完成 + spec 沉淀(`token-usage-tracking` images_token Scenario;`frontend/chat.md` 缩略图行 + DOMPurify 收缩契约;`llm-contract` Image 块 Pair 规则)
- [ ] 任务归档(`task.py archive`)

## 风险文件 / 回滚点

- `agent/chat_loop/init.rs`(群聊 dedup 守卫敏感区,改动只加不改)
- `llm/provider/wire/to_wire.rs`(两 protocol 契约核心,新分支隔离)
- 回退:PR2 独立回退 = 全量占位降级(等价现状);PR5 回退 = 前端无图入口,后端列/路由无害残留
