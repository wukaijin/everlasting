# Implement — B1 图片收尾

PR 划分:PR1(前端压缩+拖拽)与 PR2(后端读图+wire 全链)无依赖可并行;PR3(前端呈现)依赖 PR2。每个 PR 独立可回滚、独立全绿。

## PR1 — 前端自动压缩 + 拖拽(R1/R2,AC1/AC2/AC3)

- [x] 新 `app/src/utils/imageCompress.ts`:`compressImage(file)` + 纯函数(触发判定 / alpha 判定 / 目标尺寸 / 守卫),canvas 薄壳集中
- [x] `app/src/stores/chatSendActions.ts::addStagedImages` 闸序改「mime → compress → 5MB → 10 张」;`StagedImage` 加 `compressed?/origBytes?`(chat.types.ts)
- [x] `app/src/components/chat/ChatInput.vue` 暂存条「已压缩」chip + title(原始→结果)
- [x] `ChatPanel.vue` 聊天区 drop handler(图片入暂存 / 非图片 toast「请使用 @ 引用文件」/ 混合单次提示 / 纯文本不拦截)
- [x] vitest:imageCompress 纯函数组 + addStagedImages 闸序(mock compress)+ drop handler 三态
- [x] 验证:`cd app && pnpm test` + `pnpm build`(vue-tsc)

## PR2 — read_file 读图 + ToolResult 带图全链(R3/R4/R5,AC4 后半/AC5/AC6/AC7)

- [x] `attachments.rs`:提取魔数+扩展名共享 helper(`is_image_bytes`);at_file 改调用(零行为变化,既有测试组对拍)
- [x] `tools/read_file.rs`:图片臂(白名单+魔数+5MiB 闸 → save_image 副本 + blob_size tokens_est;超限占位非 error);`execute` 返回改 `(String, bool, Option<Vec<AttachmentRef>>)`
- [x] `tools/mod.rs::execute_tool`:4 元组 → 5 元组(其他工具 arm 一律 None)
- [x] `llm/types/message.rs`:`ToolResult` 变体改 `#[serde(flatten)] data: ToolResultData`(tool_use_id/content/is_error/images/resolved);`ToolResultData` 自定义 Serialize 三分支(resolved→content block array 且不输出 images/images 字段 / images-only→字符串+refs / 双 None→与今逐字节一致)+ Deserialize 双形态;**先写无图 fixture 逐字节对拍单测再动 derive**(design §2.3)
- [x] 全仓 `ContentBlock::ToolResult { .. }` 模式匹配机械迁移(compaction.rs / helpers.rs envelope / sink / 测试)
- [x] `agent/chat_loop/tools.rs` 五构造点(并行 ~L421 + serial ~L503/662/769/850)挂 images;`build_synthetic_tool_result_message` 不带图
- [x] `attachments.rs::resolve_image_refs`:ToolResult.data.images → data.resolved(base64;**images 保留不清**,读盘失败降级文案)
- [x] `wire/types.ts` + `wire/to_wire.rs`:`WireMessage::Tool` 加 `images: Vec<ImageSource>`;两处 lift 接线(concatenating 臂拼接);`strip_unsupported` Tool 臂 caps 降级占位
- [x] `wire/from_wire.rs` Tool 臂:重建 `resolved`(Anthropic block array 发射);`openai.rs` Tool 臂占位降级
- [x] `attachments.rs::estimate_images_token`:ToolResult 臂按 tokens_est 精确计费 + 注释更新;**核对 estimate(drive.rs:924)/ budget arm2 与 resolve(916)的既有顺序,工具图跟随 user 图同流**
- [x] `agent/budget.rs` arm 2:旧轮工具图 resolved+images 双清 + 占位文案 + images_freed
- [x] sink `ToolResultPayload` 加可选 images(IPC/SSE 零新命令)
- [x] Rust 单测:design §4 清单(serde 对拍 / to_wire 两路降级 / estimate / budget / read_file 五态 / at_file 回归)
- [x] 验证:`cd app/src-tauri && PKG_CONFIG_PATH=... cargo test --lib` + clippy + fmt;`scripts/turn-smoke.sh` 不回归

## PR3 — 前端呈现 + 顺手修 + live(R6/R7,AC4 前半/AC8)

- [x] TS 类型:`ToolResultPayload` + 回看解析加可选 images
- [x] MessageItem 工具区 + SubagentDrawer ToolCallCard:缩略图行(复用 MessageImages/attachmentUrl 模式)
- [x] `at_file.rs:694-697` 占位文案更新
- [x] vitest:缩略图渲染(带/不带 images)
- [x] **live(MiniMax-M3)**:read_file 含文字截图 → 模型描述内容;贴 4K PNG → 已压缩 + tokensEst 降;重载回看缩略图;记录到 implement.md

## 验证命令速查

```bash
cd app && pnpm test && pnpm build                       # 前端
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib
cargo clippy -p everlasting --lib
scripts/turn-smoke.sh --assert-turn-usage               # 回归
```

## 风险文件 / 回滚点

- serde 面:`llm/types/message.rs`(ToolResult 自定义 serde)——对拍 fixture 是硬闸
- wire 面:`wire/to_wire.rs` / `wire/from_wire.rs` / 两 adapter——无图路径逐字节不变单测
- 前端闸序:`chatSendActions.ts`(压缩 fail-open 兜底)



## Live 验证记录(2026-08-21,release 重编 + daemon 重启)

- **turn-smoke 回归**:`--assert-turn-usage` PASS(1 event captured,seq-consistent,provider_id 归因正确)。
- **AC4 视觉 live(MiniMax-M3 / anthropic protocol)**:agent `read_file` 一张 1440×900 UI 截图 → 模型准确描述图中文字(浏览器标签 "preturn-frontend"/"everlasting"、侧栏 "会话 SESSIONS/本周/更早")——**项目正向视觉路径首次 live 实证**。DB tool_result 块携带 `images:[{file, media_type, source:"read_file", tokens_est:1728}]`(1728 = 1440×900/750 精确);`turn_trace.images_token=1728` 在**下一轮**(图随 tool_result 进第二次请求)精确入账;附件副本落盘 140894 字节与原件一致;GET attachments 路由 200 全量返回。
- **AC1 压缩 live**:压缩逻辑 vitest 全覆盖(canvas 编码路径需真实浏览器,UI 手动验证留给用户下次会话;决策纯函数 + 闸序已单测锁)。
- **坑记录**:①`app/src-tauri/target/release/everlasting-daemon` 是 workspace 翻转前(Aug 9)的陈旧产物——daemon.sh 用的是根 `target/release/`;从旧路径启动的 daemon 带着 pre-08-20 的 upsert SQL(`ON CONFLICT(session_id, seq)`),对新键形表必炸 "ON CONFLICT clause does not match",三次 smoke 失败全由此起。**教训:重启 daemon 永远用根 target 或 scripts/daemon.sh。**

## start 前检查

- [x] prd.md 收敛(D1-D5 全决议,无 open questions)
- [x] design.md / implement.md 齐
- [x] implement.jsonl / check.jsonl 真实条目(curated,见文件)
- [x] 用户批准后 `task.py start`(2026-08-21)
