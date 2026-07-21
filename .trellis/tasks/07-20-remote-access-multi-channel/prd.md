# 远程访问/多通道改造:research + roadmap + 子任务编排

## Goal

把 everlasting 从"Tauri 单进程 in-process IPC"改造为"daemon 独立进程 + HTTP/SSE transport",**近期目标解锁本机浏览器访问(含 Windows 宿主访问 WSL daemon)**,远期留好跨设备远程访问的接口。用户诉求见 [docs/REMOTE-ACCESS-RESEARCH.md §0](../../../docs/REMOTE-ACCESS-RESEARCH.md#0-背景与目标)。

## Background

- **来源**:用户提出"前后端接口包装 + 多通道(网页/Electron) + 本地直连 + 远程配对"的改造诉求,需要先做完整调研评估再落地。
- **调研产出**:[docs/REMOTE-ACCESS-RESEARCH.md](../../../docs/REMOTE-ACCESS-RESEARCH.md)(调研评估稿,2026-07-20 已吸纳两份 review 修正)。
- **实施路线**:[docs/REMOTE-ACCESS-ROADMAP.md](../../../docs/REMOTE-ACCESS-ROADMAP.md)(8 个可独立验证的子阶段)。
- **本 parent 任务**:持有调研产出 + 编排 2 个 child + 记录跨 child 架构决策。**parent 本身不直接实施代码**。

## Scope

### In scope(本 parent + 2 child 覆盖)

- **Child 1 [近期]**:前端 Transport 抽象 + 后端 emit 散点收敛(= Phase 1,P1.1-P1.3)。
- **Child 2 [近期]**:daemon 拆分 + 本地 HTTP server + SSE(= Phase 2,P2.1-P2.5)。
- 跨 child 架构决策记录(见下"Decisions")。

### Out of scope(远期/可选,不建 task)

- **Phase 3 远期**:认证配对 + 跨设备远程(配对码 + token + Cloudflare Tunnel)。前置条件:Phase 2 协议经 dogfooding 稳定。设计草稿见 [RESEARCH §4.4](../../../docs/REMOTE-ACCESS-RESEARCH.md#44-phase-3远期规划认证--跨设备远程访问)。
- **Phase 4 可选**:Electron thin client。
- **飞书 channel**:同属远期,见 [BACKLOG §6](../../../docs/BACKLOG.md)。

## Decisions(架构决策记录,2026-07-20)

### D1. 单一协议统一(非后端 Facade)

**决策**:Phase 2 完成后,**所有 client(Tauri GUI / 浏览器 / 未来的 Electron)统一走 HTTP/SSE 连 daemon**;Tauri 内嵌的 `#[tauri::command]` IPC 入口废弃为死代码。

**否决方案**:后端 Facade(service 层 + Tauri/HTTP 双 adapter 长期并存,GUI 保留 in-process 离线 fallback)。

**理由**:
- 79 个 command 维护两套入口的协议 drift 成本太高(测试翻倍、type sync 复杂)
- 单用户场景下 HTTP 一跳延迟可忽略(localhost sub-ms)
- opencode 已验证"只有 HTTP server"可行

**代价**:daemon 挂了 GUI 就废,无 in-process fallback。**接受这个代价**——daemon 是单进程 + systemd/launchd 自动重启,可靠性足够;若 dogfooding 发现"daemon 频繁挂"再回来补 in-process fallback(届时 Phase 2 已有 service 层雏形,补成本可控)。

> **Carlos 决策日期:2026-07-20**(对话中明确 ack "单一协议统一")。该决策已采纳,research/review-triage-2026-07-20.md 记录在案。**ARCHITECTURE.md §4 的目标态实施路线同步**:Phase 2 archive 时 79 个 `#[tauri::command]` 入口转为 dead code(无 caller)。

### D2. 传输协议:SSE + HTTP POST(非 WebSocket)

详见 [RESEARCH §3.1](../../../docs/REMOTE-ACCESS-RESEARCH.md#31-传输协议)。高频单向流走 SSE,低频 round-trip 走 POST。

### D3. 认证配对:配对码 + Bearer token(非 mTLS)

浏览器不支持 mTLS。详见 [RESEARCH §3.2](../../../docs/REMOTE-ACCESS-RESEARCH.md#32-认证与配对)。**Phase 3 远期实施**。

### D4. 网络拓扑:本地直连 → Cloudflare Tunnel(远期)

详见 [RESEARCH §3.3](../../../docs/REMOTE-ACCESS-RESEARCH.md#33-网络拓扑三种部署形态)。Phase 2 = 形态 A(本地直连,含 WSL→Windows),Phase 3 = 形态 B2(Cloudflare Tunnel)。

## Requirements(跨 child)

- **R1** Tauri 版在 Phase 1 / Phase 2 全程保持可用(`pnpm tauri dev` 不坏)。
- **R2** Phase 1 完成后,Tauri 版行为零变化(纯抽象层,无业务逻辑改动)。
- **R3** Phase 2 完成后,本机浏览器(含 Windows 宿主访问 WSL daemon)可完整使用 agent 功能。
- **R4** Phase 2 完成后,79 个 command 在 HTTP transport 下的行为与 Tauri 版一致(回归测试对拍)。
- **R5** 10 类 SSE 事件 + 4 类 round-trip(permission/question/mode_change/task_state_transition)端到端验证通过。
- **R6** 无 dual-pool 写竞争(GUI 切 httpTransport 后不开本地 SqlitePool)。

## Acceptance Criteria(parent)

- [ ] **Child 1 完成并 archive**(Tauri 版零行为变化 + emit 散点收敛 + grep 确认无散落 import)
- [ ] **Child 2 完成并 archive**(本机浏览器可访问 + 79 command 行为一致 + WSL→Windows 验证通过)
- [ ] **[Phase 1 archive 时]** Carlos 更新 [ARCHITECTURE.md §4](../../../docs/ARCHITECTURE.md#4-决策agent-daemon-化为多-channel-接入铺路) 触发条件为"真浏览器远程访问诉求确定实施时"(owner: Carlos;时机: child 1 archive)
- [ ] **[Phase 1 archive 时]** Carlos 把 [ROADMAP B10](../../../docs/ROADMAP.md) 拆为 B10a(transport)/B10b(daemon)进第二/三档,B10c(认证远程)留第四档(owner: Carlos;时机: child 1 archive)
- [ ] [docs/REMOTE-ACCESS-RESEARCH.md](../../../docs/REMOTE-ACCESS-RESEARCH.md) 反映最新决策(D1-D4)
- [ ] 最终 integration review:在浏览器(Chrome/Edge on Windows)完整跑一轮对话 + 触发 permission/ask_user_question + subagent drawer,行为与 Tauri 版一致

## Child Tasks

| Child | 范围 | 状态 |
|---|---|---|
| [07-20-remote-access-transport-abstraction](../07-20-remote-access-transport-abstraction/) | Phase 1:Transport 抽象 + emit 散点收敛 | planning |
| [07-20-remote-access-daemon-split](../07-20-remote-access-daemon-split/) | Phase 2:daemon 拆分 + HTTP server + SSE | planning |

## References

- [docs/REMOTE-ACCESS-RESEARCH.md](../../../docs/REMOTE-ACCESS-RESEARCH.md) — 调研评估(为什么这么做)
- [docs/REMOTE-ACCESS-ROADMAP.md](../../../docs/REMOTE-ACCESS-ROADMAP.md) — 实施路线(怎么做、怎么验证)
- [docs/_reviews/REVIEW-remote-access-research-2026-07-20.md](../../../docs/_reviews/REVIEW-remote-access-research-2026-07-20.md) — MiniMax-M3 review
- [docs/_reviews/REVIEW-remote-access-research-deepseek-v4-pro.md](../../../docs/_reviews/REVIEW-remote-access-research-deepseek-v4-pro.md) — DeepSeek-v4-pro review
- [research/](./research/) — 两份 review 的独立甄别报告(本任务产物)
- [docs/ARCHITECTURE.md §4/§5](../../../docs/ARCHITECTURE.md#4-决策agent-daemon-化为多-channel-接入铺路)
- [docs/ROADMAP.md B10](../../../docs/ROADMAP.md)
