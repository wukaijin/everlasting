# Implement: 执行清单

前置阅读:`research.md`(协议差异明细)→ `prd.md`(R1-R7 / AC1-AC5)→ `design.md`(函数级设计)。分期对齐 prd「分期建议」:PR1 后端主体,PR2 外围+探测+spec;PR3(encrypted reasoning)out of scope,另立任务。

## PR1:后端主体

1. [ ] `db/types.rs`:`ProviderProtocol::OpenaiResponses` + `as_str`/`from_str_opt` 分支(design §2.1)。
2. [ ] `llm/provider/responses.rs`:`ResponsesConfig` + `build_http_body` 纯函数(instructions / input items / reasoning 含 effort 归一 / store:false / 扁平 tools + strict:false,规则见 design §2.3)。
3. [ ] `llm/provider/streaming.rs`:`parse_responses_usage`(design §2.6,`pub(crate)`,与 `parse_openai_usage` 并排)。
4. [ ] `llm/provider/responses.rs`:`impl Provider`(send 的 `stream!` 事件状态机——created/文本/推理摘要/工具聚合(output_index 键控,arguments 截断容错)/completed/incomplete/failed/**refusal part 转 Delta**/stop_reason 合成与 ignore 面见 design §2.4-2.5;HTTP client、`classify_error_response`、`warn_on_full_prefix_cache_miss`、timeout 参数照 openai.rs 平移;base_url 只追加 `/responses`)。
5. [ ] `llm/provider/mod.rs`:模块声明 + `build_provider` 加 `"openai_responses"` 分支(design §2.1)。
6. [ ] `llm/provider/tests_responses.rs`:design §4 三组用例(build 形状 / 事件机 / usage),`mod.rs` 声明。
7. [ ] 定点 + 全量回归:
    ```bash
    PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" \
      cargo test -p everlasting --lib responses
    PKG_CONFIG_PATH=... cargo test -p everlasting --lib            # 全量,AC5 的后端半边
    ```
8. [ ] live 冒烟(AC1/AC2 的前置校准):Settings 建 `openai_responses` provider + 模型(base_url 含 `/v1`),`scripts/turn-smoke.sh` 实跑;SSE 真实序列与合成序列有偏差先修状态机(design §5);群聊 speaker 场景顺带走一轮带 `speaker` 字段的会话(评审建议,替代单独 AC)。

## PR2:前端 + 探测 + spec

9. [ ] `commands/providers.rs` `test_model` + `tools/test_llm_connection.rs`:`"openai_responses"` 探测分支与错误 hint(判据 = 裸 2xx 与现有两协议一致,design §2.7;对照 `.trellis/spec/backend/test-model-contract.md`)。
10. [ ] `ProvidersTab.vue`:下拉第三项 + `protocolBadgeClass` 分支;`ModelForm.vue`:effort 选项按 provider protocol 过滤(Responses: minimal|low|medium|high;存量 `xhigh|max` 显示但标「将按 high 发送」);`stores/providers.ts` 注释(design §2.8)。
11. [ ] 前端回归:`cd app && pnpm test`(AC5 的前端半边)。
12. [ ] 探测验收(AC3):GUI 对 Responses provider 跑 test_model 成功;错 base_url/错 key 各验一次 hint 文案。
13. [ ] spec 落账:`.trellis/spec/backend/multi-provider-contract/` 新增 `scenario-responses-wire.md`(参照 scenario-openai-wire.md 结构:管线、caps 档、请求形状、事件表、stop_reason/usage 映射);`multi-provider-contract.md` 索引处补一行。

## 收尾(两 PR 合并后)

14. [ ] AC4 验证:Anthropic 会话中途切 Responses 模型实跑一轮,thinking/signature 块被裁、不 400(单测已锁形状,live 复核)。
15. [ ] Phase 3.3 spec 更新复盘:llm-contract.md 的 stop_reason/usage 章节是否需要补 Responses 行;AGENTS.md 测试计数如有漂移顺手修。

## 验证命令速查

```bash
cd /usr/local/code/github/everlasting
PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" \
  cargo test -p everlasting --lib responses   # 定点
PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" \
  cargo test -p everlasting --lib             # 全量后端
cd app && pnpm test                           # 前端
bash scripts/turn-smoke.sh                    # live 一轮(需 daemon + 真实 provider)
```

## 回滚点

- PR1/PR2 各自成 commit,任一 revert 不伤现有两协议路径;无 schema/DB 迁移,无配置文件变更。
- 事件状态机若 live 暴露形状偏差,修 responses.rs 单文件,wire 管线与枚举不动。
