# implement.md — web_search 工具

> 前置:PRD(AC1-AC8)+ design.md 已评审(含 2026-08-25 review.md P1×3/P2×7 吸收)。测试基线:后端 **1925 passed + 1 已知 flaky**(08-25 会话)、前端 1179、vue-tsc 0。
> 环境坑(AGENTS/HACKING 已载):`cargo test -p everlasting` 需 `PKG_CONFIG_PATH`;daemon 生命周期必须单命令内联起停。

## WP1 后端工具核心(AC1/AC2/AC5)

1. `tools/web_search/mod.rs`:`SearchHit` / `SearchError` / `SearchBackend` enum(design §2 契约,无 CancellationToken)+ `definition()` + `execute()`(输入校验、选路、重试环、渲染)。
2. `tools/web_search/tavily.rs` + `ddg.rs`(契约见 design §3;DDG 手写解析 + uddg 解码,UA 常量)。
3. `tools/mod.rs`:注册 append 最后 + `execute_tool_inner` match 臂。
4. `tools/stub.rs`:`STUB_CANDIDATES` 与 `STUB_DESCRIPTIONS` **两处定长数组 `[..; 10]` → 11**,加 `"web_search"` + 一行极短 stub 描述(余量 ~24 tok,控制在 ~6 词);立刻单跑 `static_token_budget_classic_chat_first_turn`——余量不足则按校准先例平移线并记录(退路见 PRD R7)。
5. `tools/tests_web_search.rs`:httpmock 锁——Tavily(200 正常/401/429 重试后成/429 重试穷尽/432 额度尽文案)、DDG(200 解析含 uddg 解码/202 RateLimited 不重试/200 空结果 Parse)、渲染截断(title 120/snippet 300/attribution 行)、count 三态(缺省 5/非法回落/clamp 上限)、query 长度拒绝、**30s 超时路径(client 构造参数注入短值测,见 design §3)**。后端 base_url 与超时均可注入(client 构造参数,测试注 httpmock 地址/短毫秒;无 SSRF 面不需 loopback 绕行)。
6. 验证:`cargo test -p everlasting --lib "tools::tests_web_search::"`(单过滤跑,勿循环多 invocation)。

## WP2 配置 + IPC(AC4)

1. `web_search` 模块内 get/set helper(provider 三值校验 + key AEAD 加解密,aad=`web_search`;**key 三态:Some(非空)/Some("")清除删行/None 不动**,单测覆盖三态)。
2. Tauri command:`get_web_search_config` / `set_web_search_config`(design §5;masked 回显)。
3. daemon route:同语义 **POST** `/get_web_search_config`、`/set_web_search_config`(照 config router 先例);route 测试照既有 daemon route 测试模式。
4. transport 前端抽象:两方法(httpTransport / tauriTransport)+ `transport-parity.test.ts` 补新命令 parity 用例。
5. 验证:cargo 单测 + `node scripts/remote-e2e-smoke.mjs` 不回归(remote 不涉 web_search,跑通即可)。

## WP3 前端 Settings 小节(AC4/AC7)

1. Settings modal「Web 搜索」小节:provider 下拉 + key 密码框(masked placeholder,留空不改)+ 「清除已存 key」动作(文案:清除后 auto 回落 DDG);样式照既有表单节(spec design-tokens / component-guidelines)。
2. `utils/audit.ts` `summarizeToolInput` + web_search→query 分支;`toolIcon` 可选加图标。
3. Vitest:小节交互(改 provider 落库、key 留空不清除、**清除动作删密文**、masked 回显)。
4. 验证:`cd app && pnpm test` + vue-tsc。

## WP4 开闸名单翻转(AC6)

1. `READONLY_TOOL_ALLOWLIST` + `"web_search"`(+ 测试断言更新)。
2. builtin researcher:registry vec + prompt 枚举(相关快照/断言更新)。
3. `GROUP_CHAT_RESEARCH_TOOLS` + `"web_search"`。
4. workflow frontmatter **四处**:builtin 两(`app/src-tauri/resources/builtin-workflow/dev/agents/researcher.md`、`.../review/agents/reviewer.md`)+ **项目层两**(`.everlasting/workflow/dev/agents/researcher.md`、`.everlasting/workflow/review/agents/reviewer.md`)——项目层 runtime 优先于 builtin,漏改则本项目 live researcher 不生效(review P1-1)。
5. 验证:既有 worker/group-chat 测试全绿 + grep 复核名单齐全(worker allowlist / researcher vec / 群聊 / frontmatter ×4)+ **运行时断言**:实际加载的 dev workflow researcher 工具表含 web_search(防 builtin-only 假绿)。

## 收尾(AC1 live / AC8)

1. 全量:`cargo test -p everlasting --lib`(基线 1925 passed + 1 已知 flaky)+ `pnpm test` + clippy/stash 对照零新增 + fmt。
2. live 冒烟:`scripts/turn-smoke.sh`——auto 无 key 走 DDG 一轮实搜(prompt 让模型搜个固定查询,断言 tool_result 含 url;**DDG 202 是预期内失败路径**,命中时验证错误文案含 web_fetch 引导,不算回归);再手配 tavily key 走 Tavily 一轮;`--assert-turn-usage` 照跑。
3. UI 冒烟(可选):`scripts/ui-review.sh --screenshots-only` 看 Settings 小节不破版;改了样式才必须。
4. spec 回写(Phase 3.3):`tool-contract/`(新增 16-web-search.md:契约 + enum dispatch 选型 + DDG 202 语义)、`multi-provider-contract` 或新 secret 存放条目(aad 隔离先例)、frontend 视改动。

## 回滚点

- 每 WP 一个 commit;WP1 独立可回滚(工具未开闸前不可达);WP4 是唯一"扩大可见面"步,出问题优先 revert WP4。
- app_config 新 key 无清理需求(纯 KV,遗留无害)。

## 风险文件

- `tools/stub.rs`(静态测试线敏感——改完立刻单跑该测试)
- `.everlasting/workflow/**` 项目层副本(runtime 优先于 builtin,WP4 四处之一,勿漏)
- `agent/subagent/registry.rs` + `group_chat_prompts.rs`(prompt 断言可能锁了工具枚举文本)
- `commands/config.rs` / daemon routes(注意 GUI/daemon 双形态都要通)
