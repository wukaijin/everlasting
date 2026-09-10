# Implement: 执行清单

前置阅读:`research/root-cause.md`(证据链)→ `prd.md`(AC)→ `design.md`(方案 A 决策)。

## 步骤

1. [x] `from_wire.rs`:新增私有纯函数 `fuse_adjacent_tool_results`,按 design §实现要点 1 的规则融合相邻纯-tool_result user 消息;在 `wire_messages_to_chat_messages` 出口挂接。
2. [x] `tests_wire.rs`:补 from_wire 融合用例(design §测试设计 ①②③④;④ 为 `[TR×2, Text(loop hint)]` 行形态——hint 出 fuse 后仍为独立 user 消息,不折入结果消息)。
3. [x] anthropic 出站回归测(AC1):复刻事故形态(assistant[thinking, tool_use×2] + user[tool_result×2]),断言出站 body 紧邻 user 消息同时携带两个 result。挂在 `tests_anthropic.rs` 或 anthropic.rs 既有测试模块。
4. [x] 全量回归:
   ```bash
   cargo test -p everlasting --lib   # PKG_CONFIG_PATH 按 AGENTS.md 坑 1
   ```
5. [x] live 冒烟(AC4):`scripts/turn-smoke.sh`;群聊链路如需 live 验证,用 deepseek-flash 单独试一轮「单消息多 tool_use + 带结果续轮」。
6. [x] Phase 3.3 spec 更新:`.trellis/spec/backend/llm-contract.md` §Pair Atomicity(C3 gotcha)增补「wire 出站形态:同一 assistant 消息的全部 tool_result 必须融合在紧邻的下一条 user 消息——严格中继(实测 wukaijin deepseek 通道)逐消息校验」;`multi-provider-contract/scenario-provider-trait-anthropic.md` 的 1:1 契约处补一句往返融合规则。

## 验证命令速查

```bash
cd /usr/local/code/github/everlasting
PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" \
  cargo test -p everlasting --lib wire          # 定点
PKG_CONFIG_PATH=... cargo test -p everlasting --lib            # 全量(AC3)
bash scripts/turn-smoke.sh                                        # AC4
```

## 回滚点

- 单挂接点:移除 `wire_messages_to_chat_messages` 里的融合调用即回到现状;无 schema / DB / 协议变更。
