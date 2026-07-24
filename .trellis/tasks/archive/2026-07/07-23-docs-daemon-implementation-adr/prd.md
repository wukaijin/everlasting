# PRD: docs sync D — 决策档案 (IMPLEMENTATION.md)

> 父任务:`../07-23-docs-sync-daemon-split/`
> 审计依据:`../07-23-docs-sync-daemon-split/research/audit-daemon-docs-drift.md` §D1
> 跨任务一致性约定:见父 prd.md

## 目标

补齐决策档案在 daemon 化这件事上的两处缺漏:① §1 自研边界过时表述;② §4 决策日志缺 07-20+ daemon 拆分 ADR。IMPLEMENTATION.md §4 是「只追加不删除、ADR 性质不可再生」档案,daemon 化作为项目最大架构变更却无任何 ADR,违反文档自身规则。

## 范围

### D1. `docs/IMPLEMENTATION.md`

#### §1 自研 agent core 决策 · "自研的边界"(行 23-25)
现状:
```
- ✅ 自己写:Tauri IPC 事件协议、session 持久化、worktree 管理
- ❌ 不自己写:LLM HTTP 协议(用 rig)、SSE 解析(用 rig)、MCP 协议(用 rmcp)
```
问题:
1. 「Tauri IPC 事件协议」已非唯一 —— daemon 化后核心是 HTTP/SSE 协议(axum)
2. 「用 rig」「用 rmcp」早已废弃(TECH.md §2/§3 明确 rig-core 2026-06-09 弃用、rmcp 2026-06-10 移除),§1 没同步,还在教读者「用 rig 做 LLM HTTP」

修法:**保留历史决策的「当时这么想」语义**(R2 风险),只加「现状已演进」注记:
- 「Tauri IPC 事件协议」补「+ HTTP/SSE(daemon 化后,见 §4 [日期] ADR)」
- rig/rmcp 行补「(2026-06 弃用,改自研;见 §4 对应 ADR)」

#### §4 决策日志 — 新增 07-20+ daemon 拆分 ADR
按 §4 既有格式(时间倒序、ADR 风格、含动机/方案/权衡),新增条目。内容来源:
- `.trellis/tasks/archive/2026-07/07-20-remote-access-transport-abstraction/`(transport 抽象)
- `.trellis/tasks/07-20-remote-access-daemon-split/`(daemon 拆分主体,design.md/implement.md)
- `docs/REMOTE-ACCESS-ROADMAP.md`(Phase 编排)
- `docs/REMOTE-ACCESS-RESEARCH.md`(调研)

ADR 应覆盖的决策点(从上述档案提炼):
1. **为什么拆 daemon**:远程访问/浏览器模式需求;agent core 与 GUI 解耦
2. **为什么 axum**(而非 actix / 裸 hyper):选型理由
3. **为什么 sidecar spawn**(而非独立 systemd / 用户手动启 daemon):Q0 决策 —— GUI spawn daemon,默认 httpTransport
4. **为什么默认 httpTransport**(而非保留 Tauri IPC 默认):逃生舱 `?transport=tauri` + Full 模式
5. **为什么 ServeDir 同源**(而非单独 nginx / 两端口):浏览器模式一条龙
6. **为什么 handler 双暴露(IPC + HTTP)**:79 个 `#[tauri::command]` 镜像为 REST,代码复用
7. **DB 路径对齐**:resolve_data_dir 对齐 Tauri app_data_dir identifier(commit 16548fd)

同时修正既有 §4 中 daemon 相关历史提及:
- 行 980「B10 飞书触发 daemon 化」→ 补「(实际由远程访问触发,已于 2026-07 落地,见 [新 ADR])」
- 其他 daemon 历史提及(行 399/1048)只加演进注记,不删

## 验收标准

- [ ] §1 自研边界:Tauri IPC 行补 HTTP/SSE 演进注记;rig/rmcp 行补弃用注记(历史语义保留)
- [ ] §1 grep「用 rig」「用 rmcp」无「裸陈述为现状」的用法(历史决策上下文可保留,但要有弃用注记)
- [ ] §4 新增 07-20+ daemon 拆分 ADR 条目,覆盖 7 个决策点
- [ ] §4 行 980「飞书触发」补演进注记
- [ ] 新 ADR 格式与 §4 既有条目一致(日期、标题、动机、方案、权衡、commit 引用)
- [ ] 不删除任何既有 ADR 条目(只追加 / 加注记)

## 风险

- **R2 历史决策不可删**:§1「用 rig」是当时真实决策,不能直接改成「自研」—— 那会篡改历史。必须保留原文 + 加「现状已演进」注记。
- ADR 决策动机要从多份档案提炼,需先读 transport-abstraction + daemon-split task 的 design.md/prd.md 确认实际决策理由(不能臆造)。
- IMPLEMENTATION.md 223KB,§4 条目位置要精准(grep 定位最新条目日期),避免插错破坏倒序。
