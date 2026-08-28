# A2+ Shell 命令只读/副作用精细判定方案

> **状态**:✅ **已实施(P1+P2 于 2026-07-04 落地,见 [IMPLEMENTATION/decisions-2026-07.md](../IMPLEMENTATION/decisions-2026-07.md);P3 沙盒为远期独立任务)**。本文档降为方案回顾,实施细节以 `.trellis/spec/backend/tool-contract.md` 「Compound command classification (A2+)」段为准。
> **日期**:2026-07-03(方案);P1+P2 2026-07-04 落地
> **关联**:[ROADMAP §2 第三档 A2+](../ROADMAP.md)、模块 `app/src-tauri/src/agent/permissions/`(尤其 `shell_trust.rs` / `check.rs`)
> **本文档职责**:讲清"为什么做、做什么、分几步、怎么和现有审批管线结合"。**不讲实现细节**(状态机、正则、函数签名留到实施时的 `prd.md` / `design.md`)。

---

## 1. 背景

### 1.1 现状

EverLasting 的 shell 权限判定由 `shell_trust::classify_prefix` 做,它把一条 shell 命令分进三档(`ShellTrust` 枚举):

| 档位 | 语义 | Plan | Edit | Yolo |
|---|---|---|---|---|
| `ReadOnly` | 纯读(`ls`/`cat`/`git diff`) | 静默 Allow | 静默 Allow | bypass |
| `SideEffect` | 可恢复副作用(`mkdir`/`cargo`/`git push`) | 弹窗 | 静默 Allow | bypass |
| `Ask` | 危险/未知/结构复杂 | 弹窗 | 弹窗 | Tier 2 仍兜 |

判定算法(`classify_prefix`)短路径五步:**取首 token → 检测结构元字符 → git 子命令细化 → 查白名单 → 默认 Ask**。

其中第 2 步"结构元字符检测"是**一刀切**:`cmd` 含 `|` / `&&` / `;` 任意一个,整条命令直接降级 `Ask`(`has_structural_metachar`)。设计动机写在模块文档里——管道和命令链能在无害首 token 后藏副作用(`git log | bash`),首 token 分类器无法可靠分类,于是"宁误弹不漏弹"。

### 1.2 两个已知缺口

这一刀切带来两个真实缺口,一个是**安全**的,一个是**体验**的:

**缺口 A — 复合命令 grant 绕过(安全)**

Tier 4 Shell 分支里,"始终允许"的 prefix-grant 短路在 `classify_prefix` **之前**(`check_prefix_grant` 在 `classify_prefix` 之前):

```
check_prefix_grant(首 token)  ──命中──▶  直接 Allow (return)
        │ 未命中
        ▼
classify_prefix(cmd)  ──▶  三档判定
```

grant 存的是首 token(`match_value_for_allow_always`)。于是用户对 `ls` 点过一次"始终允许"后,任何 `ls` 开头的命令——包括 `ls foo; rm -rf ~/notes`——首 token 都是 `ls`,grant 命中 → 直接 Allow,**结构性降级根本没机会跑**。

而 Tier 2 kill-list(`dangerous.rs`,10 条灾难性正则)只兜 `rm -rf /` 这类,**故意不挡** `rm -rf <非根>`(`dangerous.rs:143` 测试明示,理由是"靠 worktree + git 恢复")。可 `~/notes` 在项目外,git 救不回。

**用户授权时的心智模型是"只读的 ls"**,代码却把它放大成了"任何 ls 开头的命令串"。

**缺口 B — 只读管道误伤(体验)**

`git diff | head`、`ls | grep foo`、`cat x | wc -l` 这些纯读管道,因含 `|` 被一刀切降级 `Ask`,在 Edit 模式也弹窗。Plan 模式最受伤——它本就是只读分析会话,最需要 `git log | head -5` 这种调查命令,却每次都要用户放行。

### 1.3 为什么必须收

- **缺口 A** 是授权语义偏离:用户赋予的信任被静默扩大,且损害不可逆(项目外数据无 git 兜底)。这违背 A2+B7 权限系统"用户显式决策"的设计前提。
- **缺口 B** 是 Plan 模式的体验硬伤:只读调查命令反复弹窗,削弱 Mode 分档的意义。

两者同源——都是"首 token 分类器 + 一刀切结构降级"的粒度问题,可以一次性收。

---

## 2. 目标与不变量

### 2.1 目标

1. 复合命令按**真实组成**判定,而非按首 token 一刀切。
2. "始终允许"的授权边界对齐用户心智:授权 `ls` = 信任结构简单的 `ls`,不包括 `ls; rm`。
3. 只读管道在所有模式静默放行,不再误伤。
4. 判定错了有兜底,损害被限制(远期)。

### 2.2 不变量(不能破坏的现有契约)

这些是 A2+B7 落地时锁定的契约,本方案不得违反:

- **Tier 2 kill-list 无条件前置**:10 条灾难性模式在所有 Mode(含 Yolo)下硬拒,本方案不动它,它仍是最后一道硬墙。
- **Tier 4 Yolo bypass 整层**:Yolo 模式跳过 Tier 4(含本方案的新判定),只由 Tier 2 兜底。这是用户显式选 Yolo 的契约。
- **Mode 三档语义不变**:`ReadOnly` 三档静默、`SideEffect` 看 Mode、`Ask` 两档弹窗。
- **grant 表三 match_kind 不变**(`tool` / `prefix` / `path`):不改 schema,只改 `prefix` 的**短路前置条件**。
- **审计 17 类 AuditKind 不变**:可加细分 reason,不加新 Kind。
- **shell 是异构工具**:本方案仍只判定"启发式可信度",不声称完美——不确定就降级 `Ask` 的总原则不变。
- **执行层加固已落地,本方案不动执行层**:`shell.rs` 的 `env_clear()` + 白名单重注入(RULE-E-001,挡 `ANTHROPIC_API_KEY` / `OPENAI_API_KEY` 等 env 泄漏)+ `process_group(0)` + `killpg`(RULE-E-002,挡 cancel/timeout 后孤儿进程)均已落地(`apply_safe_env` / `kill_and_collect`,含防泄漏测试)。本方案只改判定层(`shell_trust` / `check`),与执行层正交——判定准了不替代执行层加固,执行层加固也不依赖判定。

### 2.3 非目标(明确不做)

- **不追求完美判定**:shell 图灵完备,变量展开 / `eval` / alias / 命令替换让"只读"在静态层面不可判定(undecidable)。本方案是"启发式 + 兜底",不是"判定器"。
- **不动 read 族 path 工具**(`read_file`/`grep`/`glob`/`list_dir`):它们走 Tier 4 Path 分支,有自己的边界检查,与 shell 判定无关。
- **不改 Mode 枚举**:`edit`/`plan`/`yolo` 三档不动。
- **不引入新 IPC / 前端组件**:PermissionModal 仍是展示终端,本方案只改判定结果,不改展示。

---

## 3. 与现有审批功能的结合(本文档重点)

### 3.1 新方案落在哪一层

现有 9 关管线(`check.rs:check()`):

```
Tier 1   Hooks (pitfall recall,注脚/软拦截)         ── 不动
Tier 2   kill-list 硬墙 (dangerous.rs,10 模式)       ── 不动(兜底)
Tier 2.5 sensitive-path deny-list (read 族项目外)   ── 不动
Tier 3   Mode (Plan 拦 write_file/edit_file)         ── 不动
Tier 4   Path / Prefix / External policy             ── ★ 本方案落点 ★
          ├─ Shell 分支:
          │   (a) prefix-grant 短路  ──▶ 阶段 1 改前置条件(grant 收紧)
          │   (b) classify_prefix    ──▶ 阶段 2 精细化(拆分 + 取 max)
Tier 5   默认 Allow (未知工具)                        ── 不动
Tier 6   审计                                         ── 不动(可加 reason 细分)
```

**全部改动集中在 Tier 4 Shell 分支的两步**(grant 短路 + classify)。Tier 1/2/2.5/3/5/6 零改动。这是本方案最大的架构优点:作用面小、回归面窄。

阶段 3(沙盒)是唯一落在 Tier 4 **之外**的部分——它是执行期隔离层,见 §4.3。

### 3.2 与各审批组件的交互

| 组件 | 现状 | 本方案带来的变化 |
|---|---|---|
| **Tier 2 kill-list** | 10 条灾难性正则,Yolo 也挡 | **不变**。仍是硬墙。本方案只是让更多"可恢复但有副作用"的命令走到正确的 SideEffect/Ask,而非被 grant 短路成 Allow |
| **Tier 4 (a) prefix-grant**("始终允许") | 首 token 命中即短路 Allow | **阶段 1 收紧**:只有"结构简单"(无 `;`/`&&`/`\|`)的命令才享受短路。复合命令强制回落 classify。grant 表 schema 不变,变的是短路的前置条件 |
| **Tier 4 (b) classify_prefix** | 首 token + 一刀切结构降级 | **阶段 2 精细化**:结构降级换成"复合命令拆分 + 子命令独立判定取 max"。`ReadOnly`/`SideEffect`/`Ask` 三档语义不变 |
| **PermissionModal** | 按 Risk(Low/Med/High/Critical)渲染,shell 恒 High(`types.rs:73`) | **不变**。仍是展示终端。modal 看到的是 Decision(Allow/Ask),本方案改的是"产生 Decision 的判定过程",modal 无感 |
| **Mode (edit/plan/yolo)** | ReadOnly 三档静默 / SideEffect 看 Mode / Ask 两档弹窗 | **不变**。本方案产出仍是三档之一,Mode 映射规则原样复用 |
| **审计日志** | 17 类 AuditKind,Tier 4 Allow/Ask 各记一行 | **不变**。可加细分 reason(如"复合命令拆分后命中危险子命令"),不加新 Kind |
| **Tier 1 pitfall recall**(P5 软拦截) | 按 `(tool, command_pattern, path_globs)` 触发 | **顺带受益**:拆分后按**子命令**匹配 command_pattern 更准(现在 `ls; cargo` 整串匹配会漏掉 cargo 的 pitfall)。非目标,但是个 nice side-effect |
| **worker subagent grant**(per-run cache) | worker 的 run-grant 同样首 token 匹配 | **阶段 1 同步覆盖**:worker 的 prefix run-grant 也应加"结构简单"前置,否则 worker 路径有同样的绕过。见 `check_prefix_grant` 的 worker 分支 |

> **关键认知**:本方案**不改任何审批组件的接口**,只改 Tier 4 Shell 分支内部的判定逻辑 + grant 短路前置。PermissionModal、Mode、审计、kill-list 全部无感。这是它能安全分阶段上线的根本原因。

---

## 4. 分阶段方案

三个阶段,**每阶段可独立上线、独立收回缺口的一部分**,互不阻塞。顺序有依赖:**阶段 1(grant 收紧)必须先于或同于阶段 2(拆分)**——因为 Tier 4 的 (a) prefix-grant 短路在 (b) classify 之前(`check_prefix_grant` 在 `classify_prefix` 之前),若先上阶段 2 的拆分器而不收紧 grant,复合命令仍会被 (a) 短路放行,拆分成果到不了。所以顺序是 P1 堵安全缺口(grant 绕过 + `>` 静默写)→ P2 恢复体验 + 精度。

### 阶段 1（P1)— grant 短路收紧 + `>` 写重定向检测(先堵安全缺口)

**目标**:立刻堵住两个**现状已有**的安全缺口——缺口 A(grant 绕过)+ `>` 写重定向静默放行。

**做什么**:两件都补安全、都只动 `classify_prefix` 前后的检测、都 < 20 行,适合同 PR 同回归:

1. **grant 短路收紧**:Tier 4 Shell 分支 (a) prefix-grant 短路(`check_prefix_grant`)加前置——命令含结构元字符(`;`/`&&`/`|`)时**不享受短路**,强制回落 (b) classify。同步覆盖 worker 的 per-run prefix grant(`check_prefix_grant` 的 worker 分支)。
2. **`>` 写重定向检测**:`classify_prefix` 的结构检测(`has_structural_metachar`)补检 `>`——命令含写重定向(`>`/`>>`/`&>` 到文件)时,整条命令**至少 SideEffect**(覆盖 `git diff > patch.txt` 这类"只读命令 + 写文件"的静默写漏判)。**fd 复制 `2>&1`/`>&N` 不升档**(无文件副作用),输入重定向 `<`/`<<`/`<<<` 不升档(纯读)。

**收的缺口**:A(grant 绕过)+ 现状 `>` 静默写漏判(独立缺口,非 P2 副产品——P2 上线前 LLM 任何一次 `>` 写重定向都会触发)。

**与审批的结合**:只动 (a) 短路门槛 + `classify_prefix` 结构检测,不动 grant 表 schema、不动 modal。用户对 `ls` 的"始终允许"从"任何 ls 开头"收窄为"结构简单的 ls";`git diff > x` 从静默放行变为 SideEffect(Plan 模式从静默写变弹窗——这是**堵漏判的正确变化**,非体验回退;Edit 模式行为不变,SideEffect 在 Edit 仍静默 Allow)。

**代价**:复合命令在现状 classify 下仍一律 Ask(要等 P2 拆分精细化)。所以 P1/P2 最好同批合,或 P1 紧跟 P2。

**回归要点**:`ls`(单条)+ grant 仍 Allow;`ls; rm`+ grant 不再短路、回落 classify→Ask;`git diff > x`→SideEffect;`cmd 2>&1 | head` 的 `2>&1` 不误升;`cat < /etc/hostname` 的 `<` 不升;worker 路径同款。

**交付物**:`check` (a) 分支前置条件 + `has_structural_metachar` 加 `>` 检测 + 回归测试。

---

### 阶段 2（P2)— classify 精细化(复合命令拆分 + 取 max)

**目标**:堵住缺口 B(只读管道误伤),同时恢复 P1 牺牲的体验。

**做什么**:把 `classify_prefix` 的一刀切结构降级(`has_structural_metachar`),换成"复合命令拆分 + 子命令独立判定取最危险档"。具体职责(不讲算法):

- 按**顶层** `;`/`&&`/`||`/`|` 把命令拆成多个子段(尊重引号/转义,引号内的不拆);
- 对每个子段独立跑现有 first-token 分类(复用白名单 + git 子命令表);
- 对每个子段独立跑 P1 的写重定向检测(语义同 P1:`>`/`>>`/`&>` 到文件升 SideEffect,`2>&1`/`>&N` 不升,`<`/`<<` 不升);
- 检测命令替换(`$()`/反引号)→ 整条降级 Ask(内容不可静态知)。v1 一律 Ask 是 fail-safe;远期可递归分析 `$()` 内子命令是否全只读以降低误伤(如 `echo $(date)`),但**不按外层命令放宽**——命令替换在 shell 展开阶段执行,`echo $(rm x)` 会真的删文件(实测确认),按外层 echo 放宽是危险误判。
- 整条命令的档位 = 各子段档位的 max(Ask > SideEffect > ReadOnly)。

**收的缺口**:B(只读管道误伤)+ 把 P1 的"复合命令一律 Ask"细化成"按真实组成判定"。

**与审批的结合**:
- `ls | grep` → [ReadOnly, ReadOnly] → **ReadOnly**,所有模式静默 Allow(Plan 模式最受益)。
- `git diff | head` → ReadOnly,Plan 静默放行。
- `ls; rm x` → [ReadOnly, Ask] → Ask,弹窗(缺口 A 在 P1 已堵,P2 让它判得准)。
- `git diff > patch.txt` → ReadOnly + 写重定向 → **SideEffect**,Plan 弹窗 / Edit 静默 Allow。(`>` 写重定向检测已在 **P1** 落地,堵现状静默写漏判;P2 在拆分后对每个子段跑同样的检测。)
- 现有白名单表 / git 只读子命令表 / 测试套件**全部复用**,只是被调用 N 次(每子段一次)而非 1 次。

**代价**:新增一个拆分器(自研,零依赖,契合项目"自研 SSE / 自研 Provider trait"风格)。引号/转义的 corner case 需要测试覆盖。变量展开 / `eval` / alias 仍判定不了——遇到 `$()`/反引号一律 Ask 兜底。

**回归要点**:测试矩阵覆盖纯读复合 / 读+写复合 / 写重定向 / 命令替换 / 引号内元字符 / 空命令 / 单条命令(不得回归现有 first-token 行为)。

**交付物**:`shell_trust.rs` 拆分器 + classify 改造 + 测试矩阵。

---

### 阶段 3（P3)— 执行期沙盒兜底(鲁棒性,远期)

**目标**:判定错了也限损。从"判定准"转向"即便不准也无害"。

**做什么**:把命令关进文件系统沙盒(候选 bubblewrap / firejail / 自研 overlayfs),可写层只覆盖项目目录 + tmp,联网默认禁或按白名单。判定(阶段 1/2)仍是入口,沙盒是执行期的兜底——即使判定为 Allow 且实际有副作用,损害被限制在沙盒可写路径内。

**收的缺口**:静态判定的固有盲区(变量展开 / `eval` / alias / 间接副作用如 `curl /delete`)。这是静态分析永远搞不定的部分,只能靠隔离。

**与审批的结合**:这是 Tier 4 **之外**的新层(执行隔离,不是判定)。引入新的 Mode/UX 决策——只读沙盒 vs 读写沙盒 vs 放行三态,需要重新设计(对齐 Claude Code Bash tool 的 sandbox 模式)。**这是本方案里唯一动 Mode/UX 的部分**,也是最重的一步。

**前置 spike**:WSL2 下 bubblewrap 的 user namespace 可用性需先验证(WSLg/微软内核对 unprivileged userns 的支持有历史坑)。若不可用,降级到自研 overlayfs 方案或 firejail。

**代价**:大。引入新依赖、新 UX、WSL 环境验证、沙盒误杀调试。收益是"判定层再也不用追求完美"。

**回归要点**:沙盒内写项目外路径应失败;联网默认禁;沙盒内命令正常完成不误杀。

**交付物**:独立 spike + 独立任务,可能拆成 P3a(spike)/ P3b(只读沙盒)/ P3c(读写沙盒 + UX)。

---

### 远期候选(非承诺)

- **tree-sitter-bash AST**:若 P2 的自研拆分器遇到引号/heredoc/进程替换的盲区,升级到 tree-sitter-bash 做 AST 遍历。代价是重依赖,收益是 corner case 更准。先做 P2,遇盲区再评估。
- **LLM 自标注 + 校验**:让 LLM 在 tool_use 自带 readonly/sideeffect 标注(类似 MCP `readOnlyHint`),权限层抽查校验。判定成本转给最懂意图的一方,但需静态校验兜底,本质仍是 P2 之上的一层优化。

---

## 5. 风险与权衡

| 风险 | 说明 | 缓解 |
|---|---|---|
| **静态判定固有盲区** | 变量展开 / `eval` / alias / `$()` 静态不可知 | P2 对 `$()`/反引号一律 Ask 兜底;P3 沙盒做最终隔离 |
| **拆分器引号误判** | 引号内元字符拆错(false positive 误降级 / false negative 漏拆) | false positive 安全(降级 Ask,用户可放行);false negative 靠测试矩阵 + kill-list 兜底 |
| **P1 单独上线的体验回退** | P1 后、P2 前复合命令一律 Ask | P1/P2 同批合,或 P1 紧跟 P2(间隔内是已知短期体验损失) |
| **grant 语义变更的存量数据** | 已有的 `prefix` grant 行在新规则下覆盖面缩小 | 无需迁移——grant 表数据不变,只是短路前置收紧;用户既有授权仍对单条命令生效 |
| **现状 `>` 写重定向静默放行**(P1 收) | 现状 `has_structural_metachar` 不检 `>`,`git diff > patch.txt` 被 ReadOnly 静默放行,Plan 模式下静默写文件 | **P1 收**:与 grant 收紧同批,在 classify 加 `>` 写重定向检测(升 SideEffect)。优先级高于 P2 的拆分器引号风险——这是现状已有攻击面,P2 是新增风险 |
| **`2>&1`/`>&N` 误升 SideEffect** | fd 复制无副作用,若重定向检测按 `2>` 子串匹配会误伤 `cmd 2>&1 \| head` | P1 检测只升"重定向到文件"(`>`/`>>`/`&>` 后跟路径),不升 `2>&1`/`>&N`/`1>&2` |
| **沙盒在 WSL 不可用** | bubblewrap 依赖 user namespace,WSL 内核支持有坑 | P3 前置 spike;不可用则降级方案 |
| **性能** | 拆分器每条 shell 命令跑一次 | 命令长度有界,单遍扫描,开销可忽略 |

---

## 6. 落地指征(实施时该做什么)

本方案 P1+P2 已实施(2026-07-04),P3 未排期。若继续 P3,实施时:

1. **建 Trellis 任务**(按阶段拆,P1+P2 可一个任务,P3 独立):`prd.md`(要解决的具体场景 + 不变量)+ `design.md`(拆分器的状态机、引号处理、取 max 的偏序、grant 短路新前置的精确条件)+ `implement.md`(分步 + 回归测试矩阵 + 回滚点)。
2. **选型 ADR 记** [IMPLEMENTATION §4 决策日志](../IMPLEMENTATION/decisions.md):自研拆分器 vs tree-sitter-bash vs 沙盒优先,讲清为什么先做 P1+P2 再 P3。
3. **回归测试矩阵**(P2 的核心交付物):
   - 纯读单条(`ls`/`git diff`)→ ReadOnly,不回归
   - 纯读复合(`ls | grep`/`git diff | head`)→ ReadOnly(新)
   - 读+写复合(`ls; rm`/`git diff && cargo build`)→ Ask
   - 写重定向(`git diff > x`)→ SideEffect(**P1** 收);`2>&1`/`<` 不误升(**P1**)
   - 命令替换(`ls $(rm x)`)→ Ask
   - 引号内元字符(`echo "a;b"`)→ 不误拆
   - grant 短路(`ls`+grant → Allow;`ls; rm`+grant → 不短路)
   - worker 路径同款覆盖
4. **spec 更新**:`.trellis/spec/backend/tool-contract.md` "Scenario: Path-based Permission" 段补复合命令判定契约。
5. **完成后**:ROADMAP A2+ 条目从第三档移到 §1.2 已实施,加 commit hash。

---

## 附录:术语对照

| 术语 | 含义 | 出处 |
|---|---|---|
| 9 关 / Tier 1-6 | 权限决策 6 层管线 | [ARCHITECTURE §2.2](../ARCHITECTURE.md) |
| kill-list | Tier 2 灾难性命令硬墙 | `dangerous.rs` |
| grant / "始终允许" | 用户授权持久化(`session_tool_permissions`) | `check.rs` Tier 4 |
| 三档 ReadOnly/SideEffect/Ask | shell 命令信任分级 | `shell_trust.rs` |
| Mode (edit/plan/yolo) | 会话模式,B7 UX 层 | [ROADMAP §4.2](../ROADMAP.md#42-b7--mode-是-a2-权限系统的-ux-层) |
