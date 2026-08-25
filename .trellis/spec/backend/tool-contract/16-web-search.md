## Scenario: `web_search` — snippet-only 网页搜索(F4,2026-08-25)

> 配套 task `08-25-web-search-tool`。两段式模型(同 Claude Code
> WebSearch/WebFetch):`web_search(query, count?)` 只返回前 N 条
> `title + url + snippet`,正文由模型对选中条目自行调 `web_fetch`——
> 不触碰 web_fetch 私有管线(`fetch_and_process` 保持私有),也不复用
> 它的 SSRF 面(固定域名固定 endpoint,无用户可控 URL,不引 `is_blocked`;
> DNS 走系统吃代理 env,与 web_fetch 一致)。

### 1. 模块结构与后端抽象(design 定稿:enum dispatch)

`tools/web_search/` 目录式(单文件工具的例外:2 后端 + 解析器):

```
tools/web_search/mod.rs     # definition/execute/parse_args/选路/重试环/渲染/配置 get-set
tools/web_search/tavily.rs  # TavilyClient:POST {base}/search(Bearer key)
tools/web_search/ddg.rs     # DdgClient:GET {base}/?q= + 手写解析
tools/tests_web_search.rs   # httpmock + axum 有状态序列(见 §5)
```

**enum dispatch 而非 trait + dyn**(评审 P1-3 定稿):两 provider、
仓内无 async-trait、edition 2021 原生 async fn in trait 不支持 dyn;
`Provider` trait 的手写 boxed stream 形态对两 variant 过重。
`enum SearchBackend { Tavily(TavilyClient), Ddg(DdgClient) }` + 一个
match 臂的 `async fn search`;「留口」= 加 variant(智谱/博查未来照此)。
签名**不带 CancellationToken**:`execute_tool` 外层通用 cancel 包装
(`tools/mod.rs` biased select)白得,reqwest future drop 即取消。

### 2. 契约

- 入参:`query`(必填,trim 非空,≤400 **字符**——CJK 按字符计);
  `count`(缺省 5;0/负数/类型错 → 默认 5,`clamp(1, 10)`。语义宽松
  同 `search_history` 的 limit 先例:错值不改搜索方向)。
- 出参渲染(纯文本):
  `N. <title 截120字符>\n   <url>\n   <snippet 截300字符>`,尾部
  attribution 注释 `<!-- searched: "<query>" via <provider> at
  <rfc3339> · N results -->`。注释在**尾部**——结果集每条自截断,
  不存在 web_fetch 那种整体截断掉头的风险(它头部前缀是防截)。
- 错误文案要**可操作**(模型读完能换路径):DDG 202 引导 web_fetch
  直取;401/432/433 分别给 key 无效/免费额度尽/PAYG 限额文案 + 指引
  Settings;网络错提示代理可能挂。

### 3. 后端行为要点(易错处)

- **Tavily**:body 显式 `"search_depth": "basic"`(1 credit)——不传
  时上游 `auto_parameters` 可能悄悄升 advanced(2 credits)。200 响应
  的 `results` 字段**必须存在**(无 serde default):形状漂移直接落
  `Parse`,不静默当零结果;单条缺 url 丢弃、缺 title 保留。
- **DDG**:202 = Ratelimit **软封锁**(基于出口 IP 信誉,非请求数,
  与 429 常规语义不同)→ 终态**不重试**;200 但解析 0 条 → `Parse`。
  UA 是**常量**(初始浏览器串;实测 n=1 与直觉反向、IP 信誉主导——
  常量化便于 202 复发时一键翻转验证)。
- **DDG 手写解析的配对单位是「结果块」不是文档序索引**:以容器标记
  `class="result `(带尾随空格,不撞 `result__a`)切块,块内各找第一
  个标题锚/片段锚。纯索引配对在中间块缺 snippet 时会把片段错挂到
  上一条(本任务实证)。href 是跳转链:实体解码(`&amp;`)**先于**
  取 `uddg` 参数,`Url::query_pairs()` 自带 percent-decode。
- **query 编码**:不用 `Url::parse_with_params`(form_urlencoded 空格
  编成 `+`);用 `percent-encoding` crate(已是直接依赖)手拼
  `?q=<NON_ALPHANUMERIC>`(空格 `%20`)。DDG 两者都收,但 `+` 形式
  httpmock 的 query_param 匹配器测不了。
- **重试环**:所有尝试共用**一个**外层 `tokio::time::timeout`
  (30s + 5s grace,不 per-attempt 另开);只重试 `Network(_)` 与
  429/5xx;退避 `base * 2^n + jitter`;`retry_open` 绑定 Provider 流
  不可复用。**超时/退避/ grace 必须经 `SearchOpts` 参数注入**(生产
  `Default`,测试注毫秒)——否则 30s 超时路径没法单测。

### 4. 配置与选路(key 三态 + aad 隔离)

`app_config` KV(零 migration):`web_search.provider`
(`auto|tavily|ddg`)+ `web_search.tavily_api_key`(AEAD 密文,
**aad = `"web_search"`**——providers 表用 provider id 作 aad 的同款
隔离先例,master key 同 `crypto::derive_master_key`)。

- **选路在每次 execute 时读**(fail-open:读失败/未知值 → auto;
  key 解密失败(machine-id 变了)→ 视为无 key,auto 静默回落 DDG)。
  **失败不跨 provider 自动兜底**:静默换道会把「Tavily 额度尽」掩盖
  成「DDG 结果质量差」;auto 只按 key 有无**静态**选路,失败原因单一。
- **key 三态语义**(IPC `set_web_search_config(provider,
  tavily_api_key: Option<String>)`,参数扁平标量——铁律):
  `Some(非空)` 重加密落盘 / `Some("")` **清除删行**(切 ddg 停用后
  残留密文不会在切回 auto 时静默复活)/ `None` 不动(前端留空=不改)。
- **明文 key 永不出后端**:GET 只回 masked(`tvly-****` + 后 4 位;
  ≤9 字符整体 `****`)。IPC 双形态:`commands/config.rs` `_inner` +
  Tauri command + daemon POST route(config router 全 POST 先例)+
  `CMD_TO_DOMAIN` 两行,四处缺一浏览器模式即 404/unknown cmd。

### 5. 权限 / 名单 / stub

| 环节 | 裁决 | 依据 |
|---|---|---|
| Tier | 5 silent Allow | `classify_tool` 未列 → `ToolKind::Other`(同 `search_history`;查询词本来就要发给 LLM provider,二跳增量小) |
| C7D stub | **第 11 员**(`Search the web.`) | 注册即撞 3960 静态线——stub 化后**零平移**(余量 57,条目 ~33 tok)。校准史见 [14-stub-registration](./14-stub-registration.md) §2 |
| L2 并行白名单 | **不进** | stub 互斥不变量(`STUB_CANDIDATES ∩ PARALLEL_WHITELIST = ∅`);搜索单发,收益低 |
| worker(并发 readonly) | 可用 | `READONLY_TOOL_ALLOWLIST` 第 7 员(l3a 守卫 6→7 显式跟改) |
| builtin researcher | 可用 | vec 6 员 + system prompt 枚举同步(tests_mod/tests_loader 精确列表断言显式跟改) |
| 群聊 | 可用 | `GROUP_CHAT_RESEARCH_TOOLS` + **participant prompt 的工具枚举串也要同步**(prompt 硬编码 `read_file / grep / ... / web_fetch`,漏改则参与者不知道自己有它) |
| workflow frontmatter | **四处** | builtin 两 + **项目层两**(`.everlasting/workflow/**`)。项目层 runtime 优先、builtin 仅编译期 fallback——漏改项目层则本项目 live researcher 不生效,grep 级验收假绿。守卫:`tests_loader::repo_workflow_agents_load_web_search` 用真实 loader 断言本仓项目层副本含 web_search 且 source=Plugin |

### 6. Tests(`tools/tests_web_search.rs` + 模块内联,共 39)

- 后端契约:httpmock 锁 Tavily(200/401 终态不重试/429 三连耗尽/
  432 文案/请求体含 basic+max_results)与 DDG(200 解析含 uddg 解码/
  202 不重试且文案含 web_fetch/200 零条 → Parse/count 切片)。
- **重试-后-成功**:httpmock 0.7 无按次数行为(无 `up_to_n_times`),
  用 **axum 迷你服务 + AtomicUsize 计数器**实现有状态序列
  (429→200、500→503→200;axum 已是 lib 依赖,daemon route 测试同款)。
- 超时路径:`SearchOpts` 注 100ms + mock `delay(2s)` 断言整体预算兜住。
- 选路:auto 无 key→DDG / auto 有 key→Tavily / 显式 tavily 无 key →
  可操作错误 / 显式 ddg 压过残留 key / 坏密文降级无 key。
- 配置三态 + masked + 三值校验(mod 内联);IPC 路由全链
  (`daemon/routes/config.rs` oneshot:get→set key→masked→清空)。

### 7. live 冒烟结论(2026-08-25)

`turn-smoke.sh` 走 debug daemon(7457,不动用户 7456):auto 无 key
→ DDG 实搜 "tokio rust tutorial" 成功,tool_result 尾行
`<!-- searched: ... via ddg at ... · 5 results -->`;stub 链路照常
(模型先 `load_tool_schemas(["web_search"])`);audit 两行
(`tool_allowed` reason=null + `tool_executed`)。**注意**:脚本报
"OK" 时 turn 可能仍在续跑(多跳 tool_use),立即杀 daemon 会留下
`[已停止]` 标记——收尾前轮询末条 assistant 是否完成。
