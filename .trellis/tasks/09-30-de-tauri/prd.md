# de-Tauri：移除 GUI 壳与 Tauri 依赖链，daemon + web 单形态

## Goal

移除 Tauri GUI 壳与整条 Tauri 依赖链（tauri / tauri-build / tauri-plugin-shell / tauri-plugin-os），`everlasting-daemon`（axum HTTP + SSE）+ 浏览器访问 `:7456` 成为唯一产品形态。

用户价值（决策依据，2026-09-30 会话拍板）：

1. **依赖图减负**：`cargo tree` 实测 everlasting 正常依赖 584 个 unique crate，tauri 正向子树 347 个（≈60%）。冷构建 / CI / 新机器环境显著缩短。
2. **系统库依赖清零**：webkit2gtk / gdk-pixbuf 系统库 + `PKG_CONFIG_PATH` 痛点整链消失（HACKING-wsl 坑 1、CI 系统依赖安装步骤），everlasting 达到 everlasting-remote 同款零系统库。
3. **架构维护面收窄**：153 个 `#[tauri::command]` 薄壳（Tauri wrapper + `*_inner` 双层）删除，单一 API 面 = inner 函数族；「Tauri arg camelCase」类特例纪律（BACKLOG FU-8）随之清零。
4. 实际使用已 100% 走 web（用户陈述）：GUI Thin 模式本就只是「spawn sidecar + webview 指向 daemon」，浏览器直连是既成事实。

**明确的非目标（诚实声明，见 Out of Scope）**：日常增量编译时间（~1-2min，本 crate 自身体量 + 链接主导）与 daemon RSS（实测 57.9MB，大头是 tokio/sqlx/tiktoken/流缓冲；tauri 代码在 daemon 进程内是未执行的死重）不会因本任务显著改善。本任务买的是冷构建、环境与架构，不是这两个指标。

## Background（确认事实，file:line 锚点）

### 架构现状：daemon 早已是核心，GUI 只是又一个 HTTP 客户端

- GUI 默认 Thin 模式（`app/src-tauri/src/lib.rs:137`）：不开 AppState、不连 DB，spawn `everlasting-daemon` sidecar，前端走 httpTransport 同源连 `:7456`；`?transport=tauri` Full 模式是逃生通道（`app/src/transport/index.ts:22-30`）。
- daemon 与 GUI 共享 `everlasting_lib`（`app/src-tauri/Cargo.toml:26-28` 双 bin：GUI `everlasting` + `everlasting-daemon`）。
- 153 个 `#[tauri::command]` 全是薄壳，`*_inner` 函数族是 Tauri 命令与 daemon 路由的共用单源（例：`app/src-tauri/src/commands/sessions.rs:48-53`）。
- evl CLI、群聊 MCP（`POST /mcp`）、定时任务、远程访问（everlasting-remote/ACP）全部挂 daemon HTTP，与 GUI 无关。

### 前端现状：web 形态已齐备，Tauri 残留是薄边角

- `app/src` 生产代码仅两处碰 Tauri API：
  - `TitleBar.vue:48-49`（getCurrentWindow + platform，自定义标题栏）——`AppHeader.vue:68` 按 `isTauriWebview()` 在 TitleBar / **BrowserHeader（零 Tauri import，现成的浏览器模式头部）** 间切换，共享 slot（ProjectTabs 等）；
  - `CloseGuardDialog.vue:43`（onCloseRequested）——已 `isTauriWebview()` 门控，纯浏览器模式今天就是无守卫（关标签只断 SSE），删除不构成行为劣化。
- transport 抽象（`app/src/transport/index.ts` + http.ts + tauri.ts）：所有组件经 `transport` 单例调用；**前端测试统一 mock `../../transport` 边界**（如 AskUserQuestionCard.test.ts:39-45），保留 facade 则测试面零改动。
- `package.json` 残留 `@tauri-apps/api` / `@tauri-apps/plugin-os` / `@tauri-apps/cli` 依赖与 `tauri` script。

### 后端耦合点（改造面）

- `AppState::load(AppHandle)` → `load_inner(dir, Option<AppHandle>)`（state.rs:304/342）：daemon 路径传 None，seam 已存在。
- 事件双通道：`ChatEventSink` trait（state.rs:825）两实现——`AppHandleSink`（GUI 窗口 emit，state.rs:957）与 `HttpSseSink`（daemon SSE）。daemon 路径不读 AppHandleSink。
- `tauri::async_runtime` 引用三类（评审 2026-09-30 核实）：async 上下文内 `spawn` / `spawn_blocking`（如 subagent_runs.rs:258 的 libgit2 merge 卸载）；**`CancellationGuard::drop`（state.rs:729）依赖 tauri runtime「无 runtime 则惰性起全局」语义**——tokio 等价物在该上下文会 panic 或静默丢清理，需专门设计（design D9）；测试内 `block_on`。
- `tauri::test` 六处均为「本项目不用 mock_app」的注释性提及（评审核实：全仓无 `tauri::test::mock_app` 调用）——随 R3 注释清理自然消解，不构成改造任务。
- 第三条 AppHandle 链：`agent/subagent/event_sink.rs:29` 的 `AppHandleSubagentSink` 与 `sink.rs:165` 的 `Option<tauri::AppHandle>` 字段（daemon 路径恒 None）。
- `daemon/tunnel/tests.rs:291-293` 真实引用 `tauri::http::Response`（tungstenite 握手 401 构造）——design §1「daemon 零改动」按字面不成立，需换型。
- build.rs 三件事：`tauri_build::build()`、sidecar staging（`binaries/everlasting-daemon-<triple>`，仅服务 GUI bundle）、读 tauri.conf.json 注入 `EVERLASTING_APP_IDENTIFIER`（= `"dev.everlasting.app"`，tauri.conf.json:5）供 `resolve_data_dir()`（`dirs::data_dir().join(identifier)`）。
- lib.rs `run()`：plugins + generate_handler!（79 命令注册）+ generate_context! + RunEvent::Exit 生命周期（Thin 杀 sidecar / Full 杀后台 shell）。
- CI（`.github/workflows/ci.yml:37-41`）：安装 libwebkit2gtk-4.1-dev 等系统依赖；`ci.yml:67-70` 手工 stage sidecar 供 tauri_build 校验。

### 与既有工作的关系

- daemon.sh 走 `target/{daemon,release}/everlasting-daemon`（daemon.sh:45），与 sidecar staging 无关，零改动。
- BACKLOG 无 de-Tauri 条目，本任务为该决策的首次记录。
- N19 缺口④（daemon graceful shutdown 链漏 kill_all）与 GUI Full 模式的 RunEvent::Exit 相关——本任务删除 Full 模式后该叙述需在 N19 落地时对齐（不阻塞本任务）。

## Requirements

### R1 后端：删 GUI 目标与 Tauri 依赖链

- 删 GUI bin（`src/main.rs` → 默认 `everlasting` bin 消失）、`default-run` 字段、`lib.rs::run()`（含 generate_handler! / generate_context! / mobile_entry_point）、`src/sidecar.rs`。
- 删依赖：`tauri`、`tauri-build`、`tauri-plugin-shell`、`tauri-plugin-os`；删 `tauri.conf.json`、`capabilities/`、`icons/`。
- build.rs：删 `tauri_build::build()` 与 sidecar staging；`EVERLASTING_APP_IDENTIFIER` 改为 build.rs 内常量（值不变，见 R4）。
- crate 名 `everlasting` / lib 名 `everlasting_lib` / 模块结构**不变**（避免大面积路径改动）。

### R2 后端：153 个 `#[tauri::command]` 壳删除

- 删全部 Tauri wrapper（`State<'_, ...>` 参数薄壳），`*_inner` 函数族成为唯一 API；daemon 路由引用零改动（同 crate 内可见性不受影响）。
- 删各文件 `use tauri::{State, AppHandle, ...}` 残留 import。

### R3 后端：AppHandle 链与测试构造清除

- 删 `AppHandleSink`（state.rs:957）及 trait 注释中的 Tauri 叙述；`ChatEventSink` trait 保留（`HttpSseSink` / `MockEmitter` 仍在用）。
- 删 `AppState::load(AppHandle)`；`load_inner` 的 `app: Option<AppHandle>` 参数与 `projects:refreshed` emit 分支删除；`load_from_dir` 成为主构造（它已是 60+ 处测试的统一入口，评审核实）。state.rs 双入口不变式叙述（`load(AppHandle).app_data_dir == load_from_dir(p)`，state.rs:1035 一带）随之作废删除。
- 第三条 AppHandle 链并入：删 `AppHandleSubagentSink`（agent/subagent/event_sink.rs）与 `sink.rs` 的 `Option<tauri::AppHandle>` 字段（daemon 路径恒 None）。
- `tauri::async_runtime` 清除分三类（非机械替换，见 design D9）：
  - async 上下文内 `spawn` / `spawn_blocking` → `tokio::spawn` / `tokio::task::spawn_blocking` 直换；
  - `CancellationGuard::drop`（state.rs:729）：按 design D9 专门设计（try_lock 同步快路径 + fallback + 兜底日志），不得机械替换——tokio 语义下静默丢弃清理 = session 永久 busy；
  - 测试内 `block_on` → `tokio::runtime::Runtime` / 现有测试基建等价物。
- `daemon/tunnel/tests.rs:291-293` 的 `tauri::http::Response` 换 `tokio_tungstenite::tungstenite::http`（tungstenite re-export，天然同 instance）。

### R4 兼容性硬约束：data dir 不变

- `EVERLASTING_APP_IDENTIFIER` 单一事实源从 tauri.conf.json 迁到 build.rs 常量，**值保持 `"dev.everlasting.app"`**（`~/.local/share/dev.everlasting.app/everlasting.db` 不搬家，现有数据零迁移）。
- **防漂测试形态（评审修订）**：现有守卫 `resolve_data_dir_ends_with_app_identifier`（bin target 内，everlasting-daemon.rs:417 附近）有两个缺陷——断言自指（`file_name()` 对比 `env!`，build.rs 值漂则两边一起漂，恒不红）；且 CI 只跑 `--lib` / `--test e2e`，bin 内测试从不执行。修订：**字面量断言（`assert_eq!(env!(...), "dev.everlasting.app")`）落在 lib target**，CI 补 `--bins` 编译/测试门（对齐 commands/evl_cli.rs 防漂先例的字面量形态）。

### R5 前端：web 形态收敛

- 删 `transport/tauri.ts`、`transport-parity.test.ts`、`main.ts` 的 tauriTransport import；`resolveTransport` 简化为恒 `httpTransport`。**保留 transport facade（index.ts + types.ts）**——它是 24+ 个组件测试的 mock 边界。
- `AppHeader.vue` 固定 BrowserHeader，删 `TitleBar.vue`；`showSearchButton` 固定 true；删 `isTauriWebview()` helper 及全部调用点。
- `CloseGuardDialog.vue` 删 Tauri 分支，**不做 beforeunload**（评审定案：web 关闭语义是 detach 特性——daemon 独立存活、任务照跑，守卫的价值前提「关窗杀 sidecar」在 web 模式不存在；期望收益≈0 而每次关标签骚扰>0）；其注释引用的 REMOTE-DEPLOY.md「detach 边界」锚点已死，一并重写。
- `package.json` 删 `@tauri-apps/*` 依赖与 `tauri` script；删 `app/dev.sh`（评审发现：`exec pnpm tauri dev` 死入口）；清理 `dev:all` 里 GUI 相关脚本（若有）。

### R6 CI：系统依赖与 staging 步骤删除

- 删 libwebkit2gtk-4.1-dev 等安装步骤与 sidecar staging 校验步骤；CI 后端 job 在无 GTK/WebKit 系统库的容器里跑通即为目标态验证。

### R7 文档收尾

- AGENTS.md（`PKG_CONFIG_PATH` 段、Thin/双 bin 叙述）、docs/HACKING-wsl.md 坑 1、docs/ARCHITECTURE.md / DESIGN.md / LIFECYCLE.md / CONTEXT.md 的 GUI/双模式叙述改为 daemon + web 单形态；docs/BACKLOG.md 记录本决策（桌面分发渠道移除，重启方式 = 将来纯壳 crate，前端已载体无关）。

## Acceptance Criteria

1. **AC1（零系统库）**：干净容器（无 webkit2gtk/gdk-pixbuf、无 `PKG_CONFIG_PATH`）中 `cargo build -p everlasting --bin everlasting-daemon` 成功；`cargo test -p everlasting --lib` 同环境全绿。
2. **AC2（依赖图）**：`cargo tree -p everlasting -e normal --prefix none | sort -u` 不含 tauri/wry/tao/gtk/webkit2gtk 系 crate；unique crate 总数相对 584 显著下降（PR 描述记录前后实数）。
3. **AC3（残留清零，口径明示）**：`grep -rni "tauri" app/src-tauri/src app/src app/dev.sh app/package.json app/vite.config.ts app/index.html` 零命中——**代码与注释均在清零范围**（「本项目不用 mock_app」系注释随改造消解）；豁免面仅 `.trellis/` 任务存档与 git 历史。
4. **AC4（data dir 兼容，PR1 合入门禁）**：防漂字面量断言（lib target）== `"dev.everlasting.app"` 通过；**新旧二进制 `resolve_data_dir()` 对拍**——旧二进制取 PR1 合入前的 main 构建产物，新旧各起一次 daemon，确认打开同一 `~/.local/share/dev.everlasting.app/everlasting.db`（PR1 描述记录两行输出）。本项不得顺延为 PR3 记录项。
5. **AC5（web 全功能）**：`daemon.sh start` + serve dist 后浏览器访问 `:7456`，`scripts/ui-review.sh` 7 界面截图无回归（标题栏区域为 BrowserHeader 形态）。
6. **AC6（daemon 链路零回归）**：`pnpm test`（app/）全绿；`node scripts/group-chat-mcp-http-smoke.mjs`、`scripts/turn-smoke.sh` 冒烟通过。
7. **AC7（观察项，非门禁）**：PR 记录 daemon 二进制体积（release 现 19M）与 RSS（现 57.9MB）前后值，允许无变化。

## Out of Scope

- **PWA / 桌面壳替代**：将来若需桌面包，重启一个纯壳 crate（前端载体无关，BACKLOG §跨设备已认 HTTP 为主路径）。
- **daemon 内存优化**：RSS 大头另有其人（tokio/sqlx/tiktoken/缓冲），另开任务度量。
- **Full 模式逃生等价物**：http 成为唯一通道；daemon 崩 = web 不可用，接受（与今天浏览器直连模式风险相同）。
- **crate 改名 / workspace 结构重排**：default-members **定案不动**（评审结论：翻转它改变所有开发者与 CI 的裸 `cargo build`/`test` 语义，与本任务「纯减法、不引入新运行时面」不变式冲突；将来如需改动走一行 follow-up）。
- daemon.sh 输出面改动（如启动成功打 `daemon ready: http://127.0.0.1:7456`）：不搭车（输出契约面另行收口）。
- `disk/webkit_cache.rs` 模块的最终删除（本任务保留并重接 daemon bin，见 design D10；后续删除 = 模块 + commands/disk.rs 的 webkit_cache key + 装配断言一起收，PR3 落 BACKLOG 行）。
- N19 缺口④的后台 shell 回收链：独立任务，本任务仅删除其 GUI Full 叙述。
