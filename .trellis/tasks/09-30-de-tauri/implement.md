# Implement: de-Tauri

前置：`prd.md`（需求/AC）、`design.md`（决策 D1-D8）。PR 切分 = design §4（PR1 后端 / PR2 前端 / PR3 CI+文档）；若评审改单 PR 则按段顺序一次走完。

## PR1 后端主体（R1-R4）

执行原则：**编译错误驱动**。先删入口与依赖（步骤 1-3），再让编译器指出全部壳与 AppHandle 引用点；不手维护删除清单。

- [ ] 1. **删 GUI 入口**：`src/main.rs`、`src/sidecar.rs`、`lib.rs::run()`（整个 tauri::Builder 块）；lib.rs 只剩 mod 声明 + 文档头更新；`Cargo.toml` 删 `default-run`。
- [ ] 2. **删 Tauri 依赖与配置**：Cargo.toml 删 `tauri` / `tauri-plugin-shell` / `tauri-plugin-os`；build-dependencies 删 `tauri-build`；删 `tauri.conf.json` / `capabilities/` / `icons/` / `src-tauri/binaries/`（staged sidecar 产物目录）。
- [ ] 3. **build.rs 常量化**：删 `tauri_build::build()` 与 sidecar staging；identifier 注入改 design D3 形态（值 `dev.everlasting.app`）。
- [ ] 4. **153 壳删除**（编译错误驱动）：删全部 `#[tauri::command]` wrapper 与 `use tauri::State`；inner 函数族保持现签名。注意 agent/chat.rs 的 `chat` / `resume_group_chat` 也是 command 壳（chat.rs:67/242）。
- [ ] 5. **AppHandle 三链清除**（design D4）：删 `AppHandleSink`（state.rs:957+）、`AppHandleSubagentSink`（agent/subagent/event_sink.rs:29）与 `sink.rs:165` 的 `Option<AppHandle>` 字段；删 `AppState::load(AppHandle)`、`load_inner` 的 `app` 参数与 `projects:refreshed` emit 分支；`ChatEventSink` trait 注释清理。
- [ ] 6. **`tauri::async_runtime` 清除（三类，非机械，design D9）**：
  - async 上下文 `spawn` / `spawn_blocking`（如 subagent_runs.rs:258）→ `tokio::spawn` / `tokio::task::spawn_blocking` 直换；
  - `CancellationGuard::drop`（state.rs:729）按 D9 方案 1 重写（try_lock 同步快路径 + `Handle::try_current` fallback + `error!` 兜底）——**禁止机械替换**（静默丢清理 = session 永久 busy）；完成后 `cargo test --lib "tests_cancellation::"` 全绿；
  - 测试内 `block_on` → tokio 等价物。
- [ ] 7. **state.rs 叙述收口**（评审 Q3：原「7 处 mock 构造改造」是幽灵任务，全仓无 `tauri::test::mock_app`）：删双入口不变式测试与叙述（state.rs:1035 一带）；重写「本项目不用 mock_app」系注释（state.rs:1035、question.rs:209/615、tests_ask_user_question.rs:1029 等 6 处，随语境消解或改写）。
- [ ] 8. **webkit_cache 重接**（design D10）：`spawn_startup_clean` 内 spawn 换 `tokio::spawn`；装配点迁 daemon bin（`resolve_data_dir()` 之后一行）；静态断言测试改 grep `bin/everlasting-daemon.rs` 源文本。
- [ ] 9. **tunnel tests 换型**（design D11）：`daemon/tunnel/tests.rs:291-293` 的 `tauri::http::Response` → `tokio_tungstenite::tungstenite::http::Response`（tungstenite re-export，同 instance）。
- [ ] 10. **防漂测试**（design D3）：字面量断言 `assert_eq!(env!("EVERLASTING_APP_IDENTIFIER"), "dev.everlasting.app")` 落 **lib target**（现有 bin 内自指断言删除——它恒不红且 CI 从不跑）。
- [ ] 11. **验证（合入门禁）**：
  - `env -u PKG_CONFIG_PATH cargo test -p everlasting --lib`（AC1：无系统库环境）全绿
  - **AC4 新旧二进制对拍**（PR1 门禁，不得顺延）：旧二进制取合入前 main 构建，新旧各起 daemon，确认打开同一 `~/.local/share/dev.everlasting.app/everlasting.db`，PR1 描述记录两行输出
  - `cargo tree -p everlasting -e normal --prefix none | sort -u | wc -l` 记录前后值（AC2）
  - `grep -rni "tauri" app/src-tauri/src` 清零（AC3）
  - daemon.sh start + 浏览器冒烟（发消息/工具/权限卡/SSE）

## PR2 前端收尾（R5）

- [ ] 1. `transport/tauri.ts`、`transport/transport-parity.test.ts` 删除；`index.ts` 恒 `httpTransport`（design D2）；`main.ts` 删 import。
- [ ] 2. `AppHeader.vue` 固定 BrowserHeader + `showSearchButton=true`；删 `TitleBar.vue`、`transport/env.ts::isTauriWebview` 及调用点（design D5）。
- [ ] 3. `CloseGuardDialog.vue`：删 Tauri 分支，**不做 beforeunload**（design D6 定案）；死锚点注释重写。
- [ ] 4. `package.json` 清 `@tauri-apps/*` + `tauri` script；删 `app/dev.sh`（design D7）；`pnpm install` 收敛 lockfile。
- [ ] 5. **验证**：`cd app && pnpm test` 全绿；`pnpm build` 成功；`grep -rni "@tauri-apps\|tauri" app/src app/dev.sh app/package.json app/vite.config.ts app/index.html` 清零（AC3 口径：代码+注释）。
- [ ] 6. **视觉**：`scripts/ui-review.sh`（AC5，重点标题栏区域）。

## PR3 CI + 文档（R6-R7）

- [ ] 1. `.github/workflows/ci.yml`：删 `Install Tauri system deps`（37-41）；sidecar staging 相关注释块（67-70 附近）删除；**保留 80-81 的 bin 编译门并改形式**为 `cargo clippy --bin everlasting-daemon`（design D12：它是全 CI 唯一构建 bin 的步骤，职责不随 staging 理由消失）；补 `--bins` 测试门支撑 R4 防漂断言。确认 job 容器无 GTK/WebKit（AC1 即 CI 本身）。
- [ ] 2. AGENTS.md：Running Tests 段 `PKG_CONFIG_PATH` 叙述、双 bin / sidecar / Thin-Full 叙述、`cargo test -p everlasting` 的「需 PKG_CONFIG_PATH」备注全部更新。
- [ ] 3. docs/HACKING-wsl.md 坑 1 标记解决/删除；docs/ARCHITECTURE.md / DESIGN.md / LIFECYCLE.md / CONTEXT.md 双模式叙述改单形态。
- [ ] 4. docs/BACKLOG.md 两行：①记录 de-Tauri 决策（桌面分发移除 + 重启路径 = 纯壳 crate）并核对 N19 缺口④叙述衔接；②webkit_cache 后续删除任务行（模块 + commands/disk.rs key + 装配断言，design D10）。
- [ ] 5. 全链冒烟：`scripts/turn-smoke.sh`、`node scripts/group-chat-mcp-http-smoke.mjs`（AC6）；记录 binary 体积与 RSS 前后值（AC7，观察项）。

## 风险文件与回滚点

- 高危单点：`build.rs`（identifier——防漂断言 + AC4 对拍双网）、`state.rs`（load 链 + sink + **CancellationGuard::drop**，D9 改动必须过 tests_cancellation）、`daemon/bin/everlasting-daemon.rs`（webkit_cache 重接装配点）；改动后必须跑防漂测试 + daemon 启动冒烟。
- 大 diff 机械区：commands/*.rs 壳删除（git diff 逐文件抽查 wrapper 删净、inner 未动）。
- 回滚：每 PR 独立 `git revert`；无数据面不可逆操作。

## 完成定义

AC1-AC6 全绿（AC7 记录不设门禁）；**AC4 对拍在 PR1 完成并记录**；评审团六问结论闭环（design §8）；BACKLOG 两行落档。
