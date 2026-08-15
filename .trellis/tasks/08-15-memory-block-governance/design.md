# Design — memory 指令块窗口治理

依据:`research/code-map-20260815.md`(2026-08-15 实测行号,实现前重扫)。

## 1. 问题与不变量

首轮注入的 memory 指令块(~7-8k tok,≈42% context)与 tools[] 同为窗口大头,C7D 治完 tools(6773→3677)后 memory 反超为最大单项。治理必须保住三个现有不变量:

- **I1 cache 断点**:banner 块是 memory 消息唯一 `cache_control: Ephemeral` 断点;system 与 memory 相互独立(RULE-A-005)。
- **I2 session 内前缀稳定**:memory 内容在 session 内固定(mtime fence);本设计不得引入"每 request 随 latest turn 变化"的注入。
- **I3 注入契约**:`build_instructions_blocks` 的 banner-first 块序 + `<primary instructions>`/`<reference>` 包裹语义;`load_for_session` 恒返 4 元素(banner 格式依赖)。

## 2. 方案选型

| 方案 | 思路 | 裁决 |
|------|------|------|
| 0. 手动瘦身文件 | 精简 CLAUDE.md 内容 | ✗ 不治本(文件还会涨),且内容属用户资产,机制任务不动内容 |
| A. **分级注入 + 按需拉取(digest)** | AGENTS.md(`<primary>`)全量;CLAUDE.md(`<reference>`)首轮只注目录摘要,`load_memory_sections` 工具按需取全文节 | ✅ **选定**。与既有 primary/reference 语义分层天然对齐;同构 C7D(模型调用前拉 schema → 模型执行任务前拉规范);摘要机械生成,确定性保 I2 |
| B. 相关性动态裁剪 | 按 session 任务匹配段落命中才注入 | ✗ 相关性信号弱(首条消息不足以判定),裁错直接伤行为;且若随 turn 重算破坏 I2。Phase 2 视 A 的拉取率数据再评估 |
| C. trigger_key 工具前召回 | 复用 autonomous-memory 召回思路 | ✗ 对象混淆:那是经验库;指令文件走 tool 前注入会引入 per-turn 内容变化(破 I2),且召回时机难覆盖"规范类"内容 |
| D. reference 层完全不注入 | 直接不注 CLAUDE.md,只留 AGENTS.md | ✗ 更简单,但 CLAUDE.md 是多数仓库的主文档载体(Claude Code 生态),一刀切丢 interop 价值;digest 保留可发现性 |
| E. 按大小一刀切 digest(不分 source) | 任何 >600 tok 层都 digest | ✗ 会把本仓库 AGENTS.md(~1.3k,Trellis 工作流 always-on 指令)也 digest 掉,风险不必要;primary/reference 的 source 语义就是作者意图声明,按 source 分层更准 |

选 A 的关键论证:tools stub 安全是因为**调用意图由模型自主产生**,参数细节可延后;memory 规范的"目录 → 按需全文"同理——模型知道有什么(目录),动手前拉相关的(全文)。风险(模型不拉导致漏规范)由 AC3 行为验证 + 开关兜底。

## 3. 详细设计

### 3.1 层级规则(injection tier)

| 源 | 包裹 | tier | 首轮注入 |
|----|------|------|---------|
| AGENTS.md(User/Project) | `<primary instructions>` | **Full** | 全文 |
| CLAUDE.md(User/Project) | `<reference>` | **Digest** | 章节目录(见 3.2) |

- CLAUDE.md 是 Claude-Code interop"参考"文件(B5 §3 Q4),天然适合降级;AGENTS.md 是为本产品写的主指令,保持全量(实测 ~1.3k,可承受)。
- 小文件豁免:digest 后体积无收益的层直接全量(阈值:层 tokens ≤ **600** 时 Full;user CLAUDE.md 36B 自然落入,零感知)。
- 已拉取节粘性:session 内加载过的节并入该层注入内容(见 3.4)。

### 3.2 digest 生成(纯结构、确定性)

- **fence-aware 切节状态机**:逐行扫描,`^```(±语言标注)` 翻转 in_fence;仅 `!in_fence` 的 `^#{1,3} ` 行是节边界(code block 内 `# 注释` 不切,见 code-map §4 陷阱)。产出示例节:`## Architecture`、`### 核心数据流`。
- 每节目录行 = `## <title> — <首句摘要>`:摘要取节内首个非空、非 header 行,截断 ≤120 chars;**回退规则**(评审补):若节内无非 fence 候选行(整节就是一个大 code block,如实测 CLAUDE.md 的 Common Commands),取节内首个非空行(允许 fence 内,如 `# 开发` shell 注释行)。无 header 的前置内容(Preamble,如 CLAUDE.md 首两行说明)单列一节。
- 目录块格式(注入在原 body 位置,包裹不变):

```text
<reference>
[CLAUDE.md · digest: 7 sections, ~7k tokens full. Call load_memory_sections to load on demand]
1. Project Overview — Everlasting — 个人 vibe coding 工作台…
2. Common Commands — pnpm dev / build / test…
…
</reference>
```

- 全程无 LLM、无 IO,`MemoryLayer.content` 上做纯函数切片;同输入同输出 → I2 成立。digest 块尾部一行调用指引是模型行为引导(同 C7D stub description 末尾附 `load_tool_schemas(["name"]) first.` 的先例)。

### 3.3 `load_memory_sections` 元工具(同构 C7D)

- def 形状:`load_tool_schemas` 的 memory 版——参数 `{ sections: string[] }`;**层寻址用 banner label 做命名空间**(评审补:两层同名 `CLAUDE.md` 必须可区分):`"Project CLAUDE.md"` / `"Project CLAUDE.md#Architecture"` / `"User CLAUDE.md"` / `["all"]`(全部 digest 层全文 = 本 session opt-back Full);节标题匹配 = 精确 → 唯一大小写不敏感前缀/子串(标题是自然语言,精确复现易错);返回**普通文本**(该层指定节原文),不进 `builtin_tools()`(侧挂 append,同 `load_tool_schemas_def()` 先例)。
- 执行:从 `MemoryCache` 现取层内容(mtime fence 保证新鲜)→ 按节定位 → 返回文本。**执行路由与 C7D 同款:`chat_loop/tools.rs` serial 顶部按名拦截**(基线 :792-828 的 `load_tool_schemas` 分支旁),不是普通工具注册。未知节名 → 错误消息附可用层/节列表(自愈)。
- 权限:read-only、silent-allow(同 `remember` 工具的权限模型先例,见 tool-contract)。
- **不做**把节内容"回注"到合成消息——返回即生效(工具结果天然进入后续上下文);3.4 的粘性只影响**下个 request 起的注入形状**。

### 3.4 粘性 registry 与 cache 影响

- `MemoryDigestRegistry`(新,同 `StubRegistry` 模式):`session_id → HashSet<section_key>`(key = `<banner label>#<节标题>`);**清理点 = `commands/sessions.rs` delete_session 处**(基线 :363 的 `stub_loaded.clear` 旁同款一行);存储挂 `AppState`(经典聊天共用;群聊豁免不受其每次新建 registry 的分叉影响)。
  > **实现期修正(08-15)**:存储未走 AppState,改为进程级 `OnceLock` 单例(`memory/digest.rs::registry()`,对标 `memory/tokens.rs` ENCODER 先例)。理由:AppState 路线要给 `run_chat_loop` 加参数,**72 个调用点**全动,收益为零;清理点仍在 `delete_session_inner`(Tauri + daemon 共用)原计划位置。
- **mtime 变更 × 粘性**(评审补):文件被编辑后,registry 中已加载 key 可能失效(节改名/删除)。规则:组装时按**当前**层内容解析 loaded key,解析不到的静默丢弃(等价该节回到未加载态);不尝试迁移。
- 下个 request 组装 `build_instructions_blocks` 时,已 loaded 的节**并入 digest 层的 body**(digest 目录仍在,节全文**追加在目录之后**——保住目录段前缀,最大化缓存保留)→ 已加载部分持续在场,不依赖模型记忆。
- cache 代价分析:节加载使该层 body 变长 → 该合成消息之后的内容一次性失效(banner 断点之前的 system 段不受影响);之后内容再次固定。即**每次拉取至多一次 prefix miss**,以 `cache_read` 率量化(AC4)。粘性而非每 turn 重算是 I2 的直接体现。

### 3.5 gate 与开关

- 开关 `memory_digest_enabled`(`db::config`,best-effort 缺省 **on**;关 = 精确回到现状注入)。
- gate = 开关 && `!effective_is_worker` && `!is_group_chat`(与 C7D 同款豁免:worker 单轮窄任务不给他探索负担;群聊多 role 注入面大,v1 控爆炸半径,Phase 2 视数据放开)。
- gate 判定位置同 C7D:`chat_loop` 每 request 读一次,传入 `build_instructions_blocks` 的调用侧(init.rs)。

### 3.5a 双注入路径的边界(评审更正)

- memory 合成消息有**两个构造点**:`chat_loop/init.rs`(经典聊天 + 群聊参与者)与 `agent/subagent/prompt.rs:63` `build_worker_messages`(worker 独立,自带断点)。design 初稿"唯一注入点"表述错误。
- v1 边界:digest 与 `load_memory_sections` 拦截**只落在 init.rs 路径**;worker 路径(`build_worker_messages`)一行不动(gate 豁免的落点就是"不改造该函数")。群聊参与者走 init.rs 但被 gate 短路,同样不动。
- WP1 度量口径随之收窄:**memory_token 只在 init.rs 路径计算落库**;worker 的 turn_trace 行该列留 null(同 pre-column 语义)。这与 C7D worker 豁免对称,避免 v1 度量面扩到第二条路径。

### 3.6 度量(WP1,先行且独立)

- `turn_trace` 加 `memory_token INTEGER`(幂等 backfill,复用 `add_turn_trace_column_if_missing`);`upsert_turn_trace_token` 扩参 `memory_token`;`TurnTraceRow` + list 查询同步。
- 计算点:init.rs 注入处对**实际注入的 blocks**(banner+wrappers+digest/full body)整体 `count_tokens`,存入 `LoopInit`,drive.rs Done 事件与 tools_token 同点 upsert。口径:memory_token 是注入内容的 cl100k 估算;占比 = memory_token / context_input,**不 double-count**(context_input 已含 memory,同 C7 TurnCard 注释);worker 行 null(§3.5a)。
- 前端:`turnTrace.ts` 加 `memoryToken`;`TurnCard.vue` 加 memory cell(复用 toolsPct computed 模式)。
- `turn-smoke.sh`:输出加 memory_token / 占比列——WP1 上线即拿全量基线,WP2 验收有数。**脚本需扩 `--turns N`**(现仅单轮单 POST;AC4 的"第二轮 cache 率"依赖同一 session 发第二条消息)。

## 4. 兼容性

- `read_memory_*` IPC / RuntimeMemoryModal 前端读的是**文件层**(MemoryLayerInfo),digest 不改文件读取路径,零影响;banner 仍显示全文件 token 数(它是"文件清单"语义)。
- autonomous-memory(Scenario 2)完全不动;`load_for_session` 签名不变(仍返 4 层),tier 化只发生在 `build_instructions_blocks` 内部 + 其调用侧。
- 多 provider:digest 是内容变换,与 provider 无关;cl100k 为近似口径(同 C7 声明)。
- group chat / worker:v1 豁免 → 行为与现状逐字节一致。

## 5. 回滚

开关 `memory_digest_enabled=off` 即回到现状注入(gate 短路);DB 新列可留(无害,同 tools_token 对旧行为 null)。极端回滚 revert PR2 不影响 PR1 度量。

## 6. 风险与缓解

| 风险 | 缓解 |
|------|------|
| 模型不主动拉全文 → 漏规范 | 目录每节带首句摘要(知道有什么);digest 块尾调用指引;AC3 真机行为验证;开关兜底 |
| **双注入路径改漏/改串**(init.rs 与 subagent/prompt.rs) | §3.5a 明确边界:prompt.rs 一行不动;grep `build_instructions_blocks` 断言调用点只剩 init.rs 在新路径上;AC5 对两路径分别断言逐字节一致 |
| 摘要质量差(首句非概要) | 目录价值主要在标题 + 调用指引;Phase 2 可给节加约定式 summary frontmatter(文件格式演进,另行) |
| fence 切节 bug | 状态机单测覆盖 fenced `#` 注释、嵌套 fence 边角 + 实测 repo CLAUDE.md 快照断言(整节即一个大 code block 的回退规则,§3.2) |
| 拉取频繁 → cache miss 增 | 粘性注入(拉一次后在场,追加在目录后保前缀);AC4 以 cache_read 率验证 |
| 节标题自然语言,模型寻址易错 | §3.3 匹配 = 精确 → 唯一前缀/子串回退;错误消息附可用清单自愈 |
