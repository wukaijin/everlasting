# Live 验证记录(2026-08-27/28)

## 结论先行

**全链路 live 验证通过**;过程中出现的「cancel_chat 死锁」经今日复盘为**观测假象**,非代码缺陷。代码零改动,今日以干净观测环境重放 + 加压 18 轮全绿。

## 验证矩阵(全部通过)

| 场景 | 结果 |
|------|------|
| turn-smoke.sh 实跑一轮(F3 闸在 spawn 链上) | PASSED(per-turn token 报表正常) |
| busy 字段 live(claim 即注册语义) | turn 开始瞬间 `busy=true`,驱动器退出后 `false` |
| get_app_config live | `{"turnCompleteNotifyEnabled":true}`(fail-open 生效) |
| 队列续轮(rB 在跑 + rC 入队 → 自动续轮) | rC `queued pos=1` → 驱动器续跑 → busy True→False |
| 完成前取消(cancel@0.5s,流未开) | `{"cancelled":true}` 瞬返,partial turn 落库,busy 清 |
| 流中取消(首 delta 后 cancel@1.3s) | 同上,日志链完整:`cancellation requested` → `cancelled — persisting partial turn` |
| 取消后 0.4s 再发同 session 新轮 | acceptance 瞬返 Started(旧驱动器已退净,无串扰) |
| cancel-race stress 8 轮(c1∈[0.15,1.5]s × gap∈[0.1,0.5]s × c2∈[0.15,1.0]s) | 8/8 全绿,零 panic,17 次 partial-turn 落库计数吻合 |

## 「死锁」复盘:三重观测污染

昨日(f6-c1/f6-c2 轮次)记录的「acceptance 悬挂 / cancel 空回复 / busy 卡 True / 日志中断」逐项定性:

1. **daemon 日志被 `| head -30` 截断**——后台启动命令带了 `head -30`,管道读者退出后其余日志全部丢弃(日志文件恰好 30 行)。「取消后 loop 再无日志」是丢弃,不是悬挂。教训:**live 探测的 daemon 必须全量落日志文件,禁止接 head/tail 管道**。
2. **cancel 探针打错路径**——正确路由是 `POST /api/v1/cancel/cancel_chat`(body `request_id` snake_case);探针曾打到 `/api/v1/cancel_chat` → 405 空回复,被误读为「cancel 悬挂」。取消从未送达 token,相关轮次自然无人终止。
3. **orphan daemon 抢端口**——`&` 后台化残留的旧进程占着 7456,导致跨 daemon 探针串台 + 「Address already in use」。

遗留的 stranded claim(session `62a07aeb` busy=true + 1 条滞留队列消息)随进程消亡,无法在干净环境复现;.transport 的 `read_timeout(60s)` 也保证上游静默流最迟 60s 走错误路径自了(term 落库 + busy 清),不构成永久悬挂机制。

## 手动验收项(留给用户)

- AC1:PC 发长任务 → 手机 PWA 冷启动见红点(serverBusy 独立于 SSE 流)
- AC2:切别的 session 收「任务已完成」toast,点击跳转;设置关掉开关后不再弹
- AC4:Thin GUI(Tauri 窗口)有 busy 时关闭弹确认;Web/PWA 关闭(Ctrl+W)不影响任务

## daemon 重启恢复

既有 `recover_interrupted_messages` 链路不受影响(闸是内存态,重启即清;DB 无新表新列)。
