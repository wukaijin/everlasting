# R2 实验结果:跨 session 大 tools=0 旁路调用不驱逐主 loop 前缀缓存(两档规模)

> 2026-09-01,本机 daemon(:7456)+ api.wukaijin.com `deepseek-v4-flash`(openai 兼容路径,
> 与原事故 session `d6728b3a` 同 provider 栈)。脚本:`r2-eviction-experiment.sh`
> (可复跑,BIG_CHARS/S2_TURNS 调规模);原始输出:`r2-run-v2.log` / `r2-run-v3.log`。

## 实验设计(臂与判读标准见 call-site-inventory.md §3)

- **A 臂(对照)**:session S 大消息 T1(冷启)→ 小消息 T2 → T2 cache_read ≈ T1 尾部
  input = 稳态命中基线成立,否则该栈缓存不可测、实验终止。
- **C 臂(决定性)**:另一 session S2 堆大上下文 → `POST compact_session`(触发
  `tools_count=0 has_system=false` 摘要旁路,daemon.log 实证;嵌入待压区 = 该栈上
  最大的 tools=0 生产请求形态)→ S 发 T3(**前缀未动**)→ T3 cache_read 塌 = 驱逐
  成立;≈ T2 水位 = 排除。T4 观察恢复。

方法学要点:per-session 模型经 `update_session_model_id` 指定(解析链
`sessions.model_id` 优先,chat.rs:783);T1 后路径守卫(daemon.log 时间戳窗口内必须
出现 `provider::openai.*deepseek`);变长随机词填充(禁重复文本夹具,Session-39
cl100k 压缩比漂移教训);SSE kind=done 终态等待(RULE-SMOKE-001)。

## v2(常规档:S 条目 31k / 旁路 ~161k)

| 轮 | input | cache_read | 说明 |
|----|-------|-----------|------|
| S.T1 | 31430 | 0 | 冷启 |
| S.T2 | 31480 | 31232 | 命中 99.4%(基线✓) |
| S2.T1–T4 | 31404→100405 | 逐轮 31232/54272/77312 | S2 正常增长命中 |
| compact | before=215272 after=54445 | — | 旁路请求 19:05:58.892 `tools_count=0 has_system=false`,嵌入 ~161k |
| **S.T3** | 31500 | **31232** | **与 T2 逐字节相同 —— 无驱逐** |
| S.T4 | 31520 | 31232 | 持续命中 |

## v3(加压档:S 条目 124k / 旁路 ~266k,对齐原事故 280k 量级)

| 轮 | input | cache_read | 说明 |
|----|-------|-----------|------|
| S.T1 | 124107 | 0 | 冷启 |
| S.T2 | 124127 | 123904 | 命中 99.8%(基线✓) |
| S2.T1/T2 | 123956 / 239701 | 0 / 123904 | S2 正常 |
| compact | before=534203 after=267617 | — | 旁路 19:11:38.366 `tools_count=0 has_system=false`,嵌入 **~266k** |
| **S.T3** | 124234 | **123904** | **与 T2 相同 —— 无驱逐** |
| S.T4 | 124293 | 124160 | 正常增长命中(增量已入缓存) |

## 结论

**驱逐假说排除**:同一 provider 栈(api.wukaijin.com 路由 → deepseek-v4-flash)上,
与主 loop 无共享前缀的大 tools=0 摘要旁路请求(161k / 266k 两档,后者对齐原事故
~280k 量级)插入后,主 loop 前缀未变的下一轮 cache_read **完全不受影响**(逐字节
同值)。时序为"旁路完成后 5–10s"(对齐原证据 5s 形态);上游未见任何
以大请求挤占/驱逐他条目的行为。

**原 seq 285 miss 的替代解释(待 R1b 远端取证确认)**:更可能是该 session **自己的
auto 压缩**(tools=0 旁路由本 session 水位触发)折叠了待压区 → 前缀机械性变化 →
miss by design。佐证:原证据 seq 401"部分回退 154,112"正是"命中到保留区边界"的
折叠签名(摘要行插在保留区之后,保留段仍命中固定值)。若 4 次 tools=0 的
has_system 均为 false 且时刻邻近本 session 压缩事件,即闭环。

## 附:三级判读的完整决策表

| 观察 | v2 | v3 |
|------|----|----|
| A 臂基线(T2 命中) | ✓ 99.4% | ✓ 99.8% |
| 旁路确实发出(log 行 + compact 响应) | ✓ | ✓ |
| S 前缀未动(仅追加大消息轮) | ✓ | ✓ |
| T3 cache_read 塌陷 | ✗(不变) | ✗(不变) |
| T4 恢复/持续命中 | ✓ | ✓ |
