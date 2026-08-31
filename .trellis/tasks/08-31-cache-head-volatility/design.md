# design:提示词头部易变注入下沉

## 设计原则

**头部(messages[0..1] + system)只放 session 生命周期内不可变的内容;一切逐轮/逐事件变化的状态提醒放对话尾部。** Anthropic 路径靠 cache_control 断点容忍头部变化,OpenAI 兼容路径没有任何断点机制,头部字节不可变是唯一杠杆。两路径共用同一消息布局,下沉后 Anthropic 路径同样受益(尾部块本就 cache_control: None,无需断点)。

## D1 breadcrumb / delegation template 下沉(核心变更)

现状:`append_workflow_breadcrumb`(inject.rs:376)push 进 `messages[0]`(instructions 合成头)的块列表;`append_delegation_template` 同构。

目标:push 进**本轮最后一条消息**的块列表;若最后一条是 user(tool_results) 消息则 append 到其块尾(与 loop-hint 同位、同 S-B guard 形态);否则(理论上不出现——每轮都有 tool 结果或新 user 消息)fallthrough 到现状的 warn+skip。

影响面:
- 语义:状态提醒从"每轮都在头部重申"变为"每轮出现在尾部最新消息"。对模型注意力而言尾部更显眼,语义不降级;system prompt 中已有 workflow 的整体说明(不动)。
- `synthetic_prefix_len`(init.rs:683)不受影响( breadcrumb 本就不计入)。
- turn_trace `breadcrumb_json` 快照管道(record_breadcrumb 读 breadcrumb_body)不变——`build_breadcrumb_block` 的内容构造不动,只动注入位置。
- 测试:现有断言 breadcrumb 在 messages[0] 的用例需同步改为尾部断言;新增"状态迁移轮头部字节不变"的 wire 级断言(见 AC)。

## D2 instruction 文件 session 内冻结

现状:每次新 chat 请求 init 从磁盘重读 AGENTS/CLAUDE(User/Project 四块)插 messages[0..1]。

方案(倾向 A):
- **A. session 级冻结**:init 读过后,将四块内容缓存在 session 的内存态(chat 命令持有的 `AppState`/session 上下文),同 session 后续请求复用;session 重开(新 request 但 DB 同 session)语义保持"首个请求的内容"。代价:agent 改 AGENTS.md 后,本 session 内不生效(下个 session 生效)——与本仓实际用法一致(instruction 是"给未来会话的规范",不是运行时配置)。
- B. 保持重读,但把 instruction 块挪到尾部——被否:instruction 是全局规范,放尾部会随历史增长被截断/压缩,语义损失大于 A。
- 佐证注释保留:RULE-A-005 关于 Anthropic cache_control 的说明继续成立。

边界:内存缓存随 daemon 进程生命周期;daemon 重启后 session 恢复的第一个请求重读一次(等价于"session 内首个请求"),可接受。

## D3 head_sha 移出 system prompt

现状:`build_system_prompt` 输出含 head_sha,每轮刷新(RULE-A-005,修 stale SHA)。

方案:head_sha 从 system prompt 文本移到**尾部状态块**(与 D1 的 breadcrumb 同一条 Text 块或紧邻),每轮刷新语义保留(模型仍看到当前 HEAD),但变化落在尾部。RULE-A-005 的动机(stale SHA 误导)不受影响;其"cache-correctness"注释改写为新不变量:**system prompt 在 session 内必须字节稳定**。

## D4 R5 调查(tools=0 辅助调用)

实现期调查项,不阻塞 D1-D3 落地:
1. 确认 4 次 tools_count=0 的调用方(预期 `agent/subagent/truncate_summary.rs` 的摘要调用);
2. 用 turn-smoke 两轮连跑 + 人为插入一次 worker dispatch,观察主 loop cache_read 是否回退;
3. 若确认干扰:优先降摘要 prompt 规模(截断更狠/只发最近 N 轮),避免与主 loop 争同一缓存容量;不做请求级串行化(拖慢 dispatch)。

## D5 可观测性

- daemon 侧:provider 收到 usage 时,若 `cache_read_input_tokens == 0 && input_tokens > 50_000`,打一条 `WARN`(字段:session_id、input、上轮 input)。正常路径(首turn/冷启动)不触发阈值。
- 该阈值日志同时覆盖 D4 调查的验证需求。

## 回滚

D1/D3 各自独立成 commit,任一回滚不影响其余;D2 的冻结缓存加 kill-switch 常量(编译期 const,默认开),异常时一行回退。
