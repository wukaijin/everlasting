# PRD: docs sync A — 架构核心文档 (CLAUDE / ARCHITECTURE / STRUCTURE)

> 父任务:`../07-23-docs-sync-daemon-split/`
> 审计依据:`../07-23-docs-sync-daemon-split/research/audit-daemon-docs-drift.md` §A1/A2/A3(本任务 source of truth)
> 跨任务一致性约定:见父 prd.md「跨子任务一致性约定」

## 目标

把 3 份架构核心文档从"Tauri 一体化进程"心智同步到 daemon 化后的"GUI + daemon 双进程 + transport 抽象"现状。这是 4 个子任务里最核心、过时最密集的一组(ARCHITECTURE.md 是重灾区)。

## 范围

### A1. `CLAUDE.md`(项目根)
据审计报告 §A1,需修正 6 处:
- 核心数据流(行 149):`invoke("chat")` → `transport.invoke()` 双路径(httpTransport 默认 / tauriTransport)
- 关键架构决策(行 158):「后期 Unix socket/WebSocket」→「已落地 axum HTTP+SSE+ServeDir」
- Architecture 目录树(行 50-145):补 `daemon/`、`sidecar.rs`、`bin/everlasting-daemon.rs`、`transport/`、`BrowserHeader.vue`
- Tech Stack 表(行 177-187):补 axum / tower / tower-http + 进程模型说明
- Common Commands(行 13-43):补 `./scripts/daemon.sh ...` / `cargo build --bin everlasting-daemon` / `?transport=tauri` / Thin-Full 说明
- Project Overview(行 7):补 GUI/daemon 分离 + 浏览器模式

### A2. `docs/ARCHITECTURE.md`(重灾区)
据审计报告 §A2,需修正 7 处:
- §1 顶部状态声明(行 10-13):删除「当前 MVP in-process / 未做进程拆分」,改写为 daemon 化已落地
- §1.1 进程拓扑(行 15-107):重画 ASCII 图为实际架构(daemon bin + sidecar + httpTransport + ServeDir),删除臆想的 Channel Router / FeishuChannel / CliChannel / Unix socket
- §1.2 关键数据流(行 109-147):改写默认路径为 httpTransport
- §2.2 16 关卡·②(行 244-260):「Tauri IPC 边界」→「transport 边界」,拆 HTTP/Tauri 两路
- §4 决策:Agent Daemon 化(行 740-757):改待办语气为「已实施」,修文件名(`daemon/` 目录)、通信(axum HTTP)、进程管理(sidecar)
- §5 Channel Adapter 抽象(行 761-791):整节臆想设计,删除或重写为实际 axum HTTP/SSE 单端点架构
- §2.3 洞察第 6 条(行 616):改现在时

### A3. `STRUCTURE.md`(项目根)
据审计报告 §A3,需修正 9 处:目录树补 daemon/sidecar/transport/browser(§1/§2/§3)、依赖图与数据流改双进程(§4)、IPC 表更新为 79 + 双暴露(§5)、设计模式补 transport/sidecar 条目(§8)、构建命令补 daemon(§10.2)、依赖表补 axum(§12)、全景图 command 数(§13.3)、更新基线 commit(行 3)。

## 验收标准

- [ ] CLAUDE.md 6 处全部修正
- [ ] ARCHITECTURE.md 7 处全部修正;全文 grep「目标态」「未做进程拆分」「in-process」(指架构时)清零
- [ ] ARCHITECTURE.md grep「Unix socket」「Named pipe」「WebSocket」「FeishuChannel」「CliChannel」「Channel Router」清零
- [ ] STRUCTURE.md 9 处全部修正;目录树含 daemon/sidecar/transport/bin/BrowserHeader
- [ ] STRUCTURE.md 基线 commit 更新到 07-23 最新
- [ ] 三份文档进程模型/通信/命令速查措辞与父 prd「统一叙事」一致
- [ ] 不误改历史 ADR 条目的「当时这么想」语义(只加演进注记)

## 风险

- ARCHITECTURE.md §1.1 进程拓扑 ASCII 图改动大,需保证与 daemon/server.rs + sidecar.rs + transport/index.ts 代码一致(读这三处确认)。
- STRUCTURE.md 目录树是高频引用对象,新增条目要放对位置(参照实际文件系统布局)。
