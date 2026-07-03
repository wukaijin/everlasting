# A2+ Shell 命令只读/副作用精细判定

> **角色**:parent/umbrella task。持有 `docs/A2-SHELL-CLASSIFICATION.md`(2026-07-03 草案)为 source requirement set,**不作 implementation target**。本 prd 只登记 source requirement、task map、cross-child 不变量与验收;技术设计/执行清单落各子任务的 `design.md` / `implement.md`。

## Source Requirement

- **方案文档**:[`docs/A2-SHELL-CLASSIFICATION.md`](../../../docs/A2-SHELL-CLASSIFICATION.md)(单一 source of truth;本文档不重复其细节)
- **起草**:2026-07-03;**登记**:2026-07-04
- **方案状态**:草案 / proposal(未排期),经双 review 修订(commit `fbb7ced` / `247ed68`)
- **落点**:`app/src-tauri/src/agent/permissions/`(尤其 `shell_trust.rs` / `check.rs`)

## 背景(一句话)

现状 `shell_trust::classify_prefix` 用"首 token 分类 + 一刀切结构降级",带两个同源缺口:A——复合命令 grant 绕过(安全,`ls; rm` 借 `ls` 的"始终允许"短路放行);B——只读管道误伤(体验,`git diff | head` 在 Plan 反复弹窗)。详见方案 §1。

## Task Map

| task | 范围 | 状态 |
|---|---|---|
| **本 parent** | source requirement + cross-child 不变量 + 最终集成 review | planning |
| 子任务 [`07-04-a2-shell-p1p2-classify`](../07-04-a2-shell-p1p2-classify/prd.md) | **P1+P2**:grant 短路收紧 + `>` 写重定向检测 + classify 复合命令拆分取 max(含 worker 路径) | planning |
| 子任务(P3,远期,**暂未建 task**) | 执行期沙盒兜底(bubblewrap/overlayfs/firejail),前置 WSL userns spike | 未排期 |

> **P3 不在本 parent 当前交付范围**:它落在 Tier 4 **之外**(执行隔离层),引入新 Mode/UX(只读沙盒 / 读写沙盒 / 放行三态),且依赖 WSL userns 可用性 spike。spike 有结论后再建独立 task(可能再拆 P3a spike / P3b 只读沙盒 / P3c 读写沙盒+UX)。

## Cross-child 不变量(所有子任务不得违反)

来自方案 §2.2,A2+B7 落地时锁定的契约:

1. **Tier 2 kill-list 无条件前置**:`dangerous.rs` 10 条灾难性模式在所有 Mode(含 Yolo)硬拒,任何子任务不动它。
2. **Tier 4 Yolo bypass 整层**:Yolo 跳过 Tier 4(含本方案新判定),只由 Tier 2 兜底。
3. **Mode 三档语义不变**:`ReadOnly` 三档静默 / `SideEffect` 看 Mode / `Ask` 两档弹窗。
4. **grant 表三 match_kind 不变**(`tool` / `prefix` / `path`):不改 schema,只改 `prefix` 的短路前置条件。
5. **审计 17 类 AuditKind 不变**:可加细分 reason,不加新 Kind。
6. **shell 是异构工具**:仍只判"启发式可信度",不确定就降级 `Ask` 的总原则不变。
7. **执行层加固正交、不动**:`shell.rs` 的 `env_clear()` + 白名单重注入(RULE-E-001)+ `process_group(0)` + `killpg`(RULE-E-002)已落地,本方案只改判定层。

## Cross-child 验收标准(parent 视角)

子任务各自有更细的 acceptance criteria,parent 层只看整体:

- [ ] **缺口 A 收紧**:用户对 `ls` 的"始终允许"不再覆盖 `ls; rm` 类复合命令(grant 短路对结构元字符失效)。
- [ ] **缺口 B 恢复**:`git diff | head` / `ls | grep foo` / `cat x | wc -l` 在所有模式静默放行。
- [ ] **`>` 写重定向漏判收口**:`git diff > patch.txt` 至少 SideEffect(Plan 弹窗、Edit 静默 Allow),不再被 ReadOnly 静默放行。
- [ ] **worker subagent** 的 per-run prefix grant 同步收紧(同款前置条件)。
- [ ] §2.2 七条不变量全部保持(回归测试通过)。
- [ ] 方案 §6 回归测试矩阵全绿(纯读单条 / 纯读复合 / 读+写复合 / 写重定向 / `2>&1` 不误升 / 命令替换 / 引号内元字符 / grant 短路 / worker 路径)。
- [ ] **spec 更新**:`.trellis/spec/backend/tool-contract.md` "Scenario: Path-based Permission" 补复合命令判定契约。
- [ ] **ROADMAP**:A2+ 条目从第三档移到 §1.2 已实施 + commit hash。
- [ ] **ADR**:选型决策(自研拆分器 vs tree-sitter-bash vs 沙盒优先)记入 `docs/IMPLEMENTATION.md §4`。

## 非目标(parent 范围)

来自方案 §2.3:

- 不追求完美判定(shell 图灵完备,静态不可判定)。
- 不动 read 族 path 工具(`read_file` / `grep` / `glob` / `list_dir`)。
- 不改 Mode 枚举(`edit` / `plan` / `yolo`)。
- 不引入新 IPC / 前端组件(PermissionModal 仍是展示终端)。
- **P3 沙盒不在本 parent 当前交付**(远期,独立 task)。

## Notes

- **子任务依赖顺序**:P1 必须先于或同于 P2(方案 §4:Tier 4 的 (a) prefix-grant 短路在 (b) classify 之前,先上 P2 拆分器而不收紧 grant,复合命令仍会被 (a) 短路放行)。所以 P1+P2 同任务、同 PR 是最自然拆法——本 parent 已据此只建一个子任务。
- **parent 何时完成**:子任务全部完成 + cross-child 验收清单打勾 + 集成 review 通过,即可 archive parent(parent 自身无代码交付)。
