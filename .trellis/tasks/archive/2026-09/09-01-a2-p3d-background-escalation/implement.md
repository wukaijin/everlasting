# Implement — 执行记录(P3d)

## 落地形态:单 PR(后端 + 前端小修 + 测试)+ docs commit

### 后端

- `agent/permissions/escalation.rs`:`ask` / `audit_grant_rerun` 增首参
  `tool_name`(前台传 `"shell"` 现状不变);`stderr_evidence_line` 改
  `pub(crate)` 供 registry 等待任务复用。
- `tools/mod.rs`:`ToolContext.tool_use_id: Option<String>` 新字段,dispatch
  对所有工具统一盖章(`tools.rs` per-call clone 处);init.rs 构造点 `None`。
  E0063 波及 ~35 处 struct literal(测试为主),脚本按编译器坐标批补。
- `background_shell/mod.rs`:`BackgroundShellNotification.escalation:
  Option<EscalationOffer>` + `EscalationOffer/EscalationBlock`(serde
  snake_case)+ `SandboxBlockKind` ↔ `EscalationBlock` 互转;trait `start`
  尾参 `origin_tool_use_id`。
- `background_shell/in_memory.rs`:entry 留 origin;start 折叠
  `escalation_origin`(沙盒 ∧ origin 才有值);等待任务 Normal+Failed 下
  classify_block 命中 → 烘 offer;inherent `escalation_source` getter
  (command/cwd/max_runtime,entry 预留字段首次有了消费者)。
- `tools/run_background_shell.rs`:`ctx.tool_use_id` 传入 start。
- `agent/chat_loop/background_escalation.rs`(新):`resolve_all` +
  `plain_text`(legacy 逐字节锚);门序 = Plan → entry 可查 → grant-hit →
  ask → 重跑;重跑 `start(sandbox=None, origin=None)`;重跑 spawn 失败
  如实上报文本。
- `agent/chat_loop/drive.rs`:drain 后接 `resolve_all`,注入循环消费
  终态文本(删原内联 format)。

### 前端(推翻 PRD"零改动")

- `ShellCard.vue`:`isPendingApproval` = ask 挂卡 ∧(!hasResult ∨
  isBackground)——后台升级卡挂在已有结果的卡上;statusText/icon 让
  ask 优先(前台无行为变化)。回归锚双向(前台 hide-on-result 保留 +
  P3d done 卡渲染审批区)。

### 测试

- `chat_loop::background_escalation::tests` 九例(offer 为纯数据,无内核
  依赖):legacy 格式锚 / plain 透传 / approve(卡挂原 toolUseId + 新 bsh
  + ask 审计行)/ deny(指引 + 无重跑)/ grant-hit(零卡 + ToolAllowed
  一行)/ 复合不享 grant / Plan 门 / entry 消失降级 / 重跑 spawn 失败。
- `background_shell::in_memory` 两例内核门控:offer 烘焙(真 Landlock 面
  外失败 → tool_use_id/block/evidence + escalation_source)/ 负矩阵
  (无沙盒或无 origin → None)。
- `tests_escalation.rs` resolver 竞态修复(见 gotcha)。

## 计划外修复 / Gotchas

1. **ask.rs 先 emit 后 register 的竞态**:mock resolver 首轮 `resolve_ask`
   合法返回 false(oneshot 未注册),原实现单发即退 → 并行负载下 120s 超时
   赢竞态(`escalation_approve_reruns_once_without_second_card` 实测 flaky)。
   修复 = 重试到 ok(true)。生产前端无此问题(人类点击延迟 >> 注册窗口)。
2. **root 环境探针失效**:`touch /proc/1/mem` 以 root 必然成功(rerun
   exit 0),测试前提"OS 恒拒"只在非 root 成立 → `geteuid()==0` 大声 SKIP
   (循内核探测 SKIP 纪律)。本机 root 环境首次暴露;用户机非 root 不受影响。
3. **工具函数参数膨胀**:run_background_task 新参折叠为单一
   `escalation_origin: Option<String>`(沙盒 ∧ origin),维持 clippy 7 参内。
4. **daemon 烟测 WARN 排查**:首跑 `--sandbox-probe` WARN 无审计行 = LLM
   该轮没调 shell(output 仅 11 token),非策略降级;重跑即
   `sandbox audit ok: 1 row`。烟测 WARN 先查 LLM 行为再查策略。

## 验证

- 后端 `cargo test -p everlasting --lib`:2215 passed / 0 failed
  (tunnel remote_cancel 单跑过 = 既有并行 flake,与本改动无关)。
- clippy --lib --tests 干净;cargo fmt 干净。
- 前端 vitest 1507 passed(2 个 root/并行环境 flake,单跑均过);
  vue-tsc + build 绿;Playwright e2e 7/7(Chromium v1234 重装后)。
- live:daemon 重建重启后 `turn-smoke.sh --sandbox-probe` OK(沙盒审计行
  落库 + 无误杀);内核门控集成测试在本机真 Landlock 实跑非 SKIP。
