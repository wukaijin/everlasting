# read 侧路径边界解耦(只读 tool 接权限层)

## Goal

让只读 tool(`read_file` / `grep` / `glob` / `list_dir`)能受控访问项目根之外的路径,拉齐 Claude Code 的 Read 能力(它默认能读项目外文件),同时不放弃审计与用户感知。

## Background — 关键发现(推翻早期判断)

早期判断"权限系统(is_within_root + Mode + 审计)已建好但 read 没用上"**不准确**。代码核查后真相是 **tool 层硬边界与权限层决策口径冲突**:

调用链(已核实):
1. `agent/chat_loop.rs:2001` `permissions::check()` 先跑 — Tier 4 `Path` 分支(`agent/permissions/check.rs:148-229` + `classify_tool` `:399-421`)对 `read_file/grep/glob/list_dir/write_file/edit_file` 一视同仁:项目外路径 → `ask_path()` 弹窗,用户可 AllowOnce / AllowAlways / Deny。
2. 用户 Allow → `Decision::Allow` → 通过 Deny 检查(`:2012`)、pitfall recall(`:2053`)→ `execute_tool`(`:2105`)。
3. `execute_tool` → `read_file::execute` → `assert_within_root`(`tools/read_file.rs:120`,锚点 `ctx.worktree_path` 即项目根)硬卡 → 项目外 → reject,返回 `path '...' rejected: ... is outside project root`。

即:**权限层的项目外 ask 是"假 ask"** — 用户在弹窗点"允许"也被 tool 层杀掉。报错措辞来自 tool 层 boundary.rs(`projects/boundary.rs:48`),不是权限层 deny。

`tools/write_file.rs:90-141` 确认写族同构(同样硬卡,且对不存在路径做了"向上走祖先再校验"特例)。本 task **不动写族**(见 Out of Scope),但需知会:write/edit 的项目外 ask 当前同样是断的(已知局限,留后续)。

## Confirmed Facts(代码已回答,不再追问)

- `check()` 在 `execute_tool()` 之前(`chat_loop.rs:2001→2105`、`:2546→2844`),Allow 之后到 execute 之间**无其他路径校验**(只有 pitfall recall,不挡路径)。
- `agent/permissions/dangerous.rs`(Tier 2 kill list)只针对 shell 命令(9 条 regex),**不含任何文件路径敏感过滤**。去掉 read 族硬卡后,敏感路径(`~/.ssh`、私钥、`.env`、`*credentials*`)只能靠权限层 ask,无硬 deny 兜底。
- `projects/boundary.rs` 两个函数分工明确:`assert_within_root`(`:28`,canonicalize + component-wise `starts_with`,tool 层硬关,不存在/broken symlink 即拒)vs `is_within_root`(`:86`,lexical、容忍不存在,权限层软判断)。
- 审计已覆盖:`AuditKind::ToolAllowed` / `ToolPermissionAsk` / `ToolDenied`(`agent/permissions/audit.rs`)三件套足以审计项目外读,不必新增 kind。
- `project-cwd-boundary.md` spec §5 把"工具内部二次 `assert_within_root`"明确定位为 defense in depth — 本 task 需同步更新该 spec:read 族不再走 defense-in-depth,权限层 ask 成为读的 source of truth。
- Mode 语义已有:`Plan` 模式 Tier 3 只拦 `write_file`/`edit_file`(`check.rs:98-125`),不拦读;`Yolo` 模式 Tier 4 整层 bypass(`:134-144`)→ 项目外读直接 Allow。

## Requirements

- R1 **去掉 read 族的 tool 层硬边界**:`read_file` / `grep` / `glob` / `list_dir` 的 `execute()` 内不再调用 `assert_within_root` 拒绝项目外路径。读不存在文件自然走 `tokio::fs` IO error(不再返回 "cannot be resolved" / "outside project root" 这两类边界错误)。
- R2 **权限层决策对读真正生效**:项目外读在 `edit`/`plan` 模式弹窗 ask(`ask_path`,已有逻辑),`yolo` 模式放行 + 审计。权限层**零改动**为目标(若实现中发现 `ask_path` 对 read 有隐藏假设,最小修订)。
- R3 **审计完整**:项目外读的 Allow / Ask / Deny / Timeout 各落对应 `AuditKind` 行(payload 带 path,C4 审计 UI 可见)。**复用现有 `ToolAllowed` / `ToolPermissionAsk` / `ToolDenied` / `ToolDeniedYolo`,不新增 kind**(撤回早期"新增 `ReadOutsideRoot`"的想法)。
- R4 **写族不动**:`write_file` / `edit_file` 的 `assert_within_root` 硬边界保留。本 task 不修 write 族"假 ask"(Out of Scope),但 PRD 明确记录该已知局限。
- R5 **spec 同步**:更新 `.trellis/spec/backend/project-cwd-boundary.md`,明确 read 族不再走 tool 层 defense-in-depth;保留 write 族 defense-in-depth 条目。
- R6 **敏感路径硬 deny-list**(Q1 决策 A2):新增路径维度的敏感路径过滤(对标 `dangerous.rs` 的 Tier 2 kill list,但针对文件路径),命中即硬 `Deny`、**含 yolo 模式**、不可绕过(无"高危确认"解锁窗)。放权限层 Tier 2 附近(`check.rs`),read 族共享;审计复用 `ToolDenied` / `ToolDeniedYolo`。覆盖范围 = **中等档**(私钥 + `.env` / `credentials` / AWS·netrc·npmrc·docker 凭证,具体 pattern 清单见 design.md)。**仅对项目外路径生效**(Q1.2 决策:项目内 `.env` / `*.pem` 信任不挡);"项目外"判定以 `ctx.worktree_path`(项目根)为准,**需把 `worktree_path` 扩进 `PermissionContext`**(权限层现状只有 `ctx.cwd`)。
- R7 **受信项目外 allow-list**(用户新增):`~/.config/everlasting/**` 路径直接放行 — 免 `ask_path` 弹窗,直接 `Allow` + 写 `ToolAllowed` 审计(payload 带 path)。适用 read 族全部 tool、所有 Mode(plan/edit/yolo)。这是 deny-list 的对称面("项目外但受信" vs "项目外且危险")。**优先级:deny-list > allow-list > ask**(敏感硬墙优先;实际两者 pattern 不重叠)。本 task hardcoded 这一条(对标 `dangerous.rs` 的 static `DENY_PATTERNS`);用户可配置 UI 留后续(OOS)。
- R8 **`~` home 展开**(review 发现的 gap):read 族 + 权限层 abs_path 用新 helper `projects::boundary::resolve_path` 展开 `~` / `~/...` → home dir。这是 R7 allow-list 实用的硬前提(LLM 自然传 `~/...`,否则解析成 `<cwd>/~/.config/...` 读不到)。6 处调用统一(read 族 4 + `check.rs` 2)。

## Acceptance Criteria

- [ ] AC1 `read_file` 能读取项目根外的真实文件(如 `/home/carlos/.config/everlasting/commands/test-b3.md`),内容正常返回。
- [ ] AC2 `edit`/`plan` 模式下项目外读触发 `permission:ask` 弹窗;用户 AllowOnce → 读取成功;Deny → 返回 deny reason(`is_error: true`);120s 超时 → Deny。
- [ ] AC3 `yolo` 模式下项目外读直接成功 + 写一条 `tool_allowed` 审计行。
- [ ] AC4 项目外读的 Allow / Ask / Deny 在 `session_audit_events` 表各留对应 kind 行。
- [ ] AC5 `grep` / `glob` / `list_dir` 项目外路径同样可受控访问(各一条手动验证)。
- [ ] AC6 读不存在的项目外文件返回 IO error(如 "No such file or directory"),不再返回 "outside project root" / "cannot be resolved"。
- [ ] AC7 `write_file` / `edit_file` 项目外写入仍被 `assert_within_root` 拒绝(行为未回归)。
- [ ] AC8 现有 boundary / read_file / permissions 测试全绿;为 read 族新增"项目外可读 + 各 Mode 行为"单测。
- [ ] AC9 `project-cwd-boundary.md` spec 已更新 read/write 族分工。
- [ ] AC10 项目外 `~/.config/everlasting/**`(如 `~/.config/everlasting/commands/test-b3.md`)直接读取成功 + 写 `tool_allowed` 审计,**无 `permission:ask` 弹窗**;`yolo`/`edit`/`plan` 三模式一致。
- [ ] AC11 `~/.config/everlasting/**` 下若存在命中 deny-list 的文件(理论上不会发生,因 pattern 不重叠),deny 仍优先于 allow(优先级回归测试)。
- [ ] AC12 LLM 传 `~/.config/everlasting/commands/test-b3.md`(`~` 形式,非绝对路径)能正确展开并命中 allow-list,免 ask 读取成功(回归 `tier4_allow_trusted_external_with_tilde_form`)。

## Out of Scope

- 修 write/edit 族的同构"假 ask"(write 项目外 ask 通过后仍被 tool 层拒)— 留后续 follow-up task。本 task 仅保证 write 行为不回归。
- 敏感路径 **redaction**(读到的密文打码后再进 context)— 区别于 R6 的"硬 deny 不让读",redaction 是"让读但抹掉敏感字段",本 task 不做。
- `web_fetch` / `shell` 的边界(它们走各自的 URL / 命令维度,不在 Path 族解耦范围)。
- worktree / subagent 场景下"项目外"语义重定义(worktree_path vs project_root 的差异)— 现状沿用 `ctx.worktree_path`,本 task 不改。
- trusted-allow-list / deny-list 的**用户可配置 UI**(本 task 两个 list 均 hardcoded static;UI + 持久化留后续)。

## Decision Provenance（brainstorm 决策 → 需求映射）

- 策略基调 = 权限层 ask + 敏感路径硬 deny-list(含 yolo,不可绕过) → R6。
- deny-list 覆盖 = **中等档**(私钥 + `.env` / credentials / 云凭证,具体 pattern 见 `design.md` §3) → R6。
- deny-list **仅项目外**生效(项目内 `.env` / `*.pem` 信任) → R6;"项目外"以 `ctx.worktree_path` 为准 → `design.md` §5。
- 受信项目外 allow-list `~/.config/everlasting/**`(用户新增) → R7。
- 审计不新增 kind(复用 `ToolAllowed` / `ToolDenied` 等) → R3。
