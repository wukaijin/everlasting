# 基线固化记录（2026-08-27，HEAD=6ce9ef4）

## allow 分布快照
- 全库 `#[allow(clippy::too_many_arguments)]` **46 处**，全文见 [baseline-allow-snapshot.txt](./baseline-allow-snapshot.txt)
- 一期范围四文件：chat_loop.rs ×4、tools.rs ×2、drive.rs ×2、init.rs ×1 = **9 处**

## 测试基线（cargo test -p everlasting --lib）
| 轮次 | 结果 | 备注 |
|---|---|---|
| 满载 run#1 | 1996 passed / **1 failed** | 失败名未捕获 |
| 满载 run#2 | 1995 passed / **2 failed** | 见下 |
| 单独重跑 | 2 条全部通过 | --test-threads=2 定向 |

已知负载敏感的两条计时型测试（满载并行时可能偶发红）：
1. `daemon::server::tests::serve_daemon_keeps_serving_without_signal_past_grace_window`（daemon/server.rs:835，5s 宽限窗断言）
2. `agent::tests_subagent::plan_mode::agent_loop_dispatch_subagent_general_purpose_plan_mode_write_denied`（plan_mode.rs:211，**RULE-A-014 守护测试**，15s 时限断言——它对本任务是"必须存活的验收物"，单独跑已确认绿）

**判定：基线有效＝功能上全绿，含 2 条环境抖动项。重构后验证若遇这 2 条失败，先单独重跑裁决，不视为行为回归。**

完整日志：baseline-lib-test.log（run#2）。另注：测试运行期日志夹带 `Switched to branch 'session/sess-*'` 输出（某 session 类测试在仓库里做分支操练后回到 main），仓库当前干净停在 main，与本任务无关。

## 重构后终验裁决（2026-08-27）

| 轮次 | 结果 |
|---|---|
| B 代理收尾轮 | 1996 passed / 1 failed（plan_mode）；两条已知抖动单跑均绿 |
| 终验 run1 | 3 failed（**名单未捕获即被覆盖，无法逐一归档**——下轮起修正确保先落盘再覆盖） |
| 终验 run2 | 1996 / 1 failed＝plan_mode；日志 final-lib-verification.log |
| 终验 run3 | 同上唯一失败仍为 plan_mode |

**plan_mode 失败机理已读码确认**：`tests_subagent/plan_mode.rs:136-155` 外层是 **2 秒**挂死兜底 timeout（断言文案"times out at 15s"为旧设计残留文案失实），内层 ask 超时 50ms；满载 nproc 并行时 parent→worker→worker→parent 多轮 mock 链挤不过 2s 即误报，单独跑 0.59s 秒绿。基线 run#2 该测试同样红过 → **判定：重构前既有的计时兜底竞态，非本次引入**。裁决口径：AC2 以"全部功能用例绿 + 该测试按协议单跑绿"通过。

顺带发现（未处理，候选后续登记）：① 断言文案与实际预算不符（15s 文案 vs 2s 实际）；② `chat.rs:147` 惰性 `#[allow(clippy::too_many_arguments)]` 挂在 enum 上无效果（migration log §5 已留痕）；③ 全量三轮无一出现两条抖动之外的失败名。
