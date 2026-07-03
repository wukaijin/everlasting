# A2+ P1+P2:grant 短路收紧 + classify 复合命令拆分取 max

> **parent**:[`07-04-a2-shell-classification`](../07-04-a2-shell-classification/prd.md) · **source**:[`docs/A2-SHELL-CLASSIFICATION.md`](../../../docs/A2-SHELL-CLASSIFICATION.md) §4 阶段 1+2(本文档不重复方案细节,只固化需求/约束/验收)
>
> 本 prd 只放 requirements / constraints / acceptance criteria。拆分器状态机、引号处理、取 max 偏序、grant 短路新前置的**精确条件**落 `design.md`;分步执行 + validation 命令 + 回滚点落 `implement.md`。

## Goal

把 `shell_trust::classify_prefix` 的"首 token 分类 + 一刀切结构降级",换成"复合命令按真实组成判定 + grant 短路对结构元字符失效",一次性收掉安全缺口 A、体验缺口 B、以及现状 `>` 写重定向静默放行的独立漏判。**P1+P2 同 PR**(方案 §4:先上 P2 拆分器而不收紧 P1 grant,复合命令仍会被 (a) 短路放行,拆分成果到不了)。

## 要解决的具体场景(Requirements)

### R1 · 安全缺口 A:复合命令 grant 绕过(必须收)

- **现状**:用户对 `ls` 点过"始终允许"后,`ls foo; rm -rf ~/notes` 首 token 仍是 `ls` → Tier 4 Shell (a) prefix-grant 短路(`check.rs:298`)命中 → 直接 Allow,**结构降级根本没跑**。`~/notes` 在项目外,git 救不回;Tier 2 kill-list 故意不挡 `rm -rf <非根>`(`dangerous.rs` 注释明示"deliberately narrow")。
- **要**:命令含结构元字符(`;` / `&&` / `||` / `|`)时**不享受 prefix-grant 短路**,强制回落 (b) classify。worker subagent 的 per-run prefix grant(`check.rs:308`)同款前置。
- **心智对齐**:授权 `ls` = 信任结构简单的 `ls`,**不包括** `ls; rm`。

### R2 · 体验缺口 B:只读管道误伤(必须恢复)

- **现状**:`git diff | head`、`ls | grep foo`、`cat x | wc -l` 因含 `|` 被一刀切降级 `Ask`(`shell_trust.rs:365`),Plan 模式(本就是只读分析会话)每次都要放行。
- **要**:纯读管道/命令链在所有模式静默放行(ReadOnly)。

### R3 · 现状 `>` 写重定向静默漏判(P1 顺手收,独立缺口)

- **现状**:`shell_trust.rs:365` 结构检测只查 `|` / `&&` / `;`,**不查 `>`**。`git diff > patch.txt` 走 first-token = `git diff` → ReadOnly → Plan 模式下**静默写文件**。
- **要**:命令含写重定向(`>` / `>>` / `&>` 到文件)时,整条命令**至少 SideEffect**(Plan 弹窗 / Edit 静默 Allow)。
- **边界**(fail-safe 精度):
  - `2>&1` / `>&N` / `1>&2`(fd 复制,无文件副作用)→ **不升档**。
  - `<` / `<<` / `<<<`(输入重定向,纯读)→ **不升档**。

### R4 · 复合命令精细化判定(P2,恢复 R2 + 精化 R1)

把一刀切结构降级换成:按**顶层** `;` / `&&` / `||` / `|` 拆段(尊重引号/转义,引号内元字符不拆)→ 每子段独立跑现有 first-token 分类(复用白名单 + git 子命令表)+ P1 写重定向检测 → 整条档位 = 各子段 max(`Ask > SideEffect > ReadOnly`)。

**命令替换特例**:`$()` / 反引号 → 整条降级 `Ask`(v1 一律 Ask 是 fail-safe)。**不按外层命令放宽**——命令替换在 shell 展开阶段执行,`echo $(rm x)` 会真的删文件,按外层 `echo` 放宽是危险误判。

## Constraints(不得违反的不变量,方案 §2.2)

子任务实现必须保持:

1. **Tier 2 kill-list 无条件前置**——`dangerous.rs` 10 条模式不动,仍是最后一道硬墙。
2. **Tier 4 Yolo bypass 整层**——Yolo 跳过 Tier 4(含新判定),只由 Tier 2 兜底。
3. **Mode 三档语义不变**——产出仍是 `ReadOnly` / `SideEffect` / `Ask` 之一,Mode 映射规则原样复用。
4. **grant 表三 match_kind 不变**(`tool` / `prefix` / `path`)——**不改 schema**,只改 `prefix` 短路的前置条件;存量 grant 数据无需迁移。
5. **审计 17 类 AuditKind 不变**——可加细分 reason(如"复合命令拆分后命中危险子命令"),不加新 Kind。
6. **执行层加固不动**——`shell.rs` 的 `env_clear()` / `killpg` 等正交,本任务不碰。
7. **"不确定就 Ask" 总原则不变**——shell 静态不可判定,启发式 + 兜底,不追求完美。

## Acceptance Criteria(回归测试矩阵,方案 §6)

`cargo test`(带 `PKG_CONFIG_PATH`)全绿,且覆盖以下矩阵(落地为 `shell_trust.rs` / `check.rs` 内联 `#[cfg(test)]`):

- [ ] **纯读单条不回归**:`ls` / `git diff` / `cat x` → ReadOnly。
- [ ] **纯读复合 → ReadOnly(新)**:`ls | grep foo` / `git diff | head` / `cat x | wc -l` → ReadOnly。
- [ ] **读+Ask 段 → Ask**:`ls; rm x` / `ls || rm x`(rm 段 = Ask,max 升 Ask)。
- [ ] **读+SideEffect 段 → SideEffect**:`git diff && cargo build`(cargo 段 = SideEffect;Plan 弹窗 / Edit 静默 Allow;旧一刀切判 Ask,是 P2 精化的预期变化)。
- [ ] **写重定向 → SideEffect(R3)**:`git diff > patch.txt` / `echo hi >> log` / `cmd &> f` → SideEffect。
- [ ] **`2>&1` / `<` 不误升(R3 边界)**:`cmd 2>&1 | head` 的 `2>&1` 不升档;`cat < /etc/hostname` 的 `<` 不升档。
- [ ] **命令替换 → Ask**:`ls $(rm x)` / `` ls `rm x` `` → Ask。
- [ ] **引号内元字符不误拆**:`echo "a;b"` / `grep "a|b" f` → 按 echo/grep 单条判定(不因引号内元字符拆段或降级)。
- [ ] **空命令 / 单条命令**:`""` → Ask;`ls`(单条)→ ReadOnly。
- [ ] **grant 短路收紧(R1)**:`ls` + grant → 仍 Allow;`ls; rm x` + grant → **不短路**,回落 classify → Ask。
- [ ] **worker 路径同款**:worker per-run prefix grant 对 `ls; rm` 同样不短路。
- [ ] **不变量回归**:现有 `shell_trust.rs` 测试(白名单重叠 / 三表 size 范围 / git 子命令 / path 前缀剥离)全部保持通过。

## 非目标

- **P3 执行期沙盒**(远期独立 task,不在本任务)。
- 不追求完美判定(变量展开 / `eval` / alias 静态不可知,`$()`/反引号一律 Ask 兜底)。
- 不动 read 族 path 工具(`read_file` / `grep` / `glob` / `list_dir`)。
- 不改 Mode 枚举、不引入新 IPC / 前端组件(PermissionModal 无感)。
- 不引入 tree-sitter-bash 重依赖(P2 自研拆分器;盲区再评估升级,见方案 §4 远期候选)。

## Notes

- **依赖顺序内嵌**:P1(grant 收紧 + `>` 检测)与 P2(拆分器)在同 PR 内,但实现顺序上 P1 的 `>` 检测会被 P2 拆分器"对每子段调用一次",所以 design.md 需把 `>` 检测设计成可被拆分器复用的 per-segment 函数。
- **拆分器风格**:自研、零依赖,契合项目"自研 SSE / 自研 Provider trait"风格。引号/转义 corner case 是主要风险面,靠测试矩阵覆盖。
- **完成后**:本任务 acceptance criteria 全绿 + parent 的 cross-child 验收清单打勾 + spec/ROADMAP/ADR 更新 → archive 本任务,然后 parent 做集成 review。
