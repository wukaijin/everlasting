# PRD — 沙箱网络策略 NetPolicy：BindOnly(Landlock v4) 档 + dev server 就绪探测

## Goal

让「沙箱模式的 shell 下 LLM 可靠启动 dev server」从**不可能**（INET socket 创建被 seccomp 全拦，实测 jjh-mono Vite `listen EPERM` 连 127.0.0.1 都拒）变为**受控可行**：新增项目级网络策略维度 `sandbox_net`，提供 `BindOnly(ports)` 档——只放行声明端口的 TCP bind（Landlock ABI v4 执法），出网仍按白名单收紧；并为后台 shell 提供工具侧就绪探测（`ready_port`），使 LLM 能拿到确定性就绪/失败信号而非自行多轮诊断。

用户价值：dev server 是前端/全栈项目最高频的开发动作之一（实测 session 中模型被迫花多轮做 node/python 双栈对照实验才定位到环境限制）；当前沙箱对该场景要么误伤（全拦）、要么放行面过宽（`off` = 文件+网络全开）。BindOnly 是中间档：文件写入仍受控、监听按声明放行、外联仍受限。

## Background / Confirmed Facts

证据全文见 `research/deliberations-2026-09-21.md`（两场审议）与 `research/live-probe-2026-09-21.md`（实测），要点：

- **根因**（spec §12.3 C 红线，独立核实）：INET 拦截在 seccomp `socket()` 创建点，早于 bind/listen；BPF 不能解引用 `sockaddr*` → 端口/回环粒度在 seccomp 层物理不可表达。唯一「只放 listen 不放出网」的机制 = Landlock ABI v4 TCP bind/connect 规则（内核 ≥6.7）。
- **现状语义**：`projects.sandbox_policy ∈ {off, readwrite, readonly}`（CHECK 内联建表，扩域需带 incoming FK 的表重建）；沙箱 = Landlock 文件规则 + seccomp INET 全拦；`Capability::probe()` 测 ABI ≥1。
- **安全中心约束**：daemon 控制面 `:7456` 零鉴权，INET filter 是沙箱与控制面之间唯一的墙——BindOnly 的 connect 白名单 `{80,443}∪bind快照` 结构性把 7456 排除在 agent 可达集外。
- **实测**（jjh-mono，evl chat session `fb7420f1`）：listen 全拦致 dev server 必败；pnpm 独立二进制（`@pnpm/exe`）exec 被拦 exit 126（归因假设：exec 根按 PATH 字面构建未 canonicalize，待验证）；失败信号链路已通（EPERM/exit/stderr 可见、无卡死，29bfe1e2 修复获验证）。
- **本机（WSL2 6.6.114）内核未启用 Landlock**（LSM 无、probe EINVAL）→ BindOnly 在本机不可 live 验证。
- spec §12.3 roadmap：本任务 = 方案 C 落地；A（readwrite_net 第四值）被正交新列替代并转化为 AllowAll 挂账；B/D 维持不进。

## Requirements

- **R1 网络策略维度**：`projects` 新增正交列 `sandbox_net`（`add_project_column_if_missing` 零重建），类型 `NetPolicy{Block, AllowAll, BindOnly(Vec<u16>)}`；缺省 Block（=现状语义）；列值 parse 失败 fail-closed 落 Block。AllowAll 枚举值本期**不提供写入口**（挂账：capability token 前置）。
- **R2 执法点互斥三态**：Block=装现行 seccomp INET filter（行为逐字节不变）；BindOnly=不装 seccomp、装 Landlock ABI v4 TCP 规则（BIND_TCP=bind 快照端口集，CONNECT_TCP=派生集 `{80,443}∪bind快照`）；AllowAll=都不装（仅类型/parse 支持，无配置入口）。两执法点互斥由类型/构造保证，不允许同时装。
- **R3 能力门与降级**：`Capability::probe()` 扩展报告 Landlock ABI ≥4 + 网络规则能力；net=BindOnly 而 probe 不支持 → 降级 Block（warn 日志 + summary 如实记录），绝不降 AllowAll。
- **R4 bind 快照（唯一授权真源）**：生效端口集 = operator 确认后存 DB 的快照；快照键带 worktree 绝对路径（或 commit），防换分支旧快照在新代码生效；LLM/manifest 端口提议仅为建议值进确认流，不直接生效。钳位：快照 ∩ {daemon 监听口} = ∅（写入时拒绝 + prepare() 防御性减除）。
- **R5 就绪探测**：`run_background_shell` 新增可选 `ready_port` 参数 + `ShellEntry.ready: ReadyState`（Pending / Ready{port, listener_pid, comm, at_ms} / TimedOut）。观察点在 daemon 侧沙箱外，遵守四铁律：TCP connect-only、loopback 字面量硬编码、随 spawn 一次性绑死不可变、仅自注册 entry；listener 经 `/proc/net/tcp` inode→PID→PGID 归因与 entry PGID 比对（端口碰撞假阳性剔除）；超时=TimedOut 绝不折叠成成功；ready_port 只喂探测、永不参与授权判定。
- **R6 pnpm/exec 拦截修复（F1）**：exec 根构建 canonicalize（PATH 目录解析符号链接后的真实路径入 exec 面）；`summary()` 审计加 exec 根 canonical 路径（格式见 research 未决项 5）。前置：实测验证归因假设（implement 第 0 步），假设不成立则重审 F1/F2 取舍。
- **R7 拦截识别与 remediation（F3）**：`classify_block` 增加 exit_code==126 + Permission denied 的 exec 面缺口归类（不放松现有宁缺勿滥锚——stdout 裸串仍不认）；非交互升级/remediation 文案改「收敛到一条 operator 指令然后停」，替代当前「逐字节重跑」的误导（对长驻进程结构性无效）。
- **R8 可观测与文案**：`SandboxSpec::summary()` 加 net 段（档位 + 生效端口集 + 降级事实）；Settings 面新增 net 档位与快照确认 UI，文案三处硬要求：net 放行≠保密性（数据可经放行端口外发）、UDP/DNS 在 BindOnly 下仍全通（如实写）、平台不支持时显示「本平台不生效」且与 `Capability::probe()` 同源。
- **R9 错误契约衔接**：listen 拦截事件的归因（L1 身份判定）增加服务端不可伪造合取项——summary() net 段证明确实装了 INET block，才允许 classify/guidance 判定为沙箱拦截；字符串特征只进 UX，永不作授权依据。

## Acceptance Criteria

- [ ] **AC1（Block 不变量）**：net 缺省/parse 失败/probe 不支持三路全落 Block；Block 档行为与改动前逐字节一致（seccomp filter 装载、guidance、审计），现有 `tests_sandbox` 全绿零语义改动。
- [ ] **AC2（BindOnly 单测）**：NetPolicy parse/存储 roundtrip（含非法值 fail-closed）；三态互斥构造（Block 与 BindOnly 不同时装）；connect 派生集计算（含钳位减除 daemon 监听口）；快照键含 worktree 路径。
- [ ] **AC3（真内核集成，支持内核上跑、否则大声 SKIP）**：BindOnly 档下声明端口 bind 成功、未声明端口 bind 被拒、connect 白名单外被拒、7456 不可达；Block 档 INET socket 仍 EPERM。本机（无 Landlock）SKIP 语义正确 + 降级路径单测覆盖。
- [ ] **AC4（就绪探测）**：ready_port 起真实监听 → Ready{listener pid 归因正确}；端口被他人占用（PGID 不匹配）→ 不误报 Ready；超时 → TimedOut；四铁律各有回归锚（loopback 硬编码/不可变/仅自注册）。
- [ ] **AC5（F1）**：归因假设实测记录进 research；canonicalize 落地后沙箱内 `pnpm --version` 可执行（或如实记录假设不成立与改道）。exec 根 canonical 路径出现在 summary。
- [ ] **AC6（F3/remediation）**：exit 126+Permission denied 命中新归类；现有宁缺勿滥锚（stdout 裸 Operation not permitted/Permission denied 不认网络拦）不回退；remediation 新文案有测试锚。
- [ ] **AC7（可观测）**：summary() net 段三态各有审计锚；Settings 档位/快照确认流 roundtrip；文案三硬要求在 UI 测试可断言。
- [ ] **AC8（回归）**：`cargo test -p everlasting --lib` 全绿；`scripts/turn-smoke.sh --sandbox-probe` 在本机跑通（Block 语义不回归；BindOnly 部分因内核 SKIP 只出 WARN）。
- **Live BindOnly 验证（blocked-on-kernel）**：在启用 Landlock 的内核上以 jjh-mono web（Vite:3001）做端到端冒烟——本机不可验，作为已知限制记录在任务结论，不作为本任务验收阻塞项。

## Out of Scope

- AllowAll 档的配置入口与落地（挂账：daemon 内存态 capability token + 「daemon TCP 回路是控制面唯一路径」不变量测试化先行）。
- `sandbox_extra_exec` 用户白名单（F2，挂账：operator 供应链风险先写进 spec）。
- prefix-grant 项目级持久化（roadmap B）——2026-09-21 用户裁定**已立并行任务 `09-21-durable-prefix-grant`**（无 Landlock 内核用户的授权面解法，与本任务正交互补；本任务 F3/remediation 文案是其触发入口）；模型意图判断（roadmap D）。
- jjh-mono 侧 `docs/specs/dev-sandbox.md` 项目约定（第一场讨论产出，属 jjh-mono 仓库）。
- daemon 控制面鉴权、UDP/DNS 维度收紧、Block 档 LLM 网络自省工具、Windows/macOS 网络沙箱。

## Risks / Deferred

- **本机内核无 Landlock**：BindOnly live 行为不可本机验证（AC3 的 SKIP 分支 + AC8 受限）；升级 WSL2 内核是环境侧动作，不属本任务。
- pnpm 126 归因假设未验证（implement 第 0 步验证，结果决定 F1 充分性）。
- 快照确认流涉及 GUI/daemon 写通道新面，是本任务改动面最大的单点（改动⑤），实现时按 implement.md 排序后置、可独立回滚。
- 讨论遗留未决 6 条见 research/deliberations §未决项；仅「exec 根 canonical 路径审计格式」（隐 private 路径权衡）需实现期定，其余不阻塞。
