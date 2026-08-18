# 综合分析:压缩方式选型与本项目适配(2026-08-18)

> 前置阅读:01(现状)~ 04(调研)。本文给跨工具结论 → 本项目约束 → 方案骨架 → 决议点与推荐。

## 1. 目标形态(跨工具共识收敛)

**"两层结构 + 一条兜底链"**,这已是行业标配:

```
turn 边界检查估算(cl100k)
  ├─ 未过触发线 → 直通
  ├─ 过线 → LLM 结构化摘要(被压缩区)
  │        + 近期逐字保留区(~15-20k token / 组边界对齐)
  │        + resident 层每请求重注入(memory/tools —— 本项目已有,零成本等价"磁盘重注入")
  │        回填 = [B5 头对] + [摘要 synthetic user 消息] + [保留区] + [当前输入]
  └─ 摘要失败/仍超 → 确定性兜底(现有机械丢组 compact_messages)→ StillOver fail-fast(保留)
```

## 2. 本项目独有约束(与调研对象的关键差异)

1. **前端每请求全量发送 messages**(daemon ChatRequest.messages)—— 摘要若只在 loop 内存态,下一请求重付一次 LLM 摘要费。⇒ **摘要必须持久化并参与历史重建**。这是与 Gemini CLI(不持久化,resume 丢失,issue #20803)的根本差异,Gemini 是反面教材。
2. **DB 是 SoT 且不可删行**:B12 checklist replay 从 DB tool_result 还原;D2 全文搜索/审计依赖全量 messages。⇒ 采用 opencode/OpenHands 哲学:"**落库无损、上下文有损**"——不删原始行,只加水位。
3. **memory 头对带 cache_control 断点**:摘要消息必须插在位置 ≥ 2(B12 注释的 load-bearing 论证)。
4. **组原子性不变量必须保留**:RULE-A-001 tool_use/tool_result 配对、thinking 不拆 —— `group_droppable_turns` 直接复用来算保留区 cut。
5. **已有旁路观测**:ContextCompacted event + turn_trace.compaction_json + TracePanel,扩展字段即可,零新管道。
6. **已有摘要调用基建**:session provider 解析 + A5+ `retry_open` + turn-smoke 烟测。

## 3. 方案骨架(推荐形态)

### 3.1 持久化:摘要消息落 messages 表 + cutoff 水位

- **摘要 = synthetic user 消息**落 `messages` 表,`metadata = {"kind": "compaction_summary", cutoff_seq, tokens_before, tokens_after, trigger: "auto", model}`(B1 已用 metadata,模式现成)。
- **水位语义**:`cutoff_seq` 之前的消息**不进后续请求的 context**,但 DB 行保留(搜索/审计/replay 完好)。多级水位链:第二次压缩时,新摘要的输入 = [旧摘要消息] + (旧 cutoff, 新 cutoff) 区间 —— 天然增量,旧摘要消息本身也被新摘要吸收(见 3.3)。
- **替换执行点在后端**:前端继续傻发全量;`chat_inner` 入口(或 drive_turn 首轮)按最新水位机械替换 [cutoff 前消息 → 摘要消息]。**前端 wire 零改动**;后续如要 UI 卡片再让前端渲染 kind=compaction_summary 消息(follow-up)。
  - 备选:前端组装时自行 cut。否决理由:wire 协议 + 前端组装逻辑双改动,收益仅省一点同源传输字节。
- **D3 交互**:edit_user_message cascade 截断若删到摘要消息之后的行,水位指向已删区间 → 替换逻辑按"水位 seq 不存在则失效"自愈(取次新有效水位)。

### 3.2 触发与保留区

- 触发沿用 turn 边界 + cl100k 估算;阈值建议从 0.80/0.50 调至 **0.85 触发**(业界 0.83-1.0,现值偏早,白丢上下文);目标不再需要压到 0.5 —— 摘要一次到位(摘要 + 保留区 ≈ 窗口 30-40%),保留 **target ≤ 0.60** 作为验收线即可。
- **保留区计算**:`保留区 token 预算 = clamp(15k, 窗口×10%, 25k)`;cut 点 = 从尾向前按 `group_droppable_turns` 的组边界累积,且**最后一条 typed user 消息所在轮必进保留区**(Cline `findCutIndex` 同款护栏)。
- B5 头对照旧 PROTECTED_HEAD 保护;摘要插在位置 2。

### 3.3 摘要调用

- **模型 = session 主模型**(MVP;与 Claude Code/Codex/Roo/opencode 一致,配置面留 `compaction model` 覆盖位但不实现)。理由:① 主模型摘要质量直接决定续跑质量;② Claude Code 曾因换模型 98% cache miss;③ 少一个配置面。
- **prompt = 结构化模板 + handoff 双话术**(Codex 前缀 + Cline/Claude Code 模板段),首版段落(对齐我们工具面定制):
  1. 目标与用户意图(**含全部用户消息逐字或近逐字压缩列表**)
  2. 关键技术概念与决策
  3. 涉及文件与改动(路径必须保留 —— Manus"可恢复"原则)
  4. 错误与修复
  5. 已完成 / 进行中 / 受阻(对齐 B12 checklist 状态语义)
  6. 下一步(引用最近对话原话,防漂移)
- **增量合并**:输入带 `<prior-summary>`(上一份摘要消息),冲突规则 "conversation wins"(opencode V2 同款),显式抑制滚雪球劣化。
- **调用约束**:无 tools、禁 thinking(省 token)、输出上限 4-8k token;`retry_open` 包裹;**熔断**:同 session 连续 3 次失败 → 本请求回退机械丢组 + 标记(session 内存态),下请求再试(对齐 Claude Code MAX_CONSECUTIVE_AUTOCOMPACT_FAILURES=3)。
- **记账**:摘要调用 usage 记入触发 turn 的 trace(`compaction_json` 扩 `summary_usage` 字段),不混入 context_input 口径。

### 3.4 兜底链(降级顺序)

1. LLM 摘要(主路径);
2. 摘要失败/超时 → 现有 `compact_messages` 机械丢组(原样保留为 fallback tier);
3. 丢完仍超 → `StillOver` fail-fast(不变,RULE-A-002);
4. provider 真实 overflow 错误 → 压缩后重试一次(opencode 双保险;MVP 可列 follow-up)。

### 3.5 Scope gate(MVP)

- 只主 loop 单聊:`!worker && !群聊`(与 memory digest 同款 gate 策略;worker 有 200 turn 上限 + resume 兜着,群聊有 30 轮编排上限,两者上下文压力形态不同,follow-up 再评估)。
- 手动 `/compact`、MAX_TURNS 软卡化、handoff —— 均为后续任务(用户已定)。

## 4. 决议点(**已全部批准,2026-08-18 用户确认,推荐项生效**)

| # | 决议点 | 选项 | 推荐 |
|---|--------|------|------|
| Q1 | 摘要持久化形态 | 甲:摘要落 messages 表 + 后端按水位替换(前端零改动) / 乙:独立 compactions 表(搜索搜不到摘要) | **甲** —— D2 搜索/审计/UI 卡片全天然工作 |
| Q2 | 摘要模型 | session 主模型 / 独立廉价模型配置 | **主模型**(MVP),覆盖位留 follow-up |
| Q3 | 摘要粒度 | 每次全历史重摘要 / 增量合并(prior-summary) | **增量合并**(成本 + 抗劣化) |
| Q4 | 阈值 | 保持 0.80/0.50 / 调 0.85 触发 + 保留区 15-25k | **后者**(现值偏早) |
| Q5 | microcompact 层 | 本期加"旧 tool result 占位符化"前置层 / 不加 | **不加**(C7D stub + memory digest 已治前置大头;机械丢组 fallback 覆盖) |
| Q6 | 摘要可见性 | 仅 TracePanel(现有) / chat 流内摘要卡片 | **MVP 仅 trace + 摘要消息可被 D2 搜索命中**;卡片 follow-up(与手动 /compact 同期) |

## 5. 风险清单

- **摘要质量劣化/任务漂移**:Claude Code #9796 教训 —— 缓解 = 双逐字锚点 + resident 层(memory)不进摘要 + 增量合并规则。
- **水位与 D3/编辑重发竞态**:cutoff 自愈规则必须显式(见 3.1)。
- **压缩抖动**(压完马上又触发):保留区 ≥ 15k + 触发线 0.85 + 摘要一次到位,天然留 40%+ 余量。
- **B1 图片**:被摘要区图片随消息退出 context(摘要提及存在过);保留区图片照常;images_token 口径自动跟随请求内容,无需改。
- ** Anthropic thinking 签名**:被摘要的 assistant turns 整组消失,无孤儿签名风险(与现有丢组一致);保留区不动。
