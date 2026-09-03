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


## Session 29: 归档 remote-access-multi-channel 全树(daemon-split 跳过 E4/E5 dogfooding)

**Date**: 2026-07-24
**Task**: 归档 remote-access-multi-channel 全树(daemon-split 跳过 E4/E5 dogfooding)
**Branch**: `main`

### Summary

用户决定全归档。daemon-split 子任务原 in_progress(等 E4/E5 dogfooding 验证,见 Session 27),现按用户决定跳过 E4/E5 标 completed 归档 —— 代码已全落地(commits 0dbc747→84d4689→e6b7a2f + 手测修复 ba41c1d/6581257/16548fd/df991a5/a2bd611),运行期验证作 nice-to-have 不再计入验收。父任务 remote-access-multi-channel 同步归档。active task 清空(0 tasks)。

### Main Changes

(Add details)

### Git Commits

| Hash | Message |
|------|---------|
| `31373f4` | (see git log) |

### Testing

- [OK] (Add test results)

### Status

[OK] **Completed**

### Next Steps

- None - task complete


## Session 30: daemon graceful shutdown 超时修复（SSE 长连接挂起）

**Date**: 2026-07-24
**Task**: daemon graceful shutdown 超时修复（SSE 长连接挂起）
**Branch**: `main`

### Summary

修复 daemon 收到 SIGTERM 后因 SSE 长连接永不自然完成导致 graceful shutdown 无限挂起（原本靠 daemon.sh SIGKILL 兜底）。signal 触发后先 SseRegistry::shutdown() 主动 drop 所有订阅者 → ReceiverStream 返回 None → SSE body 自然 end() → axum drain 亚秒完成；外层 SHUTDOWN_GRACE_SECS=3s timeout 作 defense-in-depth。1566 lib + 10 e2e passed。新建 backend/daemon-server.md spec（daemon 层 spec 种子，含 wrong/correct 对照 + streaming endpoint pattern）。用户审查时点名暴露一个 PRD 未覆盖的真问题：正在跑的 agent loop 在进程退出时被硬终止（shutdown 路径未遍历 state.cancellations cancel 活跃 request），已如实记入 PRD + spec 作独立 follow-up，修法方向已写明。

### Main Changes

(Add details)

### Git Commits

| Hash | Message |
|------|---------|
| `0a6bd1c` | (see git log) |

### Testing

- [OK] (Add test results)

### Status

[OK] **Completed**

### Next Steps

- None - task complete


## Session 31: daemon shutdown: agent loop drain(闭合硬终止缺口)

**Date**: 2026-07-26
**Task**: daemon shutdown: agent loop drain(闭合硬终止缺口)
**Branch**: `main`

### Summary

实现 daemon-server spec 的 follow-up:serve_daemon graceful shutdown 原只关 SSE,不 cancel 正在跑的 agent loop,进程退出时 runtime 销毁硬斩 spawn task 会丢「tool 已执行、persist_turn 未落库」那一轮。复用 destructive-command 路径的 cancel+drain 基础设施(cancel_inflight_for_session/await_inflight_exit/done_tx),新增 helpers::cancel_and_drain_all_agent_loops 批量版本,把「单 session 的 cancel+drain」搬到 shutdown 路径(粒度改「所有 session」)。shutdown_signal 接 Arc<AppState>,在 sse.shutdown() 后 cancel 所有 token + 并发 await inflight_exits(总 timeout 8s)。run_chat_loop 本体零改动。daemon.sh SIGKILL 窗口 8s→15s(11s 最坏路径留 4s 余量)。5 个新单测 + 1 个真实 TCP+SIGTERM 集成测试;踩了第二个 SIGTERM 测试的信号污染,用 SIGNAL_TEST_MUTEX 串行修复。spec follow-up 段改写为已覆盖。

### Main Changes

(Add details)

### Git Commits

| Hash | Message |
|------|---------|
| `4284315` | (see git log) |

### Testing

- [OK] (Add test results)

### Status

[OK] **Completed**

### Next Steps

- None - task complete


## Session 32: review plugin 可用性修复 C4+C5（breadcrumb + reviewer 派发死锁 + 跨 plugin task 归属）

**Date**: 2026-07-28
**Task**: review plugin 可用性修复 C4+C5（breadcrumb + reviewer 派发死锁 + 跨 plugin task 归属）
**Branch**: `main`

### Summary

从两次 E2E dogfooding session（99866757 + 04c62fab）诊断并修复 review plugin 多层缺陷。C4：breadcrumb state fallback 硬编码 planning 导致 review plugin intake 阶段 LLM 迷失（修 inject.rs 用 workflow_def.initial + 暴露 plugin/state/tools）。C5：dispatch_subagent enum 漏 plugin 层 agent 导致 reviewer 派发死锁（修 definition_with_cache 用 list_with_workflow）；create_task 写死 planning 对不上 review 状态机（按 plugin initial 建）；skill 指引与工具 enum 不一致 + .trellis/workflow.md 误导（改 skill 文本）。R5：同 session dev↔review 切换死锁，task.json 加 workflow_plugin 字段 + role gate/transition/breadcrumb 按 task 归属 plugin 查表 + set_session_plugin_name 切换时重映射 status。共 +1122 行，8 个新单测全过。

### Git Commits

| Hash | Message |
|------|---------|
| `072f8c6` | (see git log) |
| `81fed8d` | (see git log) |
| `1f4b506` | (see git log) |
| `ac5597c` | (see git log) |
| `b27ba14` | (see git log) |

### Status

[OK] **Completed**


## Session 33: docs 过时审查收官 — 43 文档审计 + 断链/错卷/索引处置 + 归档

**Date**: 2026-08-10
**Task**: docs 过时审查收官 — 43 文档审计 + 断链/错卷/索引处置 + 归档
**Branch**: `main`

### Summary

docs-staleness-audit 任务收官:43 个活文档逐行审计(design 误计 48,实测 43),处置 5 文档归档(_archive 日期前缀)+ 断链/死锚/相对路径修复(23+ 处)+ 错卷归位(06 卷尾部 12 条迁 07/08 卷)+ IMPLEMENTATION.md#4 锚点统一 + docs/README 索引补齐 + check-links.py 链接校验脚本 + docs 维护指南 spec。验收:38 文档链接扫描 0 失败,锚点/错卷 grep 归零。

### Git Commits

| Hash | Message |
|------|---------|
| `d8be89f` | (see git log) |
| `5f5a9b6` | (see git log) |
| `f948452` | (see git log) |
| `2596b81` | (see git log) |

### Status

[OK] **Completed**


## Session 34: 群聊参与者/主持人缓存率显示 + 上下文占用缓存率讨论

**Date**: 2026-08-10
**Task**: 群聊参与者/主持人缓存率显示 + 上下文占用缓存率讨论
**Branch**: `main`

### Summary

讨论并实现群聊缓存率显示:缓存率=单次调用 cache_read/context_input(非聚合);零 schema 改动,基于 turn_trace × messages.speaker join 取每 speaker 最近一次轮次。后端 list_speaker_cache_usage + group_chat_cache_rates 命令(tauri/daemon/http.ts 三处注册);前端 GroupChatConfigModal edit 模式参与者行+主持人区只读展示,cacheRatePercent 纯函数。check 子代理修复 max-seq 测试语义与删参与者错位两问题。cargo 1665 / pnpm 1027 全绿。spec 补 token-usage-tracking 群聊按说话人聚合 Scenario。

### Git Commits

| Hash | Message |
|------|---------|
| `0da5705` | (see git log) |
| `355a57b` | (see git log) |
| `bfb0d8c` | (see git log) |
| `607d5b7` | (see git log) |

### Status

[OK] **Completed**


## Session 35: remote epic S1–S5 归档收尾 + S6 打磨任务立项

**Date**: 2026-08-13
**Task**: remote epic S1–S5 归档收尾 + S6 打磨任务立项
**Branch**: `feat/remote-control-epic-s1`

### Summary

S5 移动端适配简单测试通过,但 UI 适配一般。按 S5 PRD 自限边界(可用不追求好用)判定合同达成,归档 S5 并顺带收尾 S1–S4(全部完成但未归档);新开 S6 08-13-mobile-polish 承接紧凑消息视图/触控手感/动效等推后项。

### Main Changes

- 归档 S1–S5(5 个 chore(task): archive 提交,epic [5/6 done])
- 创建 08-13-mobile-polish(S6 移动端体验打磨,planning,挂 epic 下)

### Git Commits

| Hash | Message |
|------|---------|
| `b25c597` | (see git log) |

### Status

[OK] **Completed**

### Next Steps

- S6 进入 planning:brainstorm 紧凑消息视图/触控手感/动效的验收标准
- epic 剩 S6 + 最终集成验收,完成后归档 parent


## Session 36: S6a+S6b 移动端打磨完成(主聊天视图+Settings 面板+三轮真机迭代)

**Date**: 2026-08-13
**Task**: S6a+S6b 移动端打磨完成(主聊天视图+Settings 面板+三轮真机迭代)
**Branch**: `feat/remote-control-epic-s1`

### Summary

S6 子任务全部归档。基于 S5 真机截图 30 个痛点(A/B/C/D 组)收敛到 DEC-1~7。S6a:header 瘦身(去 worktree)、输入区+状态条紧凑、消息流组件移动端适配、窄屏降级档(<360px)、mode 菜单动态翻转。S6b:tab 横向滚动+背景 pill、卡片信息密度降级、选中态 accent 条、关闭按钮 Done 语义、sidebar 手动可收起。三轮真机迭代修正 20+ 处(icon 32px、按钮误伤 min-width:44px、radios 14px、mask 双侧、双行布局等)。所有 4 宽度 body 零溢出,1035 tests + build 全绿。

### Main Changes

- feat(settings) S6b Settings 面板移动端适配(tab 滚动+pill+卡片密度)
- fix(mobile) S6b 真机三轮迭代(按钮宽度/sidebar 关闭/radio 缩放/id 字号/双行布局/双侧 mask)
- docs(spec) DEC-7 移动端设置面板+触控目标策略更新
- S6a+S6b 子任务归档(5+11 个 commits 全部落到 origin)

### Git Commits

| Hash | Message |
|------|---------|
| `acb8284` | (see git log) |
| `5ea3647` | (see git log) |

### Status

[OK] **Completed**

### Next Steps

- S6 跨子任务验收:Settings 主聊天视图 320-430px 真机最终确认
- S6 完毕,remote epic [5/6 done] 剩 parent 归档收尾


## Session 37: C7D tools Stub 注册:实施检查 + 部署漂移修复 + 归档

**Date**: 2026-08-14
**Task**: C7D tools Stub 注册:实施检查 + 部署漂移修复 + 归档
**Branch**: `main`

### Summary

对另一 session 实施的 C7D(10 工具 stub 化 + load_tool_schemas 拦截 + session 粘性 registry + 开关)做 trellis-check:红线逐条核验(双 gate 同源 init.rs:306-311 / 候选∩并行白名单=∅ / 保序 / registry 生命周期),cargo test --lib 1704 绿(1 预存 tunnel flaky 隔离过;clippy 1 warning 预存 blame 08-10)。独立 live 复验 AC1=3677(基线 6773,-45.7%,占比 38.5%→26%)两次一致;发现实施 session 17:07 重编后又改源码才 commit 的部署漂移(daemon 跑中间版),重编终版+重启+复测一致后闭环。非阻断记录:PARALLEL_WHITELIST 副本分叉风险(注释已声明)、同 turn 同名直呼二次放行边角。AC1 阈值 3000→3700 校准链(dispatch def 984 不可省)复核认可。

### Git Commits

| Hash | Message |
|------|---------|
| `bcf4187` | (see git log) |

### Status

[OK] **Completed**


## Session 38: 排查并修复会话 5df29977 五项异常

**Date**: 2026-08-18
**Task**: 排查并修复会话 5df29977 五项异常
**Branch**: `main`

### Summary

DB 取证 + 代码复验锁定五因并全部修复:① pitfall 召回 precision-fix(command_pattern 有值而 probe 无 command 时跳过,edit_file 不再恒注脚);② loop 干预卡片提交后立即 removePending;③ cd 进只读白名单(消 16/21 次弹窗);④ create_session INSERT mode 'chat'→'edit' + confirmYolo 失败 toast(真因 root guard 静默,z-index 假设推翻);⑤ loop 检测 L2 recency 门 + 干预继续后清窗(误报消除;hard 折叠方案因架空 3-strike 被否决)。四轮 turn-smoke live 验证通过;后端定向 + 前端 1100 全绿(全量 2 个 subagent flaky 已归因干净树)。

### Git Commits

| Hash | Message |
|------|---------|
| `bf820ff` | (see git log) |
| `fc97922` | (see git log) |
| `546cea3` | (see git log) |
| `87886ba` | (see git log) |
| `07ad211` | (see git log) |
| `974486b` | (see git log) |

### Status

[OK] **Completed**


## Session 39: 统一 token 预算表 + 关卡⑤硬卡(unified-context-budget)全程落地

**Date**: 2026-08-19
**Task**: 统一 token 预算表 + 关卡⑤硬卡(unified-context-budget)全程落地
**Branch**: `main`

### Summary

Session summary was not supplied.

### Main Changes

### Summary

推荐 → brainstorm(单任务两 WP + start 前评审 6 项独立处置:F1 估算重复计数采纳并连带修正 PRD 口径洞描述、F5 部分采纳——发现 @文件每 request 重展开使 DB spans 方案不可行,裁定同请求临时 spans)→ 4 PR 实施。**WP1**:turn_trace 三新列(at_files/system/context_window)+ budget.rs 统一估算(三部件加法,messages 已含 memory/skill/@文件/图片——评审 F1)+ 压缩三处口径统一切换(修 tools+system 挤窗漏计洞;机械路径 extra_tokens 参惠及群聊/worker)+ drive.rs 时序重排(tools 链前置 D7)+ at_file 同请求 spans(D10,fail-open)。**WP2**:关卡⑤硬卡(0.95×window,context_budget_enabled fail-open && !worker && !群聊)三臂静默裁剪(旧 @文件 span→占位 / 旧图→B1 占位降级 / memory 头→目录态快照,非破坏性请求副本)+ 臂尽 fail-fast(breakdown 错误)+ ChatEvent::BudgetTrim/审计 ContextBudgetTrim + trace 实发口径(预裁−freed)。前端:TurnCard per-model 窗口(弃 200k 硬编码)+ 预算构成条(五切片+残差)+ BudgetTrim chip/徽标 + 审计条目。坑:子代理撞 5h 限额留半成品由主会话接手补完;cl100k 对重复文本压缩比漂移使固定算术夹具脆(改自校准);Bash 进程组杀 daemon 需 run_in_background 前台承载。范围注记:C3 StillOver(0.5 target)先于 0.95 闸门中止一切确定性超线构造,臂级行为由单测锁、全 loop 只锁 no-misfire。live 烟测:三新列落值(system=795/window=200000),常态轮零裁剪。

### Testing

- [OK] 后端 1869+1 既有 flaky(subagent guard,复跑即过);前端 1122;vue-tsc 0;clippy 零新增(4 基线既有 stash 对照);fmt 干净
- [OK] live:turn-smoke 重编 daemon 实跑,system_token/context_window/at_files 落值正确

### Status

[OK] **Completed**

### Next Steps

- BACKLOG §3.1 统一预算表条目可勾;后续候选:A4+ 成本聚合视图 / C6 大输出截断统一


### Git Commits

| Hash | Message |
|------|---------|
| `ca675a1` | (see git log) |
| `484541a` | (see git log) |
| `325ad19` | (see git log) |
| `415c2fa` | (see git log) |
| `8980c5b` | (see git log) |

### Status

[OK] **Completed**


## Session 40: F5 follow-up:xlsx/xlsm 原生提取(每 sheet CSV 形态)全程落地

**Date**: 2026-08-26
**Task**: F5 follow-up:xlsx/xlsm 原生提取(每 sheet CSV 形态)全程落地
**Branch**: `main`

### Summary

F5 follow-up(xlsx 提取,pptx 用户裁定不做)从计划到 live 收口。选型:calamine 0.36 纯 Rust——依赖树核验其 zip 同为 default-features=false + deflate-only(zstd-sys 不回归),chrono feature 零新增 crate;表格→文本形态经用户三选一拍板为每 sheet CSV 块(RFC4180 转义 + 维度标题行,空 sheet 占位)。实现:ExtractKind::Xlsx + extract_xlsx(catch_unwind 包裹,xlsx 路径不做 normalize_whitespace,全空 Err 走 Degraded 兜底)+ at_file Office 分流扩 .xlsx/.xlsm(marker sheets=N;.xls/.ods 保持降级);前端 format 联合类型加 xlsx + hint 标签查找表(XLSX)。测试沿用 fixture 即代码:test_fixtures::build_xlsx 运行时手写 OOXML 部件过 calamine 解析,覆盖 CJK sharedStrings/CSV 转义/多 sheet 顺序+空表/序列日期 ISO(44927→2023-01-01)/inlineStr+#REF!/corrupt fail-soft + at_file 集成(.xlsx/.xlsm 注入,.xls Degraded)。坑:RFC4180 转义断言 needle 手抄多打一个引号(实现正确断言错)——转义期望串应逐字符对照生成。验证:后端 1991 过(1 plan_mode 满载预存 flaky 隔离复跑 0.68s 过)、vitest 1225、vue-tsc/build/clippy/fmt 基线干净;live 冒烟真实样本 xlsx(中文表头/日期/负浮点/含逗号备注/空 sheet)经 daemon 实跑 at_files_token=132、manifest {kind:extracted,format:xlsx,chars:142} 正确落库,模型精确读表并识别埋的无月份大数字歧义行。spec pattern-doc-extraction 增硬约束 #7 + xlsx 依赖结论;ROADMAP/decisions-2026-08 沉淀。

### Git Commits

| Hash | Message |
|------|---------|
| `4c35080` | (see git log) |
| `06cf2da` | (see git log) |

### Status

[OK] **Completed**


## Session 41: P2 债务清理:clippy gate + frontmatter 去重 + test_pool 去重全程收尾

**Date**: 2026-08-26
**Task**: P2 债务清理:clippy gate + frontmatter 去重 + test_pool 去重全程收尾
**Branch**: `main`

### Summary

DEBT.md 三条 P2 债(RULE-CI/FM/TESTPOOL-001)打包闭合,净删 180 行。R1:frontmatter 解析收敛 resource_loader 泛型 parse_md_resource<T: MdResource> + 共享 parse_string_array,三 loader(B3 command/skill/subagent)只留 Frontmatter struct + apply_kv 字段分支 + trait impl,parse_frontmatter/parse_allowed_tools/parse_tools_array 全变一行 thin wrapper;重复测试 apply_kv_ignores_comments_blank_unknown 收敛到共享层一份。R2:新建 db/test_support.rs(#[cfg(test)] 门控)test_pool(),15 处手写复制逐处 diff 全部逐字节等价替换零偏差(sessions/memories 测试簇 hub 用 pub(super) re-export 保持子文件 import 不动);check 代理核了全部 15 处原始实现(git show HEAD 对照)+ 孤儿 import 无残留。R3:CI 加 cargo clippy --lib -- -D warnings(排在 daemon sidecar build 后——clippy check 编译同样触发 build.rs externalBin 校验),仅剩 2 个 too_many_arguments(chat_inner/emit_max_turns_terminal)按 PRD 裁定显式 #[allow] + See DEBT.md RULE-ARGS-001 豁免。验证:fmt 干净、clippy -D warnings 零告警、cargo test --lib 1997 过/0 挂/1 ignored(基线 1991+增量-1 收敛测试),已知 flaky 未触发。注记:clippy --lib 不覆盖 #[cfg(test)] 代码,测试文件 import 卫生靠 review;共享 parse_string_array 统一了 warn 文案后缀(解析输出逐字节不变)。DEBT.md 清账 P2 4→1(剩 RULE-ARGS-001 epic + P3 RULE-DOC-001);spec 沉淀三约定到 backend/quality-guidelines.md(clippy gate 零告警硬标准 / MdResource 扩展路径禁复制解析循环 / test_pool 禁手写三件套+不往共享版加参数分支)。PRD AC1-AC5 全勾,四工作提交+归档。

### Git Commits

| Hash | Message |
|------|---------|
| `333b8bc` | (see git log) |

### Status

[OK] **Completed**


## Session 42: RULE-SHELL-001 sweeper 落地 + daemon 活体验证

**Date**: 2026-08-27
**Task**: RULE-SHELL-001 sweeper 落地 + daemon 活体验证
**Branch**: `main`

### Summary

闭合 DEBT P2 RULE-SHELL-001:InMemoryBackgroundShellRegistry 新增 sweep_completed_shells(只清 Done 超龄条目,retention 1h/interval 5min),daemon bin 经 spawn_shell_sweeper 装配(仿 backup task,GUI 零改动),4 新单测 + 全量 2001 绿 + clippy 净。trellis-implement/check 双代理流水线,外部 review 甄别(误判 1/有效 2/拒绝 1)。daemon 活体验证通过:真实 LLM 调工具 + SSE 权限应答 + 17.9s 清扫时序精确吻合;过程中发现并登记 2 条新债(RULE-SMOKE-001 turn-smoke 误杀在途 turn / RULE-PERM-002 shell 类 tool 级 grant 不生效无警告)。spec daemon-server.md 运维伴生物 Pattern 收编 sweeper。

### Git Commits

| Hash | Message |
|------|---------|
| `c4c6cf07` | (see git log) |

### Status

[OK] **Completed**


## Session 43: RULE-FE-001 staged 图片 objectURL 发送后 revoke(P2 闭合)

**Date**: 2026-08-27
**Task**: RULE-FE-001 staged 图片 objectURL 发送后 revoke(P2 闭合)
**Branch**: `main`

### Summary

闭合 DEBT.md §RULE-FE-001(P2 清零):send 成功释放 staging strip 时逐 uploaded[].localUrl revoke(~3 行,镜像 discardStagedImages 先例)。关键发现:债条登记的 reloadAfterFinalize 替换钩子方向前提已证伪——MessageImages.urlFor 自 B1 PR5 起 file 优先(daemon GET 路由),blob URL 是从不触发的防御回退(upload 先于乐观 push,失败即整轮中止),故无需动 reload 枢纽;localUrl 从不上 wire/不落库。新增 chatSendActions.test.ts 4 用例(B1 strip 生命周期此前零覆盖,含 jsdom 无原生 objectURL 的 spy 注入 + watch(currentSessionId) 需先 nextTick 排干两个 gotcha),1268 前端测试 + vue-tsc 全绿;trellis-check 零缺陷。spec state-management.md 收编 staging strip objectURL 生命周期契约(三路 revoke + file-first 论证)并修正过时 module layout 行;DEBT.md 删条目、P2 1→0、Total 13→12。

### Git Commits

| Hash | Message |
|------|---------|
| `2dd9adc1` | (see git log) |
| `40fe85fe` | (see git log) |
| `39c9f425` | (see git log) |
| `b3407214` | (see git log) |

### Status

[OK] **Completed**


## Session 44: 清理 RULE-SMOKE-001 / RULE-PERM-002

**Date**: 2026-08-27
**Task**: 清理 RULE-SMOKE-001 / RULE-PERM-002
**Branch**: `main`

### Summary

闭合两条 P3 债:turn-smoke.sh send_and_wait 改等 SSE 请求终态(kind=done 每请求恰一次),多轮工具 turn 不再被 delete_session 腰斩(live 双场景验证);grant 入口按 classify_tool 校验 kind↔类别矩阵拒绝死授权;顺带修同族坑 check_prefix_grant 读侧放宽 run_background_shell。全量 2006 测试 + clippy + fmt 绿;spec 收编 permission-layer §4.3 + pattern-terminal-done-event;P3 12→10。

### Git Commits

| Hash | Message |
|------|---------|
| `b7ddf050` | (see git log) |

### Status

[OK] **Completed**


## Session 45: 内置 dev/review 插件提示词脱栈通用化(builtin-agent-prompt-generalize)

**Date**: 2026-08-27
**Task**: 内置 dev/review 插件提示词脱栈通用化(builtin-agent-prompt-generalize)
**Branch**: `main`

### Summary

调查确认 builtin workflow 插件提示词硬编码 cargo/pnpm(经 include_str! 发给所有用户);PRD 经外部 LLM 审查修正 5 项(等价测试不存在、review 镜像漂移、README 口径、ask 边界、research 归档)。实施:checker/implementer/workflow.json 脱栈改探测链(AGENTS.md→清单文件→最小验证),.trellis 残留 3 处清零,def.rs 镜像同步并新增 builtin_dev_json_equals_default_workflow_constant 等价性测试(WorkflowDef derive PartialEq/Eq),dev+review 项目层 byte-identical 重灌消灭旧 implement/check 状态词汇表漂移,6b313ce4 model 漏传教训去标识化收编。验证:2007 tests passed,diff -r 双插件零差异,.trellis grep 零命中,lib-only clippy 干净。spec 沉淀三条内容约定;遗留:全仓 clippy --tests 有 14 个 pre-existing 错误(tests_agent_loop/at_file/db/trace 等)待立债任务。

### Git Commits

| Hash | Message |
|------|---------|
| `b1025153` | (see git log) |
| `77dbda53` | (see git log) |
| `ea69106f` | (see git log) |

### Status

[OK] **Completed**


## Session 46: RULE-TEST-002 角色门多轮刷新集成测试落地 + clippy 债务盘点

**Date**: 2026-08-27
**Task**: RULE-TEST-002 角色门多轮刷新集成测试落地 + clippy 债务盘点
**Branch**: `main`

### Summary

盘点 clippy 相关技术债:确认 RULE-CI-001/RULE-ARGS-001 已闭合,剩余 33 处 too_many_arguments 为有意豁免;ci.yml 过时注释同步闭合现状。做 RULE-TEST-002(全流程 Trellis):新增 role_gate_refresh.rs 集成用例——round-1 denial(planning 拒 checker)、mock LLM 同轮 write_file 翻盘 task.json、round-2 经 drive_turn 轮顶 resolve_current_task 刷新后放行;变异验证两类回归(门误接入口快照/轮顶刷新移除)均精确转红后复原;全量 2008 后端测试 + fmt/clippy 绿,生产代码零 diff。spec tests-required 收编第 29 条并注明已知边界(resolve_current_task 恒返 None 的第三类回归未覆盖);DEBT.md 销账,P3 余 9 条。附带核实 RULE-FE-002 现状:87886ba9 修的是 confirmYolo 第一 catch(IPC 失败),登记的 :225 第二 catch(resolve-after-success)仍在且注释自证 by-design,处置建议销账待定;cancelYolo :268 同族 console-only 亦未定性。

### Git Commits

| Hash | Message |
|------|---------|
| `e0649017` | (see git log) |
| `a45e897b` | (see git log) |
| `e336b888` | (see git log) |
| `6133c5d7` | (see git log) |
| `176e5626` | (see git log) |

### Status

[OK] **Completed**


## Session 47: chat 事件 payload 补 session_id:跨客户端实时认领

**Date**: 2026-08-27
**Task**: chat 事件 payload 补 session_id:跨客户端实时认领
**Branch**: `main`

### Summary

修复 remote PWA 看不到 local 发起的轮次实时流(只能刷新看)。根因:事件 payload 只带发起端 request_id,非发起端 activeRequests 无映射,未知-request 守卫静默丢弃。后端 3 个高频通道(ChatEvent/ToolCall/ToolResult)补必填 session_id(permission/question/mode/task 本就带);前端 streamEvents 新增 adoptForeignRequest/resolveRequest 按 session 认领(恒建新 assistant 占位 + pin session),旧 wire 无 session_id 维持丢弃语义,done/error 终结判定提前防 rid 泄漏。验证:cargo test 2008 全绿、pnpm test 1273 全绿(5 个新认领用例)、turn-smoke.sh 真实 LLM 冒烟通过、SSE wire 实测事件均带 session_id。

### Git Commits

| Hash | Message |
|------|---------|
| `68f7cadc` | (see git log) |

### Status

[OK] **Completed**

## Session 48: F6 异步 agent 任务(可观测性 + F3 并发闸 + 关闭边界)

**Date**: 2026-08-27/28
**Task**: 08-27-f6-async-agent-task
**Branch**: `main`

### Summary

F6 落地(detach 语义本就 free——chat_inner spawn fire-and-forget、SSE 零订阅静默丢、客户端断连非取消源;F6 纯编排层,零新表零迁移)。PR1 SessionSummary.busy 运行时态(list_sessions_inner 单点 enrich,双 transport 共用,wire additive);PR2 前端红点双源合流(streamingSessionIds ∪ serverBusy)+ finalizeRequest 公共出口消解 + buildTurnFinishedNotification 完成 toast(current-session/cancelled 抑制,configStore 开关 get_app_config fail-open);PR3 F3 全局 loop 信号量(AppState.loop_permits,max_concurrent_loops 缺省 4;spawn 闭包头 biased-select acquire,等闸取消走 rollback_claim_before_loop 四件套回滚;外模型评审 P1「临界区内 acquire」以死锁环论证拒绝并固化进 spec);PR4 CloseGuardDialog(isTauriWebview 门,非 transport 种类)+ ROADMAP/HACKING detach 边界。验证:backend 2014 / frontend 1287 全绿,vue-tsc/clippy/fmt/build 绿,turn-smoke live 过,busy/get_app_config live 实证,队列续轮 + 各阶段 cancel(前首字节/流中/完成后)+ 8 轮 cancel-race stress 时序矩阵全绿零 panic。

**「cancel_chat 死锁」复盘为观测假象**:daemon 启动命令带 `| head -30` 截断了全部后续日志(「loop 无下文」实为丢弃);f6-c2 的 cancel 探针打错路径(/api/v1/cancel_chat → 405 空回复被误读为悬挂;正确为 /api/v1/cancel/cancel_chat + request_id snake_case);叠加 orphan daemon 抢端口串台。stranded busy 随进程消亡不可复现,read_timeout(60s) 保证上游静默流最迟 60s 走错误路径自了。教训入 validation.md:live 探测 daemon 必须全量落日志文件,禁止 head/tail 管道;cancel 探针先核 405。

Spec 沉淀:pattern-global-loop-semaphore(含 P1 拒绝依据)、daemon-server busy enrich 模式、frontend session-busy-visibility 契约。

### Git Commits

| Hash | Message |
|------|---------|
| (pending) | F6 全量(待用户确认后提交) |

### Status

[OK] Completed(AC1/AC2/AC4 手动验收与 daemon 重启恢复项留用户)


## Session 48: 定时任务目标 session 三档:per_run 每次执行新建 session + 表单 radio 卡片重设计

**Date**: 2026-08-31
**Task**: 定时任务目标 session 三档:per_run 每次执行新建 session + 表单 radio 卡片重设计
**Branch**: `main`

### Summary

scheduled_tasks 表重建迁移(target_mode/model_id/last_run_session_id,target_session_id 可空化 + CHECK 不变式);调度器 tick 内为 per_run 新建 run session(绕过同 session 串行化/队列去重/queue-disabled,建会话失败消费 due);create/update 档位切换校验 + route/tool 适配(LLM 路径不变);前端目标区三档 radio 卡片重设计 + per_run 卡片 meta。cargo 2169 / vitest 1497 全绿,clippy 无新增,headless 截图验证桌面+移动三档切换。spec scheduled-tasks.md 收编 per_run 契约。遗留:daemon 需 restart 加载新 Rust 二进制。

### Git Commits

| Hash | Message |
|------|---------|
| `bf28a31c` | (see git log) |
| `cd212ff4` | (see git log) |
| `31e5d7dd` | (see git log) |

### Status

[OK] **Completed**


## Session 49: SSE 丢分片与提示词头部缓存失效:取证 + 双修复

**Date**: 2026-08-31
**Task**: SSE 丢分片与提示词头部缓存失效:取证 + 双修复
**Branch**: `main`

### Summary

从 DB 275 条 is_error tool_result + daemon.log 归因两类 harness 缺陷并建任务修复:(1) SseParser 无行缓冲,TCP chunk 边界切断 data: 行后半段被静默丢弃,致 tool_use input={} (Missing required parameter 30 例) 与参数/正文静默缺段(引号断裂/old_string 缺段);修复为半行缓冲 + 9 回归测试 + SSE 解析失败日志升 WARN,实测 255 次丢行归零。(2) OpenAI 兼容路径无 cache_control,breadcrumb(messages[0])/instruction 重读/head_sha(system)致状态迁移与新请求边界 cache_read=0(28 万 token 全量重付,seq 435/437 实证);下沉尾部注入 + instruction session 内冻结(带开关)+ repo-state 尾部块 + cache-miss WARN;turn-smoke 两轮实测第二轮命中 99%。遗留:tools=0 辅助调用(疑 truncate_summary)缓存干扰调查(archive 后 implement.md 步骤 6)。

### Git Commits

| Hash | Message |
|------|---------|
| `2a1b3c6a` | (see git log) |
| `b797d0c8` | (see git log) |

### Status

[OK] **Completed**


## Session 50: A2+ P3d 后台 shell 升级闭环(B 案:下轮注入时弹卡)

**Date**: 2026-09-01
**Task**: A2+ P3d 后台 shell 升级闭环(B 案:下轮注入时弹卡)
**Branch**: `main`

### Summary

P3c 留下的最后一个沙盒 UX 不对称收口:后台 run_background_shell 面外失败(写被拒/断网)从纯模型介导升级为系统驱动闭环。呈递时机用户裁定 B 案——下轮 drain 后、组装 turn 文本前解析(background_escalation.rs::resolve_all),复用前台 §5.2 全套机制:Plan 门 → escalation_source → prefix-grant 零卡直跑 / Ask 卡经原 tool_use_id 挂回原 bsh 卡(120s,turn token cancel-safe)→ 批准后 registry start(sandbox=None,origin=None) 一次性不沙盒重跑。载荷链:dispatch 对所有工具统一盖章 ToolContext.tool_use_id → registry entry → 等待任务在 Normal+Failed+沙盒+origin+classify_block 命中时烘 EscalationOffer 入通知(Killed/TimedOut/Skip 恒 None);start() 把 sandboxed∧origin 折叠成 escalation_origin 单值。无 offer 通知逐字节走 legacy 格式;LLM 永远只见终态故事,不自己发起重跑。前端一处必要改动(推翻 PRD 零改动假设):ShellCard isPendingApproval 的 !hasResult 守卫对已有结果的后台卡豁免(isBackground),前台 hide-on-result 保留,双向 vitest 锚。计划外修复:①ask.rs 先 emit 后 register 的竞态使 mock resolver 单发 resolve 合法落空,tests_escalation approve 例在并行负载下 flaky——重试到 ok 修复;②root 环境 /proc/1/mem 可写致探针前提失效,geteuid==0 大声 SKIP(本机 root 首次暴露)。验证:后端 2215 绿+clippy/fmt 净,前端 vitest/vue-tsc/build 绿+e2e 7/7(Chromium v1234 重装后),live turn-smoke --sandbox-probe OK(daemon 重建重启);Checker 子代理验收 PASS-with-minor,3 条 P3(mode 双源删 env.mode、escalation 过时注释、N/A 映射去重)当场清零。

### Git Commits

| Hash | Message |
|------|---------|
| `8fa9047d` | (see git log) |

### Status

[OK] **Completed**


## Session 51: wf-dev breadcrumb 补 commit 指引

**Date**: 2026-09-02
**Task**: wf-dev breadcrumb 补 commit 指引
**Branch**: `main`

### Summary

排查确认 wf dev 内置工作流在提示层对 commit 缺位(breadcrumb 无一句提交引导,状态门禁只管 transition;对比 Trellis Phase 3.4);在 in_progress(收尾整理逻辑 commit)与 done(归档前确认已提交)breadcrumb 补引导文案,builtin 源 + def.rs 常量 + .everlasting 镜像三处同步;全量 lib 2224 测试通过,等价性单测与镜像 diff 守护。未动权限引擎 Edit 模式放行 git commit 的既有设计。

### Git Commits

| Hash | Message |
|------|---------|
| `336a164e` | (see git log) |

### Status

[OK] **Completed**


## Session 52: wf-trellis-alignment:builtin dev workflow 对齐 Trellis 三机制

**Date**: 2026-09-02
**Task**: wf-trellis-alignment:builtin dev workflow 对齐 Trellis 三机制
**Branch**: `main`

### Summary

对齐 builtin dev workflow 与 Trellis 流程:R1 声明 in_progress→planning 回环边(三处定义同步,工具文案去前向偏置,state.rs 钉死回滚零 marker);R2 调研落盘 research/ 约定 + 隔离 worker 不可见对策(delegation 摘要通道);R3 relevant-specs.jsonl 按任务策展 {relevant_specs} 注入(fallback 逐字兼容)。外部模型评审 P1-P3 核实后合入(P1 机制修正:tasks/ gitignored 非『未提交』;P2 修 wf-overview/wf-brainstorm 误指 ask_user_question)。全量 2230 tests 绿,镜像 diff 空,spec 沉淀 workflow-plugin-builtin.md 新契约段。

### Git Commits

| Hash | Message |
|------|---------|
| `488c1632` | (see git log) |
| `c4895434` | (see git log) |
| `1e14d15b` | (see git log) |
| `d42f8fb0` | (see git log) |

### Status

[OK] **Completed**


## Session 53: Chat 运行状态面板:ActivityPanel 三合一 + background shell 可观测性

**Date**: 2026-09-02
**Task**: Chat 运行状态面板:ActivityPanel 三合一 + background shell 可观测性
**Branch**: `main`

### Summary

新增 chat 右下 ActivityPanel(子代理/后台命令/清单三 section 合并原 ChecklistCard,checklist store 零逻辑改动)。后端补 background shell 可观测性缺口:registry list_for_session + background_shell:update 事件(started/exited/pruned,emitter 注入 + daemon SSE/Tauri Full 双接线)+ list/kill_background_shell 双模式 IPC(五处注册)。subagent 行点击复用 SubagentDrawer。check 阶段修 2 个 bug:running elapsed 接收时间戳双计、ensureStarted 守卫短路;follow-up 修浮球 solo 图标居中。测试:后端 2235 通过/前端 1547 通过/clippy/vue-tsc/e2e 全绿。spec 沉淀 backend/background-shell-observability.md + frontend/chat/activity-panel.md(双时间源 gotcha:MonotonicMs 禁与 Date.now 混算)。

### Git Commits

| Hash | Message |
|------|---------|
| `d76398b1` | (see git log) |
| `1a236d83` | (see git log) |
| `651709b6` | (see git log) |

### Status

[OK] **Completed**

## Session 54: BACKLOG §5.3/FU-3 收尾——添加项目全模式统一 DirBrowserModal + native picker 整链下线

**Date**: 2026-09-03
**Task**: 09-03-dirbrowser-desktop-unify
**Branch**: `main`

### Summary

推荐选题 → brainstorm 两问收敛(整链删除深度 / 键盘导航纳入)→ 实施 → check PASS-with-minor。addProject()→openDirBrowser():桌面/浏览器/sidecar/remote 统一走 browse_dir IPC;pick_project_dir + tauri-plugin-dialog(唯一消费方)整链删除(命令/插件注册/all_command_names/Cargo 依赖/capabilities,Cargo.lock 级联收敛 rfd+tauri-plugin-fs);DirBrowserModal 补 roving tabindex 键盘导航(方向键钳边 + Enter 零 handler 原生激活 + navigate fromList 焦点复位策略);e2e 新 spec 2 用例锁全流程与键盘链路。坑:jsdom 无 Enter activation behavior,单测改两半断言(handler 不 preventDefault + click 导航),原生激活链路归 e2e 真 Chromium。Checker 独立重跑全门(clippy -D warnings/cargo test 2242/vitest 1565/vue-tsc/e2e 9)全绿,唯一 P3 = STRUCTURE.md 两处 stale 当场清零。5 commits(切换+删除 / 键盘 / e2e+文档 / spec 沉淀 / archive)。销 BACKLOG §5.3(2026-06-05 挂起),搜索框为唯一剩余项备案。桌面真机手测留 GUI-capable 机器(PRD 明示)。

### Git Commits

| Hash | Message |
|------|---------|
| `c64f7243` | refactor(projects): 添加项目全模式统一 DirBrowserModal——下线 native pick_project_dir 与 tauri-plugin-dialog 整链 |
| `1b711318` | feat(frontend): DirBrowserModal 键盘导航——roving tabindex 方向键钳边移动 + Enter 原生激活 |
| `06b27c55` | test(e2e): 项目添加全流程 route-mock 用例 + 文档销账 |
| `28209c2c` | docs(spec): 沉淀 DirBrowserModal 统一入口契约 + roving tabindex 模式 + jsdom Enter activation gotcha |

### Status

[OK] **Completed**


## Session 54: F3 磁盘治理:worktree/outputs/日志/备份/缓存限损

**Date**: 2026-09-03
**Task**: F3 磁盘治理:worktree/outputs/日志/备份/缓存限损
**Branch**: `main`

### Summary

ROADMAP 第三档 F3 磁盘余留收口(4 PR):摸底实测大头为辅助数据(备份 175M/WebKitCache 136M/日志 59M,非原设想的 worktree);P0-a 修 worker sweep 宿主断链(daemon bin 装配 spawn_disk_governor 24h 节拍首拍延迟 5min,CancellationToken 协作停机);P0-b 日志进程内 RotatingFileWriter 10MiB×3 运行期轮转(零依赖手写,daemon.sh 脚本轮转退役、bg 重定向改 /dev/null、logs 子命令不变,sidecar 模式日志首次落盘);P1 孤儿 session worktree(session_exists 判定+destroy,行在含 Detached 不动)+outputs 孤儿桶/有主 30 天按龄(开关默认开,_no_session 孤儿语义不经开关)+UUID 形态护栏+fail-keep;P2 备份 200MiB 预算自适应(至少 2 最多 7,恰等于不停)+WebKitCache GUI 启动 50MiB 阈值清理(⚠ Thin 早退陷阱:装配必须在 setup 公共区 mode resolve 后 Thin return 前,外部评审抓出的最高风险点,源码静态断言测试守护);IPC get_disk_usage/run_disk_cleanup 五处接线(手动入口不查 kill-switch,AC9);设置面新增「存储」分组 DiskTab(占用概览 7 消费点+两开关+立即清理+成功后自动刷新)。外部模型评审 7 条全部核实采纳(tracing 默认 writer 是 stdout 非 stderr 等)。门禁:cargo 2277/clippy/fmt/pnpm 1577/build/e2e 9/turn-smoke 全绿;live 实证 governor 首拍回收 24MB 旧备份。插曲:PR4 派发时实现子代理撞 5h 限额,其已写 webkit_cache.rs 主体,主会话接手修 3 处遗留(include_str 路径/mod 声明/边界值测试)并完成装配与文档;check 子代理 AC1-AC10 全 PASS+自修 2 处(database-guidelines 签名块/e2e 路由清单)。spec 沉淀 disk-governance.md 新契约+daemon-server RULE-DAEMON-001 日志段重写+database-guidelines 备份段;follow-up:DB VACUUM、进程/内存、F1 反压联动。

### Git Commits

| Hash | Message |
|------|---------|
| `27152a12` | (see git log) |
| `296e8d12` | (see git log) |
| `fb467350` | (see git log) |
| `443baaab` | (see git log) |

### Status

[OK] **Completed**
