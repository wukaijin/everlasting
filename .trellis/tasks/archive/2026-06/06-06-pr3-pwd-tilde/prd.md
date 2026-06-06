# PR3: pwd 简化为 ~/ (header 远端显示)

> Source spike: [`docs/spikes/2026-06-06-feature-requests.md`](../../../../../docs/spikes/2026-06-06-feature-requests.md) 第 2d 子项 + BACKLOG §5.1 follow-up
> 父 task: `06-06-spike-005-follow-up`
> 父 prd: [../06-06-spike-005-follow-up/prd.md](../06-06-spike-005-follow-up/prd.md) (PR3 段)
> Priority: P2
> 锁定: backend 加 `get_home_dir` Tauri command (BACKLOG §5.1 已知), frontend 缓存 + 路径前缀替换

## Goal

把 chat panel header 远端显示的 `current_cwd` 从完整绝对路径简化: `/home/carlos/code/foo/backend` → `~/code/foo/backend`。
- Backend: 新 `get_home_dir()` Tauri command, 走 `dirs::home_dir()`
- Frontend: 启动时缓存 home dir, 渲染 pwd 时替换前缀

## What I already know

- BACKLOG §5.1 已有完整 follow-up 描述 + ~30 行工作量估算
- `app/src-tauri/src/lib.rs:138-145` `get_llm_config` 是现有 Tauri command 模式 (State, no params, 返回 PublicLlmConfig)
- `lib.rs:814-829` `invoke_handler!` 注册列表 (新增 command 要加到这里)
- 父 prd 锁定使用 `dirs` crate (Tauri 2 默认就带, 不需新加 dep)
- `app/src/stores/chat.ts:207` `currentCwd` ref 存当前 session 的 cwd
- `app/src/components/chat/ChatPanel.vue:1-205` header 在 PR1 改版后会展示 pwd 远端 (PR1 还没做; 暂不实现, 本 PR 准备好 home dir cache, 等 PR1 接入)
- 或: PR1 之前先做本 PR 的 home dir cache, PR1 接入 pwd 显示时直接用

## Requirements

### Backend
- 新 `get_home_dir()` Tauri command:
  ```rust
  #[tauri::command]
  fn get_home_dir() -> Option<String> {
      dirs::home_dir().map(|p| p.to_string_lossy().into_owned())
  }
  ```
- 注册到 `invoke_handler!`
- 不需要新加 dep (`dirs` crate Tauri 2 已有)

### Frontend
- `app/src/stores/config.ts` (或新建 `app/src/stores/home.ts`):
  - 新 `homeDir: ref<string | null>(null)`
  - `onMounted` 时 `await invoke<string | null>("get_home_dir")` 加载
  - 暴露 `loadHomeDir()` action
- 工具函数 `app/src/utils/path.ts` (新):
  - `simplifyPath(path: string, homeDir: string | null): string`
  - 如果 `homeDir` 为 null 或 path 不以 `homeDir` 开头 → 返回原 path
  - 否则返回 `~${path.slice(homeDir.length)}` (Linux/Mac)
  - Windows: 暂不处理 (Tauri 桌面应用 WSL 优先, 暂不考虑 Windows path)
- `app/src/stores/chat.ts`:
  - 暴露 `simplifiedCwd` computed: `simplifyPath(currentCwd.value, configStore.homeDir)`

### 不在本 PR 范围
- ChatPanel.vue header 接入 pwd 显示 — 那是 PR1 的范围 (header 改版)
- PR1 实施时, 引用 `chatStore.simplifiedCwd` 即可

## Acceptance Criteria

- [ ] `pnpm build` 通过
- [ ] `cargo check` 通过
- [ ] `cargo test` 通过
- [ ] Tauri 启动后 `get_home_dir` 返回真实 home (e.g. `/home/carlos`)
- [ ] `/home/carlos/code/foo` 显示 `~/code/foo`
- [ ] 跨 OS 行为: Windows 路径暂不处理 (Tauri WSL 优先, 见 Out of Scope)
- [ ] 路径不在 home 下: 保留全路径 (不强行加 `~/`)
- [ ] `homeDir` 加载失败 (罕见) 时: `simplifiedCwd` 退化为原 cwd, 不报错

## Definition of Done

- 修改 ~3-4 个文件
- 跑完 standard Trellis 流程到 archived
- 视觉验证: PR1 实施时 header 显示 `~/...` 而非全路径

## Out of Scope

- ChatPanel.vue header 接入 (PR1 范围)
- Windows 路径处理 (`C:\Users\foo`)
- macOS `/Users/foo` (Tauri WSL 优先)
- 用户自定义 home (env var $HOME 覆盖, dirs crate 已支持)
- 路径组件压缩 (`..`, `.` 解析) — 显示用, 不影响
- Path 中含 `~` 字面量 (e.g. `~/my~file`) — 不影响, 只 strip 前缀

## Technical Notes

- 改动文件:
  - `app/src-tauri/src/lib.rs` (新 get_home_dir command + 注册)
  - `app/src/stores/config.ts` (homeDir 缓存) 或新建 `app/src/stores/home.ts`
  - `app/src/stores/chat.ts` (simplifiedCwd computed)
  - `app/src/utils/path.ts` (新, simplifyPath 函数)
- 风险: `dirs::home_dir()` 在 sandbox 容器里返回 None (e.g. Docker without HOME) — 已用 `Option<String>`, 退化安全
- 风险: WSL 下 HOME 是 `/home/carlos` 而非 Windows mount, OK
- 风险: 路径 hardlink / symlink 不解析 — 显示用, 不影响
- 风险: home dir 加载时机 (onMounted) 比 chat store 初始化晚 — `simplifiedCwd` computed 是 reactive, homeDir 一旦加载自动重算
- 关联: BACKLOG §5.1 follow-up 是已知项, 本 PR 解掉

## Decision (ADR-lite)

- **决策 1**: homeDir 缓存到 `configStore` 而非新建 `homeStore`
  - **理由**: 跟现有 lastActiveProjectId 同一类全局设置, 集中
  - **后果**: config.ts 多一个字段, 复杂度低
- **决策 2**: Windows 路径暂不处理
  - **理由**: Tauri WSL 优先, 项目当前未跑 Windows 集成测试
  - **后果**: Windows 用户看到完整 `C:\Users\foo` 路径, 不简化; 不阻塞
- **决策 3**: 不解析 `..` / `.` / symlink
  - **理由**: 显示层简化, 不是路径语义层
  - **后果**: `~/code/../code/foo` 不会归一化为 `~/code/foo`; 用户可读
