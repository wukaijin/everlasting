# Review:群聊评审纪要(2026-09-18)

> session `5b7cd5f5-fcf6-4e66-8593-6479ee81cc42` · MiniMax-M3 主持 + 架构/glm-5.3 + 产品/GLM-5.3-Flash + 后端/deepseek-flash(未发言)
> 实烧 631,860 token / 608s / 49 条消息 · `stop_reason: budget`(预算帽截断,moderator 未走收官总结,本文为 task owner 人工收束)
> 全文转录:`~/.local/share/dev.everlasting.app/discussions/2026-09-18-评审任务 09-18-openai-responses-provider 的需求-5b7cd5f5.md`

## 共识(同意项)

- 五项锁定决策:四项直接同意(stateless 全量回放、独立第三协议、MVP 不回传 reasoning、strict:false);speaker 内联方向同意但格式修正(见下)。
- moderator 待议点④(caps 硬编码与 `from_model_row` 分叉)**不成立**:`from_model_row` 是 `#[allow(dead_code)]` 测试路径,生产事实源就是 adapter 内联构造 caps,Responses 档与 `openai_caps` 是精确结构兄弟。
- 隔离性与回滚声明经代码核验属实(存量行回滚后走 `UnknownProtocol` 干净报错)。
- PR1/PR2 分期切分同意,无该挪的。
- 群聊 speaker 不必单开 AC:格式对齐 Anthropic 后风险缩水,PR1 的 turn-smoke live 带一轮 speaker 会话即可。

## 需修改项(已全部落回文档)

| # | 发现(提出人) | 修正落点 |
|---|---|---|
| 1 | **strip 对 Reasoning 块是 OR 语义**(`block_supported`,to_wire.rs:652)——effort 已设时 strip 保留 Reasoning,真正丢弃点是 build 层;design §2.2 原表述与代码相反(架构,moderator 复核确认) | design §2.2 重写 + §4.1 AC4 断言对象改为 `build_http_body` 输出 body |
| 2 | CC 对幸存 Reasoning 块是**回放**(RULE-D-006 `reasoning_content`),Responses 是**丢弃**——两协议语义相反需留痕(架构) | design §2.2 补不对称说明 |
| 3 | **speaker 前缀格式选错**:Anthropic 现行就是 `@name: ` 内联(anthropic.rs:371 `apply_speaker_prefix`),原稿 `Alice: ` 与仓库约定分叉,混编群聊会格式漂移;「instructions 说明转录格式」无落点(产品) | design §2.3 / §3、prd Technical Notes、research §4.3 全部改 `@name: ` + 删空头支票 + 「降级」改「与 Anthropic 同款」 |
| 4 | **探测判据对推理模型假阴**(产品,本轮最重):现有两协议判据 = 裸 2xx(providers.rs 两处 `(true, None)`),原设计「2xx 且 output_text 非空」更严格且 `max_output_tokens:16` 会被 reasoning 烧光 → gpt-5 系(目标主力)误报失败、doctor 误诊 | design §2.7 / prd R6 / AC3 改裸 2xx;AC3 删「1-token round-trip」 |
| 5 | R4 漏 **refusal/空输出路径**:refusal part 无处理会产出零文本空气泡(产品) | prd R4 + design §2.4 + §4.2 补 refusal 转 Delta 用例 |
| 6 | function_call **arguments 被 max_output_tokens 截断**时 serde from_str 失败,原测试设计无此用例(moderator 待议点③,无人接走) | design §2.4 + §4.2:parse 失败降级 `Value::String(原始串)` + warn |

## Task owner 拍板项

- 探测判据三选一 → 采纳产品推荐①(裸 2xx,同 UI 同判据,少协议特例)。
- refusal 处理 → 转可见 Delta(而非 LlmError;用户须看见拒绝理由)。
- arguments 截断 → `Value::String` 降级不丢调用(与 incomplete→max_tokens 语义衔接)。

## 未决项

- **后端角色(deepseek-flash)未发言**即被预算帽截断——实施期由 trellis-check 子代理兜底其视角(Rust 实现细节、错误路径)。
- 预算经验:本场 60 万帽偏紧(moderator 收束轮被截,summary 缺失)。同类评审建议 omit 或 ≥80 万。
