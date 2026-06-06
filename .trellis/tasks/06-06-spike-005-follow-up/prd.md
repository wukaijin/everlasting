# spike-005 follow-up: 5 个 UI/UX 修复 + 工具稳定性 + 打断机制

> Source: [`docs/spikes/2026-06-06-feature-requests.md`](../../docs/spikes/2026-06-06-feature-requests.md)
> 关联 task: `.trellis/tasks/06-06-spike-005-follow-up/` (父, status=planning)
> 关联 children: `06-06-pr1-minor-ui-tweaks` ... `06-06-pr7-first-blank-line`
> 优先级: 父 task P1;PR5 (cancel) + PR6 (markdown) P0;PR2 P1;其余 P2

## Goal

把 spike-005 记录的 5 个未做 feature request 落地为 7 个可独立合并的 sub-PR。
其中 PR5 (LLM 打断) 和 PR6 (markdown 渲染) 是 P0 体验 / 架构改动;
PR2 (git branch) 是已知 follow-up 需要 backend 探测 + DB 迁移;
PR3 (pwd 简化) 是 BACKLOG §5.1 已知 follow-up;
PR1 + PR4 + PR7 是轻 UI 微调 + 诊断。

## What I already know

- spike 文档原文 5 条 (已读)
- `ChatPanel.vue:55-57` 注释明确: backend 尚未暴露 `git_branch`,当前 chip 是静态 "git" 占位
- `BACKLOG.md §5.1` 已记录 `cwd 简化为 ~/` 的 follow-up,工作量 ~30 行
- `MessageItem.vue:87-90` 当前是纯文本 `{{ message.content }}` 渲染,无 markdown 库
- `MessageItem.vue:145` `white-space: pre-wrap` — 换行/空格保留,但 markdown 字符直接展示
- `lib.rs:506-734` Agent Loop 无 cancel 通道;`tauri::async_runtime::spawn` 起的 task 没存 handle
- `chat.ts:693-767` `send()` 无 abort; `currentRequestId` 只用于事件过滤
- `write_file.rs:46-156` `execute()` 有 5 个 is_error 出口,前 4 个确定性,第 5 个 IO 错误可能"偶发"
- `app/src-tauri/src/projects/detector.rs` 已有 `is_git_repo_sync` + `is_git_repo` async pair (1s timeout + spawn_blocking)
- `db.rs:287-301` 现有 `pragma_table_info` + `ALTER TABLE ADD COLUMN` 幂等 migration pattern
- `app/src-tauri/src/tools/shell.rs` 走 `tokio::process::Command` shell-out 模式
- `package.json` 暂无 `marked` / `dompurify` / `markdown-it` / `vitest` 依赖
- 父 task 的 base branch 是 `refactor/ui-dark-theme-tailwind`
- Tauri / WSL / Vue 3 / reka-ui 等技术栈已锁定 (CLAUDE.md)

## Assumptions (resolved)

- [A1] ✅ spike 里的 5 条都接受为 "本 task 范围内",无排除项
- [A2] ✅ 5 条拆 7 个 sub-PR(按 spike 原文 1:1,见 PR1-PR7)
- [A3] ✅ markdown 库: `marked@18.0.5` + `dompurify@3.4.8` (research 推荐)
- [A4] ✅ git 探测: `std::process::Command` shell-out (research 推荐; 复用 detector.rs 模式)
- [A5] ✅ cancel 机制: 方案 A — `cancel_chat` Tauri command + `CancellationToken` + 前端 stop 按钮

## Open Questions

(全部 resolved)

## Requirements

### PR1 — 圆点+header 高度/字号 微调 (P2)
- sessions 状态圆点放最左侧,size 8px
- chat panel header 高度 28px
- session title 字体变小

### PR2 — git branch 真显示 (P1)
- `projects` 表加 `is_git_repo: bool` + `git_branch: Option<String>` 列(幂等 migration 复用 `db.rs:287-301` pattern)
- `projects/detector.rs` 新增 `current_branch_sync(path) -> Option<String>` + `current_branch(path) -> Option<String>` (1s timeout + spawn_blocking,镜像 is_git_repo pair)
- `create_project` / `update_project_path` 时探测并存 DB
- `ChatPanel.vue:74` 把静态 "git" 替换为 `currentProject.value.git_branch`
- detached HEAD: DB 存 `"HEAD"` 字符串(让 UI 区分)
- 旧数据 migration: 现有 projects 启动时 lazy 探测补字段(避免一次性 batch migration 锁库)

### PR3 — pwd 简化为 ~/ (P2)
- Backend: 新 `get_home_dir` Tauri command,`dirs::home_dir()`
- Frontend: 缓存 + 路径前缀替换(`/home/carlos/code/foo` → `~/code/foo`)
- 复用 BACKLOG §5.1 已估工作量 ~30 行

### PR4 — write tool 加 tracing (P2)
- `write_file.rs::execute()` 入口: `tracing::debug!({ path, content_len, is_existing, tail_components }, "write_file called")`
- 失败点: `tracing::debug!({ path, error }, "write_file failed")`
- 不改业务逻辑,~5 行

### PR5 — LLM 打断机制 (P0)
- Backend: `AppState` 加 `Arc<Mutex<HashMap<String, CancellationToken>>>` (用 `tokio_util::sync::CancellationToken`)
- `chat` 命令入口拿/建 token 存 map
- `stream.next().await` 改用 `tokio::select!` 同时等 token
- 新 `cancel_chat(request_id)` Tauri command → 拿 token 取消
- `chat` task 退出时清 token
- Frontend: 加 `Stop` 按钮(在 `ChatInput.vue` 或 `ChatPanel.vue`),调 `invoke("cancel_chat", { requestId })`
- `chat.ts:send()` 返回的 assistant 消息区分 `cancelled` 状态(不同于 `error`)
- 已收到的 tokens 仍然 `persist_turn` 落 DB 不丢
- **测试**: cargo test 加 cancel token 并发用例(token 取消后 stream 不再 emit)

### PR6 — markdown 渲染 (P0)
- `package.json` 加 `marked@18.0.5` + `dompurify@3.4.8`(lockfile 锁精确版本)
- `app/src/utils/markdown.ts` 新文件:`renderMarkdown(text: string) -> string`,内部 `marked.parse(text)` → `DOMPurify.sanitize(html)`
- `MessageItem.vue:87-90` 把 `{{ message.content }}` 替换为 `v-html="renderedContent"`
- 流式期间 50ms debounce 合并 delta 后再渲染
- `marked v8+ 已删 sanitize`,必须外配 DOMPurify 无例外
- BACKLOG §5 `code_block` 高亮本 v1 不集成,届时单独评估
- **测试** (PR 顺手开 vitest 基础): 加 XSS fixture (docs/ 留痕 + 单元测试 `expect(renderMarkdown('<script>...</script>')).not.toContain('<script>')`)

### PR7 — 首行空行排查 (P2)
- 先抓一次实际 LLM 输出复现问题(看内容 strip 后是不是还空)
- 根因可能: (a) LLM `\n` 开头 → 渲染前 `content.trimStart()` (b) CSS padding → 调 `.msg__bubble { padding-top: 0 }` (c) Vue transition 时机
- 修法取决于根因,~5-30 行

## Acceptance Criteria

### 全局(7 个 PR 共用)
- [ ] 每个 sub-PR 走完 standard Trellis 流程 (start → implement → check → commit → finish)
- [ ] `pnpm tauri build` 通过 (vue-tsc + vite + Rust release build)
- [ ] `cd app/src-tauri && cargo test` 通过
- [ ] 关联 docs (`BACKLOG.md` / `IMPLEMENTATION.md` / `HANDOFF.md`) 按需更新

### PR1
- [ ] sessions 状态圆点视觉确认: 大小 8px, 位置最左
- [ ] header 高度实测 28px
- [ ] session title 字体缩小

### PR2
- [ ] 新建 git 项目: chip 显示真实分支名
- [ ] 新建非 git 项目: chip 不显示(沿用当前逻辑)
- [ ] 在 git 项目里 detached HEAD: chip 显示 "HEAD @ abc1234"
- [ ] `update_project_path` 切换到非 git 目录: chip 消失, git_branch=None
- [ ] DB migration 幂等(老库二次启动不报错)
- [ ] cargo test: detector::current_branch_sync 覆盖正常/detached/非 git 三种 case

### PR3
- [ ] `/home/carlos/code/foo` 显示 `~/code/foo`
- [ ] 跨 OS: Windows (`C:\Users\...\foo`) 处理
- [ ] 路径不在 home 下: 保留全路径(不强行加 `~/`)

### PR4
- [ ] 手动跑 `RUST_LOG=debug` 触发 write_file,日志包含 path/content_len
- [ ] 业务逻辑零变化(原 cargo test 全过)

### PR5
- [ ] 流式期间点击 Stop 按钮: 1s 内前端 `sending=false`, 后续 delta 不再追加
- [ ] 取消时已收到的 assistant tokens 持久化到 DB
- [ ] 取消后 `currentRequestId` 清空
- [ ] 切换 session 不影响正在取消的 stream
- [ ] cargo test: 并发 cancel 不死锁、不 panic、不重复 emit

### PR6
- [ ] 流式期间: 看到 markdown 实时渲染(标题加粗/列表/链接等)
- [ ] 流式期间性能: 50ms debounce 后渲染(肉眼无延迟)
- [ ] XSS 防护: `<script>alert(1)</script>` 输入不执行
- [ ] vitest 装好, 基础 XSS fixture 留痕在 `docs/`
- [ ] marked v18 lockfile 锁定, README/docs 注明不能自动升

### PR7
- [ ] 复现首行空行后记录根因(文档或 commit message)
- [ ] 修法后,流式期间首行不再有空行

## Definition of Done

- 7 个 child task 全部 archived
- 全局 cargo test / vitest / pnpm tauri build 通过
- 父 task status=finished
- 任何破坏性变更在 CHANGELOG / docs/ 里有记录
- 新依赖(`marked`, `dompurify`, `vitest`)在 `package.json` + `package-lock.json` 里,lockfile 锁定
- 新 DB 列在 `db.rs` migration 里幂等处理

## Out of Scope

- 完整三栏 UI (在 3b-2 暂缓项里)
- rig-core 迁移
- BACKLOG §1-§7 的 7 个大功能(图片 / @ / /command / Skill / Memory / 角色 / 生成式 UI / 飞书 / 云端)
- BACKLOG §5 generative UI 的 `code_block` 高亮(单独 PR)
- 自动滚屏、消息搜索、消息分页(消息列表层 PR)
- 写文件并发/原子性(目前 write_file 是覆盖式,无 backup)

## Technical Approach (汇总)

### Backend (Rust)
- `lib.rs`: `AppState` 改造 + `cancel_chat` command + stream select 包裹
- `projects/detector.rs`: 新增 `current_branch` sync+async pair
- `projects/store.rs`: create/update_project_path 时调用探测
- `db.rs`: projects 表加 `is_git_repo` + `git_branch` 列(幂等 migration)
- `tools/write_file.rs`: 加 `tracing::debug!`

### Frontend (Vue 3 + TS)
- `ChatPanel.vue`: 静态 "git" 替换, height padding 调
- `app/src/components/chat/MessageItem.vue`: `v-html` 渲染 markdown
- `app/src/utils/markdown.ts`: 新文件, `renderMarkdown()` + 流式 debounce
- `app/src/stores/chat.ts`: cancel 状态机 + abort 逻辑
- `app/src/components/chat/ChatInput.vue` 或 `ChatPanel.vue`: Stop 按钮
- `app/package.json`: 加 `marked` + `dompurify` + `vitest`

### Test
- `app/src-tauri/src/projects/detector.rs`: 加 cargo test (current_branch_sync 三 case)
- `app/src-tauri/src/lib.rs`: 加 cargo test (cancel token 并发)
- `app/src/utils/markdown.test.ts`: vitest XSS fixture
- `app/vitest.config.ts`: 基础 config

## Decision (ADR-lite)

### Decision 1: 7 个 sub-PR 按 spike 原文 1:1 拆
- **Context**: 5 个 feature 异构(2 P0 + 中 + 轻 UI),混 PR review 难
- **Decision**: 7 个独立 sub-PR,每个对应 spike 一条 (除 #2 拆为 PR1+PR2+PR3)
- **Consequences**: 7 个 child task 管理成本;但 PR review 清晰,失败可单 PR revert

### Decision 2: markdown 库 marked v18 + DOMPurify
- **Context**: research 三个候选(marked / markdown-it / micromark),需要 XSS 防护 + 流式容忍
- **Decision**: `marked@18.0.5` + `dompurify@3.4.8` (lockfile 锁版本)
- **Consequences**: 23KB gzipped, 零运行时依赖;v18 刚发 2 天需观望;必须外配 DOMPurify

### Decision 3: git 探测 std::process::Command
- **Context**: research 三个候选(Command shell-out / git2 / gix),需要复用现有 detector 模式
- **Decision**: `std::process::Command("git")` 复用 `detector.rs` 现有 async + 1s timeout + spawn_blocking pair
- **Consequences**: 零新依赖;`git2-rs` 锁给 step-4 worktree 阶段

### Decision 4: detached HEAD 存 "HEAD" 字符串
- **Context**: `git rev-parse --abbrev-ref HEAD` 在 detached 时返回字面量 "HEAD"
- **Decision**: DB 存 `Some("HEAD")`,UI 区分正常 branch / detached
- **Consequences**: 保留 "detached" 状态信息;需 `HEAD @ abc1234` UI 渲染

### Decision 5: cancel 机制方案 A (cancel_chat command + CancellationToken)
- **Context**: 三个方案(A 真取消 / B 假停止 / C 含 tool 中断),需平衡改动量 vs 彻底性
- **Decision**: 方案 A — 真取消 stream + 已有 tokens 持久化
- **Consequences**: ~120 行;tool 执行中的中断留给 v2 考虑(目前 tool 5min timeout 已能硬卡)

### Decision 6: 流式期间也渲染 markdown (B 方案)
- **Context**: marked 自动补全未闭合链接/容忍半成品 fence,但每次 delta 渲染 + sanitize 性能开销
- **Decision**: 50ms debounce 合并 delta 后再渲染
- **Consequences**: 体验丝滑;状态机稍复杂(需要管理 debounce timer)

### Decision 7: 本任务顺手引入 vitest
- **Context**: PR5 cancel + PR6 markdown 都需要单元测试;项目当前无前端测试框架
- **Decision**: PR6 负责 vitest 基础架构(装包 + config + 1 个 fixture);后续 PR 复用
- **Consequences**: 项目从此有前端测试能力;PR6 工作量 +20 行

## Technical Notes

- 改动文件主要:
  - `app/src/components/chat/ChatPanel.vue`
  - `app/src/components/chat/MessageItem.vue`
  - `app/src/components/chat/ChatInput.vue`
  - `app/src/stores/chat.ts`
  - `app/src/utils/markdown.ts` (新)
  - `app/src/utils/markdown.test.ts` (新)
  - `app/vitest.config.ts` (新)
  - `app/package.json`
  - `app/src-tauri/src/lib.rs`
  - `app/src-tauri/src/projects/detector.rs`
  - `app/src-tauri/src/projects/store.rs`
  - `app/src-tauri/src/db.rs`
  - `app/src-tauri/src/tools/write_file.rs`
- 关联 docs: `docs/BACKLOG.md §5.1` (cwd 简化)、§5 (generative UI)
- 关联 spec: `.trellis/spec/...` (待 `get_context.py --mode packages` 列后填 implement.jsonl / check.jsonl)
- 关联 research: `research/markdown-library.md`, `research/git-detection-rust.md`

## Research References

* [`research/markdown-library.md`](research/markdown-library.md) — 推荐 `marked@18.0.5` + `dompurify@3.4.8` (23KB gzipped, 零传递依赖, 流式行为最优雅, 强制 XSS 防护)
* [`research/git-detection-rust.md`](research/git-detection-rust.md) — 推荐 `std::process::Command` shell-out (零新依赖, 复用现有 `projects/detector.rs` 模式; `git2` crate 推迟到 step-4 worktree 阶段)
