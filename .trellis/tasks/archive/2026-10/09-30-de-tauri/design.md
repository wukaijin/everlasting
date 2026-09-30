# Design: de-Tauri —— daemon + web 单形态

对应 PRD：`prd.md`（同目录）。本文档记录技术决策与迁移方案，评审团重点议题见文末 §8。

## 1. 架构边界：删什么、留什么

```
删除（GUI 壳层）                      保留（daemon + web 主干）
─────────────────────────            ─────────────────────────
src/main.rs（GUI bin 入口）            everlasting-daemon bin（axum + SSE + MCP）
lib.rs::run()（Builder/handler/Exit）  153 个 *_inner 函数族（唯一 API 面）
src/sidecar.rs（sidecar 生命周期）     daemon/routes/*（HTTP 路由，引用不变）
AppHandleSink（GUI 事件通道）          HttpSseSink + SseRegistry（SSE 通道）
AppHandleSubagentSink + sink.rs       ChatEventSink trait（HttpSseSink/MockEmitter 在用）
  的 Option<AppHandle> 字段
AppState::load(AppHandle)             load_from_dir / load_inner（去 Option 参数）
153 个 #[tauri::command] wrapper      daemon/tunnel/tests.rs（仅换 tauri::http 类型，D11）
tauri / tauri-build / 两个 plugin     axum / tower / tokio 全家（不动）
tauri.conf.json / capabilities/       build.rs 的 identifier 注入（常量化）
icons/ / bundle 配置                   crates/everlasting-acp（ACP shim，零 GUI 耦合）
transport/tauri.ts + parity 测试       disk/webkit_cache.rs（保留并重接 daemon bin，D10）
TitleBar.vue                           transport facade（index.ts + types.ts + http.ts）
app/dev.sh（pnpm tauri dev 死入口）    BrowserHeader / AppHeader（固定 web 形态）
CI webkit2gtk 安装 + sidecar staging   daemon.sh / evl / 群聊 / 定时 / remote（零改动）
                                        CI bin 编译门（保留改形式，D12）
```

不变式：**crate 名 `everlasting`、lib 名 `everlasting_lib`、模块树、DB schema、daemon HTTP 契约（docs/DAEMON-API.md）全部不动**。本任务是纯减法，不引入任何新运行时面。例外仅两处类型级改动：`daemon/tunnel/tests.rs` 的 `tauri::http` 换型（D11）与 webkit_cache 装配点迁移（D10）。

## 2. 决策记录

### D1 一步到位删除，不做 feature 门控

`gui` feature 门控 153 个 command 壳 + State/AppHandle 签名 + lib.rs run() 的 cfg 分支，成本高于直接删除且留下永久的双路径测试负担。删除后 git 历史即回滚手段（见 §7）。

### D2 保留 transport facade，删 tauri.ts 实现

`transport/index.ts` 的 `transport` 单例是 24+ 组件测试的 mock 边界（`vi.mock("../../transport")`）。虽然只剩 httpTransport 一个实现，facade 保留为显式测试接缝（对齐「mock 边界优于 mock 具体实现」惯例）。`resolveTransport()` 简化为直接导出 `httpTransport`；`?transport=tauri` / `?transport=http` query 分支删除。

### D3 identifier 常量化与 data dir 兼容（最高风险点）

现状：build.rs 读 `tauri.conf.json` 的 `identifier` 注入 `EVERLASTING_APP_IDENTIFIER`，`resolve_data_dir()` = `dirs::data_dir().join(env!("EVERLASTING_APP_IDENTIFIER"))`。

方案：build.rs 删除 tauri.conf.json 解析，改为：

```rust
// build.rs —— 值必须与历史 tauri.conf.json 一致，data dir 兼容性锚点
println!("cargo:rustc-env=EVERLASTING_APP_IDENTIFIER=dev.everlasting.app");
```

- **值漂移 = 全体用户数据目录搬家（DB/附件/转录全丢）**。防漂测试形态（评审修订）：现有守卫 `resolve_data_dir_ends_with_app_identifier` 落在 **bin target**（everlasting-daemon.rs:417 附近）且断言自指——`dir.file_name()` 对比 `env!("EVERLASTING_APP_IDENTIFIER")`，build.rs 值漂则两边一起漂，恒不红；且 CI 只跑 `--lib` / `--test e2e`，bin 内测试从不执行。修订为**字面量断言落 lib target**（`assert_eq!(env!("EVERLASTING_APP_IDENTIFIER"), "dev.everlasting.app")`，对齐 `commands/evl_cli.rs` 防漂先例）+ CI 补 `--bins` 门（D12）。
- `resolve_data_dir()` 调用面（daemon bin / disk 治理 / evl_cli 安装落点）零改动——env! 消费点不变。
- **AC4 新旧二进制对拍是 PR1 合入门禁**（防路径搬家的真网，防漂断言只是字面量网）。

### D4 事件通道：删 AppHandleSink，trait 与 SSE 不动

`ChatEventSink` trait（state.rs:825）的实现矩阵：`AppHandleSink`（删）、`HttpSseSink`（daemon 生产路径）、`MockEmitter`（测试）。删 AppHandleSink 后 trait 变两实现，注释中「Only AppHandleSink …」系列条件叙述同步清理。`projects:refreshed` 的 AppHandle emit 分支（state.rs:464-494）删除——daemon 路径今天就是 skip（web 前端经 SSE/轮询获知刷新，现状不变）。

**第三条链（评审补充）**：subagent 侧另有 `AppHandleSubagentSink`（agent/subagent/event_sink.rs:29）与 `sink.rs:165` 的 `Option<tauri::AppHandle>` 字段（daemon 路径恒 None，sink.rs:150 注释自认仅历史原因保留）——与 AppHandleSink 同批删除。

### D5 AppHeader 固定 BrowserHeader

`AppHeader.vue:68` 的 `shell = isTauriWebview() ? TitleBar : BrowserHeader` 改为固定 BrowserHeader；删 `TitleBar.vue` 与 `isTauriWebview()`（transport/env.ts:31）。连带：`showSearchButton = !isTauriWebview()` 固定 true；`App.vue:27` 的挂载注释更新。共享 slot（ProjectTabs / HiddenProjectsMenu / PendingBadge）已在 AppHeader，无迁移。

### D6 CloseGuardDialog：纯删除，不做 beforeunload（评审定案）

现状纯浏览器模式关标签**无任何守卫**（CloseGuardDialog.vue:6 注释自认：只断 SSE）。评审结论：web 关闭语义是 **detach 特性**——daemon 独立存活、任务照跑，这是远程场景的正资产而非缺陷；CloseGuardDialog 的价值前提（「关窗会杀 sidecar、需拦一道」）在 web 模式根本不存在。beforeunload 期望收益≈0，而每次关标签的通用确认骚扰>0。处置：删 onCloseRequested 分支与 `isTauriWebview` 依赖；注释重写（原注释引用的 REMOTE-DEPLOY.md「detach 边界」锚点已死，改指 docs/REMOTE-ACCESS-E2E.md 或直接内联说明）。

### D7 前端依赖清理

`package.json` 删 `@tauri-apps/api`、`@tauri-apps/plugin-os`、`@tauri-apps/cli`、`tauri` script；`dev:all` 保留（它本来就是 vite + daemon 并行，无 GUI 参与）；删 `app/dev.sh`（`exec pnpm tauri dev` 死入口，评审发现）。`pnpm-lock.yaml` 同步收敛。

### D8 workspace default-members：定案不动（评审收口）

翻转它会改变所有开发者与 CI 的裸 `cargo build` / `test` 语义，与本任务「纯减法、不引入新运行时面」不变式冲突。everlasting 虽去系统库化，仍带 vendored-libgit2 C 编译 + 全量 lib。将来如需改动，走独立的一行 follow-up。

### D9 CancellationGuard::drop 的 runtime 无关化（新增，评审 Q6c）

现状（state.rs:729）：`Drop` 内 `tauri::async_runtime::spawn` 清理两把 tokio Mutex map（cancellations + session_active_request）。tauri runtime 语义 = 「无 runtime 则惰性起全局 runtime」，所以 Drop 里永远能 spawn。直换 tokio 的两个坑：`tokio::spawn` 在 runtime 上下文外 **panic**；`Handle::try_current` 失败静默丢弃 = **session 永久 busy**（map 里的 request 永不清理）。

**拍板方案 1：Drop 内同步 try_lock 快路径 + fallback spawn + error! 兜底**：

```rust
fn drop(&mut self) {
    // 快路径：两把 map 锁的持有者都是短临界区（remove/insert 级），
    // Drop 时点通常无人持锁，try_lock 同步清完即返回。
    // 慢路径：try_lock 失败 → Handle::try_current() 有 runtime 则 spawn
    // 异步重试；两者皆败 → error! 兜底日志（可观测，优于静默 busy）。
}
```

锁竞争面评估：两把锁的临界区均为纯 map 操作（无 await 跨点持锁的长事务），try_lock 失败窗口极小；fallback 链保住「清理最终发生」语义。`agent/tests_cancellation.rs`（:124 一带有 Drop 时序相关断言）必须全绿后才算完成。**本条不按机械替换处理**。

### D10 disk/webkit_cache.rs：保留并重接 daemon bin（新增，评审 Q6a）

`spawn_startup_clean`（webkit_cache.rs:106）内部用 `tauri::async_runtime::spawn`，装配点在 lib.rs setup 公共区（lib.rs:134），且有**装配级源码静态断言测试**（grep lib.rs 源文本——PR1 删 `run()` 后必炸）。裁决：

- 本任务**保留模块**：`spawn_startup_clean` 内 spawn 换 `tokio::spawn`，装配一行迁到 daemon bin（`resolve_data_dir()` 之后），静态断言测试改 grep daemon bin 源文本（everlasting-daemon.rs）。WebKitCache 是 GUI webview 的产物——GUI 删除后理论上不再增长，但历史存量仍需清理，且模块删除牵连 `commands/disk.rs:99` 的 webkit_cache key 与七条目断言，不宜顺车。
- PR3 落 BACKLOG 行：后续任务一起收（模块 + disk key + 装配断言 + settings 存储页条目）。

### D11 daemon/tunnel/tests.rs：tauri::http 换型（新增，评审 Q6b）

tests.rs:291-293 真实引用 `tauri::http::Response`（tungstenite 握手回调构造 401）。tauri::http 是 tauri 内嵌的 http crate re-export，恰好与 tungstenite 同 instance。换 `tokio_tungstenite::tungstenite::http::Response`——tungstenite 自己的 re-export，天然同 instance，无需新增直接依赖。机械改动，但必须进 implement 步骤清单，否则 AC3 grep 才是第一发现点（太晚）。

### D12 CI bin 编译门：保留职责、更换形式（新增，评审 Q6d）

ci.yml:80-81 的 clippy 步骤是**全 CI 唯一编译 `bin/everlasting-daemon.rs` 的地方**（`--lib` / `--test e2e` 均不构建 bin target）。PR3 删 sidecar staging 时该步骤的存在理由（staging 校验）消失但职责（bin 编译门）不消失：保留步骤、改注释、形式换 `cargo clippy --bin everlasting-daemon`（PR1 起顺带补 `--bins` 测试门支撑 R4 防漂）。

## 3. 兼容性与迁移

### 用户面

评审修正：迁移成本被**高估**而非低估——唯一真实用户（作者本人）已 100% 浏览器直连，下表 GUI 行全部是死路径，删除它们没有用户面损失；列出的唯一目的是把「放弃的东西」记录在案。

| 现状（死路径） | 之后 |
|---|---|
| GUI 双击启动（Thin：spawn sidecar + webview） | `daemon.sh start` + 浏览器开 `http://127.0.0.1:7456`（现行方式，不变） |
| 关 GUI 窗口自动杀 daemon | `daemon.sh stop`（detach 语义本就是 web 模式的正资产） |
| 桌面安装包（bundle） | 移除。重启路径 = 将来纯壳 crate（前端载体无关） |
| `?transport=tauri` 逃生 | 移除（http 唯一通道） |

数据零迁移（D3）；daemon HTTP/SSE/MCP 契约零变化；evl CLI、远程访问、ACP shim、定时任务、群聊全部不受影响。

### CI 面

backend job 删 `Install Tauri system deps`（ci.yml:37-41）与 sidecar staging 校验（ci.yml:67-70）；frontend job 不变。CI 容器本身即 AC1 的「干净环境」验证。

## 4. 实施顺序与切分（详见 implement.md）

**后端先行、前端收尾、文档搭车**：

- **PR1（后端主体）**：R1+R2+R3+R4——删 GUI bin/run()/sidecar/依赖/153 壳/AppHandle 链/测试构造 + identifier 常量化防漂。此 PR 后仓库已无 GUI 可构建，`app/` 前端在浏览器下照常工作（它不依赖 GUI 存在）。
- **PR2（前端收尾）**：R5——transport 收敛、TitleBar 删、CloseGuard 改造、依赖清理。
- **PR3（CI + 文档）**：R6+R7。

PR1 独立可合（web 形态当刻即完整）；PR2/PR3 不阻塞 PR1。若评审倾向单 PR 也可（diff 大但机械），见 §8 Q1。

## 5. 测试策略

- **后端**：`cargo test -p everlasting --lib`（~2484 用例）——7 处 tauri::test 构造改造为 `load_from_dir(tempdir)`；防漂单测新增；其余零改动（inner 函数族本就是被测对象）。
- **前端**：`pnpm test`——transport mock 边界不动，预期零改动；`transport-parity.test.ts` 随 tauri.ts 删除。
- **live 冒烟**：`daemon.sh start` + 浏览器手测清单（发消息/工具执行/权限弹卡/SSE 流式/Settings 各页）+ `scripts/turn-smoke.sh` + `node scripts/group-chat-mcp-http-smoke.mjs`。
- **视觉**：`scripts/ui-review.sh`（7 界面，标题栏区域形态变化重点看）。

## 6. 风险与缓解

| 风险 | 等级 | 缓解 |
|---|---|---|
| identifier 值漂移 → data dir 搬家 | 高（数据丢失级） | D3 字面量防漂断言（lib target）+ AC4 新旧二进制对拍 = PR1 合入门禁 |
| `CancellationGuard::drop` 换 tokio 后 session 永久 busy | 高（评审升级） | D9 专门设计（try_lock 快路径 + fallback + error! 兜底）；tests_cancellation 全绿门 |
| webkit_cache 静态断言在删 run() 后炸 | 中（评审发现） | D10：保留模块重接 daemon bin，断言改 grep daemon bin 源文本 |
| 153 壳删除误删 inner / 漏改 import | 中 | 编译错误驱动 + AC3 grep 清零 + lib 全量测试 |
| 测试构造改造破坏测试语义 | 低（评审降级：mock_app 本就不存在，load_from_dir 已是统一入口） | 双入口不变式叙述作废删除 + 注释重写；question/mode_change 系用例跑绿 |
| 前端隐性 Tauri 依赖漏网（动态 import） | 低 | AC3 grep（含 vite.config.ts / index.html / dev.sh）+ `pnpm build` 产物验证 |
| CI bin 编译门随 staging 一起被误删 | 中（评审发现） | D12：保留步骤改形式，PR3 逐行核对 |

## 7. 回滚

单 PR 维度 `git revert` 即可（纯删除 + 少量常量化，无 DB/契约迁移）。任务维度回滚 = revert 三个 PR；数据面无任何不可逆操作。

## 8. 评审结论记录（2026-09-30，session `720948b8`，review 预设，66 轮收官）

六问均已定案，文档已按结论修订（D6/D8/D9/D10/D11/D12 为新增或改判条目）：

- **Q1 ✅ 三 PR、后端先行成立**（前端 tauri.ts 依赖 npm 包而非 Rust GUI 进程，缝真实存在）；附加硬约束——AC4 对拍升格为 PR1 合入门禁。
- **Q2 ✅ 常量化可行，防漂测试形态修订**（现有 bin 守卫自指 + CI 从不运行 → 字面量断言落 lib target + CI `--bins` 门，见 D3/D12）。
- **Q3 ✅ 原步骤 7 是幽灵任务**（全仓无 `tauri::test::mock_app`，六处全是「本项目不用」注释；`load_from_dir` 已是 60+ 处测试统一入口）；真实工作 = 去 `Option<AppHandle>` 参数 + 双入口不变式叙述作废 + 注释重写。PRD R3 / implement 步骤 7 已重写。
- **Q4 ✅ 不做 beforeunload，D6 定案纯删除**（web 关闭 = detach 特性，守卫价值前提不存在）；§3 迁移表修正为死路径记录。
- **Q5 ✅ default-members 定案不动**（D8 收口，将来一行 follow-up）。
- **Q6 ✅ 九条遗漏面全部入档**：webkit_cache（D10）、tunnel::http 换型（D11）、Drop 内 spawn（D9）、CI bin 编译门（D12）、spawn_blocking 入清单、dev.sh 死入口（D7）、AppHandleSubagentSink 第三链（D4）、AC3 口径与范围（PRD）、CloseGuard 死锚点注释（D6）。

评审遗留的四个开放项，修订时拍板如下：Drop 形态取方案 1（D9）；AC4 对拍为手测形态、旧二进制取 PR1 前 main（PRD AC4）；daemon ready 输出行不搭车（PRD Out of Scope）；webkit_cache 后续删除由 PR3 落 BACKLOG 行（D10）。

转录：`~/.local/share/dev.everlasting.app/discussions/2026-09-30-评审任务 09-30-de-tauri 的设计文档（de-Tauri：移除 GU-720948b8.md`。
