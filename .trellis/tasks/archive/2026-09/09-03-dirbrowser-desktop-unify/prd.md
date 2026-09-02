# 桌面模式项目添加切 DirBrowserModal,下线 native pick_project_dir

## Goal

Tauri 桌面模式的「添加项目」从 native `pick_project_dir`(WSLg 下 GTK 风格对话框)切换到
`DirBrowserModal`,与浏览器模式共用同一条目录浏览交互,消除双模式 UX 分叉。销掉
BACKLOG §5.3 / FU-3(2026-06-05 用户偏好「dialog 由前端渲染」,挂了近三个月)。
顺带补齐键盘导航(方向键 + Enter)。

## Background / Confirmed Facts(代码取证 2026-09-03)

- **基建已就位(2026-09-02 `7a747379`)**:`browse_dir` IPC(daemon POST route +
  Tauri command 双注册,`commands/projects.rs:336`,参数 `path`/`show_hidden`,
  Tauri 2 自动 camelCase↔snake_case 映射)、`DirBrowserModal` 组件、projects store
  `dirBrowserOpen` 状态机。目前仅浏览器模式 degrade 路径生效。
- **`addProject()` 现流**(`stores/projects.ts:200-233`):invoke `pick_project_dir` →
  TransportError(status 0,浏览器模式无 daemon route)则翻 `dirBrowserOpen`;成功
  picked → `registerPickedPath`。两个消费方(`ProjectTabs.vue:53` /
  `EmptyProjectState.vue:33`)都忽略返回值。
- **`addProjectByPath(path)`**(模态框「选择此目录」出口)与 native picker 成功路径
  共享同一注册尾巴 `registerPickedPath`(visible 去重 / hidden unhide 恢复 /
  create + focus,RULE-FrontProj-001)——切换后注册语义零变化。
- **`pick_project_dir` 是 Tauri-only 链,且是该插件唯一消费方**(前端
  `@tauri-apps/plugin-dialog` 零使用):
  - `commands/projects.rs:207-230` 命令本体 + `:18` `use tauri_plugin_dialog::DialogExt`
  - `lib.rs:455` Tauri 注册 + `:143` `tauri_plugin_dialog::init()` 插件
  - `commands/mod.rs:128` `all_command_names` 清单 + `:16` doc 注释
  - `Cargo.toml:36` `tauri-plugin-dialog = "2"` 依赖
  - `capabilities/default.json:27` `"dialog:default"` 权限
  - 无 daemon route、无 `CMD_TO_DOMAIN` 映射
- **stale 注释**:`daemon/routes/mod.rs:34` 把 `pick_project_dir` 列进 projects(9)
  路由清单(实际路由表 `routes/projects.rs:148-161` 从来没有它);
  `daemon/routes/projects.rs:139` doc 注释亦提及;`projects.ts` 头部 Q8v2 语义注释。
- **测试锚点**:`projects.test.ts` ~6 用例 mock `pick_project_dir`
  (cancel / visible / hidden / new / error / degrade→dirBrowserOpen);
  `DirBrowserModal.test.ts` 7 用例组件级;routesync 守卫只校验 daemon 路由 →
  CMD_TO_DOMAIN 方向,删除该命令不影响。e2e 现有 3 条 spec 均不涉及 projects 流程。
- **BACKLOG §5.3 原始设想**(2026-06):HTML 树形目录 + 搜索框 + 键盘导航,~150 行。
  实际交付的 DirBrowserModal 是列表式(单击进入 / .. / 路径直达 / 隐藏目录开关),
  无搜索框、无键盘导航;2026-09-02 web 模式 headless Chromium 全流程实测通过。

## Requirements

- **R1(桌面切换)**:`addProject()` 不再 invoke `pick_project_dir`,重命名为
  `openDirBrowser()`——直接翻 `dirBrowserOpen = true`(桌面 / 浏览器 / sidecar /
  remote 全模式同一条路)。两个消费方同步改名。
- **R2(后端整链删除,2026-09-03 用户拍板)**:删 `commands/projects.rs` 命令本体 +
  `use tauri_plugin_dialog::DialogExt`、`lib.rs` 命令注册 + 插件注册、
  `commands/mod.rs` `all_command_names` 条目、`Cargo.toml` 依赖、
  `capabilities/default.json` `dialog:default` 权限(Cargo.lock 随 cargo check 收敛)。
- **R3(注释收口)**:`projects.ts` 头部 Q8v2 注释重写;`routes/mod.rs:34` 路由清单
  去掉从未存在的 `pick_project_dir`;`routes/projects.rs:139`、`commands/mod.rs:16`、
  DirBrowserModal 头注释中「browser-mode degrade」定性改为「全模式统一入口」。
- **R4(键盘导航,2026-09-03 用户拍板纳入)**:DirBrowserModal 列表行 roving
  tabindex——方向键在「.. + 目录行」间移动焦点(钳边,不环绕),Enter 在 focus 行
  原生触发进入;输入框聚焦时方向键不劫持;列表发起的导航完成后焦点复位到新列表
  首行,非列表发起(路径直达 / 前往)不抢焦点。Esc 关窗沿用 reka-ui Dialog 现状。
- **R5(测试对齐)**:`projects.test.ts` 锚定新流(openDirBrowser → dirBrowserOpen,
  零 pick IPC;registerPickedPath 三路径回归保留);`DirBrowserModal.test.ts` 补
  键盘导航用例;新增 e2e route-mock 用例锁「无项目空态 → 添加项目 → 模态框选目录 →
  注册成功」全流程(RULE-TEST-001 确定性准入:route-mock 全拦截、无 daemon、无网络)。
- **R6(文档销账)**:BACKLOG §5.3 标记已落地;ROADMAP §1.2 加一行(一句
  「做了什么 + 时间 + 链接」)。

## Acceptance Criteria

- [ ] AC1:Tauri 桌面模式点 ProjectTabs「+」/ EmptyProjectState「添加项目」→
      打开 DirBrowserModal;选定目录后注册成功(新建 / 已存在 focus / 隐藏恢复
      三路径回归)。桌面真机手测可留 GUI-capable 机器;浏览器模式路径由 e2e 锁。
- [ ] AC2:grep `pick_project_dir` / `tauri_plugin_dialog` 生产代码零命中
      (spec/任务归档文档除外);`cargo check` 过、`cargo test -p everlasting --lib`
      绿;routesync / transport-parity 测试绿。
- [ ] AC3:`pnpm test` + `vue-tsc` + `pnpm build` 全绿;`pnpm test:e2e` 含新用例
      全绿。
- [ ] AC4:键盘导航行为符合 R4 契约(vitest 组件级 + e2e 至少覆盖方向键移动 + Enter 进入)。
- [ ] AC5:文档收口(BACKLOG §5.3 状态 + ROADMAP §1.2 行 + stale 注释清零)。

## Out of Scope

- 搜索框 / 目录名过滤(BACKLOG §5.3 设想中唯一剩余未交付项,后续按需另立)。
- 树形视图(列表式交互已验收,不翻案)。
- `update_project_path` 等其它项目 CRUD 交互。
- native 对话框作为设置选项保留(明确不做——GTK 风格正是要消除的对象)。
