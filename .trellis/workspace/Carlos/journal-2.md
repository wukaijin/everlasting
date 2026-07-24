# Journal - Carlos (Part 2)

> Continuation from `journal-1.md` (archived at ~2000 lines)
> Started: 2026-07-23

---



## Session 27: daemon 浏览器模式手动测试 — 修复 7 个路径/渲染类 bug + scripts/daemon.sh

**Date**: 2026-07-23
**Task**: daemon 浏览器模式手动测试 — 修复 7 个路径/渲染类 bug + scripts/daemon.sh
**Branch**: `main`

### Summary

MANUAL-TEST-P2 验收暴露并修复 default-run / resolve_dist_dir / resolve_data_dir / TitleBar 崩溃 / 滚动条 / daemon.sh。代码全提交,task 留 in_progress 等 E5 dogfooding。

### Main Changes

## 概要

daemon 浏览器模式手动测试 Session。按 `docs/MANUAL-TEST-P2.md` 验收时,第一步就挂(
`pnpm tauri ui` binary 歧义),随后逐步暴露并修复了一串 Phase 2 daemon 拆分遗留的路径
/ 渲染类 bug。代码全部修完并提交;E4 手动 smoke + E5 dogfooding 计时留运行期。

## 修复的 bug(7 个,均已 commit)

1. **`default-run` 缺失**(`ba41c1d`):P2.2 引入 daemon `[[bin]]` 后 crate 有两个 binary,
   `pnpm tauri dev`(底层 `cargo run` 无 `--bin`)报 "could not determine which binary to run"。
   Cargo.toml 加 `default-run = "everlasting"`。

2. **`resolve_dist_dir()` 路径推算错**(`6581257`):原 `exe.parent().join("..").join("dist")`
   固定一级 `..`,但 daemon 二进制位置随构建模式变(sidecar `binaries/` 一级、`target/release`
   三级、`target/debug` 两级)。`target/release` 裸跑时定位到不存在的 `src-tauri/dist` → GET /
   返回 404。改为从 `current_exe()` 向上搜索 `src-tauri/` 目录,取其兄弟 `dist/`。

3. **`resolve_data_dir()` 路径不一致**(`16548fd`):daemon 用 `dirs::data_dir().join("everlasting")`
   拼成 `everlasting/`,但 Tauri `app_data_dir()` 是 `join(config.identifier)` = `dev.everlasting.app/`。
   daemon 裸跑打开空 db,丢失 GUI 的 151 条历史消息。build.rs 读 `tauri.conf.json` 的 identifier,
   注入 `EVERLASTING_APP_IDENTIFIER` 编译期 env,daemon bin `env!()` 读取对齐。加 2 个单测。

4. **TitleBar `getCurrentWindow()` 崩溃**(`df991a5`):浏览器环境 `<script setup>` 顶层同步调用
   `@tauri-apps/api/window::getCurrentWindow()` 抛异常(未包 try/catch)→ 整个 AppHeader 子树
   (含 ProjectTabs 项目切换 bar)不渲染。新增 `transport/env.ts::isTauriWebview()` + `BrowserHeader.vue`
   (浏览器版顶部 bar,无 Tauri API),AppHeader 按 `isTauriWebview()` 分流 TitleBar/BrowserHeader。
   这同时让 P2.4 D6 的 manual-path fallback(浏览器手动输入项目路径)生效——之前因 ProjectTabs
   没渲染而看不到。

5. **全局滚动条样式**(`128e01f`):可滚动区域回退浏览器默认浅灰粗滚动条,跟深色主题不协调。
   `style.css` 加全局 `::-webkit-scrollbar` + Firefox `scrollbar-width/color`,沿用 ProjectTabs
   /ThinkingBlock 已确立的 token 约定。

6. **`scripts/daemon.sh`**(`a2bd611`):daemon 浏览器模式管理脚本(start/bg/stop/restart/status
   /logs/rebuild)。PID 文件管理进程避免 `pkill -f` 自匹配误杀 shell;自动注入 PKG_CONFIG_PATH;
   Q1 防多实例。

7. **bookkeeping**(`2a1a07d` `.zcode/` ignore,`b42edb7` implement.md 已知后续项记录)。

## 未修(记后续项)

- **daemon graceful shutdown 不及时退出**:有浏览器 SSE 长连接时,收到 SIGTERM 后
  `axum with_graceful_shutdown` 等连接完成而挂起,靠 `scripts/daemon.sh` 的 SIGKILL
  兜底(SIGTERM → 8s → SIGKILL)清理。不影响使用。已记 `implement.md` 末尾"已知后续项"段。

## 验证

- `cargo test --bin everlasting-daemon`:2 passed(identifier 拼接 + 非旧 hardcoded)
- `pnpm build`:0 err
- `pnpm test`:942 passed(60 files)
- daemon 裸跑 db_path 自动指向 `dev.everlasting.app/`,API 可见历史 projects ✓

## 状态

代码收尾完成,task 保持 `in_progress`(E5 dogfooding ≥ 2 周计时未起)。E4 手动 smoke
留 GUI-capable 机器手动进行中。


### Git Commits

| Hash | Message |
|------|---------|
| `ba41c1d` | (see git log) |
| `6581257` | (see git log) |
| `2a1a07d` | (see git log) |
| `16548fd` | (see git log) |
| `df991a5` | (see git log) |
| `128e01f` | (see git log) |
| `a2bd611` | (see git log) |
| `b42edb7` | (see git log) |

### Testing

- [OK] (Add test results)

### Status

[OK] **Completed**

### Next Steps

- None - task complete


## Session 28: docs-sync-daemon-split 收尾: subtask C (HACKING/DEBUG_DB) + D (IMPLEMENTATION ADR) — 父任务 4/4 全 archived

**Date**: 2026-07-24
**Task**: docs-sync-daemon-split 收尾: subtask C (HACKING/DEBUG_DB) + D (IMPLEMENTATION ADR) — 父任务 4/4 全 archived
**Branch**: `main`

### Summary

续父任务 docs-sync-daemon-split。子任务 C (HACKING-llm/wsl/DEBUG_DB): 默认模型 GLM-4.7→MiniMax-M2.7 + env/DB catalog 优先级章节 + daemon env 传递; DEBUG_DB 行号修正 + daemon 视角 DB 路径 + WAL writer 归属 daemon; HACKING-wsl 补 scripts/daemon.sh 用法 + 多实例警告 + daemon 健康检查。子任务 D (IMPLEMENTATION): §1 自研边界演进注记 (Tauri IPC→+HTTP/SSE; rig/rmcp 弃用, R2 保留历史); §4 新增 2026-07-20 daemon 拆分 ADR 7 决策点 (拆 daemon/axum/sidecar/httpTransport/ServeDir/双暴露/DB 路径), 决策依据取自 daemon-split design.md 非臆造; §4 飞书触发 + L3a daemon 提及补演进注记。所有代码事实改动前核对源码。父任务 4/4 子任务全 archived, 跨文档飞书/Unix-socket 叙事清零, 集成检查通过。

### Main Changes

(Add details)

### Git Commits

| Hash | Message |
|------|---------|
| `8c3b0bd` | (see git log) |

### Testing

- [OK] (Add test results)

### Status

[OK] **Completed**

### Next Steps

- None - task complete
