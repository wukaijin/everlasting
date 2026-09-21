# 实测记录（2026-09-21，evl chat 端到端 + 本机环境探针）

## 1. evl chat 沙箱链路实测（被测项目 jjh-mono，/usr/local/code/jjh/jjh-mono）

### 尝试 0：yolo 档
`evl chat … --mode yolo` → daemon 拒绝：`POST /api/v1/permissions/set_session_mode → HTTP 400: Cannot enable Yolo as root`。
**root 下 yolo 被安全闸硬禁（预期防护，未绕过）**；测试降档 edit。

### 尝试 1（edit 档，session `fb7420f1`，约 2 分钟，stop_reason end_turn，permission_denials=1）

对端 LLM（everlasting agent loop）自主完成的三次尝试 + 对照实验：

| 尝试 | 命令 | 结果 |
|---|---|---|
| 1 | `pnpm --filter @jjh/web dev`（run_background_shell，项目根） | exit 126——pnpm 独立二进制 exec 被拒：`/root/.local/share/pnpm/bin/pnpm: 39: exec: /root/.local/share/pnpm/bin/../global/v11/29228-.../@pnpm/exe/pnpm: Permission denied`；文件权限位 `-rwxr-xr-x`（uid 501）正常、当前 root——**沙箱策略拦截非 fs 权限**。同步 shell `pnpm --version` 同样复现（非后台特有）。node（nvm 路径）与 esbuild 原生二进制 exec 正常。另有系统提示「后台 shell 沙箱写拦截升级请求未获批准」（= permission_denials 1 的来源） |
| 2 | 绕过 pnpm：`cd apps/web && node node_modules/vite/bin/vite.js` | exit 1——`Error: listen EPERM: operation not permitted 0.0.0.0:3001`（`{code:'EPERM', errno:-1, syscall:'listen'}`；vite.config `host:true` 绑 0.0.0.0、端口 3001） |
| 3 | CLI 覆盖 `--host 127.0.0.1 --port 3001 --strictPort`（零落盘） | exit 1——`127.0.0.1` 同样 EPERM → listen() 被全局拦 |
| 对照 | node 裸 socket `127.0.0.1:4321` / `python3 -m http.server 4322 --bind 127.0.0.1` | 双双 EPERM——**沙箱封的是 socket()/listen 系统调用本身，与 vite/node/端口/地址无关** |

**失败信号链路已通**（29bfe1e2 修复行为获真实验证）：EPERM 报错、exit code、stderr 全部可见，LLM 正确归因（最终报告明确「环境限制，非项目问题」），无卡死、无误判成功。LLM 自主行为良好：自选后台 shell 起长驻进程、CLI 参数绕 0.0.0.0 不改文件、识别 pgrep `sh -c` 自匹配假阳性、三后台 shell 全退 + `ss` 确认端口释放。

**结论：当前沙箱默认策略下 dev server 无法启动，直接原因=INET socket 创建被 seccomp 全拦（连 127.0.0.1 回环都拦）。**

### jjh-mono 相关事实（第一场讨论 verified + 实测）

- web = Vite，`vite.config.ts` `host:true`（0.0.0.0）、端口 **3001**；server = NestJS start:dev，端口 **3000**；开发需两长驻进程。
- `/health`（MySQL 必需、Redis 可选只上报）/ `/health/ready`（MySQL+Redis 任一挂 503）；MySQL 缺席=静默重试→`process.exit(1)` 永不 503。
- 日志：`LOG_FILE` 滚动 JSON（默认 true、含 pid）> stdout（pino-pretty ANSI）。

## 2. 本机（WSL2）环境探针

- 内核 `6.6.114.1-microsoft-standard-WSL2`；`/sys/kernel/security/lsm` **无 landlock**；`landlock_create_ruleset(444,0,VERSION)` → **EINVAL(22)**。
  → **[2026-09-21 实施期修正：本条结论不成立，实测 ABI v3 可用，见 §5]** Landlock TCP bind/connect 规则需 ABI v4（内核 ≥6.7 且 CONFIG_SECURITY_LANDLOCK=y 入 LSM 链）。
  → **本任务 BindOnly 档在本机不可 live 验证**，只能单测 + 降级路径 + probe 报告；live 验证 blocked-on-kernel（换启用 Landlock 的 WSL2 内核或 Linux CI 机）。
  → **解锁路径（2026-09-21 查证，按成本序）**：① Windows 侧 `wsl --update` + `wsl --shutdown`——新版官方 WSL2 内核已启用 Landlock（WSL kernel release notes；此前 stock 内核 not set 见 WSL issue #9131），版本 ≥6.7 即 ABI ≥4 满足需求；② 兜底自编内核（microsoft/WSL2-Linux-Kernel 开 CONFIG_SECURITY_LANDLOCK=y，.wslconfig kernel= 指向 bzImage）；③ 外部 6.7+ Linux（Ubuntu 24.04 内核 6.8 默认启用）/ CI。更新后复查：`uname -r` + `dmesg | grep -i landlock`（"Up and running"）+ python syscall 探针返回 ≥4。代码侧 probe 不支持 = 大声 SKIP + WARN，现有环境跑测试不红。
- 端口现状：3000/3001 被用户沙箱外手动起的 node dev server 占用（pid 43969/43849）；Redis 6379 在跑；MySQL 3306 未跑。
  → 即使沙箱放行 bind，沙箱内再起 dev server 也会 EADDRINUSE（Landlock 只管授权不管占用）。
- Windows TCP 排除段（netsh.exe 实测，动态、每次重启重划）：3364-3463、3464-3563、3854-3953、3954-4053、6007-6106、6107-6206、50000-50059。**5174 当前不在段内**；WSL2 内 517x 无监听。
  → 「Windows 保留端口前例 5174」是时运非固有属性；此类失败归 OS 层（讨论共识：errno⨝沙箱事件表 join，EACCES 无沙箱事件→OS 层）。

## 3. 通用性推演（回答"其他项目适用吗"，分析非实测）

- **纯监听型**（npm/yarn/pnpm 前端 Vite/webpack/Next dev、Java Spring Boot 监听、数据库进程本身）→ BindOnly 机制普适：Landlock 端口级 bind 放行，语言无关；registry https 443 在 connect 派生集；postgres 默认 Unix socket 直接受益（AF_UNIX 放行）；数据目录写走既有 fs 维度。
- **connect 依赖型**（任何连本地 DB/中间件的后端：3306/6379/9092）→ **第一刀共同缺口**：connect 派生集=`{80,443}∪bind 快照`，不含这些端口。出路：operator 把端口写进快照（机制推演：快照即 connect 派生源）、break-glass（挂账）、或 L3 降级态。
- **动态换端口的 dev server**（Vite 5173 被占自动跳 5174+）与固定快照摩擦：跳出的端口不在快照→bind 被拒（有沙箱事件、归因正确）。干净解法=项目固定端口+`--strictPort`（jjh-mono 固定 3001 即好实践）或快照预声明候选端口。
- **Java 特有**：JDK 多经符号链接安装→同吃 F1 canonicalize 修复；Gradle/Maven daemon 自起本地 socket/锁→快照留端口或容忍降级；JVM⨝Landlock 兼容性未实测（推测）。
- **平台边界**：Landlock Linux 独有；macOS/Windows 上 BindOnly 整体不适用（改动 ⑦「本平台不生效」文案）。

## 4. F0 归因验证（2026-09-21 实施第 0 步，沙箱内单测探针）

**方法**：`tests_sandbox.rs` 临时探针（跑完即删）——用生产构造器 `policy::build_spec`（ReadWrite 档、真实 PATH）构建 spec，`prepare/apply` 后沙箱内跑 `sh -c 'command -v pnpm; readlink -f "$(command -v pnpm)"; pnpm --version'`。

**结果 1（正向，复现 evl 实测）**：exit **126**，stderr 与 evl session fb7420f1 逐字一致：
`/root/.local/share/pnpm/bin/pnpm: 39: exec: /root/.local/share/pnpm/bin/../global/v11/29228-1a0063bbcf6-4c44d6e14090b541/node_modules/@pnpm/exe/pnpm: Permission denied`

**结果 2（exec 面清单比对）**：生产 exec 根 = PATH 目录全集（含 `/root/.local/share/pnpm/bin`）+ /lib /lib64 /usr/lib /dev /tmp + worktree + spill + linuxbrew。exec 目标 `/root/.local/share/pnpm/global/v11/29228-.../node_modules/@pnpm/exe/pnpm` —— `global/` 与 PATH 里的 `bin/` **平级**，不在任何 exec 根之下。目标文件本体是 ELF（非脚本），经中间目录符号链接 canonicalize 后落在 pnpm store（`/root/.local/share/pnpm/store/v11/links/@pnpm/exe/directory/<hash>/...`）。

**结果 3（反向验证）**：同 spec 的 exec 面追加 `/root/.local/share/pnpm/global` + `/root/.local/share/pnpm/store` 两根后，沙箱内 `pnpm --version` → **exit 0，stdout 11.21.0**。证明 exec 面缺口是唯一阻塞（seccomp/Landlock 其余面不干扰 pnpm 运行）。

**F0 结论：原归因假设（机制层）不成立**。
- 假设说的是「exec 根按 PATH 字面构建未 canonicalize → 符号链接目标目录不在面内」。实测：pnpm wrapper 是**普通脚本非符号链接**，其 exec 目标根本不在任何 PATH 目录之下（global/ 与 bin/ 平级），canonicalize PATH 目录前后该目标都不进面。
- F1（exec 根 canonicalize）对 pnpm 126 场景**不充分**——按 implement.md Step 5 预案：任务内仍做 F1（对「PATH 目录自身是符号链接」场景是真实防御 + summary 审计 canonical 路径），但明确记录 pnpm 解法在 F2（`sandbox_extra_exec` 白名单收 `~/.local/share/pnpm/{global,store}` 类叶子目录），F2 提前进入挂账评审。
- 连带修正 evl 实测的根因表述：jjh-mono「pnpm dev server 起不来」是**双重独立缺口**——① exec 面 126（本节，F2 挂账解）② listen EPERM（Landlock net 缺席，本任务 BindOnly 解）。

## 5. 环境修正（2026-09-21 实施期复核，推翻 §2 一处结论）

- **§2 「本机内核完全未启用 Landlock」结论有误**。复核：`landlock_create_ruleset(NULL, 0, VERSION)` syscall 探针实返 **3** = **Landlock ABI v3 可用**（`/sys/kernel/security/lsm` 不存在只是 securityfs 未挂载，不构成无 Landlock 的证据；§2 的 python 探针传参有误得出 EINVAL）。这也与 §1 evl 实测自洽——若真无 Landlock，`Capability::probe().ok()` 为假、沙箱整体 fail-open，listen EPERM / exec 126 都不会发生。
- 推论不变的部分：BindOnly 需 **ABI ≥4**（内核 ≥6.7 的 TCP bind/connect 规则），本机 6.6 = ABI v3 → `net_rules_supported()` 为假 → BindOnly 降级 Block（WARN），**live BindOnly 验证仍 blocked-on-kernel**（解锁路径 §2 所列 `wsl --update` 等，需内核 ≥6.7）。
- 推论变化的部分：本机沙箱**文件面是活跃的**（landlock=abi3 + seccomp 双真），故 AC1 Block 档、F1 canonicalize、ready_port（Block 档下的行为面）都可本机 live 验证；AC3 真内核 BindOnly 用例本机 SKIP（大声）+ 降级路径单测覆盖。

## 6. 遗留环境事实（与任务验收相关）

- 本机 root：yolo 禁用是 daemon 侧设计（`Cannot enable Yolo as root`），保持不动（讨论结论）。
- pnpm exec 126 归因假设（exec 根未 canonicalize，符号链接目标目录不在 exec 面）**尚未实测验证**——验证法：沙箱内 `command -v pnpm` + `readlink -f` 比对 exec 根清单；任务 implement 第 0 步。
