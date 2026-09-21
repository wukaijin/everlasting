# 两场跨模型群聊审议结论固化（2026-09-21）

> 来源：everlasting 群聊审议引擎（`arch` 双人配方：glm-5.3 架构 + deepseek-flash 后端，MiniMax-M3 主持）。
> - 第一场 session `b3530a54`（--cwd jjh-mono，10min/45msg/655k tok）：议题=沙箱 shell 下 LLM 可靠启动 dev server，产出方案骨架 + 项目侧约定。转录：`~/.local/share/dev.everlasting.app/discussions/2026-09-21-议题：沙箱模式 shell 下，如何保证 LLM 能正常启动开发服务（以 jjh-b3530a54.md`
> - 第二场 session `7c16fdf4`（--cwd everlasting，19min/73msg/2076k tok）：议题=以两次实测为据从 everlasting 代码出发设计工具侧落地。转录：`~/.local/share/dev.everlasting.app/discussions/2026-09-21-议题：以两次实测为据，从 everlasting（本仓库，AI 编码工具本体：d-7c16fdf4.md`
> 结论分 `verified`（代码锚点核实）/ `inferred`（推断）两档，锚点为讨论参与者在仓库中核实过的 file:line。

## 第一场（jjh-mono 视角）：8 条共识

1. **根因（inferred）**：契约错配——dev server 长驻「就绪即成功、永不返回」而沙箱 shell 同步「退出码即信号」。方案主体在工具侧：后台运行 + 进程注册表 + 显式就绪探测做成一等公民，前台同步 shell 禁止起 dev server。
2. **就绪探测 L0-L3 四层状态机（inferred）**，证据绑定本 run：L0 退出短路（确定性退出码立即返回 exit_code+日志尾）→ L1 身份（端口属主∈本 run 进程树，降级到日志特征）→ L2 可达（errno⨝沙箱事件表 join 分类，裸 errno 不直达 LLM）→ L3 三态（no-listener / listener-deps-degraded 附最后 503 明细 / ready）。HTTP 是必要条件而非唯一证据。
3. **jjh-mono 就绪端点语义（verified）**：`/health` 存活（MySQL 必需、Redis 可选只上报，apps/server/src/health/health.controller.ts:25-57）、`/health/ready` 就绪（MySQL+Redis 任一挂即 503）。沙箱下 Redis 可缺 → 依赖降级必须是可诊断中间态而非启动失败。`/health` 请求日志已被排除（logger.module.ts:82-86）轮询无噪音。
4. **MySQL 缺席波形（verified，apps/server/src/database/database.module.ts:18,36 + main.ts:75）**：全程无 listener 的静默（TypeORM 重试窗口，时长未实测）→ bootstrap 失败 `process.exit(1)`，**永不 503**，与沙箱拦 listen 波形同形 → wait_ready 需默认预算含重试窗口、静默期强制读本 run 日志归因，manifest 需「启动预算」「静默期归因特征」字段。
5. **日志源优先级（verified）**：滚动 JSON 文件（LOG_FILE 默认 true，纯 JSON 无 ANSI、含 pid）> stdout 捕获（pino-pretty 带 ANSI 需剥色；启动失败走 standalone logger 格式不同）。JSON 文件跨 run 共享 → 单条「服务已启动」不构成身份证据，须叠加 pid/时间窗。
6. **沙箱豁免是数据不是代码（verified，apps/web/vite.config.ts:24,28）**：bind 集由项目 manifest 声明、工具侧读取执行；vite `host:true`(0.0.0.0) 是 WSL 下既定需求不得改掉；syscall 拒绝必须产出结构化事件；EACCES 无对应沙箱事件时归 OS 层（Windows 保留端口前例 5174）。
7. **项目侧沉淀（verified）**：jjh-mono 新建 `docs/specs/dev-sandbox.md`（内嵌机器可读 manifest，解析失败 fail-closed 全 deny + 显式告警；含进程表 3000/3001、启动序 shared→server→web、就绪端点语义、MySQL 波形、0.0.0.0 需求、日志两源、stale 进程排查）+ AGENTS.md 领域约定索引一行。**此项属 jjh-mono 仓库，非本任务范围。**
8. **不做 dev:all 合并脚本（inferred）**：注册表下两个独立 background run 更干净。

未决 4 条：Windows 杀树语义、nest --watch 重启窗口、TypeORM 重试时长、manifest 承载格式。

## 第二场（everlasting 视角）：10 条结论 + 改动清单 + 安全边界

### 已核实的关键机制事实

- **（verified，sandbox/seccomp.rs:78 + spec §12.3 C 红线）** INET 拦截在 seccomp cBPF 的 `socket(AF_INET/AF_INET6)` 创建点（早于 bind/listen）；BPF 不能解引用 `sockaddr*` → **端口/回环粒度在 seccomp 层物理不可表达**，filter 层唯一自由度=装或不装；「拦 connect 放 socket」不成立（UDP sendto 无需 connect）。
- **（verified，db/migrations/schema.rs:41 + columns.rs:31）** 网络维度从 `sandbox_policy` 枚举拆为正交新列 `sandbox_net`：存量库 CHECK 域内联在建表、扩域需带 incoming FK 的表重建；新增列走 `add_project_column_if_missing` 零重建。类型一次落 `NetPolicy{Block, AllowAll, BindOnly(Vec<u16>)}`；net 列 parse fail-closed(Block)。叙事：「凡承诺 containment 的维度未知态倒向 containment」。
- **（verified，seccomp.rs:8 + spec §4）** 安全中心论点：本地 daemon 控制面 `:7456` 零鉴权，INET seccomp filter 是沙箱进程与控制面之间唯一的墙（AF_UNIX 放行）；任何拆 filter 档位的第一问=7456 是否仍在 agent 可达集。full AllowAll 在现控制面下等同 off 级信任 → 不进第一刀。
- **（verified，spec:310,294）** 第一刀只落 Block/BindOnly 两档；BindOnly 用 **Landlock ABI v4 TCP bind/connect 规则**执法（socket 创建放行、seccomp filter 不装、两执法点互斥建模）；ABI<4 降级 Block 不降 AllowAll；scoped 升为推荐主路径（结构上排除 7456 → 不需给控制面加鉴权）。
- **（verified，spec:139=陷阱6 同类）** pnpm exe 126 头号归因假设：exec 根按 PATH 字面目录构建、未 canonicalize——pnpm 符号链接目标目录（`@pnpm/exe` 在 pnpm global 目录）不在 Landlock exec 面 → execve EACCES。验证法：沙箱内 `command -v pnpm` + `readlink -f` 比对 exec 根清单。
- **（verified，spec:199 + §10/§11）** 非交互 ask 全拒的死因=缺 durable operator 授权出口（P3c 一次批准=逐字节重跑，对长驻进程结构性无效）→ remediation 文案改「收敛到一条 operator 指令然后停」；root 下 yolo 400 保持不动。
- **（verified，background_shell/in_memory.rs:44）** 就绪探测落点：`run_background_shell` 可选 `ready_port` + `ShellEntry.ready ReadyState`。
- **（verified，spec:278 + error-handling §12.3 D）** 字符串特征（stdout_smells_net_block 等）只进 UX 永不作授权依据；listen 拦截 L1 身份判定加服务端不可伪造合取项（summary() net 段证明确实装了 inet block）；UI 对 net=block 的承诺与 `Capability::probe()` 同源。

### 改动清单（第一刀，7 条）

| # | 落点 | 机制 | 为什么 |
|---|---|---|---|
| ① | `db/migrations/schema.rs` + `columns.rs` + `sandbox/policy.rs` | 正交列 `sandbox_net` + `NetPolicy{Block, AllowAll, BindOnly(Vec<u16>)}`，parse fail-closed | 文件/网络维度正交；存量 CHECK 不可扩；加列零重建 |
| ② | `sandbox/mod.rs` `prepare()`/pre_exec | bpf 改 `Option<Vec<sock_filter>>` 三态成对（Block=装 seccomp / BindOnly=不装 seccomp 装 landlock net rules / AllowAll=不装）；`summary()` 加 net 段 + exec 根 canonical 真实路径 | 装/不装是 filter 层唯一自由度；summary 是审计与身份判定的服务端证据 |
| ③ | `sandbox/landlock.rs` | ABI v4 TCP bind/connect 执法点 + capability gate，ABI<4 降 Block | 唯一「只放 listen 不放出网」机制；scoped 结构性排除 7456 |
| ④ | `background_shell/in_memory.rs` + `tools/run_background_shell.rs` | `ready_port` + `ReadyState`（Pending / Ready{port, listener pid, comm, at_ms} / TimedOut）；观察点在 daemon 沙箱外，四铁律：TCP connect-only、loopback 字面量硬编码、随 spawn 一次性绑死不可变、仅自注册 entry；listener 经 `/proc/net/tcp` inode→PID→PGID 归因与 shell entry PGID 比对杀端口碰撞假阳性；超时=TimedOut 绝不折叠成成功；ready_port 只喂探测永不喂授权 | net=Block 时沙箱内无 in-band 自证手段，观察点必须在 daemon |
| ⑤ | daemon 写通道 | bind 快照=operator 确认后存 DB（manifest/LLM 提议仅建议，声明伪造面消解），快照键带 worktree 绝对路径或 commit（防换分支旧快照在新代码生效，先例 preset_key 快照语义）；钳位 `bind_snapshot ∩ {daemon 监听口} = ∅`（快照写入时 daemon 拒绝 + prepare() 防御性减除）；mutation 血统拒授（与 ④ 同一套 /proc 机器） | 快照是唯一 durable 授权出口；钳位挡 7456 走私回派生 connect 集 |
| ⑥ | `tools/shell.rs` + escalation | F3：`classify_block` 加 exit_code==126 + Permission denied 归类 exec 面缺口；remediation 改「收敛到一条 operator 指令然后停」 | 126 是强信号且不踩宁缺勿滥锚；非交互 ask 全拒下死路变一轮收敛 |
| ⑦ | Settings/文案 | net=allow=「保密性保护不再适用」、非 Linux「本平台不生效」与 `Capability::probe()` 同源、UDP/DNS 残留如实写 | 信任语义明示 |

- **connect 派生集**：`{80,443} ∪ bind_snapshot` 写死零配置（端口语义审不出回环/远端维度，追加端口对保密性边际伤害趋零，假控制比没控制糟）；不开第二条快照面。HMR 属 bind 侧入站而非 connect 需求；ssh git(22) 与自建 registry mirror(4873) 走 break-glass(AllowAll) 并跟踪频率，成模式再加旋钮。
- **F1（exec 根 canonicalize）先行、F2（sandbox_extra_exec 白名单，默认空、拒 $HOME/XDG 宽根与 writable 重叠、只收叶子工具目录）挂账、F3 见 ⑥。**

### 安全边界共识（7 条）

(a) INET filter 是沙箱与零鉴权 `:7456` 之间唯一的墙，拆 filter 档位先答「7456 还在不在可达集」；(b) 第一刀只落 Block/BindOnly，AllowAll 不进（解锁前置=capability token）；(c) connect={80,443}∪bind_snapshot 派生，不开第二条快照面；(d) manifest/LLM 端口提议永远只是建议，生效值=operator 快照；(e) 字符串特征只进 UX 永不作授权依据；(f) containment 承诺维度的未知态一律倒向 containment；(g) 血统判定是门槛非硬边界，硬边界只有 scoped 的 connect allowlist。

### 挂账（不进第一刀）

- AllowAll 档（解锁=daemon 内存态 capability token + 「daemon TCP 回路是控制面唯一路径」不变量测试化；文件承载 secret 无效因读面不受控；PGID 血统 mutation 拒授只提高门槛——setsid 可断——不是硬边界）
- `sandbox_extra_exec`（F2，解锁=operator 供应链风险写进 spec）
- capability token 机制细节

### 未决项（6 条）

1. pnpm canonicalize 假设需实测验证（沙箱内 `command -v pnpm` + `readlink -f` 比对 exec 根清单）→ 决定 F1 是否充分、F2 是否必要
2. BindOnly 下 UDP 全通（DNS 隧道外泄残留）的 Settings 信任文案表述
3. capability token 细节 + 「daemon TCP 回路是控制面唯一路径」不变量测试化（现全仓 daemon 下 UnixListener/UnixStream 零命中，需钉成回归锚）
4. Block 档下 LLM 网络自省通道（net_probe 类工具）是否需要
5. exec 根 canonical 全路径进 summary/审计行的格式与隐私权衡（当前只输出计数）
6. AllowAll 最终是否落地（取决于 token + 控制面不变量 + break-glass 频率跟踪）

## 与 spec §12.3 roadmap 的对照

- 第二场主方案 = roadmap **C**（Landlock ABI v4 端口白名单）从「长期」推进为第一刀完整设计；C 的技术红线（seccomp 永远做不了端口级）被独立核实。
- roadmap **A**（`readwrite_net` 第四值）被替代：CHECK 域扩值需表重建（verified），改走正交新列 `sandbox_net`；A 的信任语义（网络=任意外联=数据外发面）转化为 AllowAll 档挂账 + Settings 文案。
- roadmap **B**（prefix-grant 项目级持久化）维持不进；**D**（模型意图判断）维持远期，「模型判断不可作边界」已推广到「文本判断不可作授权依据」（边界共识 e）。
