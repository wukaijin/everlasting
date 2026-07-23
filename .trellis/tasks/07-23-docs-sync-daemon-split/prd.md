# PRD: docs sync — daemon 化文档过时同步

## 背景

2026-07-20 ~ 07-23 完成了项目迄今最大的架构变更:**daemon 化**(remote-access / daemon-split 任务系列,commits `0dbc747` → `3307d93`,~15K 行代码)。将 Rust agent core 从 Tauri GUI 进程中拆出为独立 daemon 进程,通过 axum HTTP + 同源 SSE + ServeDir 通信,前端引入 transport 抽象层(`httpTransport` 默认 / `tauriTransport` 逃生舱),新增 sidecar spawn + 浏览器模式。

但文档集体停留在 07-17,共审计出 **35+ 处过时**,跨 11 份文档。详见 `research/audit-daemon-docs-drift.md`(本任务 source of truth)。

## 目标

把所有描述旧"Tauri 一体化进程"架构的文档同步到 daemon 化后的现状,消除:
1. 技术事实错误(通信方式、进程模型)
2. 状态过时(把已落地写成"未来/暂定/留接口")
3. 结构/栈文档缺 daemon/sidecar/transport 物理落点
4. 决策档案缺 daemon 拆分 ADR
5. 跨文档不一致的"飞书触发 daemon 化"叙事

## 非目标

- 不改任何代码(daemon 功能本身已验证可用)
- 不新增 daemon 功能文档(remote-access 已有 `REMOTE-ACCESS-ROADMAP.md` / `REMOTE-ACCESS-RESEARCH.md` / `MANUAL-TEST-P2.md`,本次只同步既有项目级文档)
- 不重构文档结构,只做内容同步

## 范围拆分(父任务 own 协调,子任务 own 执行)

本任务为**父任务**,own:审计报告、跨子任务一致性、统一叙事修正、最终集成检查。不直接改文档(除非某子任务范围外的零碎项)。

### 子任务 A — 架构核心文档
范围:`CLAUDE.md` + `docs/ARCHITECTURE.md` + `STRUCTURE.md`。见审计报告 §A1/A2/A3。
最核心、最密集(ARCHITECTURE.md 是重灾区)。

### 子任务 B — 路线图 / 状态 / 索引文档
范围:`docs/ROADMAP.md` + `README.md` + `docs/CONTEXT.md` + `docs/BACKLOG.md` + `docs/README.md`。见审计报告 §B1-B5。

### 子任务 C — Hacking / 调试文档
范围:`docs/HACKING-llm.md` + `docs/HACKING-wsl.md` + `docs/DEBUG_DB.md`。见审计报告 §C1/C2/C3。

### 子任务 D — 决策档案
范围:`docs/IMPLEMENTATION.md`(§1 自研边界 + §4 新增 daemon 拆分 ADR)。见审计报告 §D1。
ADR 内容需要从 daemon-split 任务档案 + REMOTE-ACCESS-ROADMAP 提炼决策动机。

## 跨子任务一致性约定(父任务 enforce)

1. **统一叙事**:daemon 化触发源 = 远程访问/浏览器模式需求(非 B10 飞书)。所有子任务统一这个口径。各文档原有「飞书触发」表述一律修正。
2. **统一通信表述**:axum HTTP + 同源 SSE + ServeDir 服务 SPA。不写 Unix socket / Named pipe / WebSocket。
3. **统一进程模型术语**:GUI 进程(Tauri,瘦客户端 / Full 模式) + daemon 进程(`everlasting-daemon` bin,agent core) + sidecar(GUI spawn daemon)。transport 抽象层(httpTransport 默认 / tauriTransport 逃生舱)。
4. **命令速查统一**:`./scripts/daemon.sh {start|bg|stop|restart|rebuild|status|logs}` + `cargo build --bin everlasting-daemon` + `?transport=tauri`。
5. **交叉引用**:各文档更新后应相互链接(CLAUDE.md → REMOTE-ACCESS-ROADMAP;HACKING-wsl daemon 章节 → scripts/daemon.sh;DEBUG_DB → daemon 视角路径)。

## 验收标准(父任务)

| 条件 | 要求 |
|------|------|
| 4 个子任务全部 archived | ✅ |
| 审计报告 35+ 处过时全部修正或注明"保留原因" | ✅ |
| 跨文档「飞书触发 daemon 化」叙事清零 | ✅ |
| 跨文档「Unix socket/WebSocket 通信」错误清零 | ✅ |
| grep `rig-core\|用 rig\|用 rmcp` 在 IMPLEMENTATION.md §1 清零(历史 ADR 保留) | ✅ |
| `IMPLEMENTATION.md §4` 有 07-20+ daemon 拆分 ADR 条目 | ✅ |
| ROADMAP §1.2 有 daemon 化 epic 条目 + commit 引用 | ✅ |
| CLAUDE.md / STRUCTURE.md 目录树含 daemon/sidecar/transport/browser | ✅ |
| 用户/新 session 阅读任一文档不会被导入「in-process」错误心智 | ✅(主观,父任务最终复查) |

## 风险

- **R1 文档体量大**:IMPLEMENTATION.md 223KB、ARCHITECTURE.md 45KB、ROADMAP.md 56KB。子任务需用 grep 精准定位,避免误改历史 ADR。
- **R2 历史决策不可删**:IMPLEMENTATION.md §4 是「只追加不删除」档案,改 §1 边界要保留历史决策的「当时这么想」语义,只补「现状已演进」注记。
- **R3 跨子任务重复内容**:多份文档都有命令速查/进程模型,需保持措辞一致(见上"统一叙事"),避免 A 改完 B 又用旧措辞。
