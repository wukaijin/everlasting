# tools=0 并发辅助调用缓存干扰调查(压缩摘要旁路嫌疑)

> 来源:`08-31-cache-head-volatility` 归档时的遗留调查项(原 R5 / design D4,implement.md
> 步骤 6 未勾)。该任务修完了头部易变注入(breadcrumb 尾部化 / instruction 冻结 /
> head_sha 下沉),seq 435/437 类全量 miss 已闭环;seq 285 的归因仍存疑,独立成任务。

## Goal

判定 tools=0 的辅助 LLM completion(压缩摘要旁路)是否挤占 OpenAI 兼容路径上游的
前缀缓存条目、导致主 loop 下一次请求 cache_read 回退;若确认,落地缓解(降摘要
prompt 规模方向);若排除,留证据结案。

## 背景与证据(承接 08-31 取证)

- session `d6728b3a`(deepseek-v4-flash 经 api.wukaijin.com)~110 轮命中正常,
  **seq 285(09:13:03)cache_read=0** 归因存疑:其 **5 秒前**(09:12:58)有一次
  **tools_count=0** 的辅助请求,无 tools、输入含大段共享历史,疑似挤占上游缓存容量
  (loop-hint 已排除:注入在对话尾部,不破坏前缀)。
- 父任务证据(daemon.log)当天共 **4 次 tools_count=0 请求**(08:50:58 / 09:00:35 /
  09:12:58 / 09:24:56),其中 09:00:35.509 tools=0 → 09:00:35.550 tools=13,**间隔仅
  40ms**(紧邻);09:12:58 tools=0 → seq 285(09:13:03)先后 5s;09:24:56 tools=0 →
  seq 401(09:28:20)部分回退 154,112。**时序形态是"先后/紧邻",不是严格并发**
  ——机理上更支持"前一个大请求插入缓存条目、挤占主 loop 热条目,后续主请求 miss"。
- 原任务把这 4 次归因为"worker dispatch 前的 `truncate_summary` 摘要调用"(依据
  40ms 紧邻 tools=13 的配对指纹)—— **本轮代码预核已证伪该归因**:
  `agent/subagent/truncate_summary.rs` 是纯格式化/落盘 helper(transcript cap /
  final_text 格式化),不发 LLM 请求。4 次的真正调用方待 R1 重新归因。
- tools=0 生产调用点全景(2026-09-01 预核,`→ LLM request … tools_count=` 日志打在
  openai transport 层 `llm/provider/openai.rs:679`、不挑调用方,故该嫌疑池封闭):
  - `agent/chat_loop/drive.rs:2611` — **auto 压缩摘要**(loop 内水位触发,prompt =
    `build_compaction_prompt(compressible, ...)`,把待压消息区整段嵌进单条 user
    prompt,**输入大**,与主 loop 历史同量级;主要嫌疑);
  - `agent/compaction.rs:949` — 手动 `/compact`;`compaction.rs:1220/1248` — 手动
    focus/retry 变体(共用 `send_summary_completion`,`compaction.rs:775`,
    `retry_open(..., vec![], ...)` tools 恒空;**输入大**);
  - `agent/auto_reflect.rs:453` — `reflect_to_pitfall` 失败反思,`provider.send(
    Some(REFLECT_SYSTEM_PROMPT), messages, vec![])`,fire-and-forget,**输入小**
    (单条 user,截断的失败上下文;驱逐嫌疑低,但归因池内必列)。
  主 loop 本体(`drive.rs:1189`,含 worker 复用)带会话工具集(该 session 主 loop
  tools_count=29;日志里的 tools=13 跟随者是 worker 还是别态,R1 一并判)。
- 机理假设:OpenAI 兼容路径前缀缓存按字节 0 严格匹配且容量有限;辅助请求前缀与主
  loop 不同(不命中),但作为大输入会**插入新缓存条目**,LRU 淘汰主 loop 的热条目
  → 主 loop 下轮全量重付。
- 已有的验证设施:turn_trace per-turn cache_read;D5 落地的
  `cache_read==0 && input>50k` WARN 日志(顺带覆盖本调查的观测需求)。

## Requirements

- **R1 调用点清单 + DB/log 归因**:research/ 落上述预核清单的核实版(file:line +
  触发条件 + 输入规模档);对 daemon.log 的 **4 次 tools=0 请求**(08:50:58 /
  09:00:35 / 09:12:58 / 09:24:56)逐一归因到封闭嫌疑池 {auto 压缩、手动 /compact、
  auto_reflect} 中的具体调用方——判据含紧邻 tools=13 请求的真实身份(worker 本体 /
  其他 session 态)、request_id/turn_trace join、以及各调用点触发条件与时间戳的
  交叉验证。
- **R2 复现实验**:构造可对照的实验——主 loop 稳态高命中(两轮连跑验证)后插入一次
  摘要旁路调用(强制触发 auto 压缩或手动 /compact),观察主 loop 下一次请求
  cache_read 是否回退;同场景不插辅助调用作对照。可用 turn-smoke / daemon live,
  记录每轮 cache_read 数值**及请求间时间差**(区分"紧邻插入"与"先后间隔"两种
  时序对结果的影响,对齐证据里 40ms / 5s 两种观测形态)。
- **R3 结论 + 条件缓解**:
  - 若确认干扰:优先**降摘要 prompt 规模**(截断更狠 / 只发最近 N 轮 / 压缩待压窗口
    上限),避免与主 loop 争同一缓存容量;**不做请求级串行化**(拖慢 dispatch,
    D4 已裁定)。
  - 若排除:证据落 research/,结案;不引入投机性改动。
- **R4 回写**:结论写回本任务 research/(及 implement.md 若走缓解臂);若产出可复用
  的缓存观测方法或约定,按 `trellis-update-spec` 评估收编。

## Acceptance Criteria

- [x] AC1 research/ 有 tools=0 生产调用点清单(file:line + 触发条件 + 输入规模档,
      含 `auto_reflect.rs:453`),且 daemon.log 的 4 次 tools=0 请求(08:50:58 /
      09:00:35 / 09:12:58 / 09:24:56)逐一归因到具体调用方,附 DB/log 证据。
      —— 清单✅(call-site-inventory.md);4 次归因:原始数据在另一台机,判据与
      命令已固化为 r1b-forensic-pack.md,待远端执行回填(见 implement.md 遗留)。
- [x] AC2 有可复现实验的记录(命令/脚本 + 每轮 cache_read 数值 + 请求间时间差),
      明确给出"干扰确认"或"干扰排除"结论;两种结论都可结案,不许悬而不决。
      —— **排除**(r2-experiment-results.md:v2 31k/161k + v3 124k/266k 两档)。
- [x] AC3(仅确认干扰时)缓解改动落地 —— **N/A(排除臂)**,未动摘要旁路任何代码。
- [x] AC4 不做请求级串行化;不改动 Anthropic 路径 cache_control 布局;不破坏
      摘要旁路现有失败语义(熔断 registry / RULE-A-011)。—— 零生产代码改动。

## Non-goals

- 请求级串行化 / 主 loop 与辅助调用排队(D4 已否)。
- 上游 provider 缓存内部机制(黑盒,只观测不假设内部实现细节 beyond 前缀匹配)。
- Anthropic 路径(有 cache_control 断点保护,不受此机理影响)。

## Notes

- 调查型任务:PRD-only 起步;若 AC3 缓解臂触发(确认干扰 + 要动 `build_compaction_prompt`
  / 窗口策略),补 design.md + implement.md 再实施。
- 实验时 live 探测 daemon 必须全量落日志文件,禁止 head/tail 管道截断
  (validation.md 教训,F6 复盘)。
