# 执行记录 — 09-01-aux-call-cache-interference

## 执行步骤与结果

- [x] 1. R1a 调用点清单 + 判别器 → `research/call-site-inventory.md`
      (封闭池三族;has_system/落库指纹判别器;机械性 miss vs 驱逐的框架修正)
- [x] 2. R1b 取证包 → `research/r1b-forensic-pack.md`
      (原始数据在另一台机;命令即拿即用;**待远端执行后回填最终归因**)
- [x] 3. R2 对照实验 → `research/r2-eviction-experiment.sh`(可复跑)+
      `research/r2-experiment-results.md`(v2/v3 两档数字)
      - v2:S 条目 31k / 旁路 161k → T3 cache_read 与 T2 逐字节相同
      - v3:S 条目 124k / 旁路 266k(对齐原事故 280k)→ 同上
      - 实验中修正:v1 create_session 的 model 参数是 legacy 标签不生效
        (实际解析链 sessions.model_id 优先)→ 改 update_session_model_id;
        守卫 tail -N 窗口被大请求日志密度冲掉 → 改时间戳窗口
- [x] 4. R3 结论:**排除臂** —— 跨 session 大 tools=0 旁路不驱逐主 loop 前缀缓存;
      原 seq 285 miss 的最优解释 = 本 session 自己的 auto 压缩折叠前缀(机械性
      miss,by design),待 R1b 远端取证最终确认。不引入缓解改动(不投机)。
- [x] 5. R4 spec 评估:见下节

## AC 状态(对照 prd.md)

- [x] AC1 调用点清单:✅(code 侧完整;**4 次请求的逐一归因留待远端取证包执行**,
      本机无原始日志/DB —— 判据与命令已固化,非悬而不决,是数据物理不在本机)
- [x] AC2 可复现实验:✅ 排除结论(v2+v3 两档,脚本+原始日志留档)
- [x] AC3:N/A(排除臂,PRD 预设分支)—— 未动 build_compaction_prompt / 窗口策略
- [x] AC4:✅ 零生产代码改动,红线天然满足

## spec 收编判断(R4)

新增知识候选:`token-usage-tracking.md` 增补"tools=0 请求归因判别器"
(openai.rs:679 请求行 has_system 字段二分:压缩摘要族 false / auto_reflect true;
配合 messages kind=compaction_summary + autonomous_memories kind=pitfall 落库指纹)
+ "缓存 miss 归因次序:先排本 session 压缩折叠(机械性),再排跨请求驱逐(实验已
排除)"。价值:下次再见 cache_read 回退,按此决策表 5 分钟定位,不再立调查任务。

## 遗留

- R1b 远端取证(另一台机):跑 `research/r1b-forensic-pack.md` 步骤 1–3,把 4 次
  tools=0 的归因结论回填本文件(预期:has_system=false + 本 session 压缩事件邻接
  → 机械性 miss 闭环)。
- 实验脚本 `r2-eviction-experiment.sh` 留在 task research/(一次性调查工件,不进
  scripts/ 主干;复跑方法已文档化)。
