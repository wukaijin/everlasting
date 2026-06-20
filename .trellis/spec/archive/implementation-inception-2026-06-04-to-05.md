<!-- ARCHIVED 2026-06-21: 只读历史,不再追写。源: docs/IMPLEMENTATION.md §4 (2026-06-04/05 段) -->

# Implementation — 2026-06-04/05 项目启动期决策 (ARCHIVED)

> **Status**: ARCHIVED (2026-06-21). 项目启动期决策。已迁出 `docs/IMPLEMENTATION.md §4`(2026-06-04/05 段),降低主决策日志的认知噪音。
>
> **Cross-references**:
> - Main decision log: [../../../docs/IMPLEMENTATION.md](../../../docs/IMPLEMENTATION.md)

---

### 2026-06-04 — 项目正式启动

- **决策**:自己写 agent core,不用 Anthropic Agent SDK 包装
  - **原因**:学习目标 + 控制粒度 + 厂商解耦
- **决策**:WSL 优先,Windows 不主动适配
  - **原因**:个人使用场景就是 WSL;Tauri 在 WSL 跑得通
- **决策**:每个 session 一个 git worktree
  - **原因**:多 session 并行 / 互不污染 / 切换瞬时
- **决策**:agent core 用 rig-core(后期切),前期手写
  - **原因**:先学再依赖,避免黑盒
- **决策**:MCP 只外暴露,内部通信不绕
  - **原因**:内部直接调 Rust 函数最快;外部用 MCP 生态杠杆
- **决策**:SQLite 是唯一存储
  - **原因**:单文件、零运维、FTS5 搜索、sqlx 异步
- **决策**:不做 workflow 编排(到 v2 再说)
  - **原因**:单 agent 体验没做透之前,workflow 是空中楼阁
- **决策**:不做团队协作、不做云端部署、不做移动端
  - **原因**:个人工具,这些是另一个产品的事
- **决策**:Skill / Memory / Role 共用 frontmatter loader
  - **原因**:三者载体都是 Markdown + YAML,加载时机不同;统一 loader 省重复代码
- **决策**:Agent Daemon 化(agent core 从 Tauri 进程拆出为独立 daemon)
  - **原因**:为多 channel 接入铺路(飞书不能依赖 GUI);GUI 重启不打断长跑任务;长跑任务稳定性。详见 [ARCHITECTURE.md §4](./ARCHITECTURE.md#4-决策agent-daemon-化为多-channel-接入铺路)
- **决策**:生成式 UI 走约束式(LLM 输出 JSON,前端按 type 渲染),默认关闭
  - **原因**:自由式(LLM 生成 HTML)沙箱难做、调试难;约束式够覆盖 80% 用例。详见 [BACKLOG.md 远期（v3+）段](./BACKLOG.md#远期v3暂不评估)
- **决策**:Channel Adapter 抽象(`Channel` trait,3 个实现:TauriGui / Feishu / Cli)
  - **原因**:多入口统一接口;新增 channel 不用改 agent core;测试友好。详见 [ARCHITECTURE.md §5](./ARCHITECTURE.md#5-决策channel-adapter-抽象为多入口铺路)
- **决策**:候选功能方向锁定(7 个),但暂不排优先级
  - **原因**:先把方向沉淀下来,实施前再按"价值/成本/依赖"评估。详见 [BACKLOG.md](./BACKLOG.md) 全章
  - 方向:输入层(图/@ /command) / Skill / 多层 Memory / 多角色+多模式+编排 / 生成式 UI / 飞书 / 云端同步

### 2026-06-04 — 方案 C:VPS 自托管 daemon(远期留接口)

- **决策**:v2 之后考虑在 VPS 上跑自托管 daemon,做多设备同步;**前期不展开,只留接口**
  - **原因**:个人用 + 学习目的,前期过度设计分布式拓扑是负担;但 Channel Adapter 协议要设计成 network-ready,worktree 路径要跨机器一致,否则后期改不动
- **决策**:Channel Adapter 协议必须走明文 JSON,载体无关(Unix socket / HTTPS / WSS 都能承载)
  - **原因**:不锁传输层,后期接 VPS 客户端不用改 agent core
- **决策**:worktree 路径统一用 XDG 标准 `~/.local/share/everlasting/worktrees/<project_hash>/<session_id>`
  - **原因**:跨机器接续时,worktree 路径必须可预测;XDG 标准是 Linux 共识
- **前期动作**(本决策已落地):
  - ARCHITECTURE §3 改 worktree 路径
  - ARCHITECTURE §5 Channel trait 注明 network-ready 约束
  - BACKLOG §4 列 v2 跨设备候选
- **后期展开**(v2 再说):
  - 多设备接续、配置/状态/session 列表同步、显式"工作树迁移"流程
  - 接续前置条件:必须 push 过 + 目标机器不能在跑 LLM
  - 详见 [BACKLOG §4 跨设备（v2 候选）](./BACKLOG.md#4-跨设备v2-候选)

### 2026-06-05 — 步骤 3b-1 follow-up 沉淀 (FU-1/2/3 项目决策)

