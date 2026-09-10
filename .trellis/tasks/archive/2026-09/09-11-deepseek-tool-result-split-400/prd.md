# 修复 wire 层 tool_result 拆条致 deepseek 中继 400 中断

## Goal

群聊(及一切走 Anthropic 协议的会话)中,模型单条 assistant 消息发出 ≥2 个 tool_use 时,后续带 tool_result 的请求不再被 wukaijin 中继的 deepseek 通道以 400 拒绝。出站 payload 恢复「同一批 tool_result 合并在紧邻 assistant 消息之后的**一条** user 消息里」的形态。

## Background

2026-09-11 凌晨群聊会话 `caa5020a`(前端专家 = deepseek-flash)三轮全部 `[生成出错中断]`。根因已完成定位,完整证据链见 `research/root-cause.md`,摘要:

- 引擎侧落库正确(一条 user 消息携带全部 tool_result);
- Anthropic adapter 出站前的 wire 往返把该消息**拆成 N 条连续 user 消息、每条一个 tool_result**(`to_wire.rs` 提升为独立 `WireMessage::Tool`,`from_wire.rs` 逐条映回);
- 原生 Anthropic 合并连续 user 消息故无害,但 wukaijin 的 deepseek 通道逐消息严格校验 → 第二个 tool_use 的 result 成孤儿 → 400;
- glm 通道宽容,故同场 moderator/架构师/产品经理单消息 2–4 个 tool_use 全部通过,唯 deepseek 三振。

## Requirements

- **R1(核心)**:Anthropic 协议出站请求中,任一 assistant 消息内全部 tool_use id 对应的 tool_result,必须位于紧随其后的**同一条** user 消息内;不得因 wire 层往返产生「连续多条、每条单 result」的拆分形态。
- **R2(回归契约)**:恢复 wire 往返引入拆分之前的出站形态——`.trellis/spec/backend/multi-provider-contract/scenario-provider-trait-anthropic.md` 要求 Anthropic 路径行为与 pre-PR2 legacy 1:1(spec 原文措辞);legacy 从不拆条。
- **R3(影响面)**:OpenAI adapter 路径行为不变(其 wire→OpenAI 转换仍按需逐条 role:"tool",该形态是 OpenAI 协议原生要求);群聊 role_history、DB 持久化格式均不变。
- **R4(非目标)**:不改群聊重试/错误恢复策略(重试对 InvalidRequest 结构性无效是另一个议题);不动 `apply_deepseek_reasoning_fix` / `apply_speaker_prefix` 既有补丁逻辑。

## Acceptance Criteria

- [x] **AC1(单测)**:构造「assistant[thinking, tool_use×2] + user[tool_result×2]」历史,经 AnthropicProvider 出站 body 断言:紧邻 assistant 之后的 user 消息**有且仅有一条**,且同时携带两个 tool_use id 的 tool_result 块(复现事故形态的反向断言)。
- [x] **AC2(单测)**:wire 往返(`chat_request_to_wire` → `wire_messages_to_chat_messages`)对「一条 user 消息 N 个 tool_result」保持合并形态;`[TR×N, Text(loop hint)]` 行(tools.rs ⑬ 的真实形态)融合后 hint 不被折入结果消息、配对完整。
- [x] **AC3(回归)**:2026-09-11 实测 2378 passed / 0 failed:`cargo test -p everlasting --lib` 全绿(含既有 wire/anthropic 测试组,~1995 用例基线)。
- [x] **AC4(live 验证)**:daemon 修复版重启后,deepseek-flash 群聊单消息双 tool_use(read_file+list_dir)→ 第二轮开流成功产出总结,零新增 400(daemon.log 2026-09-10T19:38-39 UTC,群聊 a7f2cd0d);glm 通道经 agent/chat 直连一轮同形态亦通过:`scripts/turn-smoke.sh` 跑通;有条件时用 deepseek-flash 模型跑一轮含「单消息多 tool_use」的 live 冒烟,确认第二轮请求不再 400。

## Notes

- 修复层位选择(合并点放在 from_wire 反向映射 vs 出站 body 后处理)见 `design.md`,实现遵循其决策。
- 直测生产中继即可复现:wukaijin deepseek 通道对拆分形态稳定 400(2026-09-10 18:37:37/18:37:45/18:39:39 三次实录)。
