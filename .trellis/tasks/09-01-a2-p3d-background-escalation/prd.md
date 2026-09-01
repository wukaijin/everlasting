# A2+ P3d 后台 shell 升级闭环:run_background_shell 面外失败 → 审批 → 一次性不沙盒重跑

## Goal

补齐 P3c 留下的最后一个沙盒 UX 不对称:前台 `shell` 面外失败有完整升级闭环
(prefix-grant → Ask 卡 → 一次性不沙盒重跑),后台 `run_background_shell` 只有
指引文案、模型介导。本任务把同一闭环带到后台路径,**呈递时机 = 下轮注入时**
(drain 后、LLM 前),呈递方案用户已裁定(2026-09-01,B 案)。

## Background

- P3c 把 per-project 默认档改为 `readwrite` = 全命令进沙盒(行为变更)。
  后台命令面外失败(写被拒 / 断网 EPERM)只会在 stderr 留下 EACCES/EPERM 痕迹
  + 失败通知,用户没有恢复入口,LLM 只能换路径绕。
- 后台 shell 完成时前端今天就没有任何主动提示(ShellCard 不追踪后台状态,
  通知仅下一轮注入给 LLM)。本任务不改变这一现状(非回归,补洞另立)。
- ask 卡经 `toolUseId` 匹配挂到会话内已有工具卡(ShellCard.vue:129),升级卡
  可精确挂回当初的 `run_background_shell` 卡。

## Requirements

### R1 通知携带升级载荷

- 面外失败的后台 shell,其完成通知携带升级判定所需的最小载荷:原始
  `tool_use_id`、block 分类(写/断网)、stderr 证据行;重跑所需的
  command / cwd / max_runtime_ms 经 registry 查询获得(通知保持精瘦,
  LLM 可见面不加宽)。
- 非 sandbox 启动的 shell(Decision::Skip)、正常完成、用户 kill、超时、
  spawn 失败:**不带载荷**,通知与注入文本逐字节同现状。

### R2 注入点升级闭环(drive.rs drain 之后、组装 turn_messages 之前)

对每个带载荷的通知,复用前台 §5.2 机制:

1. **门**:当轮 `mode != Plan`(Plan 的确定性只读身份不可被升级穿透,与
   前台 D3 同源);取消/超时标记的失败不升级(与前台 `!timed_out` 同源);
   载荷缺失或 registry 条目已不可查 → 降级为普通失败通知。
2. **prefix-grant 先查**(与前台同闸:复合命令结构元字符不享 grant;读侧
   本就 `IN ('shell','run_background_shell')`,跨前台/后台同一 grant 命名
   空间是有意语义)→ 命中直接不沙盒重跑、零弹卡,补 ToolAllowed 审计行
   (与前台 `audit_grant_rerun` 同形)。
3. **Ask 卡**:复用 `ask_path`(reason_override 带拦截原因 + 原命令 +
   stderr 证据),`tool_use_id` 用**原 run_background_shell 调用的 id**
   (卡挂回原 bsh 卡);120s 超时 / 取消 / 拒绝 → 视同 Denied。
4. **批准 → 一次性不沙盒重跑**:逐字节同 command/env/cwd(RULE-E-001/002
   由 registry 既有 spawn 路径保证),`sandbox=None`,产生新 `bsh_*`;重跑
   不再进升级分支(结构上一次性)。
5. **注入文本取代原始失败通知**:无论升级与否,每个带载荷的通知最终恰好
   产生一条用户消息,描述终态(已批准重跑 → 新 bsh id + 后续 shell_status
   指引;拒绝/超时 → 失败 + 指引文案)。LLM 永远只看到一个连贯故事,
   不会在升级悬而未决时对失败采取行动。

### R3 审计

- ask 分支:ask_path 既有 kinds(tool_permission_ask / permission_granted /
  tool_denied)原样落库,零新 kind。
- grant-hit 分支:ToolAllowed 行带 reason(与前台同形)。
- 重跑本身的沙盒审计行不写(重跑不沙盒,与前台一致)。

### R4 双端一致性

- worker 路径:后台 shell 由发起 session 持有(Q7 session-scoped),升级
  在该 session 的 turn 内发生,无跨 session 面。
- GUI / daemon 双 transport:本任务纯后端(agent loop 内部),不改 IPC 面;
  前端零改动(ask 卡复用既有渲染)。

## Non-goals

- 后台 shell 完成/失败的主动 UI 提示(今天即无,补洞另立)。
- 完成即弹卡(A 案,detached 生命周期,已裁定不做)。
- bwrap 增强档 / 网络白名单(ROADMAP 另列)。
- 后台 `max_runtime_ms` 超时触发的升级(超时 kill 的部分 stderr 不可信,
  与前台 `!timed_out` 同源排除)。

## Acceptance Criteria

- [x] AC1. readwrite 档后台 shell 写面外失败 → 下一条用户消息触发 turn 时
      弹卡(挂原 bsh 卡),批准 → 新 bsh 不沙盒重跑;注入文本描述重跑。
- [x] AC2. 拒绝 / 120s 超时 / turn 取消 → 注入文本 = 失败 + 模式感知指引,
      不重跑。
- [x] AC3. 先前「总是允许」前缀命中 → 零卡直接重跑 + ToolAllowed 审计行;
      复合命令(结构元字符)不享 grant,照常弹卡。
- [x] AC4. Plan 模式 turn 内不升级(注入文本 = 失败 + Plan 指引)。
- [x] AC5. 非 sandbox 启动的后台 shell 失败:通知与注入文本与现状逐字节一致
      (回归锚)。
- [x] AC6. 审计:ask 分支零新 kind,grant-hit 分支 ToolAllowed 带 reason;
      重跑无沙盒审计行。
- [x] AC7. 全量回归绿(后端 cargo test --lib + clippy/fmt),新增用例覆盖
      AC1-AC6。

> 实施备注(2026-09-01):R4 的"前端零改动"假设被证伪——升级卡挂在已有
> 结果的 run_background_shell 卡上,ShellCard 的 `isPendingApproval` 前台
> `!hasResult` 守卫会吞掉审批区;按 isBackground 豁免(vitest 双向锚)。
> 详见 design.md §1.5 / spec sandbox-executor §11。
