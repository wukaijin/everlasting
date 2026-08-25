# F4 Web 搜索工具(web_search tool)

> ROADMAP 第三档 F4。调研:`research/search-backend-options.md`(后端选型 + 本机网络实测)、`research/local-web-fetch-reuse.md`(本地复用面,file:line 级)。
> 评审修订:2026-08-25 `review.md` P1×3 + P2×7 已吸收(P1-1 项目层副本 / P1-2 key 清除 / P1-3 enum dispatch 等,详见各条括注)。

## Goal

给 agent 补 `web_search` 工具:输入查询词返回前 N 条 `title + url + snippet`,配合既有 `web_fetch` 取正文,让 LLM 能自主完成「查最新文档 / API 变更 / 报错解法」类任务。工具执行面零 IPC(agent 进程内联,search_history 先例)、零 DB migration;配置面走既有 app_config + 双形态 IPC 小增量(command + daemon route)。

## Background(调研定案的事实)

- **网络实测(2026-08-25 本机)**:代理挂掉时 DDG 与 Brave API 直连超时死;Tavily(0.8s)/智谱(0.09s)直连可用。DDG HTML 走代理也两请内吃到 202 软封锁 → 只能当零配置兜底。
- **后端行情**:Tavily 1000 免费 credit/月、免信用卡、国内直连(主力);智谱 search_std ¥0.01/次(留口不实现);Brave 免费层取消 + 绑卡 + 代理依赖(否决);博查 ¥0.036/次(留口不实现)。
- **注册必撞 stub token 静态测试**(`tools/stub.rs:363-365` assert,校准史 3700→3900→3960,当前实测 3903):web_search 须进 `STUB_CANDIDATES`,由此不能进并行白名单(不变量 `STUB_CANDIDATES ∩ PARALLEL_WHITELIST = ∅`,`stub.rs:281-290`;search_history 同现状)。
- **配置基建现成**:`app_config` KV + `get_config_value`/`set_config_value`(`db/config.rs:34-53`);`crypto.rs` 通用 AEAD(`encrypt/decrypt(master_key, plaintext, aad)`);config IPC 先例是 command inner + daemon route 双形态、**route 动词全 POST**(`daemon/routes/config.rs` router:`/get_remote_config`、`/set_remote_config`)。

## Requirements

- **R1 snippet-only 两段式**:`web_search(query, count?)` 只返回 N 条 `title + url + snippet`;正文由模型对选中条目自行调 `web_fetch`。不触碰 web_fetch 私有管线(`fetch_and_process` 保持私有)。
- **R2 provider 抽象 + 双后端 + Settings UI**:
  - 后端抽象(实现形态见 design §2 enum dispatch),首期 `tavily`(keyed 主力)+ `ddg`(HTML 抓取,零配置兜底);智谱/博查/SearXNG 留口不实现。
  - 配置存 `app_config`:`web_search.provider`(`auto|tavily|ddg`,默认 `auto`:有 tavily key → tavily,否则 → ddg)+ `web_search.tavily_api_key`(AEAD 加密,aad=`web_search`,明文不落盘、不出后端)。
  - Settings modal 加「Web 搜索」小节(provider 下拉 + key 输入 + 「清除已存 key」动作,GET 返回 masked key 不回明文);IPC 走 Tauri command + daemon route 双形态(浏览器/手机模式可配),route 照 config 先例 POST(`/get_web_search_config`、`/set_web_search_config`)。
  - **key 三态语义**(review P1-2):`Some(非空)` = 重加密落盘,`Some("")` = 清除(删行),`None` = 不动——防止切 ddg 停用后残留密文在切回 auto 时静默复活 Tavily。
- **R3 权限静默放行**:web_search 归 `ToolKind::Other`(Tier 5 默认放行,search_history 先例),权限层零改动;Tier 6 审计照记。
- **R4 结果集形状**:
  - tool 入参:`query`(必填,≤400 字符)、`count`(可选,默认 5;非法值(0/负数/类型错)回落 5,合法域 `clamp(1, 10)`)。
  - 每条:`title`(截 120 字符)+ `url` + `snippet`(截 300 字符);尾部 attribution 行(provider + rfc3339 时间戳,同 web_fetch 精神)。
  - 预估 ~0.5-2K tok/次。
- **R5 重试与降级**:
  - 429/5xx/网络瞬时错:指数退避 + 抖动,≤2 次重试(自研小环,借 `RetryPolicy::wait` 概念;`retry_open` 是 Provider 流绑定,不复用)。
  - DDG 202(Ratelimit 软封锁):不重试,返回可操作错误文案(建议模型改用 web_fetch 直取已知 URL)。
  - 失败不做跨 provider 自动兜底(auto 只按 key 有无静态选路),失败原因可见。
  - 工具整体超时 30s。
- **R6 开闸范围(全开)**:主会话 + 并发只读 worker(`READONLY_TOOL_ALLOWLIST` +1)+ builtin `researcher`(定义 vec + system prompt 枚举)+ 群聊(`GROUP_CHAT_RESEARCH_TOOLS`)+ workflow-plugin frontmatter **四处**:builtin 层 `app/src-tauri/resources/builtin-workflow/dev/agents/researcher.md`、`app/src-tauri/resources/builtin-workflow/review/agents/reviewer.md`,以及项目层副本 `.everlasting/workflow/dev/agents/researcher.md`、`.everlasting/workflow/review/agents/reviewer.md`(runtime loader 项目层优先、builtin 仅编译期 fallback——`agent/workflow/builtin.rs:5`;本仓两副本与 builtin 已分叉,漏改则本项目 live researcher 不生效,review P1-1)。
- **R7 stub 降级**:web_search 进 `STUB_CANDIDATES`(`stub.rs` 的 `STUB_CANDIDATES` 与 `STUB_DESCRIPTIONS` 两处定长数组 `[..; 10]` 同步改 11)+ 一行极短 stub 描述。当前 3903 / 线 3960,新条目 ≈33 tok 后余 ~24——目标零平移;若仍超线,按校准先例(3700→3900→3960)平移并在 spec 记录。

## Acceptance Criteria

- [ ] **AC1 工具可用**:主会话 classic-chat 首轮 tools[] 含 web_search(完整描述形态;stub 开启时为 stub 形态);LLM 调用返回 ≤count 条 title+url+snippet + attribution 行;`scripts/turn-smoke.sh` 实跑一轮含 web_search 的 turn 通过。DDG 202 软封锁是**预期内失败路径**(错误文案引导 web_fetch),live 冒烟遇 202 不算回归,改验文案。
- [ ] **AC2 token 线**:`static_token_budget_classic_chat_first_turn`(assert ≤3960)通过——零平移为目标,余量不足按校准先例平移并记录(见 R7);web_search 在 `STUB_CANDIDATES` 且不在并行白名单(既有不变量测试护住)。
- [ ] **AC3 权限零弹窗**:首次调用不产生 ask 流(权限层无 web_search 分支),审计事件照记。
- [ ] **AC4 配置链路**:auto(无 key)→ DDG;配置 tavily key 后同 query 走 Tavily;显式 `provider=tavily` 而无 key → 可操作错误文案;Settings UI 可改 provider/key,GET 不回明文,DB 中 key 为 AEAD 密文(sqlite3 查证无明文);「清除已存 key」后密文行删除、auto 回落 DDG 不复活;daemon 直连模式(浏览器)Settings 同样可配。
- [ ] **AC5 后端行为契约**:count 默认 5/非法回落/clamp(1,10) 生效;title 120 / snippet 300 截断;30s 超时;429/5xx 重试 ≤2 次;DDG 202 不重试且错误文案含 web_fetch 引导——全部有 httpmock 单测锁定。
- [ ] **AC6 开闸面**:并发 worker(READONLY_TOOL_ALLOWLIST)、builtin researcher、GROUP_CHAT_RESEARCH_TOOLS、workflow frontmatter 四处(builtin 两 + 项目层两)均含 web_search,各有测试或 grep 级验证;另以**运行时断言**复核本项目实际加载的 researcher 工具表含 web_search(项目层优先,防 builtin-only 假绿,review P1-1)。
- [ ] **AC7 前端渲染**:通用 ToolCallCard 正常展示结果;审计 chip 对 query 显示可读摘要(summarizeToolInput 加 web_search→query 分支);vue-tsc 0 错。
- [ ] **AC8 回归全绿**:`cargo test -p everlasting --lib`(基线 1925 passed + 1 已知 flaky,零新增失败)、`pnpm test`、clippy 零新增、fmt 净;remote 两 crate 不受影响。

## Out of Scope

- 智谱 / 博查 / SearXNG / Serper provider 实现(抽象留口)
- Bing HTML 抓取后端(实测脆)
- web_fetch 自身任何行为变更;跨 provider 失败自动兜底
- 每 domain / per-engine 细粒度过滤参数(Tavily include_domains 等不上 tool 入参,内部不透传)
