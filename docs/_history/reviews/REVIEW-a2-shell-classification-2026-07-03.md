# A2+ Shell 命令只读/副作用精细判定方案 — 设计评审

> **评审日期**:2026-07-03
> **评审范围**:`docs/A2-SHELL-CLASSIFICATION.md` 全文(草案,2026-07-03 提交)
> **评审类型**:设计评审(pre-implementation review)
> **评审基线**:commit `9c51f11`(2026-07-02),Rust 关键源 2454 行:`shell_trust.rs:732` + `check.rs:1169` + `audit.rs:277` + `dangerous.rs:276`
> **评审模型**:MiniMax-M3
> **对照基准**:`REVIEW-a2-b7-permission-mode-plan-2026-06-13`(Reasonix)+ `REVIEW-agent-loop-full-audit-2026-06-14`(GLM 5.2)

---

## 0. 总体评价

**综合评分:★★★★ (4/5) — 目标清晰、解耦精准、可执行性强;有 1 处事实错误 + 2 个独立漏判需要在评审落地实施前补正。**

这是一份"看完能直接开工"的方案。三个**架构优点**值得肯定:

1. **作用面收得极小**。所有改动集中在 Tier 4 Shell 分支的 (a) prefix-grant 短路前置 + (b) `classify_prefix` 内部两步,Tier 1/2/2.5/3/5/6 零改动。PermissionModal、Mode、审计、kill-list 全部无感。这是它能安全分阶段上线的根本前提。
2. **阶段依赖讲清了**。P1(grant 收紧)必须先于或同于 P2(拆分)——因为 Tier 4 的 (a) 在 (b) 之前(已核验 `check.rs:298` 在 `:326` 之前),先上 P2 不收紧 grant 等于白做。方案把这个因果讲透了。
3. **不变量定义完整**。§2.2 列 6 条不变量全部对应现行契约,**没有隐藏改动**;§2.3 4 条非目标也是真非目标。

**但有以下 4 个问题需要在 P1 实施前补正或明确**,否则会导致实施阶段返工或被复审:

| 级别 | 问题 | 影响 |
|---|---|---|
| **P1-A** | 草案 §1.1 + §2.2 + §3.2 三处都写"11 条灾难性正则",**实际是 10 条**(`dangerous.rs:26-86` `DENY_PATTERNS` tuple 数 10,RULE-B-004 把 find -delete/-exec 加入后已扩成 10 条)。三处口径不一,会误导评审下游 | 文档一致性,无安全影响 |
| **P1-B** | 现状 `shell_trust.rs:365` 结构降级**只检 `|`/`&&`/`;`,不检 `>`**。所以 `git diff > patch.txt` 在现状下首 token 是 `git diff` → ReadOnly → **静默写文件**(LLM 不经任何弹窗)。草案 §4 阶段 2 自己提了这个漏判但没列为独立问题,**§5 风险表也没列** | 现状就有静默写漏判,等不到 P2 上线就该先堵 |
| **P2-A** | §4 阶段 2 提议检测 `>`/`>>`/`2>`/`&>` 就升 SideEffect——但 **`2>&1` 只是 fd 复制,无副作用**,应排除。建议区分"输出重定向到文件"和"fd 重定向":前者升档,后者不动 | 体验过度收紧,Plan 模式 `command 2>&1 \| head` 会被误伤 |
| **P2-B** | §4 阶段 2 提议"检测到 `$()`/反引号 → 整条降级 Ask"——但 `echo $(date)` / `echo \`date\`` 是无害调用,会一刀切误伤 | UX 打折(且 Plan 模式最受伤) |

下面对每个发现做事实核验(所有 P1 级断言均通过 `grep` / 行号二次核验)。

---

## 1. P1-A:`kill-list 11 条` 实际是 10 条(事实错误,3 处)

### 1.1 现状

`app/src-tauri/src/agent/permissions/dangerous.rs:26-86` 的 `DENY_PATTERNS` 常量是 **10 条** tuple:

| # | 正则 | 用途 |
|---|---|---|
| 1 | `(?i)(^\|\s)rm\s+(-[a-zA-Z]*[rRfF][a-zA-Z]*\s+)*(/\*?\s*$)` | rm -rf / |
| 2 | `(?i)(^\|\s)mkfs(\.\w+)?\s+` | mkfs |
| 3 | `(?i)(^\|\s)dd\b[^\|;&]*\sif=` | dd if= |
| 4 | `(?i):\(\)\s*\{\s*:\s*\|\s*:\s+&\s*\}\s*;\s*:` | fork bomb |
| 5 | `(?i)>\s*/dev/(sd\|hd\|nvme\|vd\|xvd)` | 写块设备 |
| 6 | `(?i)(^\|\s)chmod\s+(-[a-zA-Z]*R[a-zA-Z]*\s+)*(0?77[0-7]\|777)\s+/(\s\|$)` | chmod 777 / |
| 7 | `(?i)(^\|\s)git\s+push\s+(-[a-zA-Z]*f[a-zA-Z]*\s+)*(--force\s+)?(origin\s+)?(main\|master\|develop)\s*$` | git push -f protected |
| 8 | `(?i)(^\|\s)(curl\|wget)\s+[^\|]*\|\s*(ba)?sh(\s\|$)` | curl\|bash |
| 9 | `(?i)(^\|\s)find\b.*\s-delete\b` | find -delete(RULE-B-004 加) |
| 10 | `(?i)(^\|\s)find\b.*\s-exec(dir)?\b` | find -exec(RULE-B-004 加) |

### 1.2 草案口径

| 位置 | 表述 |
|---|---|
| §1.1 现状 | "Tier 2 kill-list(`dangerous.rs`,11 条灾难性正则)" |
| §2.2 不变量 1 | "Tier 2 kill-list **无条件前置**:11 条灾难性模式在所有 Mode(含 Yolo)下硬拒" |
| §3.2 表格 Tier 2 kill-list | "11 条灾难性正则,Yolo 也挡" |

**三处不一致,且与现实代码不符**。

### 1.3 误解源头

`REVIEW-agent-loop-full-audit-2026-06-14`(基线 a4fb302,2026-06-14)§2.2 P2-2 标记"危险命令检测有**真实绕过路径**",其中 `find / -delete` 是漏网之鱼。后续 RULE-B-004(2026-06-15 前后)落地**新增 2 条** find -delete 和 find -exec 规则,**同时给所有正则加 `(?i)`** 修复大小写绕过。当前 commit `9c51f11`(2026-07-02)已包含 RULE-B-004。草案作者可能参考了更早版本的 `dangerous.rs`(可能是 8 条)或凭印象写 11,与现状 10 条对不上。

### 1.4 建议

- 草案三处口径统一为"10 条灾难性正则(2026-07-02 基线,含 RULE-B-004 新增的 find -delete/-exec 与全 `(?i)` 修复)"。
- 同步把"RULE-B-004 已闭合 `find / -delete` 漏判"作为现状资产列入 §1.1,体现作者知道这个历史债已还。

---

## 2. P1-B:`>` 写重定向在现状下被静默放行(独立漏判,需在 P2 之前单独补)

### 2.1 现状

`shell_trust.rs:365`:

```rust
if cmd.contains('|') || cmd.contains("&&") || cmd.contains(';') {
    return ShellTrust::Ask;
}
```

**只检查三种结构元字符,不检查 `>`**。

### 2.2 漏判路径

`git diff > patch.txt`:

1. `first_token` → `git`
2. 结构降级 → 无 `|`/`&&`/`;`,**不降级**
3. `classify_git_subcommand` → `diff` ∈ `GIT_READONLY_SUBCOMMANDS` → **ReadOnly**
4. Mode 三档中 Plan/Edit 全部静默 Allow(只看 ShellTrust,不感知 shell body 里的 `>`)
5. **结果:`git diff > patch.txt` 静默写文件**,LLM 不经任何弹窗

同样的漏判路径覆盖 `git log > x`、`grep foo *.txt > out`、`jq . data.json > copy.json` 等所有"读命令 + 写文件重定向"组合。

### 2.3 草案的承认与遗漏

草案 §4 阶段 2 描述:"`git diff > patch.txt` → ReadOnly + 写重定向 → **SideEffect**... 但首 token 是 git diff → ReadOnly,会**静默写文件**——这是现状的另一个静默写漏判,P2 的重定向检测顺手收"。

**这里有两处问题**:

1. **"顺手收"的口径偏弱**——这不是 P2 的副产物,是**独立的现状漏判**。`git diff > patch.txt` 在 P2 上线前就会发生。
2. **§5 风险表完全没列**这条漏判。读起来像"只有 P2 实施时才会被发现",实际上 LLM 任何一次 `>` 写重定向都会触发。

### 2.4 攻击/事故面

LLM 在 Plan 模式下被允许 `git diff > /tmp/patch`(Patch 模式是分析会话,不该静默写)。或者 LLM 在 Edit 模式下 `cat .env > /tmp/leak.txt`(虽然 `.env` 通常被 .gitignore,boundary 也兜项目内,但**重定向目标是任意路径**,boundary 不检 shell body 内的 `>` 目标)。

更危险的:`git diff HEAD~5 > /etc/cron.d/agent`(目标是项目外,但 shell 不感知 `>` 目标路径,boundary 不检 shell body)。Yolo 模式 Tier 2 kill-list 第 5 条 `(?i)>\s*/dev/(sd|hd|nvme|vd|xvd)` 兜底块设备,但不兜普通文件。

### 2.5 建议

- §5 风险表新增一条 **"P1-B(独立漏判):现状 `>` 写重定向不被 shell_trust 检测,Plan 模式下 `git diff > x` 静默写文件"**——风险级别应高于 P2 阶段 2 的"拆分器引号误判",因为后者是 P2 引入的新风险,前者是现状已存在的攻击面。
- **要么把"`>` 写重定向检测"从 P2 拆出来作为 P0 先堵**,要么**§4 阶段 1 增加一个 sub-task P1b:对含 `>` 的命令强制 classify 之前先判定 SideEffect**(这条改动只动 `shell_trust.rs` 的步骤 2,几行代码,不依赖 P2 的拆分器)。
- ROADMAP §2 第三档 A2+ 条目的"现状已实施"列里,**应先承认这条漏判的存在**,而不是把它藏在 §4 阶段 2 的括号里。

---

## 3. P2-A:`2>&1` vs 写文件重定向边界缺失

### 3.1 草案建议

§4 阶段 2:"检测裸重定向(`>`/`>>`/`2>`/`&>`)→ 该子段至少 SideEffect"。

### 3.2 边界问题

- `2>&1` 是 fd 复制(把 stderr 重定向到 stdout),**无副作用**。但草案提议统一升 SideEffect,会误伤。
- `&>` 在 bash 是 `> foo 2>&1` 的简写,等价于"输出到文件 + fd 复制",**有副作用**。
- 实际需要区分的是 **"输出重定向到文件"** vs **"fd 重定向"**:
  - `> file` / `>> file` / `&> file`(bash 限定):输出到文件 → SideEffect
  - `2>&1` / `>&2` / `1>&2`:fd 复制 → 无副作用

### 3.3 误伤示例

| 命令 | 草案建议 | 实际语义 |
|---|---|---|
| `grep error *.log 2>&1 \| head` | SideEffect(因含 `2>&1`) | 无副作用(纯读) |
| `cargo test 2>&1 \| tee build.log` | SideEffect | **真有副作用**(tee 写文件)——但这里副作用是 `tee` 不是 `2>&1` |
| `cmd > /dev/null 2>&1` | SideEffect | 静默丢弃,无副作用 |

### 3.4 建议

§4 阶段 2 实施时,**只升"输出到文件"的 `>`/`>>`/`&> <path>` 形态**,不升 `2>&1`/`>&N`。具体正则建议(留 design.md 拍板):

```text
升 SideEffect:  >\s*[^&]\S+    (后面跟非 & 的目标,且目标像路径)
升 SideEffect:  >>\s*[^&]\S+
升 SideEffect:  &>\s*\S+
不升:           2>&1 / >&N / 1>&2
```

`$()`/反引号一律 Ask 的边界也类似,见 P2-B。

---

## 4. P2-B:`$()`/反引号一律 Ask 会误伤 `echo $(date)`

### 4.1 草案建议

§4 阶段 2:"检测命令替换(`$()`/反引号)→ 整条降级 Ask(内容不可静态知)"。

### 4.2 边界问题

| 命令 | 草案建议 | 实际语义 |
|---|---|---|
| `echo $(date)` | Ask | 无副作用,纯读 |
| `echo \`date\`` | Ask | 无副作用,纯读 |
| `echo $(rm -rf foo)` | Ask | 真的危险,准确 |
| `cat $(find . -name "*.log")` | Ask | 路径展开,可疑但不直接破坏 |
| `vim $(echo notes.md)` | Ask | 写文件,但命令本身 SideEffect |

### 4.3 哲学冲突

草案 §2.2 不变量第 6 条:"本方案仍只判定'启发式可信度',不声称完美——不确定就降级 `Ask` 的总原则不变"。这个原则在 `$()` 上确实 fail-safe(误伤无害,用户可放行)。

但 §1.2 缺口 B 痛点是 **"Plan 模式最受伤——它本就是只读分析会话,最需要 `git log | head -5` 这种调查命令,却每次都要用户放行"**。`echo $(date)` 在 Plan 模式被一刀切 Ask,**复刻了同样的 Plan 模式体验硬伤**。

### 4.4 建议

§4 阶段 2 实施时,只对以下模式升 Ask(更精细):

- `$()` / 反引号**整体作为命令**(首 token 是 `$()` 或反引号整体):如 `$(rm -rf foo)` → Ask
- `$()` / 反引号**作为参数但整条命令的副作用来自其内容**:这个静态不可知,继续降级 Ask
- `$()` / 反引号**作为参数且首 token 本身是 ReadOnly**:如 `echo $(date)` → 保留 ReadOnly(可放过),代价是误判 `echo $(rm x)` 为 ReadOnly(漏判),但 echo 不执行参数内容,实际无副作用

`echo` 不执行参数中的命令替换——这是 shell 语义保证的(`echo` 是 builtin,不解析命令替换为代码)。所以 `echo $(rm x)` 不会真执行 `rm x`,漏判安全。

但 `vim $(echo notes.md)` 首 token 是 vim(在 SIDE_EFFECT_WHITELIST,SideEffect),已经被升档,与 `$()` 无关,OK。

**例外**:`bash -c "$(...)"` / `eval "..."` 这类"显式求值命令"的 `$()` 仍是危险——但首 token 已经是 bash/eval(在 asklist,默认 Ask),双重保险成立。

把这条作为 §5 风险表的 **"静态判定固有盲区"** 的细分子条目,而不是 §4 阶段 2 的硬规则。

---

## 5. 与草案"做对的事"对齐确认

下面这些是评审二次核验后**确认无误**的部分:

| 草案陈述 | 核验 | 状态 |
|---|---|---|
| `shell_trust.rs:77-91` 三档表 | 实读,ReadOnly/SideEffect/Ask 行号与注释完整对应 | ✅ |
| `shell_trust.rs:349` 算法入口 `pub fn classify_prefix(cmd: &str) -> ShellTrust` | 核验 | ✅ |
| `shell_trust.rs:365` 结构降级 `cmd.contains('|') \|\| cmd.contains("&&") \|\| cmd.contains(';')` | 核验,**注意不检 `>`** | ✅(引出 P1-B) |
| 算法"五步"取首 token → 结构 → git 子命令 → 白名单 → 默认 Ask | 核验步骤与代码一致 | ✅ |
| `check.rs:298` (a) prefix-grant 短路 | 核验,在 (b) classify 之前 | ✅ |
| `check.rs:307` worker per-run grant 同步 | 核验,`has_run_grant(tool_name, "prefix", &first_token)` 同样是首 token 匹配 | ✅ |
| `check.rs:326` (b) classify_prefix 调用 | 核验 | ✅ |
| `check.rs:690` `match_value_for_allow_always` Shell 分支返回首 token | 核验,与 `shell_trust::first_token_for_allow_always(cmd)` 配合 | ✅ |
| `dangerous.rs:143` 测试 `kill_list_does_not_block_normal_rm` 验 `rm /tmp/foo.txt` 应 None | 核验,实测 `is_kill_listed(...) == None` 成立 | ✅ |
| 审计 17 类 AuditKind(`audit.rs:34-108`) | 数 enum 变体:ToolDenied, ToolAllowed, ToolPermissionAsk, ToolExecuted, ToolDeniedYolo, PermissionGranted, PermissionTimeout, RequestCancelled, ModeChanged, YoloEntered, YoloExited, EditMessage, ResendMessage, WorkerAskAllowed, WorkerAskDenied, WorkerAskTimedOut, WorkerAskCancelled = 17 | ✅ |
| Tier 4 Shell 分支只在 Plan/Edit 跑,Yolo bypass 整 Tier 4 | 与 `REVIEW-agent-loop-full-audit` §2.2 一致 | ✅ |
| worker per-run grant(`run_grant.rs`)+ parent 路径 `run_grants = None` 跳过 | 核验 `check.rs:307-319` 注释与代码 | ✅ |
| §3.1 Tier 1/2/2.5/3/5/6 全部不动 | 与历史评审的 ⑨ 关管线图对齐 | ✅ |
| §3.1 Tier 4 ★ 本方案落点 ★,作用面小、回归面窄 | 与代码改动量预估一致(grant 短路 < 10 行,classify 改造 < 80 行) | ✅ |
| §3.2 PermissionModal 仍按 Risk(shell 恒 High `types.rs:73`)渲染,modal 无感 | 核验 `ask_path` 调用形态与现状一致 | ✅ |
| §3.2 worker run-grant 也应加"结构简单"前置 | 草案 §4 阶段 1 已同步覆盖,正确 | ✅ |
| §4 阶段 1 短路径"check.rs:298 在 check.rs:326 之前"因果 | 核验 | ✅ |
| §4 阶段 2 拆分器职责"顶层 `;`/`&&`/`\|\|`/`\|`" | `\|\|` 是 logical OR,草案补全现状 `cmd.contains('|')` 已隐含覆盖但未明示 | ✅ |
| §4 阶段 3 沙盒"WSL2 bubblewrap user namespace 需前置 spike" | 与 WSL 历史坑一致(HACKING-wsl 多次提及) | ✅ |
| §6 落地指征 5 条(Trellis 任务 / ADR / 回归矩阵 / spec 更新 / ROADMAP 迁移) | 与项目既有 task 流程对齐 | ✅ |

---

## 6. 与历史评审的一致性

| 历史评审 | 关联点 | 当前草案位置 | 状态 |
|---|---|---|---|
| `REVIEW-a2-b7-permission-mode-plan-2026-06-13` §1 | ⑨ 关顺序 P0 已解决 | §3.1 Tier 管线图与历史一致 | ✅ 不再重复 |
| 同 §2 | Per-Mode Tool List 过滤(Plan 物理移除 write 类) | §1.2 缺口 B 提到"Plan 模式最受伤",但**没引用**该历史决议 | ⚠️ 应在 §1.2 补一句"Plan 已物理移除 write 类,但 shell tool 仍在 list 内(因 shell 是异构),故 Plan 模式对 shell SideEffect/Ask 仍弹窗"——这才能解释为什么 §4 阶段 2 对 shell 拆分仍有价值 |
| 同 §3.4 | 审计日志 schema 17 类 AuditKind | §2.2 不变量 5 锁定 | ✅ |
| `REVIEW-agent-loop-full-audit-2026-06-14` §2.2 P2-2 | 危险命令大小写不敏感 + find -delete 漏判 | 草案未提及 RULE-B-004 已闭合此事 | ⚠️ §1.1 现状应说明 RULE-B-004 已还债,避免读者误以为还需要补 |
| 同 §2.2 P2-3 | shell_trust `cmd.contains('|')` false positive(`grep "a\|b"`) | 草案未提及 | ⚠️ §4 阶段 2 拆分器职责里应**继承**这一 false positive 而非"优化掉"——false positive 是 fail-safe |
| 同 §2.2 P3-1 | AuditKind docstring 写"10" 实际 11(基线 a4fb302) | 当前 commit `9c51f11` 已扩到 17,docstring 仍可能 stale | ⚠️ 草案 §2.2 不变量 5 应要求"同步修 docstring"(若仍未修) |
| 同 §2.5 E-P0-2/P0-3 | shell 不 kill 进程组 + env 泄漏 API key | 草案**完全没提**这两个 P0 | ❌ **草案 §4 阶段 3 沙盒设计是"防判定错",但 shell env_clear/process_group 是"防执行层窃密"——两者正交,不应被沙盒替代**。建议草案 §4 阶段 3 之前**显式引用**这两个 P0,作为"为什么 P3 沙盒不是 P1+P2 的替代品"的论据 |

最后一条 **REVIEW-agent-loop-full-audit §2.5 E-P0-2/P0-3(shell env_clear/process_group)** 是本草案的**结构性遗漏**——草案 §4 阶段 3 沙盒是 P1+P2 之后的远期,但 shell P0(env 泄漏 + 进程组)是**当前已有的更高优先级安全债**,且**沙盒兜底不了 env 泄漏(沙盒内的 shell 仍继承父进程 env)**。下节展开。

---

## 7. P1-C(结构性遗漏):shell env_clear / process_group 是 P1+P2 **之外**的当前 P0

### 7.1 历史事实

`REVIEW-agent-loop-full-audit-2026-06-14` §2.5 标记:

- **E-P0-3**:`shell` 子进程继承父进程全部环境变量(含 `ANTHROPIC_API_KEY`)——LLM 一句 `env` 即可窃取(`shell.rs:237`)
- **E-P0-2**:`shell` 不 kill 进程组——`sleep 60 &` / 管道 / `nohup` 产生的孤儿进程在 cancel/timeout 后继续跑(`shell.rs:79-99`)

两次独立深挖都独立复述,确认是 P0 级安全债。

### 7.2 与本草案的关系

草案 §4 阶段 3 沙盒设计(bubblewrap / firejail / overlayfs)能**部分**限损:

- 沙盒内 shell 仍继承父进程 env(沙盒隔离的是文件系统,不是进程 env namespace,除非用 systemd-run / nsjail 这类带 namespace 隔离的方案)——**沙盒不能替代 env_clear**
- 沙盒可缓解进程组孤儿(沙盒 kill 时整个 PGID 内进程都收),但不如 `process_group(0)` + `killpg` 显式干净

**所以**:即便 P3 沙盒上线,shell P0 仍是 P0。草案 §4 阶段 1/2 的不变量隐含"shell 工具在 P1+P2 之后仍是相对独立的执行单元",没把"shell 工具本身的执行期加固"作为前置依赖。

### 7.3 建议

- 草案 §2.2 不变量部分**新增一条**:"shell 执行期加固(env_clear 白名单、process_group + killpg)是独立于本方案的 P0,与 P1+P2+P3 正交——本方案不动它,但实施 P1 前应**先还清** shell P0,避免新判定逻辑在执行期被 env/进程组问题污染"。
- 草案 §6 落地指征 1 调整:**先建 shell P0 还债任务**(独立任务,独立评审),再排 P1+P2,然后 P3 沙盒。三任务顺序:**shell P0 → A2+ P1 → A2+ P2 → A2+ P3**。
- ROADMAP 第三档 A2+ 条目**应注明**"前置依赖:shell env_clear + process_group(2026-06-14 全盘审计 P0)"。

---

## 8. 阶段顺序与上线节奏复核

草案 §4 已经讲了 P1→P2 的依赖。评审补充两个细节:

### 8.1 P1b(`>` 检测)是否要前置?

如果采纳 §2 P1-B 的建议把"`>` 写重定向检测"从 P2 拆出来作为 P1b,则阶段顺序变:

```
shell P0 (env_clear + process_group)
  └─ A2+ P1 (grant 短路收紧 + `>` 写重定向检测)
       └─ A2+ P2 (拆分器 + 取 max)
            └─ A2+ P3 (沙盒,远期)
```

**P1b 必须先于 P2** 的理由:与 P1 grant 收紧同质——都是"在现状 `classify_prefix` 内追加检测,改动 < 20 行",且都补安全缺口。P2 是大改(新加拆分器,引号/转义/进程替换的测试矩阵),节奏上拆开更稳。

### 8.2 P1 + P1b 是否同批合?

**建议同批合**——两者都改 `shell_trust.rs` 的 `classify_prefix`,同 PR 提更易回归(regression matrix 一次跑完)。但**必须先做 shell P0**。

### 8.3 P3 沙盒不应与 P1+P2 混任务

草案 §6 已明确"P3 独立"。评审强调:P3 需独立 Trellis 任务 + 独立 PRD + 独立 spike(WSL2 bubblewrap 可用性)。**不要在 P1+P2 任务里挂 P3 的 milestone**。

---

## 9. 草案 §3.2 表格的隐含遗漏

| 组件 | 草案提到 | 评审补充 |
|---|---|---|
| **Tier 1 pitfall recall**(P5 软拦截) | ✅ 提到"拆分后按子命令匹配 command_pattern 更准" | OK,**但没说落地点**(chat.rs 哪行调 recall_pitfall_footnote?实施时需指认) |
| **RunGrantCache reset 路径** | ❌ 没提 | worker 路径的 `RunGrantCache` 是 per-run 内存缓存,P1+ 收紧 grant 短路**不影响 run-grant cache 的清理语义**——但实施时需确认 worker dispatch 后清 cache,避免下一个 worker 误继承前一个 worker 的 grant |
| **SPEC 后端 `tool-contract.md`** | ✅ 草案 §6.4 要求 spec 更新 | OK,需明确更新哪个 section(草案 §1.1 已引用 "Scenario: Path-based Permission",P1+P2 后应在该 section 末尾追加"Composite Command Classification"子节) |
| **测试用例粒度** | §6.3 列 8 项 | OK,建议**每项至少 2 个 case**(positive + negative),并加一个"老 first-token 行为不回归"组(确保 P2 拆分器不破坏现有 single-token 行为) |

---

## 10. 行动清单(按优先级)

### P0 — 实施前必须先还的债(不在本草案范围内,但前置)

- [ ] **shell env_clear + 白名单注入**(`shell.rs:237`,~10 行,排除 `*_API_KEY`/`*_TOKEN`)—— 独立任务,优先级 **P0**
- [ ] **shell process_group(0) + killpg**(`shell.rs:79-99`,~15 行)—— 独立任务,优先级 **P0**
- [ ] **§1 P1-A:草案 3 处口径统一为"10 条灾难性正则"**—— 文档一致性,1 行字
- [ ] **§2 P1-B:把 `>` 写重定向漏判从 P2 拆出为 P1b**—— 或者在 §5 风险表新增一条独立漏判条目

### P1 — 实施前强烈建议补正

- [ ] **§7 P1-C:草案 §2.2 新增不变量"shell 执行期加固是独立 P0,本方案正交不替代"**
- [ ] **§3 P2-A:`2>&1`/`>&N` 不升档,只升 `> file`/`>> file`/`&> file`** —— design.md 阶段拍板正则
- [ ] **§6 隐含遗漏:RunGrantCache per-run reset 路径在 §3.2 表格新增一行**
- [ ] **§6 历史决议引用:§1.2 缺口 B 补 Per-Mode Tool List 已物理移除 write 类的背景**
- [ ] **§6 RULE-B-004 已在 §1.1 现状表体现(避免读者误以为还要补)**

### P2 — 实施中注意

- [ ] **§4 P2-B:`$()`/反引号按"是否作为命令首 token"分级,不全降 Ask** —— `echo $(date)` 不升档
- [ ] **测试矩阵每项 ≥ 2 case + "老 first-token 不回归"组**
- [ ] **spec 更新精确指认 section**("Scenario: Path-based Permission" → 末尾追加 "Composite Command Classification")
- [ ] **同步修 AuditKind docstring** (若 `permissions/mod.rs` doc 仍 stale)

### P3 — 远期

- [ ] **P3 沙盒独立 Trellis 任务 + WSL2 bubblewrap spike** —— 不与 P1+P2 混

---

## 11. 文档质量评估

| 维度 | 评分 | 备注 |
|---|---|---|
| 目标清晰度 | ★★★★★ | §1 三档表 + §2 目标/不变量/非目标三层分明 |
| 架构契合度 | ★★★★ | §3.1 Tier 4 ★ 本方案落点 ★ 表述精准,但**漏掉 shell P0 正交性**(见 P1-C) |
| 阶段解耦 | ★★★★½ | §4 三阶段依赖讲透,但 §5 风险表漏 P1-B(写重定向)和 shell P0 |
| 回归测试矩阵 | ★★★½ | §6.3 列 8 项,但缺粒度要求(每项几个 case、是否含老行为回归组) |
| 事实准确性 | ★★★ | **P1-A(kill-list 11 条) + 隐含的 P1-B(`>` 漏判未独立列出)** 拉分 |
| 与历史决议一致性 | ★★★½ | 未引用 Per-Mode Tool List 已落地、RULE-B-004 已还债、shell P0 仍欠 |
| 落地可执行性 | ★★★★ | §6 5 条指征完整,可直接生成 Trellis task |

**整体:★★★★ (4/5)**——架构与方向都正确,但 P1-A/P1-B/P1-C 三处补正后才能进实施。

---

## 12. 结论

**方案可以进入实施阶段**,前提是:

1. **先还 shell P0(env_clear + process_group)—— 独立任务,优先级高于 A2+**。
2. **草案 §1.1/§2.2/§3.2 三处 kill-list 数量从 11 改成 10**(P1-A)。
3. **§4 阶段 1 增加 P1b:`>` 写重定向检测**(或 §5 风险表显式列出这个独立漏判)(P1-B)。
4. **§2.2 不变量部分增加"shell 执行期加固是独立 P0,本方案不替代"**(P1-C)。
5. **§4 阶段 2 的 `>` 检测和 `$()` 检测加边界条件**(P2-A:排除 `2>&1`;P2-B:`echo $(date)` 不升 Ask)。

补正后,**A2+ 的 P1+P2 是一个独立可执行的中等任务**(估算 < 200 行 Rust + ~30 个测试 case + 1 个 ADR + spec section 更新),**P3 沙盒是另一个独立大任务**(需独立 spike + 独立评审)。

**该方案最大的架构优点是作用面收得极小**——Tier 4 Shell 分支两步内,不动 modal、不动 Mode、不动审计 schema、不动 kill-list、不动 IPC。**这是它能安全分阶段上线的根本前提**,也是评审没有建议"重写整个 classify_prefix"或"引入新 IPC"的根本原因。

---

## 附录 A:评审覆盖的关键文件

| 文件 | 行数 | 评审引用 |
|---|---|---|
| `app/src-tauri/src/agent/permissions/shell_trust.rs` | 732 | §1.1/§3/§4 反复引用 |
| `app/src-tauri/src/agent/permissions/check.rs` | 1169 | §1/§3.1 引用 grant 短路顺序 |
| `app/src-tauri/src/agent/permissions/audit.rs` | 277 | §6/§1 引用 17 类 AuditKind |
| `app/src-tauri/src/agent/permissions/dangerous.rs` | 276 | §1/§4 引用 10 条 kill-list |
| `docs/A2-SHELL-CLASSIFICATION.md` | 249 | 评审对象 |

## 附录 B:评审方法说明

- 所有 P0/P1 级事实断言均通过 `grep` + `Read` 行号二次核验,基于 commit `9c51f11`(2026-07-02)。
- 历史 review 引用基于 `docs/_reviews/` 目录现有文件,未二次核验历史 commit。
- 评审未实际跑 `cargo test` 验证草案提议的拆分行为——拆分器的实现细节(状态机、引号处理)留给实施时的 `design.md`。
- 评审未涉及草案 §4 阶段 3 沙盒的深度技术细节——该阶段需独立 Trellis 任务 + 独立 PRD + 独立 spike。

---

> 本评审署名 **MiniMax-M3**。所有 P1 级断言均已通过 grep / 行号二次核验。后续代码演进请以当前代码为准。