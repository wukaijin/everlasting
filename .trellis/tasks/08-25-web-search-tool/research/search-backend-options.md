# F4 Web 搜索工具 — 后端选型调研

> 调研日期:2026-08-25(所有网络实测均当日从开发机 WSL 发出)。
> 结论先行:**推荐「snippet-only 搜索 + 既有 web_fetch 取正文」的两段式;provider 抽象首期实现 Tavily(有免费额、免信用卡、国内直连)+ DDG HTML(零配置兜底)**,智谱 search_std 作为国内付费备选。关键否决事实:Brave API 与 DDG 在无代理直连下全部超时,DDG HTML 走代理也实测吃到 202 软封锁。

---

## 1. 网络可达性实测(本机 = 目标运行环境)

本机 shell 环境带全局代理 `http_proxy=http://127.0.0.1:7897`(Clash 类);reqwest 默认 client 吃这些环境变量(web_fetch 的 reqwest::Client::builder() 未关代理,且 `web_fetch.rs:114` 已有 fake-ip 代理注释、`:691` 有 fake-ip DNS 死解析兜底)。**daemon 继承环境变量,所以"代理在"与"代理挂"是两种真实运行态,都得测。**

### 1.1 走代理(代理正常运行时)

| 端点 | 结果 |
|---|---|
| `https://html.duckduckgo.com/html/?q=test` | 首次 200(1.5s);**换 Mozilla UA 再请求 → 202 软封锁**(14KB 异常页,0 个结果) |
| `https://lite.duckduckgo.com/lite/?q=test` | **202** |
| `https://api.search.brave.com/res/v1/web/search` | 422(可达,缺参/缺 key) |
| `https://api.tavily.com/search` | 401(可达) |
| `https://www.bing.com/search?q=test` | 200 |
| `https://api.bochaai.com/v1/web-search` | 405(可达) |
| `https://open.bigmodel.cn/api/paas/v4/web_search` | 401(可达) |

### 1.2 直连(`curl --noproxy '*'`,模拟代理挂掉)

| 端点 | 结果 |
|---|---|
| `html.duckduckgo.com` | **超时(10s 死)** |
| `api.search.brave.com` | **超时(10s 死)** |
| `api.tavily.com` | **401,0.8s — 直连可用** |
| `www.bing.com` | 302(跳区域站,可用) |
| `api.bochaai.com` | 405,0.15s — 可用 |
| `open.bigmodel.cn` | 401,0.09s — 可用 |

**含义**:Brave/DDG 类「代理依赖型」后端在 Clash 停摆时整链路死;Tavily/智谱/博查双态皆活。选型必须把这个当一维变量。

### 1.3 DDG/Bing HTML 抓取质量实测

- DDG html:同一出口 IP 两次请求内即触发 202(共享 Clash 出口 IP 信誉差,与社区反馈一致——`duckduckgo_search`/`ddgs` 库的 `RatelimitException` 是 issue tracker 最常见报错,2024-2025 趋严,基于 IP 信誉而非请求数)。→ **只能当零配置兜底,不能当主力**。
- Bing html:200 但整页仅 1 个 `class="b_algo"`(bot-wall / 懒渲染),解析脆。→ 不选。

---

## 2. 候选后端矩阵

| 后端 | Key | 免费额度 | 之后价格 | 国内直连 | 走代理 | 输出质量 | 判定 |
|---|---|---|---|---|---|---|---|
| **Tavily** | `tvly-` | **1000 credits/月,免信用卡** | $8/1k(basic 1 credit,advanced 2) | ✅ 0.8s | ✅ | LLM 优化:结果带清洗过的 content 片段 + relevance score | **首选主力** |
| **智谱 web_search** | 智谱 key | 无免费额 | **¥0.01/次**(search_std)/ ¥0.03(pro)/ ¥0.05(sogou/quark) | ✅ 0.09s | ✅ | title/link/content/publish_date + search_intent | 国内付费备选(用户若已有智谱 key 顺手) |
| **博查 Bocha** | key | 无 | ¥0.036/次 | ✅ 0.15s | ✅ | 近千亿网页 + 生态源,Bing 平替定位 | 备选(比智谱贵 3.6×,暂不实现) |
| **Brave API** | key | 免费层已取消,改 $5/月 credit(≈1000 次),绑卡进计量 | $5/1k,50 QPS | ❌ 超时 | ✅ | 标准 SERP(title/url/desc) | 否决:代理依赖 + 免费层取消 + 要绑卡 |
| **DDG html/lite** | 无 | — | — | ❌ 超时 | △(实测 202) | SERP 抓取 | **零配置兜底**(best-effort,失败给模型可操作的错误文案) |
| Bing 抓取 | 无 | — | — | △ | △ | 差(实测 1 条) | 否决 |
| Serper.dev | key | 2500 次(一次性) | $50/50k | 未测(Google 系,大概率代理依赖) | ✅ | Google SERP | 不实现,provider 抽象留口 |
| SearXNG 自host | 无 | — | VPS 资源 | 自建于国内 VPS 只能聚合 CN 可达引擎 | ✅ | 聚合 | 否决 MVP:多一个运维件;JSON API 需开 `search.formats`,limiter 要关;国内 VPS 聚合不到 Google/DDG 意义减半 |

## 3. 已锁定 API 契约(官方文档核对)

### Tavily(实现时照抄)

- `POST https://api.tavily.com/search`,头 `Authorization: Bearer tvly-xxx`,JSON body:
  - `query`(必填);`search_depth`: `basic|advanced|fast|ultra-fast`(默认 basic);`max_results` 0-20(默认 5);`topic`: `general|news|finance`;`time_range`: `day|week|month|year`;`include_domains`/`exclude_domains`(≤300/150);`include_answer` bool;`include_raw_content` bool|`markdown|text`
- 200 响应:`{query, answer?, results:[{title, url, content, score, raw_content?, favicon, id}], response_time, usage}`
- 计费:basic/fast/ultra-fast = 1 credit,advanced = 2;`auto_parameters` 可能自动升 advanced(想省钱需显式 basic)
- 错误码:400 参数 / 401 key / 429 限速 / 432 套餐尽 / 433 PAYG 限额

### 智谱 web_search(国内备选)

- `POST https://open.bigmodel.cn/api/paas/v4/web_search`(Bearer 智谱 key),body:
  - `search_engine`: `search_std|search_pro|search_pro_sogou|search_pro_quark`;`search_query`(必填);`count` 1-50(默认 10);`search_domain_filter`;`search_recency_filter`(如 `noLimit`);`content_size`(`medium` 默认/`high`)
- 响应:`{created, id, request_id, search_intent:[{intent, keywords, query}], search_result:[{title, link, content, icon, media, publish_date, refer}]}` —— 注意字段名是 **`link` 不是 `url`**
- 无流式(standalone API)

### DDG html(兜底,抓取式)

- `GET/POST https://html.duckduckgo.com/html/?q=<query>`,解析 `a.result__a`(title;href 是跳转链,真实 URL 在 `uddg` 查询参数里)+ `a.result__snippet`
- **202 = Ratelimit 软封锁**(非 429,别按常规重试语义处理);返回给模型的文案要指导它改用 web_fetch 直取已知 URL

## 4. 同类 agent 设计参照

- **Claude Code `WebSearch`**:query → ~10 条 title+url+snippet(**不含正文**),正文由独立的 `WebFetch` 取。搜索与取文分离。服务端实现、仅美区。
- **OpenCode `websearch`**:多 provider 可配(DDG 免 key 默认 / Tavily / Brave / Exa…),config 选 provider + key。
- **Open WebUI** auto 模式 provider 优先级:Exa→Perplexity→Tavily→Brave→Firecrawl→SearXNG→DDG。
- **本项目仓内既有材料**(历史,值得翻):`docs/_deprecated/REVIEW-tool-comparison-2026-06-12.md`(Claude Code/OpenCode/Cline 的 WebFetch/WebSearch 对比表)、归档任务 `06-12-feat-tools-web-fetch-agent-api-p1/research/web-fetch-api-design.md`(当时就画了 WebFetch/WebSearch 边界,web_search 被显式推迟——`spec/backend/tool-contract/02-web-fetch.md:106`)。

## 5. 给 PRD/brainstorm 的决策点(带倾向)

> **终裁(2026-08-25,brainstorm + review.md 评审后)**:D1-D6 已全部定稿,**以 `prd.md` R1-R7 为准**。与下文"倾向"的两处分歧:D2 实现形态定为 **enum dispatch**(非 trait+dyn,review P1-3,见 design §2)且补 **key 三态清除语义**(P1-2);D5 定为**全开**(researcher/群聊/frontmatter 四处全加,推翻 08-17"只动 allowlist"先例的沿用)。

1. **D1 搜索→正文策略**:倾向 **snippet-only + 复用 web_fetch 取正文**(两段式,同 Claude Code 模型)。Tavily 的 `include_raw_content` 能一步出正文,但绕开 web_fetch 的 SSRF/attributon/截断管线且不省 credit,不倾向。ROADMAP F4 原文「搜索 → 取前 N 条结果正文」在此模型下 = 模型 search 后自己连发 web_fetch。
2. **D2 provider 策略**:倾向 provider trait + 首期两实现(`tavily` keyed / `ddg` 零配置兜底),配置项选 provider;`zhipu` 留口不实现。key 存哪是新问题(providers 表是 LLM 形状;可能 settings kv / 复用 crypto.rs 加密模式——见 06-24-p1-api-key-encryption 的 keyring 已否决结论)。
3. **D3 权限档位**:两个先例二选一 —— `search_history` 式 `ToolKind::Other` 静默放行(spec 15 走过这条,零权限层改动)vs web_fetch 式 Tier 4 ask(查询词外泄给搜索引擎,论敏感度低于任意 URL fetch)。倾向 MVP 静默放行,理由:查询词本来就要发给 LLM provider,二跳外泄增量小;若要 ask,泛化 WebFetch 分支即可。
4. **D4 token 预算**:注册即撞 `stub.rs:326` 静态测试(tools[] ≤3960 tok,spec 15 §4 预警过这一刻)——**必做**:把 web_search 加进 `STUB_CANDIDATES`(一行 stub 描述)。注意不变量 `STUB_CANDIDATES ∩ PARALLEL_WHITELIST = ∅`:选了 stub 就不能进并行白名单(search_history 也不在并行白名单,同命运,可接受)。结果集大小:N 默认 5、上限 10;每条 title+url+snippet,snippet 截 ~300 字符。
5. **D5 worker/群聊开闸**:`READONLY_TOOL_ALLOWLIST` +1 行(并发 worker 即得);`GROUP_CHAT_RESEARCH_TOOLS` 与 builtin `researcher` 是否同步扩,brainstorm 定(08-17 先例:只动 allowlist 不动 researcher)。
6. **D6 重试/降级**:429/5xx 小自研重试(借 `RetryPolicy::wait` 概念,`retry_open` 是 Provider 流绑定不可复用);DDG 202 不重试,直接返回可操作错误文案。代理挂掉时的表现 = 错误文案里说明网络不可达。

## 6. 调研方法备忘

- 可达性全部第一手 curl 实测(本机,2026-08-25),非文档转述;`-w "%{http_code} %{time_total}s"`,`--noproxy '*'` 测直连。
- 价格/契约来自官方文档页(Tavily docs API reference、智谱 docs.bigmodel.cn);社区反馈(DDG 202/RatelimitException)来自 PyPI/Reddit/GitHub issues 多源交叉。
- 未烧任何 API 配额(只做无 key 探测,401/405/422 即证明可达)。
