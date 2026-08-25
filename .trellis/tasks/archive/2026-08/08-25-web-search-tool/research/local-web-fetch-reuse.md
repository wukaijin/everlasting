# F4 Web 搜索工具 — 本地代码复用面摸底(web_fetch 及周边)

> 2026-08-25,基于 Explore 全量扫描(结论均带 file:line,实施前按行号复核一遍防漂移)。
> 结论先行:**零 IPC / 零 DB / 零 daemon 改动**(工具在 agent 进程内联执行,同 search_history 先例);主要工作是 `tools/web_search.rs` 新模块 + `mod.rs` 一条 match 臂 + 若干开关名单;两个必办连锁:stub token 静态测试、(可选)权限层取型。

---

## 1. web_fetch.rs 解剖(`app/src-tauri/src/tools/web_fetch.rs`)

**公开面**(模块 `crate::tools::web_fetch`):

- `definition() -> ToolDef` — `:292`,name `web_fetch`,description + JSON schema(url 必填;format 枚举 markdown/text/html;timeout)
- `execute(&serde_json::Value, &ToolContext) -> (String, bool)` — `:337`,返回 `(content, is_error)`
- `execute_for_test(...)` — `:347`,`#[cfg(test)]`,绕 SSRF 让 httpmock(127.0.0.1)可用 —— **web_search 测试直接照抄此模式**
- `enum WebFetchError` — `:146`,7 变体(InvalidUrl/BlockedAddress/RedirectBlocked/TooLarge/HttpStatus/Timeout/Tls/Network),thiserror,LLM 见 Display 字符串

**可直接调用的 `pub(crate)` 件**:

| 件 | 行 | 用途 |
|---|---|---|
| `is_blocked(ip, allow_private)` | `:221` | SSRF 黑名单判定 |
| `truncate_output(s)` | `:646` | 100KiB head/tail 截断(50+50KiB,UTF-8 边界安全) |
| `html_to_text(html)` | `:608` | 剥标签 + 6 实体解码 + 空白折叠 |
| `resolve_and_check_sync(...)` | `:744` | 同步 DNS+黑名单(redirect policy 回调用) |

**私有、web_search 若需要则提权或复制**:`fetch_and_process`(`:377`,完整管线,web_search 若做"一步出正文"才需要;两段式方案下**不需要**)、`resolve_public`(`:683`)、`build_redirect_policy`(`:792`,逐跳 SSRF 复检,`MAX_REDIRECTS=5`)、`classify_reqwest_error`(`:533`)。

**关键常数**:body cap `MAX_BODY_BYTES = 5 MiB`(`:79`);超时默认 30s/上限 120s(`:360`);DNS 20s 上限(`:128`);UA `Everlasting/<ver>`(`:130`)。reqwest client 未关系统代理(fake-ip 代理适配注释 `:114`/`:691`)。

**测试**:`tools/tests_web_fetch.rs`,34 个 httpmock 用例。

## 2. 工具注册与契约(无 Tool trait,约定式)

- **注册**:`tools::builtin_tools() -> Vec<ToolDef>`(`tools/mod.rs:140-241`),web_fetch 在 `:149`。**约定:新工具追加在最后**(顺序喂 provider prefix cache;search_history 先例 `mod.rs:236-239` + `stub.rs` 不变量 #2)。
- **派发**:`execute_tool_inner` 的 `match name`(`mod.rs:469`),web_fetch 臂 `:532-535`;web_search = 一条新臂。外层通用 cancel 包装(`tokio::select! biased`,`mod.rs:455`)白得。
- **ToolDef**:`crate::llm::types::ToolDef { name, description, input_schema }`;LLM 可见描述在各工具 `definition()` 里。
- **turn 时过滤链**(纯 Vec<ToolDef> 过滤器,`chat_loop/drive.rs` 串):`filter_tools_for_mode`(Plan 模式只剥 write/shell/merge,web_search 可存活)/ `filter_tools_for_workflow`(`mod.rs:256`)/ `filter_tools_for_session_type`(`mod.rs:299`)+ stubify(见 §4)。

## 3. 权限层(⑨关 5-tier)

`agent/permissions/check/permission.rs:52` `check()`:Tier 1 hooks → Tier 2 deny → Tier 2.5 敏感路径 → Tier 3 mode → **Tier 4 按 `ToolKind`** → Tier 5 默认放行 → Tier 6 审计。

- `enum ToolKind { Path, Shell, WebFetch, GitMutation, Other }`(`:576-592`);`classify_tool(name)`(`:594-614`)把 `web_fetch` → `WebFetch`(`:603`),Tier 4 分支 `:470-517`:无 grant 则 ask modal。
- **web_search 两条路**:
  - (a) 静默放行:什么都不做 → `Other` → Tier 5(search_history / remember / use_ui 同类,`permission.rs:560-567`;spec `15-search-history.md` 记录此路为零权限层改动);
  - (b) 复用 WebFetch ask:把 `classify_tool` 里 `"web_search"` 也归 `WebFetch` 分支(一行)。
- `risk_for_tool`(`permissions/types.rs:71`):web_fetch 走 `_ => Risk::Low` 默认。
- "总是允许" 持久化:`match_value_for_allow_always`(`:821`)WebFetch → `("tool", None)` 整工具粒度。

## 4. token 预算连锁(必办)

- web_fetch 自己 `truncate_output` 截到 100KiB,下游不再二次截(串行 `chat_loop/tools.rs:1655`、并行 `:331` 原样透传)。web_search 结果集天然小(N×snippet),自带截断即可。
- **关卡⑤ unified budget**(`agent/budget.rs`):`estimate_request_tokens(system, tools_json, messages)`(`:54`),`budget_line = 0.95×window`(`:34,98`);裁剪序 @files→images→memory,**不动 tool result**,无新工作。
- 压缩 transcript:`agent/compaction.rs:492-494` 把 tool_result cap 2000 字符(摘要时),自动覆盖 web_search。
- **C7D stub:`web_fetch 已在 `STUB_CANDIDATES`**(`tools/stub.rs:33-44`,10 个)。**注册 web_search 必撞静态测试 `stub.rs:326`(classic-chat 首轮 tools[] ≤3960 tok)**——spec `15-search-history.md` §4 原话预警"后续每注册一个新 tool 都会撞这条线——先评估扩 STUB_CANDIDATES,平移线是最后手段"。**首选:web_search 进 STUB_CANDIDATES(一行 stub 描述)**。
- **并行白名单不变量**:`NAME_ELIGIBLE = ["read_file","grep","glob","list_dir","use_skill"]`(`agent/chat_loop.rs:1782` 附近;规范副本 `tools/stub.rs:55`);web_fetch 不在内(Tier4 ask + 并行 modal 无解,`chat_loop.rs:1739-1744` 注释)。`stub.rs:281-290` 不变量:**STUB_CANDIDATES ∩ PARALLEL_WHITELIST = ∅** → 选了 stub 就不能进并行白名单(search_history 同现状,可接受)。

## 5. worker / 群聊开关名单

| 名单 | 位置 | web_search 动作 |
|---|---|---|
| `STRUCTURALLY_DISABLED` | `agent/subagent/tools_filter.rs:24-70` | 不加(串行 worker 自动获得) |
| frontmatter allowlist | `tools_filter.rs:80`(`filter_tools_for_subagent`) | 空 tools 的 general-purpose 自动获得 |
| `READONLY_TOOL_ALLOWLIST` | `tools_filter.rs:128-135`:`["read_file","grep","glob","list_dir","web_fetch","search_history"]` | **+1 行**(并发 worker 即得) |
| builtin `researcher` | `agent/subagent/registry.rs:94-100`(硬编码 5 工具 + prompt `:80-92`) | brainstorm 定(08-17 先例:不动) |
| `GROUP_CHAT_RESEARCH_TOOLS` | `agent/group_chat_prompts.rs:195`,穷举白名单 | brainstorm 定 |

worker 权限语义:worker ask 走 WorkerAskBanner,父 session grant 继承(spec `02-web-fetch.md` §1,`check.rs:474` 用父 session id)。workflow-plugin frontmatter 同理可加:`resources/builtin-workflow/dev/agents/researcher.md:4`、`review/agents/reviewer.md:4`。

## 6. IPC / daemon 面

**零工作**。工具在 agent 进程内联执行:GUI(Tauri `chat` → `agent/chat.rs:58`)与 daemon(`daemon/routes/agent.rs:44`)共用 `chat_inner` → `run_chat_loop` → `tools::execute_tool`;唯一跨边界的是结果事件(`ToolResultPayload`,`state.rs:638`,Tauri event / SSE)。`src/daemon/` 下无任何 web_fetch 引用。search_history 先例:"IPC / 前端 / DB migration 零触碰"(spec 15 §1)。

> **补注(2026-08-25,review.md P2-1/P1-1)**:①「零工作」指**工具执行面**;后续 PRD R2 决议引入配置面 IPC(command + daemon POST route + transport 两方法 + Settings UI),见 design §5。② 本节「workflow-plugin frontmatter 同理可加」不完整——builtin(`app/src-tauri/resources/builtin-workflow/**`)只是编译期 fallback,**项目层 `.everlasting/workflow/**` runtime 优先**(`agent/workflow/builtin.rs:5`),本仓两副本已与 builtin 分叉;开闸须四处同改(已在 PRD R6 修正)。③ 本节引用的 `stub.rs:326` 行号有误,静态测试 assert 实际在 `:363-365`。

## 7. 前端

- `ToolCallCard.vue` 全工具通用卡片;per-tool 特例只有图标 `toolIcon(name)`(`utils/messageFormat.ts:140`,web_fetch 本来就落默认扳手)与 accent `toolAccentVar(name)`(`:123`)。**web_search 无需新卡片**,可选加一个图标 case。
- 输出体 `ToolOutputBody.vue` 完全通用(`{content,isError,durationMs}`,500 字符显示截断);信封 `{"result":..., "cwd":...}`(`agent/helpers.rs:48-55`)前端宽松解析,无 web_fetch 特例。
- 已有 web_fetch 特例(可选跟进):`PermissionAskBody.vue:157`(无 path 的 ask modal);`utils/audit.ts:545` `summarizeToolInput` 对 web_fetch 取 url 展示(有通用 url fallback,`web_search` 的 query 字段需确认 fallback 展示效果)。

## 8. 周边设施

- **无出站 HTTP 限流基建**(全库唯一 RateLimiter 在 everlasting_remote 隧道配对)。
- **`retry_open`(`llm/retry.rs:186`)不可直接复用**:绑定 `&dyn Provider`/ChatMessage 流;可借概念(`RetryPolicy::wait` `:103`、可重试/终态分类)。web_search 对 429/5xx 自写小重试环。
- **API key 存放先例**:providers 表 + `crypto.rs`(06-24-p1-api-key-encryption:keyring 已否决,WSL fallback);搜索 key 是否新开 settings kv = brainstorm 决策点。
- **httpmock 0.7** 已是 dev-dependency(`Cargo.toml:177`),`tests_web_fetch.rs` 的 loopback-bypass 模式直接可抄。
- **仓内既有调研材料**:`docs/_deprecated/REVIEW-tool-comparison-2026-06-12.md`(同类 agent WebSearch 对比)、归档任务 `06-12-feat-tools-web-fetch-agent-api-p1/research/web-fetch-api-design.md`。spec `02-web-fetch.md:106` 显式把 web_search 留作独立任务。
- `grep web_search` 现无实现,只有上述历史提及。

## 9. 相关 spec 清单(实施时读)

`.trellis/spec/backend/tool-contract.md`(索引)/ `tool-contract/01-tool-set-extension.md`(新工具基模)/ **`tool-contract/02-web-fetch.md`**(web_fetch 圣经:签名、10 步管线、7 错误表)/ `tool-contract/14-stub-registration.md`(stub + 3960 校准史)/ **`tool-contract/15-search-history.md`**(最近的"加只读工具"全程样板,权限/过滤矩阵 + token 预算预警)/ `permission-layer.md`(⑨关)/ `agent-loop-architecture.md` / `daemon-server.md` / `token-usage-tracking.md`;前端 `frontend/chat.md`。
