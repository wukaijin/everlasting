# Daemon 化文档过时审计报告

> 审计日期:2026-07-23
> 审计基准:daemon 化代码已落地(commits `0dbc747` → `3307d93`,07-20~07-23,~15K 行)
> 用途:本任务的 source of truth,各子任务按"文档域"认领对应章节。

## 审计基准(代码事实,已全部核实存在)

| 事实 | 路径 |
|------|------|
| daemon server | `app/src-tauri/src/daemon/server.rs` / `sse.rs` / `error.rs` / `mod.rs` / `routes/`(20 路由文件) |
| daemon bin | `app/src-tauri/src/bin/everlasting-daemon.rs` |
| sidecar spawn | `app/src-tauri/src/sidecar.rs`(GuiMode::Thin/Full) |
| 前端 transport 抽象 | `app/src/transport/`(http.ts / tauri.ts / health.ts / env.ts / types.ts / index.ts + 4 test) |
| 管理脚本 | `scripts/daemon.sh`(start/bg/stop/restart/rebuild/status/logs) |
| 浏览器模式 | `app/src/components/layout/BrowserHeader.vue` |
| 依赖 | `Cargo.toml` axum 0.7 + tower 0.5 + tower-http 0.6 + tokio-stream + clap + tauri-plugin-shell |
| 默认 transport | `httpTransport`(前端 + 浏览器);`?transport=tauri` 为 Full 模式逃生舱 |

**通信真相**:axum HTTP + 同源 SSE + ServeDir 服务 SPA。**不是** Unix socket / Named pipe / WebSocket。
**触发真相**:由远程访问/浏览器模式需求触发,**不是** B10 飞书。
**provider 配置真相**:DB catalog(`providers` 表 + 加密 `api_key_enc`)是生产路径;`LlmConfig::from_env()` 仅冷启动兜底(state.rs:284 注释 "DB provider catalog takes precedence")。默认模型 `MiniMax-M2.7`(anthropic.rs:79),非 `GLM-4.7`。

---

## 子任务 A:架构核心文档(CLAUDE / ARCHITECTURE / STRUCTURE)

### A1. CLAUDE.md 【高】
- **核心数据流(行 149)**:仍写 `invoke("chat", {requestId, sessionId, messages})` Tauri 一体化。实际默认 `httpTransport`(HTTP→daemon)。需改为前端 `transport.invoke()` → httpTransport/tauriTransport 双路径。
- **关键架构决策(行 158)**:「daemon 化:后期...通过 Unix socket / WebSocket IPC」。三错:① 已落地非"后期";② 通信是 HTTP/SSE 非 Unix socket/WebSocket;③ 把已落地写成未做。
- **Architecture 目录树(行 50-145)**:无 `daemon/`、无 `sidecar.rs`、无 `bin/everlasting-daemon.rs`、无 `transport/`、无 `BrowserHeader.vue`。
- **Tech Stack 表(行 177-187)**:漏 axum / tower / tower-http;无进程模型说明(GUI + daemon + sidecar)。
- **Common Commands(行 13-43)**:无 `./scripts/daemon.sh`、无 `cargo build --bin everlasting-daemon`、无 `?transport=tauri`、无 Thin/Full 说明。
- **Project Overview(行 7)**:「Tauri 2 + Vue 3 + Rust,自研 agent core」未反映 GUI/daemon 分离 + 浏览器模式。

### A2. docs/ARCHITECTURE.md 【高·最密集】
- **§1 顶部状态声明(行 10-13)**:「当前 MVP:agent core 跑在 Tauri 进程内,未做进程拆分,无独立 daemon」+「触发条件:BACKLOG §6 飞书」。全错。
- **§1.1 进程拓扑(行 15-107)**:标题「(daemon 化后,目标态)」;ASCII 图把 daemon 画成未来;通信画成「Unix socket / Named pipe / WebSocket」;画了不存在的 `FeishuChannel`/`CliChannel`/`Channel Router`。
- **§1.2 关键数据流(行 109-147)**:行 111「当前 MVP 实测路径:invoke('chat')...没独立 daemon」;行 115-123 全是 Tauri 一体化描述。
- **§2.2 16 关卡详解·② Tauri IPC 边界(行 244-260)**:整段基于「Tauri IPC 是唯一入口」。实际默认 HTTP。关卡②应改「transport 边界」或拆 HTTP/Tauri 两路。
- **§4 决策:Agent Daemon 化(行 740-757)**:写成待办语气;文件名错(`daemon.rs` vs 实际 `daemon/` 目录);通信错;进程管理错(写 systemd,实际 sidecar.rs + tauri-plugin-shell);无「已实施」标注。
- **§5 决策:Channel Adapter 抽象(行 761-791)**:整节臆想设计(`Channel` trait / `TauriGuiChannel` / `FeishuChannel` 实际都不存在),还用「✅ 已实现」标注。需删除或重写为实际 axum HTTP/SSE 架构。
- **§2.3 关键洞察 第 6 条(行 616)**:「(daemon 化后新增)Channel 是状态边界」—— daemon 已落地,改现在时;Channel 概念未实现。

### A3. STRUCTURE.md 【高】
- **§1 顶层结构(行 33-45)**:缺 `scripts/daemon.sh`、缺 `app/src/transport/`。基线声明停在「2026-07-10 commit f08d61e」(行 3)。
- **§2 前端树(行 53-107)**:无 `transport/`、无 `BrowserHeader.vue`;streamController 注释(行 386 §8.1)仍写「IPC ↔ Pinia 唯一入口」。
- **§3 后端树(行 131-228)**:无 `daemon/`、无 `sidecar.rs`、无 `bin/everlasting-daemon.rs`。
- **§4 模块依赖图 + §4.2 跨层数据流(行 249-283)**:画成「前端 ↔ 后端同进程 Tauri IPC」;行 262-263「Tauri IPC (invoke + listen)」;行 278-282「invoke('chat')」。
- **§5 Tauri IPC 表(行 287-314)**:表头「~60 命令」实际 79;未反映「同一 handler 双暴露(IPC + HTTP)」Q0 决策。
- **§8 关键设计模式(行 382-419)**:无 transport/sidecar/GuiMode/daemon HTTP-SSE 模式条目。
- **§10.2 构建命令(行 478-487)**:无 daemon bin / daemon.sh / 浏览器模式。
- **§12 依赖表(行 510-527)**:无 axum/tower/tower-http。
- **§13.3 ASCII 全景(行 577-606)**:「33 tauri commands」(实际 79) + 一体化心智。

---

## 子任务 B:路线图 / 状态 / 索引文档(ROADMAP / README / CONTEXT / BACKLOG)

### B1. docs/ROADMAP.md 【高·最严重】
- **§1.2 已实施表(行 37-92)**:完全缺 daemon 化。最后条目是 07-14 E2。P2.1~P2.5 + transport + sidecar + ServeDir + 浏览器模式 + E2E(~10 commit)按 §5 维护承诺(行 190-191)应补入。
- **§2 第四档 B10(行 140)**:「飞书 IM | 触发 daemon 化,重大架构变更」。触发条件已满足,卡点理由 stale。
- **§2 四档分类(行 96-143)**:daemon 化不属于 V2 编号体系,既不在已实施也不在计划,形成盲区。

### B2. README.md(顶层)【高】
- **状态行(行 21)**:「当前:2026-07-17。MVP 主体 + V2 第一/二/三档 25 项」。停在 07-17,未含 daemon 化。
- **能力矩阵(行 54-88)**:无运行形态/transport/daemon/ServeDir/浏览器模式小节。
- **约束(行 110)**:「不做移动端 / Web 版 / 云端部署」与浏览器模式(本地 daemon → localhost 浏览器)需澄清,否则误导。

### B3. docs/CONTEXT.md 【高】
- 整文件(1-135)无 daemon / transport / sidecar / 浏览器模式 / ServeDir / httpTransport / GuiMode 术语定义。行 92 唯一 "daemon" 字样是 L1a shell 注释。
- **行 133 vs 行 53 内部矛盾**:行 133「Checklist 为规划中术语,实现决策待定」vs 行 53「B12 已落地(2026-06-19)」。
- **行 115**:「AuditKind 10 类」stale(实际 20+)。〔非 daemon,顺手修〕

### B4. docs/BACKLOG.md 【高】
- **§4 跨设备(行 91-132)**:行 101「形态(暂定方向,留接口)」+ 行 106「前期已留的接口」+ 行 115「VPS daemon 部署文档」。本地 daemon 化已落地,不再「暂定/留接口」。需精确区分:**本地 daemon 化(已做,指向 ROADMAP §1.2)** vs **VPS 跨设备(未做)**。Channel Adapter 协议(行 107)已升级为实际 httpTransport。
- **§3.2(行 57)**:「多 channel 共享 session 状态 → 集中到 agent daemon」措辞为未来式,daemon 已存在。

### B5. docs/README.md(索引)【中】
- **文档结构表(行 12-27)**:无 daemon/transport/remote-access 专题文档条目(已有 `REMOTE-ACCESS-ROADMAP.md` / `REMOTE-ACCESS-RESEARCH.md` / `MANUAL-TEST-P2.md` / `_reviews/REVIEW-remote-access-*`)。
- **行 18**:CONTEXT.md 条目描述「A4 Token 术语」陈旧。〔非 daemon,顺手修〕

---

## 子任务 C:Hacking / 调试文档(HACKING-llm / HACKING-wsl / DEBUG_DB)

### C1. docs/HACKING-llm.md 【高·严重过时】
- **默认模型名(行 13/14/145/198)**:`GLM-4.7` 全文过时,实际 `MiniMax-M2.7`(anthropic.rs:79)。
- **env-var 假设是主路径(行 11-20 / 140-156 / 196-204)**:实际 DB catalog 才是生产路径,env 仅冷启动兜底。文档完全没提优先级,给人「改 env 切 provider」错误印象。
- **checklist 第 1 项(行 144)vs 差异 5(行 425-461)**:base_url 约定内部不一致(Anthropic 裸 host vs OpenAI 含 /v1)。
- **未提 daemon 进程 env 传递(全文缺失)**:sidecar 模式 daemon 继承 GUI env;裸跑继承 shell env。应交叉引用 DEBUG_DB §4 RULE-D-001。

### C2. docs/HACKING-wsl.md 【中】
- **§远程访问 daemon 部署(行 600-703)**:内容准确(e6b7a2f 加的 112 行),但**缺 `scripts/daemon.sh` 用法**(a2bd611 之后新增)。仍写裸 `cargo build` + 手动 `./target/release/... --port`。缺失:`./scripts/daemon.sh start/bg/stop/restart/status/logs` + PID 管理 + 多实例保护警告。用户裸跑会撞数据分裂。
- **通用检查清单(行 558-592)**:未整合 daemon 健康检查(`curl localhost:7456/api/v1/health` / `ss -tlnp | grep 7456` / `./scripts/daemon.sh status`)。

### C3. docs/DEBUG_DB.md 【高】
- **路径表(行 13-19)**:本身准确(16548fd 已对齐)。✅
- **源码引用行号错(行 19)**:「state.rs:212-214」实际路径常量在 state.rs:304;daemon 侧 `resolve_data_dir()` 在 `bin/everlasting-daemon.rs:173-200`。
- **§1 完全没提 daemon 视角 DB 路径(行 9-30)**:三条解析路径(GUI app_data_dir / daemon resolve_data_dir / sidecar `--data-dir` 显式传参)只覆盖 GUI 一条。缺孤儿 DB 坑(commit 16548fd 之前的 `~/.local/share/everlasting/` 无 `dev.` 前缀)。
- **§4 WAL writer(行 161)**:「Tauri 进程持有 WAL writer」。Thin 模式 GUI 不开 DB pool,实际 daemon 才是 WAL writer。
- **§5 reap_orphaned_runs(行 173)**:「app 启动时」歧义,Thin 模式 GUI 不调 load_inner,reap 发生在 daemon 启动。

---

## 子任务 D:决策档案(IMPLEMENTATION.md)

### D1. docs/IMPLEMENTATION.md 【高】
- **§1 自研 agent core 决策·"自研的边界"(行 23-25)**:「✅ 自己写:Tauri IPC 事件协议」「❌ 不自己写:LLM HTTP 协议(用 rig)、SSE 解析(用 rig)、MCP 协议(用 rmcp)」。双错:① Tauri IPC 已非唯一(HTTP/SSE 是核心);② rig/rmcp 早已废弃(TECH.md §2/§3 明确 2026-06-09/10 弃用),§1 没同步。
- **§4 决策日志缺 07-20+ daemon 拆分 ADR(行 29-31 起)**:最新条目 07-10 workflow task.json hardening。daemon 化(项目最大架构变更:为什么拆、为什么 axum、为什么 sidecar 非 systemd、为什么默认 httpTransport)在决策档案零记录。全文 grep "daemon" 仅 3 处历史提及,行 980 还写「B10 飞书触发 daemon 化」。违反本节「每次重大决策都加一条」规则。

---

## 跨文档系统性问题(影响多份文档,子任务需协同)

1. **「daemon 化 = 飞书 B10 触发的未来计划」叙事**:CLAUDE.md(158) / ARCHITECTURE.md(§1 顶部+§4+§5) / DESIGN.md(88) / IMPLEMENTATION.md(980) / ROADMAP.md(140) / BACKLOG.md(101) 一致出现。实际触发是远程访问/浏览器模式且已落地。**统一叙事修正**。
2. **「通信走 Unix socket / WebSocket」技术错误**:CLAUDE.md(158) / ARCHITECTURE.md(§1.1 图+§4:753)。实际 axum HTTP+SSE+ServeDir。**技术事实修正**。
3. **「当前 in-process / 无 daemon」自我否定声明**:ARCHITECTURE.md §1 顶部(10-13)+ §1.2(111)。
4. **transport/sidecar/daemon 目录在所有「结构/栈」文档集体缺席**:CLAUDE.md / STRUCTURE.md / TECH.md。
5. **IMPLEMENTATION.md §4 缺 daemon ADR**:该写没写,违反文档自身规则。

## 非 daemon 顺手修(低优先,各子任务可捎带)
- CONTEXT.md:115 AuditKind「10 类」→ 20+
- CONTEXT.md:133 vs 53 Checklist 状态矛盾
- STRUCTURE.md §13.3 command 数 33 → 79
- docs/README.md:18 CONTEXT.md 条目描述陈旧
- TECH.md §1.4:62「daemon 化换 impl」措辞改现在时
