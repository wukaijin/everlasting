# 09-03 全局开关:问询永不超时(ask_no_timeout)

## Goal

加一个全局「问询永不超时」总开关(app_config `ask_no_timeout`)。开启后,后端不再自动收尾两类会超时的用户问询——无限等待直到用户响应 / Stop / 删会话:

1. **权限审批**(Tier3/Tier4 权限卡):现状 120s 无响应 → 自动 Deny(`ASK_TIMEOUT`)。含 shell 沙盒升级(前台 + 后台 escalation)、worker 子代理审批——全部汇入 `permissions::ask::ask_path` 的 timeout 臂。
2. **轮数上限软卡**(撞 MAX_TURNS=200 的「继续?」浮动卡):现状 600s 无响应 → `timeout_stopped` 自动停。

对所有会话生效(含定时任务无人值守触发的会话)——用户已确认「全部会话 + 开关负责到底」。

请求类工具(ask_user_question / 任务状态变更 / 模式切换 / C2+ 干预)本就无超时(会一直挂着),不在本次改动范围。

## Requirements

- 单个总开关,新增 UI 设置在设置面板「通用」分类(GeneralTab)。
- 后端单源读法 `ask_no_timeout_enabled(db)`,方向与 kill-switch 先例**相反**:仅字面 `"true"` 开;未存/读失败 → `false`(缺省关 = 今日行为零回归)。
- 前端本地镜像计时器(`permissions.ts` `ASK_TIMEOUT_MS` 120s,会主动 deny + toast)需与后端一致:开关开则不 arm。
- 开关只影响生效后**新发出**的问询;不回溯已挂起的卡。
- 前端文案明示无人值守定时会话弹出的审批在开关开时也会无限挂起(需用户回来处理或 Stop)。

## Acceptance Criteria

- [ ] 权限审批:开关开 → 不再 120s 自动 Deny,挂起直到用户 resolve / cancel(Stop/删会话仍可解卡)。
- [ ] 权限审批:开关关(缺省)→ 行为与今日逐字节一致(120s auto-deny)。
- [ ] 轮数软卡:开关开 → 不再 timeout_stopped 自动停,挂起直到用户点选或 Stop。
- [ ] 轮数软卡:开关关(缺省)→ 行为与今日一致。
- [ ] 前端开关行渲染于 GeneralTab;切开关写 `set_app_config_flag` `ask_no_timeout`;开时前端不 arm 120s 本地 timer。
- [ ] daemon(`get_app_config`/`set_app_config_flag`)经同一 `_inner`,无需新路由。
- [ ] 测试不硬等真实时长:新增用例靠 cancel/响应收尾(天然不等);回归用例沿用 `with_ask_timeout_for_test(50ms)` 与 `EVERLASTING_SOFTCAP_TIMEOUT_MS=50` 缩短机制。

## Notes

- 无人值守无限挂起是用户确认的产品决策(2026-09-03 评审);UI 文案 + spec 明示。
- 覆盖面完整清单见 implement 阶段代码盘点:ask.rs worker(~304)/parent(~504) 两处 timeout 臂 + chat_loop.rs softcap(~1150)。
- 既有「始终允许/拒绝」与 session cancel 路径不因开关改变。
