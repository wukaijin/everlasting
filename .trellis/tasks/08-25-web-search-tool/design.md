# design.md — web_search 工具

> 依赖:PRD R1-R7;research 两份(file:line 级事实都在里面,本文只做结构决策)。
> 模块模板:`spec/backend/tool-contract/01-tool-set-extension.md`(新工具基模)+ `15-search-history.md`(最近的只读工具全程样板)。
> 评审修订(2026-08-25 review.md):P1-3 契约定稿(enum dispatch、无 CancellationToken)、P1-2 key 三态、P2-2 POST 动词、P2-3 clamp、P2-6 UA 常量、P1-1 frontmatter 四处——均已融入正文。

## 1. 模块结构

目录式(web_fetch 是单文件,但 web_search 有 2 个后端 + 解析器,目录更清晰):

```
app/src-tauri/src/tools/web_search/
  mod.rs        # definition() + execute() + 渲染 + 配置选路 + 重试环 + 测试入口
  tavily.rs     # TavilyClient:POST api.tavily.com/search
  ddg.rs        # DdgClient:GET html.duckduckgo.com/html + 解析
tests_web_search.rs  # tools/ 下,与 tests_web_fetch.rs 平级(httpmock)
```

## 2. 核心契约(P1-3 定稿:enum dispatch,无 dyn、无新依赖)

```rust
// mod.rs
pub struct SearchHit { pub title: String, pub url: String, pub snippet: String }

pub(crate) enum SearchError {
    RateLimited,            // DDG 202:不重试,文案引导 web_fetch
    HttpStatus(u16),        // 4xx/5xx 终态(401/432/433 = key/额度问题,文案区分)
    Network(String),        // 瞬时:可重试
    Timeout,
    Parse(String),          // 页面/响应形状变了
}

// enum dispatch:仅两 provider。edition 2021、仓内无 async-trait,
// 原生 async fn in trait 不支持 dyn;Provider trait 先例是手写 boxed
// stream(llm/provider/mod.rs),两 variant 用不上那么重的形态。
// 「留口」= 加 variant(智谱/博查未来照此扩)。
pub(crate) enum SearchBackend {
    Tavily(TavilyClient),
    Ddg(DdgClient),
}

impl SearchBackend {
    fn name(&self) -> &'static str { /* "tavily" | "ddg",attribution 用 */ }
    async fn search(&self, query: &str, count: u8) -> Result<Vec<SearchHit>, SearchError> { /* match 两臂 */ }
}
```

- 签名**不带 CancellationToken**(定稿):`execute_tool` 外层通用 cancel 包装(`tools/mod.rs:455`)白得,reqwest future drop 即取消。
- 输入校验在 `execute()` 收口:query trim + ≤400 字符;count 手动取数(`as_u64`),0/负数/类型错 → 默认 5,合法值 `clamp(1, 10)`。
- 渲染(纯文本,进 tool_result):

```
1. <title 截120>
   <url>
   <snippet 截300>
2. ...
<!-- searched: <query> via <provider> at <rfc3339> · N results -->
```

attribution 注释放尾部(本工具结果集自截断,不存在被截风险;web_fetch 头部是为防截,此处无需)。

## 3. 后端实现

### Tavily(tavily.rs)

- `POST https://api.tavily.com/search`,头 `Authorization: Bearer <key>`(key 从配置解密后构造 client 时注入,不进日志)。
- body:`{"query": q, "max_results": count, "search_depth": "basic"}`——**显式 basic**(防 `auto_parameters` 悄悄升 advanced 烧 2 credit;不传 auto_parameters)。
- 响应:`results[].{title, url, content}` → `SearchHit { snippet: content }`;`score`/`raw_content` 忽略。
- 错误映射:401/432/433 → `HttpStatus`(文案区分「key 无效 / 免费额度尽 / PAYG 限额」);429 与 5xx → 可重试。
- 复用 web_fetch 的 reqwest client 构建惯例:UA `Everlasting/<ver>`、10s connect timeout、整体 30s(`tokio::time::timeout` 外包 +5s grace)。**超时与 connect 常量提为 client 构造参数(默认生产值,测试注短值)**——否则 30s 超时路径没法单测。**SSRF 面不存在**(固定域名固定 endpoint,无用户可控 URL)——不引 `is_blocked`;DNS 走系统(吃代理 env,与 web_fetch 行为一致)。

### DDG(ddg.rs)

- `GET https://html.duckduckgo.com/html/?q=<urlencoded>`,UA 提为**常量**(初始值用浏览器串;实测样本 n=1 无统计力——`Everlasting/0.1` 200 / Mozilla 202,IP 信誉主导——常量化便于 202 复发时一键翻转验证)。
- 解析:零新依赖手写——定位 `class="result__a"`(标题 + href)、`class="result__snippet"`(片段);href 是 DDG 跳转链,真实 URL 在 `uddg=` 查询参数,`percent_decode` 还原(仓内已有 urlencoding 工具则复用,实施时查)。fixture 锁已知形状。
- **202 → `RateLimited`**,不重试;200 但 0 条结果 → `Parse`(形状漂移信号)。

## 4. 配置与选路(mod.rs + db/config + crypto)

```
app_config:
  web_search.provider      = "auto" | "tavily" | "ddg"     (缺省 auto)
  web_search.tavily_api_key = AEAD 密文(crypto::encrypt(master_key, key, aad="web_search"))
```

- 选路在**每次 execute 时**读 provider 配置(best-effort;读失败 → auto,同 `tools_stub_enabled` 的 fail-open 读法,`chat_loop.rs:628` 先例);tavily key 解密失败 → 视为无 key。
- `auto`:解密出非空 key → Tavily,否则 DDG。显式 `tavily` 无 key → 直接返回可操作错误(指引 Settings 配置)。
- master key:`crypto::derive_master_key()`(与 providers 同源;keyring 已在 06-24 决策否决,WSL fallback 路径)。

## 5. IPC / 前端

- **Tauri command**:`get_web_search_config`(返回 `{provider, tavily_key_masked: Option<String>, tavily_key_set: bool}`)+ `set_web_search_config(provider, tavily_api_key: Option<String>)`。**key 三态语义**(P1-2):`Some(非空)` = 重加密落盘,`Some("")` = 清除(删 `app_config` 行),`None` = 不动。`web_search.provider` 三值校验。
- **daemon route**:同语义,**动词照 config router 全 POST 先例**(`/get_remote_config`、`/set_remote_config`,`daemon/routes/config.rs` router()):`/get_web_search_config`、`/set_web_search_config`(浏览器/手机 PWA 模式);command inner + route 分层照 tunnel config 模式(`commands/config.rs:164-218`)。
- **transport**:`app/src/transport/` 抽象层加两方法(默认 httpTransport 走 daemon,tauriTransport invoke command);`transport-parity.test.ts` 补新命令 parity 用例。
- **Settings modal**:新「Web 搜索」小节——provider 下拉(auto/tavily/ddg)+ key 密码框(placeholder 显示 masked,留空 = 不改)+ 「清除已存 key」动作(文案说明清除后 auto 回落 DDG);用既有 Settings 表单样式(spec `frontend/design-tokens.md`、component-guidelines)。
- **audit chip**:`utils/audit.ts` `summarizeToolInput` 加 `web_search → query` 分支(通用 fallback 落 JSON.stringify 截断,不可读)。
- 工具卡片:通用 ToolCallCard,`toolIcon` 可选加搜索图标(非必须)。

## 6. 名单翻转(R6/R7)

| 名单 | 位置 | 动作 |
|---|---|---|
| `builtin_tools()` | `tools/mod.rs:140-241` | **append 最后**(prefix cache 不变量) |
| `execute_tool_inner` | `mod.rs:469` | 新 match 臂 |
| `STUB_CANDIDATES` / `STUB_DESCRIPTIONS` | `tools/stub.rs:33-44`(CANDIDATES)、`stub.rs:60+`(DESCRIPTIONS),两处定长 `[..; 10]` → 11 | + `"web_search"` + 一行极短描述 |
| `READONLY_TOOL_ALLOWLIST` | `agent/subagent/tools_filter.rs:128-135` | + `"web_search"` |
| builtin researcher | `agent/subagent/registry.rs:94-100` + prompt `:80-92` | vec + `"web_search"`,prompt 工具枚举同步 |
| `GROUP_CHAT_RESEARCH_TOOLS` | `agent/group_chat_prompts.rs:195` | + `"web_search"` |
| workflow frontmatter ×4 | builtin:`app/src-tauri/resources/builtin-workflow/dev/agents/researcher.md:4`、`.../review/agents/reviewer.md:4`;项目层:`.everlasting/workflow/dev/agents/researcher.md:4`、`.everlasting/workflow/review/agents/reviewer.md:4` | tools 行 + web_search。**项目层 runtime 优先、builtin 仅编译期 fallback**(`agent/workflow/builtin.rs:5`);本仓项目层副本已与 builtin 分叉,漏改则本项目 live researcher 不生效(P1-1)|

## 7. 重试环(mod.rs)

- 只对 `Network(_)` 与「429/部分 5xx」重试:退避 `base * 2^n + jitter`(借 `RetryPolicy::wait` 公式概念),≤2 次重试;每试内嵌 30s 整体预算(重试共用外层 `tokio::time::timeout`,不另开)。
- 取消:外层通用 cancel 包装(`tools/mod.rs:455`)持有;provider 签名不带 CancellationToken(§2 定稿)。

## 8. 兼容与回滚

- 纯新增:无 DB migration(app_config 表已在)、无 wire 格式变更(tool_result 走通用信封)、无 settings 破坏性变更。回滚 = revert 单 commit 序列。
- 老版本 daemon + 新前端:web_search config route 404 → Settings 小节显示加载失败并允许重试(transport 层既有错误处理模式)。
- DDG/Tavily 上游形状漂移:`Parse` 错误文案保留原始片段长度信息,便于诊断;fixture 单测锁已知形状。

## 9. 权衡记录

- **enum dispatch 而非 trait+dyn**(P1-3):两 provider、零新依赖;加后端 = 加 variant,未来重构成本可忽略。
- **不进并行白名单**:stub 互斥不变量 + search_history 同现状;搜索通常单发,收益低。
- **失败不跨 provider 兜底**:静默换道会把「Tavily 额度尽」掩盖成「DDG 结果质量差」,排障困难;auto 的静态选路让失败原因单一。
- **key 三态而非「留空=清除」**:清除是显式动作,防误清;残留密文复活风险由清除语义 + auto 选路注释共同封死。
- **DDG 手写解析 vs 引 scraper**:一页两选择器,引 DOM 依赖不值;fixture 锁形状 + `Parse` 错误兜底。
- **key 不回明文给前端**:GET 只回 masked(`tvly-****后4位`);与 providers key 展示策略对齐(实施时对齐现状,若 providers 回明文则本工具仍坚持 masked——新面不扩大旧债)。
