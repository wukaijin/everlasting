# PRD（草稿，planning 中）— durable 授权出口：多 token 前缀 grant 项目级持久化

## Goal

无 Landlock 内核用户（旧 WSL2 内核 / 旧发行版，内核 <6.7 或未编 Landlock）的 dev server 放行解法——**授权面而非执法面**：把 AllowAlways 从「首 token 粒度 + session 级」升级为「多 token 前缀 + 项目级持久化」，批准语义 = 「该命令模式可免沙箱**启动**」（结构性适配长驻进程，替代 P3c 逐字节重跑的 one-shot 缺陷）。零内核依赖，全平台可用。

产品裁定（2026-09-21，用户）：工具不应要求用户折腾内核才能用基本功能；本任务与 `09-21-sandbox-net-bindonly`（Landlock 执法面，正交互补）**并行推进**，共享 F3/remediation 地基。

## Background / Confirmed Facts

- **现状缺陷**（spec `sandbox-executor.md` §12.3 A 动机段 + §10/§11）：AllowAlways 粒度 = 首 token——批过 `pnpm dev` = 整个 `pnpm` 免沙箱（含 install 脚本）；升级重跑 = 逐字节 one-shot，长驻进程重启再批；grant 现为 session 级，不跨 daemon 重启。
- **第二场审议结论 8**（research 见 09-21-sandbox-net-bindonly/research/deliberations-2026-09-21.md）：非交互 ask 全拒的死因 = 缺 durable operator 授权出口；remediation 改「收敛到一条 operator 指令然后停」已在 sibling 任务 F3 落地——**本任务是该指引的承接终点**（用户按指引批准 → durable grant 生效 → dev server 免沙箱启动）。
- **安全边界沿用**（两场共识）：D4 双执行边界保留（第一遍在沙箱内失败，危险部分未发生才轮到批准）；审批绑定确切命令文本；免沙箱命令 = 该进程文件+网络全开（信任面比 BindOnly 宽，须在批准卡文案明示）；字符串特征永不作授权依据。
- 环境事实：本机 WSL2 6.6.114 无 Landlock（probe EINVAL）；非 Linux 平台沙箱本来 fail-open 不受拦——本任务目标用户 = 「Linux 但无 Landlock」群体。

## Requirements（初稿，正式规划时收敛）

- **R1 多 token 前缀语义**：grant 从首 token 升级为多 token 前缀（如 `pnpm --filter @jjh/web dev` 整条）；复合命令闸（`has_structural_metachar`）沿用——含管道/重定向的复合命令不进前缀匹配。
- **R2 项目级持久化**：grant 落 projects 维度，跨 session、跨 daemon 重启有效；GUI 可查看/撤销。
- **R3 长驻进程适配**：批准语义 = 该前缀命令可免沙箱启动（含 `run_background_shell` 路径）；不再依赖「失败→重跑」结构。
- **R4 授权面收口**：升级弹卡（P3c 前台 / P3d 后台）批准 AllowAlways 时落 durable grant（替代现状 session 级）；批准卡文案明示信任面（该命令免沙箱 = 文件+网络全开）。
- **R5 审计**：grant 建立/命中/撤销全量审计（复用既有 ask/audit 通道，`audit_grant_rerun` 家族）。

## Acceptance Criteria（初稿）

- [ ] 批准 `pnpm --filter @jjh/web dev` 后：同前缀命令在新 session 直接免沙箱执行（无二次弹卡）；`pnpm install`（不同前缀）仍进沙箱。
- [ ] daemon 重启后 grant 仍生效；GUI 撤销后恢复沙箱。
- [ ] 复合命令（管道/重定向）不命中前缀 grant。
- [ ] 批准卡文案含信任面明示；审计三事件（建立/命中/撤销）可查。
- [ ] 无 Landlock 内核环境端到端：dev server 被 Block 拦 → remediation 指引 → 批准 → 免沙箱启动成功（与 sibling 任务的 F3 文案衔接验证）。

## Out of Scope

- Landlock BindOnly / NetPolicy（sibling 任务 09-21-sandbox-net-bindonly）。
- AllowAll 档 / capability token（挂账不变）。
- 通配符前缀、正则匹配（如需要另立）。

## Open Questions（正式规划时解决）

1. 前缀 token 数上限与语义细节（固定 N token？命令完整文本哈希？spec §12.3 B 提示「grant 语义先从首 token 升级到多 token 前缀」的具体形态需设计）。
2. 持久化载体（新表 vs 既有 grants 表加列；projects 维度 keying）。
3. 与既有 grant kind↔类别矩阵 / prefix_grant_hit 命中路径的关系（扩展 or 平行通道）。
4. 跨平台行为差异（macOS/Windows fail-open 下沙箱不启用，grant 面是否仍统一）。
5. 需要的 spec 勘察：`agent/permissions/escalation.rs`（prefix_grant_hit / has_structural_metachar）、`permissions/check/permission.rs` grant 存储、P3c/P3d §10/§11 全文。
