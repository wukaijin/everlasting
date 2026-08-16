# B1 图片支持(multimodal)

> **状态(2026-08-16)**:议程 7 题已全部商讨定案(见 Requirements),PRD 已过收敛 pass。`task.py start` 前需补 design.md + implement.md(跨 DB/wire/工具/UI 四层复杂任务);design 层待定点见文末 Notes。

## Goal

把输入层从纯文本通道升级为支持图片(multimodal):用户能把图片给到 agent(粘贴 + @文件两路入口),支持图片的模型能"看到"并据此推理;同时补齐 token 治理的"图片"切片(BACKLOG §3.1:tools/memory 已落,图片是第三片,统一预算表评估由此凑齐前置)。

## Background(代码/文档取证事实)

- **wire 层现状**:`llm/types/message.rs:60` `ContentBlock` 只有 Text/Thinking/RedactedThinking/ToolUse/ToolResult,无 Image;全链路纯文本。项目仅两个 protocol(Anthropic / OpenAI 兼容),wire 转换单点 = `content_block_to_wire_block`(`llm/provider/wire/to_wire.rs:369`)→ `WireBlock` → 各 adapter 序列化。
- **B2 @文件预留口**:`agent/at_file.rs:53` `FileKind::Image` 分支 + `at_file.rs:495` 占位降级文案"[image: … 纯文本通道,B1 计划]"——本任务升级点。
- **能力列先例**:`models` 表 `supports_thinking INTEGER NOT NULL DEFAULT 0`(`db/migrations/schema.rs:221`)+ `ModelRow.supports_thinking`(`db/types.rs:107`);wire 层 caps 消费先例 = 不支持的块被适配层处理(`to_wire.rs:504` Reasoning 过滤);前端 tag 先例 = `DefaultTab.vue:67` thinking tag / `ModelRow.vue:78` / `ModelForm.vue` checkbox。
- **messages.metadata**:`schema.rs:166` 列建表即有、至今未用(`sessions.metadata` 已被群聊首用)——附件引用载体。
- **daemon 路由现状**:全部 POST JSON(`daemon/routes/files.rs` 仅 list_files/list_files_at),无 GET 二进制路由;浏览器/PWA 是一等公民,`<img>` 显示图必须走 daemon HTTP route(本地路径在浏览器模式不可用)。
- **既有安全缺口(本任务顺手闭合)**:`utils/markdown.ts:70` DOMPurify `USE_PROFILES:{html:true}` 默认放行 `<img>`,LLM 输出外链图今天就会加载——BACKLOG §3.3"不渲染 LLM 之外的图"目前并不成立。
- **群聊块改写**:`role_history` 状态机对 assistant 行按角色改写(他人 thinking/工具对剥离);user 行透传。
- **存储目录先例**:worktree 与 DB 同根 `~/.local/share/dev.everlasting.app/`(RULE-E-006);`delete_session` 有级联清理模式(worktree destroy / StubRegistry / MemoryDigestRegistry)。
- **token 度量先例**:`turn_trace.tools_token`(C7)/`memory_token`(memory-gov)同款幂等 migration helper + TurnCard cell。

## Requirements(2026-08-16 议注定案)

- **R1(用户立项补充)**:provider model 配置增加**是否支持图片**配置项——`models` 表加 `supports_images` 列 + `ModelRow` 字段 + Settings ModelsTab checkbox,模式对齐 `supports_thinking`。
- **R2(Q1)**:MVP 入口 = **ChatInput 粘贴图片 + @文件注入升级**(占位 → 真注入)两路;拖拽文件进聊天区不进 MVP(follow-up)。
- **R2a(用户 2026-08-16 交互确认)**:粘贴采用**暂存列表**交互——ChatInput focus 时粘贴图片,**不立即发送**,在输入框区出现一个简单的图片预览列表(缩略图 + 删除按钮),随下一条消息一起发出(消息 = 文本 + 暂存图片集)。**允许纯图发送**(空文本 + 图可直接发,看图问答不逼打字);暂存为**内存态**(切 session / 刷新即清,未发送的图不落盘);粘贴文本照旧不受影响,白名单外格式粘贴 → toast 拒绝;多图拖拽排序不做(follow-up)。
- **R3(Q2)**:`supports_images=false` 模型收到图 → wire 层**占位降级**(Image 块替换为文本占位,风格同 at_file 现有文案,模型明确知道没收到图)+ 发送时**前端轻提示**(toast);model picker 加 "vision" tag(照搬 thinking tag 先例)。
- **R4(Q3)**:存储 = **文件系统 + 引用**:图片落 `app_data_dir/attachments/<session_id>/<uuid>.<ext>`;`messages.metadata` 记引用(`{type:"image", path, mime, ...}`);daemon 新增 **GET 附件路由**(浏览器/PWA/Tauri 统一走 HTTP);wire 发送时 daemon 侧读文件转 base64;`delete_session` 级联删 attachments 目录;@ 引用的图**复制副本**进 attachments(统一渲染/发送通路,session 自包含)。DB 零膨胀(不存 BLOB)。
- **R5(Q4)**:wire 适配**两个 protocol 都进 MVP**——`ContentBlock::Image` + `WireBlock::Image` + Anthropic(base64 source)/ OpenAI 兼容(image_url data URL)各自发射。
- **R6(Q5)**:token 度量 + 硬上限——`turn_trace` 加 `images_token` 列(attach 时按尺寸公式估算,存 metadata,TurnCard 加 cell);前端硬上限**单图 5MB / 单轮 10 张**(对齐两家 API 限制取严),超限拒绝 + 提示;自动压缩 follow-up。
- **R7(Q6)**:安全落法——DOMPurify 收紧:**img 只允许自家 `/api/v1/attachments/` 路由**,LLM 输出外链图降级为链接文本(点击可打开);格式白名单 **png/jpg/jpeg/webp**(gif/bmp/tiff/heic 拒绝);附件路由只收 `<session_id>/<uuid>.<ext>` 严格格式(防 traversal)。
- **R8(Q7)**:**群聊也支持**——群聊 session 贴图与 @图均生效,`role_history` 对含图 user 行透传、Image 块进参与者视图(无视觉能力的参与者模型按 R3 占位降级);**worker/subagent 豁免**(只收文本 task,不动);`read_file` 读图**维持二进制降级**(工具侧读图 follow-up)。

## Acceptance Criteria

- [ ] R1:migration 幂等可重跑;Settings checkbox 读写生效;既有行默认 `0` 现有行为零变化。
- [ ] R2:粘贴 png 进 ChatInput → 附件预览 → 发送 → 支持图片的模型收到 image 块(live 验证一轮实跑);@ 引图片文件 → 注入真 image 块,占位文案消失(单测锁)。
- [ ] R3:`supports_images=false` 时 wire 层 Image → 文本占位(单测锁);前端发送时 toast 提示;picker 显示 vision tag。
- [ ] R4:附件按 `<session_id>/<uuid>.<ext>` 落盘;GET 路由 200(合法)/ 404(不存在)/ 400(traversal 或格式非法);`delete_session` 后目录与引用清理;浏览器模式(非 Tauri)`<img>` 正常渲染。
- [ ] R5:Anthropic 与 OpenAI 兼容两 adapter 各自发射正确 image wire 格式(单测锁两种 payload);live 各验一轮(视觉模型可用性实施时从 provider catalog 确认)。
- [ ] R6:`turn_trace.images_token` 落值 + TurnCard cell 展示;>5MB 单图 / >10 张单轮被拒且提示;LLM 输出 `![](http://外链)` 渲染为链接文本、不发网络请求(单测锁 DOMPurify 配置);白名单外格式拒绝。
- [ ] R7:群聊 session 贴图,各参与者请求上下文含 image 块(或按其模型 caps 占位);既有群聊回归测试全绿;worker dispatch 路径零改动(既有 L3 测试回归)。
- [ ] 全量:`cargo test -p everlasting --lib` + 前端 vitest + `vue-tsc --noEmit` 全绿;`turn-smoke.sh` 不回归。

## Out of Scope

- 拖拽文件进聊天区(follow-up)
- 超限自动压缩 / 缩略图生成(follow-up)
- `read_file` 工具读图返回 image 块(follow-up)
- worker/subagent 传图(dispatch 通道,follow-up)
- PDF/Office 多模态(B2 占位降级继续兜底)
- 图片编辑/裁剪 UI

## Notes(design.md 待定点)

1. `role_history` 含图 user 行的透传细节(user 行本就 verbatim 透传,确认 Image 块无需改写即可;参与者模型 caps 逐人判定)。
2. 附件 content-hash 去重(同图重复粘贴/多 session 引用同图)——可选优化。
3. `images_token` 估算公式(尺寸/750 类启发式,attach 时算好存 metadata,免每轮重算)。
4. SSE 链路:附件经 metadata 静态引用即可,无需新 ChatEvent(确认 rehydrate 通路)。
5. Anthropic prompt-cache 断点与 image 块位置交互(首块 Ephemeral 是否受影响)。
6. ~~粘贴交互细节~~ 已定:R2a(暂存列表 + 纯图可发 + 内存态 + toast 拒白名单外格式;排序 follow-up)。
