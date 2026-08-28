# 远程访问 / 多通道改造 调研评估 — 设计审查报告

> 审查日期：2026-07-20
> 审查模型：deepseek-v4-pro
> 审查范围：[`docs/REMOTE-ACCESS-RESEARCH.md`](../2026-07-20-remote-access-research.md)(约 800 行,6 章 + 附录 A)
> 关联文档：[ARCHITECTURE §4/§5](../../ARCHITECTURE.md#4-决策agent-daemon-化为多-channel-接入铺路) / [ROADMAP B10](../../ROADMAP.md) / [BACKLOG §6/§7](../../BACKLOG.md)
> 审查角度：范围适当性、技术选型合理性、改造路径可行性、风险完备性、与现有架构的契合度、遗漏问题

---

## 一、总体评价

这是一份**调研扎实、路径务实**的技术评估文档。从现状盘点 → 外部对标 → 技术选型 → 改造路径 → 风险评估，5 层递进逻辑完整。最值得肯定的是**渐变式改造策略**（Phase 1 零风险 transport 抽象 → Phase 2 daemon 拆分 → Phase 3 远期远程）——每个阶段独立交付价值、Tauri 版始终保持可用、不搞大爆炸式重写。

**核心优势**:
- 现状盘点精细（57 个 invoke / 9 个 emit / 22 个前端文件，逐项列出）
- 发现了已存在的关键抽象（`AppHandleSink` 实现 `ChatEventSink` trait），大大降低了 daemon 化成本
- 外部对标有取舍（选 opencode 模式做参照，不用 Claude Code 的云中转）
- "近期/远期"切割干脆（Phase 1+2 近期,Phase 3 留远期,Electron 可选）
- 明确的"不做"列表（mTLS / WebSocket / 多用户 / agent 上云）

**核心不足**: Phase 2 的工作量估（1-2 周）偏乐观、部分跨进程技术细节（oneshot → RPC 转换、Tauri `AppHandle` 依赖摘除）描摹不够、浏览器访问的端到端链路（从 Windows 宿主浏览器到 WSL daemon）未展开、缺失测试策略和持续验证方案。

下面从 9 个维度展开分析。

---

## 二、范围适当性 — 这个功能适合现在做吗？

### ✅ 适合的理由

1. **用户诉求明确**：§0.1 列出的 4 个诉求（浏览器远程访问/本地直连/远程配对/Electron）是真实需求，"浏览器关掉 agent 继续跑"是当前单进程架构的硬限制。

2. **触发条件已成熟**：原文 [ARCHITECTURE §4](../../ARCHITECTURE.md#4-决策agent-daemon-化为多-channel-接入铺路) 把 daemon 化挂在"飞书 IM 决定实施时"，但本次调研正确地把触发条件从"飞书"扩展为"真浏览器远程访问"，更务实——飞书是远期，但 transport 抽象 + daemon 拆分本身就有独立价值。

3. **抽象点已预留**：`AppHandleSink`（`state.rs:647-711`）已经是 `ChatEventSink` trait —— 这是项目早期最有远见的设计决策之一。agent loop 不直接调 `app.emit`，换 transport 只需新写一个 sink 实现。**这是整个调研文档最有价值的发现**（§1.2b）。

4. **Phase 1 零风险**：22 个文件机械替换 `import { invoke }` → `import { transport }`，后端零改动，全量 vitest + `pnpm tauri dev` 验证。即使后续不推进 Phase 2，这一步本身提高代码清晰度。

### ⚠️ 时机考量

调研文档把 Phase 1+2 定为"近期"(§6.2)，Phase 1 工作量估 1-2 天，Phase 2 估 1-2 周。这个排期在当前项目阶段（V2 第四档仍有 3 项未启动，包括 A2+ P3 shell 沙盒兜底、B10 飞书、B11 云端同步）是合理的——transport 抽象是基础设施，先铺路再盖楼。

但有一个**隐含优先级冲突**：Phase 2 的 1-2 周投入会阻塞其他第四档功能。如果同时有更紧急的 bug 或体验问题，Phase 2 可能需要让路。当前调研未讨论与其他 roadmap 项的排期冲突。

**建议**：在启动 Phase 1 前明确当前 roadmap 中是否有更高优先级的阻塞项（例如 A2+ P3 沙盒如果撞到了实际使用场景的 shell 逃逸问题，它比浏览器访问更紧急）。Phase 1 是 1-2 天的轻量投入，风险低，可以随时插队；但 Phase 2 需要 1-2 周的专注窗口。

---

## 三、现状盘点质量

### §1.2 IPC 表面盘点 — 🟢 优秀

57 个 invoke 字符串的模块分组表、9 个 emit 事件的负载类型和触发场景、错误协议的 wire shape——这是**教科书级的现状盘点**。权威源明确（`lib.rs:155-333` 的 `generate_handler!` 宏），dead_code/deprecated 的 4 个命令已标注（`test_provider` / `grant_tool_permission` / `get_pending_question` / `test_model`），`apply_ui_diff` 裸错误问题已指出。

**小建议**：`command_palette` 行的 `list_commands` / `get_command_body` 标注了"与 panel 重叠"——如果两处是同一个 command 在不同模块注册了两次，建议标注"同函数、重复注册"以明确不是两个独立实现。

### §1.2b emit 事件表 — 🟢 优秀

发现了 **`AppHandleSink` 已实现 `ChatEventSink` trait**，这是整篇调研最关键的技术发现。直接影响了 Phase 1 后端零改动、Phase 2 只需加一个 `HttpSseSink` 的决策。这个抽象点的价值怎么强调都不为过。

**小修正**：事件表列了 10 行但文档说"9 个"，数一下确实是 10 个（`chat-event` / `tool:call` / `tool:result` / `permission:ask` / `tool:question` / `mode:change:request` / `task:state:transition:request` / `subagent:event` / `subagent:finished` / `projects:refreshed`）。"9 个"应该是写作时的笔误。

### §1.3 共享状态 — 🟢 清晰

`AppState` 字段表 + daemon 化影响列非常实用。`oneshot` 通道的问题——"跨进程时 oneshot 必须在 daemon 内 resolve,client 只能发'用户答了'消息"——点到了最核心的跨进程 IPC 挑战。

**但缺少一个关键细节**：当前 `permission_asks` 和 `question_store` 都是 `Arc<Mutex<HashMap<String, oneshot::Sender<...>>>>`。跨进程后，`permission:ask` 事件推给 client，client 调用 `POST /api/permission/respond` 回答——daemon 端收到后如何找到原来的 `oneshot::Sender` 并 resolve？需要一个 **request-id → Sender 的注册表** (当前已有)，以及 HTTP handler 中通过 request-id 查找并 resolve 的逻辑。这个转换的代码量不大但需要明确接口。

### §1.5 前端 transport 耦合点 — 🟢 精确

22 个文件的完整列表 + `streamController.ts` 作为"单点 funnel"的识别非常准确。Phase 1 的改造面精确到文件级。

**遗漏**：除了 `invoke` / `listen` 和 `getCurrentWindow` / `plugin-os`，前端可能还有对 Tauri 事件 payload 类型的隐式依赖。例如 `streamController.ts` 接收的 `ChatEventPayload` 类型——它是从 `@tauri-apps/api` 推导的还是手动定义的 interface？如果是前者，transport 抽象后类型定义需要独立出来。建议在 Phase 1 前做一次 `grep -r "from '@tauri-apps" app/src/ --include="*.ts"` 确认没有遗漏的间接依赖（如 `@tauri-apps/api/event` 的类型导出）。

---

## 四、外部对标质量

### §2.1 Claude Code Remote Control — 🟢 有价值的借鉴

四个关键设计模式（读写不对称/FlushGate/BoundedUUIDSet/JWT epoch）提炼精准。特别是**读写不对称**——"远程客户端默认只读，写操作需要显式 grant"——与 Everlasting 的 `permission:ask` / `tool:question` / `mode:change:request` / `task:state:transition:request` 四类 emit 事件完美对应。这个设计模式应该在 Phase 3 详细设计时作为核心参考。

**小争议**：文档说 Claude Code 是"所有流量经 Anthropic API over TLS 中转"。这确实是官方 remote control 的做法，但 Claude Code 也支持 `--serve` 模式的本地 HTTP server（类似 opencode 模式）。建议在 §2.1 补充一句"Claude Code 也支持 `--serve` 本地 HTTP server 模式"避免误导读者以为 Claude Code 只有云中转一条路。

### §2.2 opencode — 🟢 主参照选择正确

opencode 的"自托管 HTTP server + 多 client"模式确实是 Everlasting 最接近的目标形态。TypeScript SDK + 类型共享的部分跟调研提出的 `ts-rs` / `typeshare` 方案一致。

### §2.4 对标小结 — 🟢 结论清晰

"opencode 模式是主参照、Claude Code 设计模式作协议层借鉴"的结论定位准确。

---

## 五、技术选型评估

### §3.1 传输协议 — 🟢 SSE + HTTP POST 是正确的选择

**SSE vs WebSocket 的论证充分**：
- 3 篇外部参考（BuildMVPFast / karls.io / LinkedIn）+ 行业共识（所有主流 LLM provider 都选 SSE）
- 对 Everlasting 的两类流量（高频单向流 / 低频双向 round-trip）分析准确
- SSE + HTTP POST 方案最简、浏览器原生支持、SSE 自带重连

**但有一个隐含假设需要验证**：SSE 的 `EventSource` API 的自动重连机制在 daemon 重启场景下是否够用？`EventSource` 默认 3 秒重试，但如果 daemon 重启时间超过这个间隔，客户端会进入更长的退避。Phase 2 实现时需要测试"daemon 重启 → 浏览器自动恢复 SSE 连接"的端到端行为。

**WebSocket 的排除理由可加强**：文档说"WebSocket 的双向能力用不满，徒增连接管理、重连、心跳复杂度"。这是正确的，但还应该加一点：**WebSocket 在企业代理/NAT 下穿透更难**（这个点在 §3.1b 提到了但没有带入 §3.1d 的结论中）。如果未来要支持企业环境下的远程访问（Phase 3），WebSocket 被代理阻断的概率高于 SSE（SSE 就是普通 HTTP）。这个论证放在 §3.1d 会让排除 WebSocket 的理由更完整。

### §3.2 认证与配对 — 🟢 方案务实

**配对码 + 派生 Bearer token** 是"浏览器可访问"约束下的唯一可行方案。mTLS 正确出局。设计要点（6 位码 + 5 分钟过期 + 单次使用 + rate limit + 32 字节随机 token + epoch 作废）覆盖了主要威胁。

**遗漏 1: token 存储安全**：文档说"Browser 存 token 到 localStorage / IndexedDB"。localStorage 可以被 XSS 读取（如果前端有注入漏洞）。对于浏览器场景，应该考虑 `httpOnly` cookie（防 XSS）+ CSRF token（防跨站请求伪造）。但 cookie 方案不适合 SPA 的 `Authorization: Bearer` header 模式。**建议**：在 Phase 3 设计时明确 token 存储策略，至少标注"localStorage 的 XSS 风险"。如果前端本身不渲染用户生成内容，XSS 风险可控；如果未来允许渲染 LLM 输出的 HTML/markdown 中的 script，则需要更严格的防护。

**遗漏 2: token 撤销 UX**：文档提到了 epoch 可批量作废，但没有设计"用户在已配对设备上管理其他设备"的 UI 流程。Phase 3 需要：列出所有已配对设备 + 逐个撤销 + 撤销确认。

### §3.3 网络拓扑 — 🟡 形态 B2 的选择有前提

Cloudflare Tunnel 的方案在技术上是合理的，但有一个**用户成本问题**：
- 需要用户拥有域名（`agent.yourdomain.com`）并配置 DNS
- 需要安装 `cloudflared` 客户端
- 需要 Cloudflare 账号

这些对开发者用户是合理的要求，但调研应该明确标注这些前提。建议在 Phase 3 的用户文档中给出"三步上手指南"（域名 → cloudflared → config.yml）。

另外，**Tailscale Funnel 被排除的理由不够充分**。文档引用了 Tailscale vs Cloudflare 对比表，结论是 Cloudflare Tunnel。但 Tailscale Funnel 的"P2P mesh VPN,设备直连"有一个优势：**用户不需要域名**，Tailscale 自动分配 `https://<hostname>.<ts-domain>.ts.net`。对于不想买域名的用户，Tailscale Funnel 的零配置体验更好。建议 Phase 3 把 Tailscale Funnel 也作为备选方案记录，两套部署文档并存。

### §3.4 进程管理 — 🟢 systemd 方案成熟

systemd unit + SIGTERM handler + CancellationToken + `TimeoutStopSec=30s` 的设计是 Rust daemon 的标准模式。`KillMode=mixed` 的选择也很关键——daemon 应该负责任地管理它 spawn 的子进程。

**WSL 特殊性处理正确**：WSL 2 默认不带 systemd，提供了手动启动脚本降级方案。这个处理符合项目的 WSL-first 定位。

**小建议**：`sd_notify(READY=1)` 标注为"可选"——建议升级为"推荐"。systemd 的 `Type=notify` + `sd_notify` 可以避免竞态条件（`Type=simple` 下 systemd 认为服务已启动，但实际 HTTP server 可能还在 bind 端口），而且 `sd-notify` crate 的集成只需要 3 行代码。成本极低，收益明确（正确通知启动完成、支持 systemd 的 restart 节流）。

---

## 六、改造路径设计审查

### Phase 1 — 🟢 设计优秀

transport 抽象的设计恰到好处：
- `Transport` interface 最小化（`invoke` + `listen`），不 overdesign
- `tauriTransport` 直接映射现有 API，零行为变化
- `isTauri()` 运行时判断切换 transport

**一个细节**：`listen` 的签名返回 `Promise<() => void>`。Tauri 的 `listen` 返回的 unlisten 函数是同步的，但包装成异步也兼容。不过 `EventSource` 的关闭也是同步的——建议接口保持 `() => void` 而非 `Promise<() => void>`，减少不必要的 Promise 包装。

**另一个细节**：`Record<string, unknown>` 不够精确。 `invoke<T>` 的 args 类型应该是针对每个 command 的具体参数类型，而不是宽泛的 `Record<string, unknown>`。当然 Phase 1 可以先用宽泛类型（不改变调用处的类型推断），Phase 2 用 `ts-rs` 生成类型后收紧。

### Phase 2 — 🟡 工作量估偏乐观，几个技术细节需要展开

#### 2a. 工作量估

"1-2 周"的估算是**乐观情形**（所有事情都顺利）。更现实的估算是 **2-3 周**，原因：
- 57 个 command → HTTP handler 的机械映射看似简单，但实际上每个 handler 都需要处理 Tauri `State<'_, Arc<AppState>>` → axum `Extension<Arc<AppState>>` 的转换
- `HttpSseSink` 需要处理 session-scoped 路由（当前 Tauri 的 `app.emit` 是全局广播，SSE 需要按 session_id 路由到正确的 EventSource 连接）
- `ChatEventSink` trait 的 emit 方法签名可能需要调整（当前 `emit(&self, event_name: &str, payload: impl Serialize)` 在 SSE 场景下需要追加 session_id 路由信息）
- 前端 `httpTransport.listen` 需要处理 `EventSource` 的连接管理（重连、错误处理、多个 listen 共享同一个 SSE 连接还是每个事件开一个连接）
- `pick_project_dir` 的浏览器替代方案（`<input type="file" webkitdirectory>`）需要额外的前端开发

#### 2b. Tauri `AppHandle` 依赖摘除

文档 §4.3 提到"复用 `AppState::load` 逻辑(去掉 `AppHandle` 依赖,改用 env var / config 文件)"。这是一个**被低估的改造点**。

当前 `AppState::load` 接受 `AppHandle` 参数，用于：
- `app_handle.path().app_data_dir()` — 获取数据目录
- `app_handle.path().home_dir()` — 获取 home 目录

这些在 daemon 进程中需要用 `dirs` crate 替代（`dirs::data_dir()` / `dirs::home_dir()`），但必须保证**与 Tauri 版本的路径一致**，否则 daemon 读写的是另一个 SQLite 文件。这是一个需要显式验证的迁移点——建议 Phase 2 的第一步就是让 `AppState::load` 同时支持 `AppHandle` 和纯 `PathBuf` 两种初始化方式，然后比较两种方式产出的路径是否一致。

#### 2c. SSE session 路由

当前 Tauri `app.emit("chat-event", payload)` 是**全局广播**——所有前端 listener 都收到。在 SSE 模式下，daemon 需要知道"哪个 SSE 连接属于哪个 session"，否则所有浏览器标签页都会收到所有 session 的事件。

**方案**：`GET /api/stream/{session_id}` SSE endpoint，daemon 维护 `session_id → Vec<EventSource>` 的映射。`HttpSseSink` 收到 emit 时根据 `request_id` → `session_id` 路由到正确的 EventSource。`ChatEventSink` trait 可能需要扩展以携带 session 上下文，或者 daemon 层在 sink 外维护 request_id → SSE 连接的映射。

这个设计应该在 Phase 2 的详细设计中展开，当前调研只提了"实现 `HttpSseSink`"但没有说明 session 路由机制。

#### 2d. 前端 `httpTransport.listen` 的 SSE 连接模型

前端有 4 个文件调 `listen`（streamController / permissions / projects / subagentRuns），每个 listen 监听不同事件。浏览器端有两个选择：
- **方案 A**：每个 `listen` 独立开一个 `EventSource` 连接（4 个 SSE 连接）
- **方案 B**：全局一个 `EventSource` 连接（`GET /api/stream/{session_id}`），收到所有事件后在 client 端按 `event:` 字段分发

方案 B 更好（减少连接数），但需要在 `httpTransport` 内部实现事件分发逻辑。当前调研的 `Transport.listen` 接口设计为每个 listen 独立订阅，在方案 B 下需要内部的复用逻辑。

**建议**：Phase 2 先用方案 B（全局一个 SSE 连接），`httpTransport` 内部维护事件 → handler 的映射表。

#### 2e. GUI 进程的双模式

文档 §4.3 提到"GUI 进程是否还跑 agent core?" 倾向"保留 GUI 内嵌 agent core 作为离线 fallback"（选项 A），但"Phase 2 先做 B"。

这个决策是正确的——先简单（GUI 变 thin client，agent 只在 daemon），再补双模式。但需要明确：**Phase 2 做完后，当前 `pnpm tauri dev` 还能正常工作吗？** 答案取决于 daemon 的启动方式：
- 如果 Tauri app 启动时自动拉起 daemon 进程 → `pnpm tauri dev` 照常工作
- 如果 daemon 需要手动启动 → 开发体验劣化（多一步）

**建议**：Phase 2 的 Tauri app 增加 daemon 进程管理能力（启动时尝试连接 daemon，连不上就自动 spawn daemon 子进程，关闭 Tauri 时清理 daemon）。这样开发体验基本不变。

### Phase 3 — 🟢 远期设计草稿质量合理

配对码流程、HTTPS、Cloudflare Tunnel、"读写不对称"的设计都有 Claude Code 的成熟模式可参照。调研文档对 Phase 3 的定位（"设计草稿，留待启动时细化"）实事求是。

**一个需要提前考虑的 Phase 3 前置条件**：读写不对称需要在前端区分"本地连接"和"远程连接"——本地同机的浏览器不应该弹二次确认（否则体验劣化）。判断逻辑可以简单：`window.location.hostname === 'localhost' || '127.0.0.1'` → 本地模式。但这个判断需要 transport 层暴露给上层。建议在 `Transport` interface 预留一个 `isLocal: boolean` 属性，Phase 1 实现时 `tauriTransport.isLocal = true`，`httpTransport.isLocal` 根据 hostname 判断。

### Phase 4 — 🟢 Electron 分析客观

"Tauri + Web 浏览器访问已覆盖全部场景，Electron 是 nice-to-have"的结论公正。对比表（包大小/内存/原生能力/浏览器复用/维护成本）有说服力。

---

## 七、风险完备性

### §5.1 已识别风险 — 🟢 覆盖了核心技术风险

8 项风险的识别和缓解措施合理。特别值得肯定的是：
- **协议 drift** 的缓解（`ts-rs` / `typeshare`）精准
- **DB 并发** 的决策（daemon 独占 DB）正确——SQLite 不适合多进程并发写
- **认证 bypass** 标注"极高"，缓解措施具体

### 需补充的风险

#### 风险 9: **WSL 网络边界（中风险）**

当前架构下 Tauri webview 和 Rust 后端同进程，走 Tauri IPC，没有网络边界问题。daemon 化后，daemon 监听 `127.0.0.1:PORT`，但：

- **WSL 2 的网络隔离**：WSL 2 默认有独立的虚拟网络接口，`localhost` 在 WSL 内和 Windows 宿主上不是同一个。从 Windows 宿主浏览器访问 WSL 内的 daemon，需要通过 WSL 的虚拟 IP（`localhost` 在 WSL 2 默认 forwarded，但端口需要显式映射或使用 `wslhost.exe`）。
- **端口冲突**：daemon 选的端口可能与 Windows 宿主上的其他服务冲突。

当前调研没有涉及 WSL-specific 的网络配置。`pnpm tauri dev` 现在能工作是因为 Tauri 处理了 WSLg 窗口转发，与网络无关。但浏览器访问需要网络层连通。

**缓解**：Phase 2 启动时把 WSL 网络配置作为第一步验证——确认 `curl http://localhost:PORT` 从 Windows 宿主可用（localhost forwarding 通常是 WSL 2 默认行为,但需要验证端口转发）。如果不可用，提供 `wslhost.exe` 或 `netsh interface portproxy` 的配置说明。

#### 风险 10: **前端构建产物的部署（中风险）**

当用户通过浏览器访问 daemon 时，前端 Vue 代码需要从某个 web server 加载（不是 Tauri webview 内嵌）。当前 `pnpm build` 产出 `app/dist/`，但 daemon 需要：
- 要么内嵌一个静态文件 server（axum 加 `tower-http` 的 `ServeDir`）
- 要么用户另外开一个 web server（如 `pnpm preview`）

调研没有讨论前端产物的分发方式。如果 daemon 内嵌静态文件 server，前端 JS 中的 `fetch('/api/...')` 会自动走同源请求（不需要 CORS）。如果用户另起 web server（如 `localhost:5173` dev server），就需要 CORS 配置。

**建议**：Phase 2 的 daemon 内嵌静态文件 server（axum + `tower-http::services::ServeDir` 指向 `app/dist/`），实现单二进制部署。这样浏览器访问 `http://localhost:PORT` 就能同时拿到前端代码和 API。

#### 风险 11: **`pick_project_dir` 浏览器降级的兼容性（低风险）**

`<input type="file" webkitdirectory>` 的支持情况：
- Chrome/Edge: ✅ 完全支持
- Firefox: ✅ 支持但 UI 略有差异
- Safari: ✅ 支持

调研说"需要 transport 适配"，但适配的是**一个完整的 Tauri command → HTTP handler + 前端 UI 替换**。`pick_project_dir` 在 Tauri 版调的是原生 dialog，返回值是文件系统路径。浏览器版用 `<input webkitdirectory>` 只能拿到 `File` 对象和相对路径名——**拿不到绝对路径**。这是一个浏览器安全限制，无法绕过。

**缓解**：浏览器版的目录选择需要两步走：
1. `<input webkitdirectory>` 让用户选目录
2. 后端通过其他方式获取该目录的绝对路径（比如让用户手动输入路径作为备选，或者在 `<input>` 旁边显示"选择目录后粘贴路径"的说明）

这比"transport 适配"更复杂，需要明确 UX 设计。建议 Phase 2 对 `pick_project_dir` 做一个专门的 UX spike。

---

## 八、遗漏/未充分讨论的问题

### 8.1 测试策略 — ⚠️ 完全缺失

整个调研没有提及测试策略：
- Phase 1 如何验证？文档说"全套 vitest + `pnpm tauri dev` 跑通"，但 vitest 当前 mock 了 `invoke` 吗？如果没有，怎么 mock transport？
- Phase 2 如何测试？HTTP handler 的单元测试？SSE endpoint 的集成测试？daemon 进程的端到端测试？
- 回归测试：daemon 化后确保所有 57 个 command 的行为不变？

**建议**：在 Phase 1 的设计中明确 vitest mock 策略（如用 `vi.mock('@/transport', () => ({ transport: mockTransport }))` 替换现有 Tauri mock）。Phase 2 设计与 axum 的测试工具（`axum::test` + `TestRequest`）结合，至少覆盖关键 handler 的单元测试。

### 8.2 数据库迁移策略 — ⚠️ 缺失

调研 §1.3 正确指出了"daemon 独占 DB"的决策，但没有讨论：
- daemon 和 Tauri 使用的是否是**同一个** SQLite 文件？如果是，Tauri GUI 在 daemon 化后还读 DB 吗？
- 如果 Tauri 变 thin client，不再直接读 DB，那么 Tauri 的 `load_session` 等 command 需要通过 HTTP 调用 daemon 的对应 handler，还是直接从 DB 读（本地 IPC 快）？
- 新表 `devices`（Phase 3）的 migration 策略是什么？

**建议**：明确"daemon 独占 DB，Tauri thin client 所有数据操作走 HTTP RPC"的策略。这消除了双进程读同一个 SQLite 的 WAL 锁竞争风险，也让 Tauri 端代码更简单（不需要 SqlitePool）。

### 8.3 CancellationToken 跨进程 — ⚠️ 描摹不够

文档 §1.3 说 `cancellations: Arc<Mutex<HashMap<String, CancellationToken>>>` 必须随 daemon 迁移，这没问题。但跨进程 cancel 的实现细节需要展开：

当前流程：前端 `invoke('cancel_chat', { requestId })` → Tauri command 直接访问 `state.cancellations` 拿到 token 并 cancel。

daemon 化后：前端 `fetch('POST /api/cancel', { request_id })` → HTTP handler → 查找 daemon 的 `cancellations` map → cancel。

这个映射在 HTTP 场景下是直接的。但有一个**边界情况**：如果用户在浏览器关了标签页，cancel 请求发不出去，daemon 里的 agent loop 会继续跑。PS：这里需要 daemon 端的"client 断开检测"——当 SSE 连接的 client 断开时，自动 cancel 该 session 的 active request。

**建议**：Phase 2 的 `HttpSseSink` 在 `EventSource` 断开时触发 daemon 层的清理逻辑（cancel active request + 标记 session 为 idle）。这不是 MVP 必须的，但应该标注为已知边界情况。

### 8.4 `oneshot` 通道的 RPC 化 — ⚠️ 关键实现点被一笔带过

§1.3 提到"跨进程时 oneshot 必须在 daemon 内 resolve,client 只能发'用户答了'消息"。这是正确的方向，但缺一个具体的实现路径。

以 `permission:ask` 为例，当前流程：
1. agent loop 调 `permission_store.request_permission(...)` → 创建 oneshot channel
2. `app.emit("permission:ask", payload)` → 前端弹出 modal
3. 用户点"允许" → 前端 `invoke("permission_response", { rid, decision })` → Tauri command → `permission_store.resolve(rid, decision)` → oneshot sender 发送结果 → agent loop 的 receiver 收到

daemon 化后：
1. agent loop 调 `permission_store.request_permission(...)` → 创建 oneshot channel + 注册到 `rid → sender` map
2. `HttpSseSink.emit("permission:ask")` → SSE 推到浏览器
3. 用户点"允许" → 前端 `fetch('POST /api/permission/respond')` → HTTP handler → `permission_store.resolve(rid, decision)` → oneshot sender 发送结果

步骤 3 中 HTTP handler 如何拿到 `permission_store`？当前 `permission_store` 是 `AppState` 的一个字段，通过 Tauri `State<>` 注入到 command 函数。在 axum 中，需要用 `Extension<Arc<AppState>>` 提取。这个转换是机械的但需要明确写出来。

**建议**：Phase 2 详细设计时给一个"permission round-trip 的跨进程端到端序列图"（从 agent loop 发 ask → 浏览器用户点确认 → agent loop 收到结果），作为所有 4 类 round-trip（permission/question/mode_change/task_state_transition）的参考实现。

### 8.5 `background_shell` 的跨进程通知 — ⚠️ 缺失

§1.3 正确标注了 `BackgroundShellRegistry` 必须在 daemon 进程，但 §1.4 的"daemon 化难度:高"描述需要展开：

当前后台 shell 的完成通知是通过 agent loop 每轮 `drain_notifications` + APPEND user message 实现的。daemon 化后，这个机制不变（agent loop 在 daemon 内，drain_notifications 正常工作）。但：
- 前端如何查询某个后台 shell 的状态？（`shell_status` tool 的返回需要通过 emit 推给前端，但这个 emit 是 SSE 了）
- 前端如何 kill 后台 shell？（`shell_kill` tool 在 agent loop 内执行，不需要额外 IPC）

这两个问题在 daemon 化后实际上**更简单了**——所有 shell 生命周期管理都在 daemon 进程内，不需要跨进程协调。调研 §1.4 标注的"高难度"可能高估了——只要 daemon 独占 `BackgroundShellRegistry`，agent loop + shell registry 之间的交互完全不受进程边界影响。**真正跨进程的是前端查看 shell 状态的需求**（如果用户想在浏览器看到 shell_status，需要 SSE 推送）。但这个场景的优先级不高——shell_status 是给 LLM 看的，用户不需要实时看。

**建议**：调低 `BackgroundShellRegistry` 的 daemon 化难度评估为"中"，并标注 daemon 进程独占后反而简化了生命周期管理。

### 8.6 WSL-specific 的浏览器访问路径 — ⚠️ 缺失

Everlasting 项目是 WSL-first 设计。daemon 化后的核心使用场景是：
- **daemon 跑在 WSL 2 内**（操作 WSL 内的代码）
- **用户在 Windows 宿主打开浏览器**（Chrome/Edge）访问 daemon

这个场景需要：
1. WSL 2 的 localhost 端口转发（默认启用，但需要确认 daemon 端口不被防火墙拦截）
2. 或者用户直接访问 WSL 2 的虚拟 IP（`172.x.x.x:PORT`，不固定）

调研完全没有涉及这个链路。建议在 §1.1 或 §4.3 增加一个"WSL 2 浏览器访问拓扑"小节，明确：
- 推荐方案：daemon 监听 `0.0.0.0:PORT`，Windows 宿主浏览器访问 `http://localhost:PORT`（利用 WSL 2 默认的 localhost 转发）
- 验证方法：Phase 2 的第一步就是 `curl http://localhost:PORT/api/health` 从 Windows 宿主确认连通性
- 降级方案：如果 localhost 转发不可用，使用 WSL 2 IP（`ip addr show eth0`）+ 配置 Windows 防火墙

### 8.7 TypeScript 类型生成工具的选择 — ⚠️ 未展开

调研 §4.3 说"用 serde + TypeScript codegen(`ts-rs` 或 `typeshare`)保证前后端类型一致"。两个工具各有优劣：

| 工具 | 优点 | 缺点 |
|------|------|------|
| `ts-rs` | 成熟，支持复杂类型（enum/union/generic），可与 axum 集成 | 生成的类型偏 Rust 风格（`snake_case`），需手动调整 |
| `typeshare` | 支持多语言（TS/Swift/Kotlin），CLI 友好 | 对复杂 Rust 类型的支持不如 ts-rs |

当前项目 TS interface 已经是 snake_case（与 Rust 一致，这是 §1.5a 的 BACKLOG §5.2 决策），所以 `ts-rs` 的 snake_case 不是问题。建议 Phase 2 选定 `ts-rs` 并验证它能否覆盖 `ChatEvent` 这样的复杂 enum（带 `#[serde(tag = "type")]` 的内部 tagged enum）。

---

## 九、与现有架构的契合度

### ✅ 契合点

| 现有组件 | 调研如何对接 | 评价 |
|---------|------------|------|
| `AppHandleSink` (state.rs:647) | Phase 2 新写 `HttpSseSink` 实现同一 trait | 🟢 天然对齐，agent loop 零改动 |
| `AppCommandError` (error.rs:74) | 已是 JSON-ready，跨进程零成本 | 🟢 幸运的早期设计决策 |
| `AppState` (state.rs:74) | daemon 独占，`load()` 是 daemon main 蓝本 | 🟢 集中式状态管理有利于迁移 |
| `streamController.ts` | 单点 funnel，transport 抽象后是最大受益者 | 🟢 架构巧合，但非常有用 |
| 57 个 `#[tauri::command]` | 机械映射到 HTTP handler | 🟢 虽然数量大但映射规则简单 |
| `BackgroundShellRegistry` | daemon 独占，agent loop 交互不受影响 | 🟢 前面讨论过，跨进程后反而更简单 |

### ⚠️ 摩擦点

| 摩擦 | 影响 | 缓解 |
|------|------|------|
| `pick_project_dir` 依赖 Tauri dialog | 浏览器版需要全新实现 | §8.6 已分析，建议做 UX spike |
| `open_memory_in_editor` spawn 外部进程 | 浏览器版需要降级（提供文件内容，让用户手动打开） | 中——memory 文件通常不大，text 展示即可 |
| `get_home_dir` 依赖 `AppHandle.path()` | daemon 用 `dirs` crate 替代 | 低——需要验证路径一致性 |
| `apply_ui_diff` 裸 `String` 错误 | 协议化时需要改 `Result<ApplyUiDiffResult, AppCommandError>` | 低——调研已识别 |
| `PerMissionStore` 的 oneshot 通道 | 跨进程需要 registry + HTTP handler 解析 | §8.4 已分析 |
| `QuestionStore` 的 oneshot 通道 | 同 permission，跨进程需要相同模式 | 复用 permission 的实现模式 |

---

## 十、建议与行动计划

### 10.1 调研文档本身的修正建议

| # | 位置 | 问题 | 建议 |
|---|------|------|------|
| 1 | §1.2b | "9 个"应该是 10 个事件 | 修正数字为 10 |
| 2 | §1.2a | `command_palette` 与 panel 重叠 | 标注"同函数重复注册"而非"重叠" |
| 3 | §2.1 | Claude Code 也支持 `--serve` 模式 | 补充一句避免误导 |
| 4 | §3.1d | WebSocket 排除理由 | 加"企业代理/NAT 穿透"论证 |
| 5 | §3.4d | `sd_notify` 标注为可选 | 建议升级为"推荐" |
| 6 | §6.2 | Phase 2 工作量估 | 1-2 周 → 2-3 周，或标注为"乐观估计" |

### 10.2 Phase 1 启动前的待确认项

1. **前端 TypeScript import 盘点**：`grep -r "from '@tauri-apps" app/src/ --include="*.ts" -l` 确认 22 个文件的列表是否完整，是否有遗漏的类型导入（如 `@tauri-apps/api/event`）
2. **vitest mock 策略**：确认现有 vitest 如何 mock `invoke` / `listen`，设计 transport mock 的替代方案
3. **`Transport.listen` 签名**：确认是否真的需要 `Promise<() => void>`（vs `() => void`），`EventSource.close()` 也是同步的

### 10.3 Phase 2 启动前需要补充的详细设计

1. **WSL 2 浏览器访问拓扑图**（daemon 在 WSL，浏览器在 Windows 宿主）
2. **SSE session 路由机制**（request_id → session_id → EventSource 映射）
3. **前端 `httpTransport.listen` 的连接模型**（方案 B：全局单 SSE + client 端分发）
4. **`AppState::load` 去 `AppHandle` 的路径一致性验证计划**
5. **daemon 内嵌前端静态文件 serve 的方案**（`tower-http::ServeDir`）
6. **`pick_project_dir` 浏览器降级的 UX spike 计划**
7. **permission round-trip 的跨进程端到端序列图**（含 oneshot → HTTP handler 的解析流程）
8. **测试策略**（axum handler 单元测试 + SSE 集成测试 + 端到端回归测试）

### 10.4 推荐的启动顺序

```
1. 修正调研文档的 6 处笔误/遗漏（见 §10.1，30 分钟）
2. 确认 Phase 1 启动前的 3 个待确认项（见 §10.2，半天）
3. 实施 Phase 1: transport 抽象 + 22 文件机械替换（1-2 天）
   ├─ vitest 全量跑通
   └─ pnpm tauri dev 验证零行为变化
4. 补充 Phase 2 的 8 项详细设计（见 §10.3，1-2 天设计文档）
5. 实施 Phase 2: daemon 拆分 + 本地 HTTP server（2-3 周）
   ├─ Step 1: AppState::load 去 AppHandle（验证路径一致性）
   ├─ Step 2: axum HTTP server + 57 handler 机械映射
   ├─ Step 3: HttpSseSink + SSE endpoint + session 路由
   ├─ Step 4: 前端 httpTransport + 内嵌静态文件 serve
   ├─ Step 5: Tauri GUI 自动管理 daemon 进程
   ├─ Step 6: WSL 2 浏览器访问端到端验证
   └─ Step 7: 全量回归测试
```

### 10.5 一句话总结

> **调研质量高、路径务实、渐变式改造策略正确。Phase 1 零风险立即可做。Phase 2 需要补充 WSL 浏览器访问链路、SSE session 路由、oneshot → HTTP handler 转换三个关键设计细节，工作量估偏乐观——建议按 2-3 周规划。Phase 3 远期设计草稿质量合理但需留好抽象的预留点（Transport.isLocal / SSE 连接断开检测）。整体方向正确，值得推进。**

---

> 审查人：deepseek-v4-pro
> 审查日期：2026-07-20
