# B5 Memory — 2 层先做(user + project)

> V2 第一档收尾项;后续 B6 Subagent 硬依赖。Phase 1:Plan 收敛版(2026-06-10)。

## Goal

让 agent 在 ⑤ Context 构造关(ARCHITECTURE §2.2 第 ⑤a)自动加载 **2 层 × 2 文件 Memory**(User CLAUDE.md / User AGENTS.md / Project CLAUDE.md / Project AGENTS.md),顶部 banner + 独立占段拼到 system prompt。**V2 1 期**:2 层(不含 Session / Runtime),不做 use_memory 工具,不做 FTS5 检索,不限制 token。Loader 接口分时设计,为 B6 / Runtime 留接口位。

## What I already know

### 来自 BACKLOG §3 多层 Memory 与约束

- 4 层设计(从外到内,优先级递增):User / Project / Session / Runtime
- 加载优先级:User → Project → Session
- UI 实时反映:用户编辑 user memory 文件,下一个 user message 立即生效

### 来自 ARCHITECTURE §2.2 第 ⑤ 关

- 5a 加载 4 层 Memory(5b 注入 Role / 5c 列 Skill 描述)
- §2.5.5 Context 超限降级:Memory 在"不动"区(必须保护)
- 16 关顺序错就 bug

### 来自 ROADMAP V2 第一档(2026-06-10 重排)

- B5 排在 🟢 第一档
- B6 Subagent 依赖 B5(worker 需要 user/project memory 上下文)

### 来自 A4 PRD 范本(2026-06-10)

- LLM 解析 → agent loop → DB / FS schema → 前端监听 → UI 渲染,任一环节缺失中间态不可跑
- spec 沉淀到 `.trellis/spec/backend/...` + `frontend/...` 新增段
- 决策日志追加 1 条到 `docs/IMPLEMENTATION.md §4`

## Decisions 锁定(grill-with-docs 阶段 9 题)

### 1. 范围 expansion(2026-06-10 锁定)

- ✅ **loader 接口分时设计** — 留 Session / Runtime 接口位
- ✅ **MemoryPreview 面板** — Settings + Project 双入口
- ✅ **失败兜底** — `tracing::warn!` + skip 该层 + 继续,绝不阻断

### 2. 文件名(2026-06-10 锁定)

- **User 层**:`~/.config/everlasting/` 下 2 文件:`CLAUDE.md` + `AGENTS.md`
- **Project 层**项目根目录下 2 文件:`CLAUDE.md` + `AGENTS.md`
- 顺序:User CLAUDE → User AGENTS → Project CLAUDE → Project AGENTS

### 3. 加载时机(2026-06-10 锁定)

- 启动一次 + `notify` 监听 4 个固定文件(inotify 防抖 1s)
- 新建 memory 文件需重启 session 生效

### 4. 加载状态反馈(2026-06-10 锁定)

- session **首条 message** 顶部 banner:`已加载 4 个 memory: [User CLAUDE.md] (xxx tokens) / ...`
- 文件变更后**下一条 message** 轻提示:`memory 重新加载: [User CLAUDE.md] updated`
- StatusBar / toast 不弹(不打扰)

### 5. 注入位置(2026-06-10 锁定)

- system prompt 顶部 banner + 4 个文件独立占段(每段带 `[User CLAUDE.md]` 等标注)
- 顺序:Memory → Role(5b)→ Skill 描述(5c)→ history → new user msg

### 6. 覆盖规则(2026-06-10 锁定)

- **各文件独立占段**,不覆盖不拼接,LLM 自己看

### 7. Token 限制(2026-06-10 锁定)

- **不限制**(2K 在 200K context window 只占 1%)
- 上下文超限通用裁剪归 C3 压缩

### 8. UI 入口(2026-06-10 锁定)

- **只读预览 + 外部编辑器跳转**($EDITOR)
- 显示 4 个文件位置 + 分层内容 + token 数
- Settings 页 + Project Tabs 旁双入口

### 9. PR 拆分(2026-06-10 锁定)

- 2 PR(后端 loader + 前端 UI 预览分开)
  - PR1 后端:`memory loader` 模块 + `notify` 监听 + agent loop 注入 + spec + 测试
  - PR2 前端:`<MemoryPreview>` 组件 + Settings/Project 入口 + token 数显示 + $EDITOR 跳转

## Requirements

### R1 — 后端 memory loader 模块

- 新增 `app/src-tauri/src/memory/` 模块(`mod.rs` + `loader.rs` + `file.rs` + `tokens.rs` + `tests.rs`)
- `loader.rs::load_for_session(project_id) -> Vec<MemoryLayer>` 返回分层结构:
  ```rust
  pub enum MemoryKind { User, Project, Session, Runtime }
  pub struct MemoryLayer {
      pub kind: MemoryKind,           // V2 1 期只填 User/Project
      pub source: MemorySource,       // CLAUDE.md | AGENTS.md
      pub path: PathBuf,
      pub content: String,            // 含 frontmatter
      pub tokens: u32,
      pub status: LayerStatus,        // Loaded | Missing | Error
  }
  ```
- 4 个文件固定路径:
  - User:`~/.config/everlasting/CLAUDE.md` + `AGENTS.md`
  - Project:`<project.path>/CLAUDE.md` + `AGENTS.md`
- 失败兜底(每个文件独立 try):文件不存在 / 权限 / 编码 / symlink / > 100KB → `LayerStatus::Error` + `tracing::warn!`,**不**阻断其他层
- `tokens.rs::count_tokens(text) -> u32`:用 `tiktoken-rs` cl100k_base 估算(token 数估算足够,不需要逐模型精确)

### R2 — notify 监听 + session 缓存

- 启动时为 4 个固定路径各注册 `notify::RecommendedWatcher`(inotify 防抖 1s)
- 监听事件:写入完成(Modify + CloseWrite 合并)
- 收到事件 → invalidate 缓存 → 下一条 user message 重新 `load_for_session`
- **新建 memory 文件需重启 session**(启动时才确定 4 个固定路径)
- 缓存结构:在 `state.rs` 加 `MemoryCache { user: RwLock<[Option<MemoryLayer>; 2]>, project: RwLock<HashMap<ProjectId, [Option<MemoryLayer>; 2]>> }`
- `delete_session` / `delete_project` 时 `cache.invalidate_project(project_id)`

### R3 — agent loop 注入到 ⑤ 关

- 在 `agent/chat.rs::build_context` 函数(对应 §2.2 第 ⑤ 关)插入 `memory::loader::load_for_session(project_id)`
- 拼到 system prompt 头部:
  ```rust
  let system_prompt = format!(
      "{banner}\n{layers_block}\n{role_prompt}\n{skill_descriptions}",
      banner = "已加载 N 个 memory: [User CLAUDE.md] (xxx tokens) / ...",
      layers_block = layers.iter().map(|l| format!("[{}]\n{}\n", l.label, l.content)).collect::<Vec<_>>().join("\n"),
      ...
  );
  ```
- 首条 message / 变更 message 在 chat 消息头加 banner(用 `system_reminder` 风格 XML 标签 `<system>...</system>`,Anthropic 原生支持)

### R4 — Tauri commands

- 新增 `app/src-tauri/src/commands/memory.rs`:
  - `read_memory_layers() -> Vec<MemoryLayerInfo>` — 给前端预览用(只返 summary,content 走单独命令)
  - `read_memory_content(path) -> String` — 单文件内容(前端预览面板用)
  - `open_memory_in_editor(path)` — 走 `$EDITOR` 或 `tauri-plugin-shell` 启动外部编辑器
- 前端走 `invoke('read_memory_layers')` 在 Settings / Project 入口展示,点击"打开"调 `open_memory_in_editor`

### R5 — 前端 `<MemoryPreview>` 组件

- 新增 `app/src/components/memory/MemoryPreview.vue` + `MemoryLayerItem.vue`
- 显示:每层一个 card,标题 `[User CLAUDE.md] (xxx tokens)`,内容(只读,带 `<Markdown>` 渲染),底部"在外部编辑器打开"按钮
- Settings 页(`SettingsModal`)加 Tab:"Memory" → 展示 2 个 User layer
- Project Tabs 旁加下拉"Memory" → 展示 2 个 Project layer
- 状态展示:`Loaded`(绿) / `Missing`(灰) / `Error`(黄,hover 看 reason)
- 监听后端 reload 事件(refetch layer list,不改内容)

## Acceptance Criteria

### 后端

- [ ] `memory::loader::load_for_session` 单测覆盖:文件全有 / 全无 / 部分有 / 权限错 / 编码错 / 超大文件 6 种场景
- [ ] `memory::tokens::count_tokens` 单测:已知 ASCII / 中文 / 混合 / 空字符串 4 种
- [ ] `notify` 监听单测:模拟文件 write → 1s 内 invalidate 缓存(mock 路径)
- [ ] agent loop 注入单测:断言 system prompt 头部出现 `<system>已加载 N 个 memory: ...</system>` + 4 个独立占段
- [ ] `delete_session` / `delete_project` 触发 cache invalidate 单测
- [ ] `read_memory_layers` / `read_memory_content` / `open_memory_in_editor` 3 个 Tauri command 单测

### 前端

- [ ] `<MemoryPreview>` 组件 vitest(如果引入 vitest;否则 manual smoke):3 种 status 渲染正确
- [ ] Settings 页 Memory Tab 可见 + Project 下拉可见
- [ ] "在外部编辑器打开" 走 `tauri-plugin-shell` 启动 `$EDITOR`(失败 fallback 到 `xdg-open`)

### 集成

- [ ] session 启动 → 首条 message 顶部 banner 显示已加载数量 + 4 个 layer 名称
- [ ] 用户修改 `~/.config/everlasting/CLAUDE.md` → 1s 内 watcher 触发 → 下一条 message 顶部 banner 追加 "memory 重新加载: [User CLAUDE.md] updated"
- [ ] system prompt 顺序:Memory → Role → Skill → history(对齐 §2.2 第 ⑤ 关)
- [ ] User CLAUDE.md 缺失 → UI 显示 Missing(灰),LLM 仍能正常工作
- [ ] Project `AGENTS.md` 缺失 → UI 显示 Missing(灰),不影响其他层

### 质量

- [ ] `cargo test` 全过(新增 ≥ 15 项)
- [ ] `pnpm build` / `vue-tsc --noEmit` 干净
- [ ] spec 沉淀:`.trellis/spec/backend/memory.md`(loader + notify + 注入) + `.trellis/spec/frontend/memory-ui.md`(组件 + 入口)
- [ ] `docs/IMPLEMENTATION.md §4` 追加 1 条决策日志

## Definition of Done

- ✅ 2 PR 合入 main(后端 + 前端)
- ✅ 所有 acceptance criteria 勾选
- ✅ spec 文档更新
- ✅ 决策日志更新

## Out of Scope(本期明确不做)

- Session-level memory(SQLite `session_instructions` 表)— V2 2 期
- Runtime memory(SQLite `memories` + FTS5)— V2 2 期
- `use_memory` tool 暴露给 LLM — Runtime 期再做
- 审计日志 — 归 C4
- Token 硬卡的 LLM 摘要降级 — 归 C3 上下文压缩
- 跨设备 memory 同步 — 归 🔴 第四档 B11
- 新建 memory 文件时 hot-reload(只监听 4 个固定文件,新建需重启)
- 内嵌 Markdown 编辑器(本期只读 + 跳外部)

## Technical Notes

- 现有资源加载器:ARCHITECTURE §1 架构图 "Resource Loaders" 块列出 Memory / Skill / Role / Command 共用 frontmatter loader,**还没有实际代码** — 本期只做 Memory loader,Skill / Role / Command loader 等各自任务时再做
- 16 关里第 ⑤a 子步骤是 Memory 加载位置(`agent/chat.rs::build_context`)
- 监听到文件变化后:下一个 user message 重新加载(在 `chat_stream_with_tools` 入口前 invalidate)
- 4 个固定路径用 `dirs::home_dir()` 解析 `~/.config/everlasting/`
- `notify` 推荐 watcher:linux 用 `INotifyWatcher`,macOS `FsEventsWatcher`,Windows `ReadDirectoryChangesWatcher`
- Tiktoken 选 `cl100k_base`(兼容 OpenAI / Claude 估算)
- `tauri-plugin-shell` 已在前端依赖里(用于 worktree `git` 命令,扩展用于 $EDITOR)
- 前端 Markdown 渲染:已用 `marked` + `DOMPurify`(A4 后 PR 落地的)

## Decision (ADR-lite)

**Context**:B5 Memory 是 V2 第一档收尾项 + B6 Subagent 硬依赖;V2 1 期锁 2 层先做(Session / Runtime 后补);多文件加载有 4 种策略可选。

**Decision**:
1. 文件名 = 4 固定文件(CLAUDE.md + AGENTS.md × User + Project),不限制单层单文件
2. 覆盖规则 = 独立占段,不覆盖不拼接(简化 loader 逻辑,LLM 自己看)
3. 加载时机 = 启动一次 + notify 监听(用户行为合理,不打扰)
4. Token 限制 = 不限制(2K 在 200K context 占 1%,硬限制增加裁剪复杂度)
5. UI = 只读 + 跳外部编辑器(不引入内嵌 Markdown 编辑器,降低范围)
6. Loader 接口 = `Vec<MemoryLayer>` 数组,`MemoryKind` 枚举预留 Session / Runtime

**Consequences**:
- ✅ Loader 接口天然支持 B6 Subagent(B6 worker 直接复用 loader,加 `Project` 即可)
- ✅ 4 文件独立占段 = 调试可观测(用户能看到 LLM 实际看到哪 4 段)
- ✅ 不限制 token = 实现简单,功能等价(2K 实际占用小)
- ⚠️ 4 个文件监听固定路径,新建 memory 文件需重启 session 生效(用户已接受)
- ⚠️ 没 use_memory tool,LLM 不能主动 read(够用 1 期,Runtime 期再做)
- ⚠️ 没内嵌编辑器,用户改 memory 需外部编辑器(已接受,符合"User memory 应该是用户在他自己的环境里编辑"哲学)
