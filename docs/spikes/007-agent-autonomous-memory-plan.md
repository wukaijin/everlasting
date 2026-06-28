# spike-007: Agent 自主记忆系统 — 设计计划

**日期**: 2026-06-29
**状态**: 设计已收敛(4 轮需求探讨定稿),待落 spec / 切任务
**依赖**: 复用现有 memory 模块(`build_instructions_blocks`)、permissions Tier1 拦截链、`db::persist_turn`、`emit_tool_result` 信号源
**性质**: 本文档是**设计计划**(非技术验证 spike),记录从"指令文件加载"升级到"agent 自主记忆"的完整方案。与 spike-005 / spike-006 同主题但独立成文,文末有对那两份的吸收对比。

## 1. 背景与定位

现状的 `memory/` 模块(loader.rs / file.rs / watcher.rs / tokens.rs)实质是**开发者手写的指令文件加载**:4 个固定文件(User/Project × CLAUDE.md/AGENTS.md)→ `load_for_session()` → `build_instructions_blocks()` → 带上 `cache_control: Ephemeral` 的 synthetic user message → 每 session 全量注入。它是**静态 config**,不是记忆。

目标:agent 能**自主产生、跨 session 召回**的经验知识。典型场景——agent 多次因路径问题调用测试命令失败、最终成功后记住这个坑,在另一个 session 遇到同类操作时**第一时间想到规避**。

**本质跃迁**:不是"加几张表",而是**谁产生内容 + 什么时刻进入 context** 都变了——从"开发者写、全量吃"到"agent 写、按需召回"。

## 2. 核心矛盾与设计原则

### 核心矛盾(决定整个设计的生死线)

选定的组合是 **"agent 全自主写 + 背景召回强制注入"**。这两者合起来会**放大噪音**:

> 全自主写天然倾向"该记就记" → 记忆库快速膨胀 + 碎片化;而背景召回是**强制把记忆塞进每个 session 的 prompt**。噪音不会安静躺在库里,而是被自动分发到每一个 session。

这个矛盾决定了:**写入端必须装质量漏斗(状态机),召回端必须挑对时机 + 精确率优先**。这两件事是整个设计的两个抓手,存储反而是最不重要的环节。

### 设计原则

1. **写入永远即时、低门槛**——不破坏全自主;质量靠"晋升/淘汰"漏斗,不靠"写入审批"。
2. **召回精确率 > 召回率**——漏一条能用主动 recall 补,注入一条错的会污染整个回答。
3. **记忆是"经验"不是"规则"**——注入措辞降格为提示,矛盾记忆共存明示,让 agent 当下裁决。记忆库定位是经验库,非规则库。
4. **最大化复用现有管线**——注入复用 instruction blocks,工具执行前召回复用 permissions Tier1,事件监听复用 `emit_tool_result`,存储复用 db migrations 范式。

## 3. 写入设计

### 两条写入路径

| 路径 | 触发 | 初始 status | 理由 |
|---|---|---|---|
| **路径 1 · 主 agent `remember` tool**(含用户显式"记住这个") | agent 自主 / 用户显式 | `candidate` | agent 自主写的可能琐碎,要过漏斗证明价值 |
| **路径 2 · 旁路事件 reflection** | 连续 ≥2 次同名工具失败后成功 | `active` | 事件本身就是强置信信号,直接生效——正是路径坑场景 |

### 记忆状态机(质量漏斗)

```
   写入
    │
    ▼
 candidate ──(被主动 recall 命中 / 用户在 UI 看到未删 / 复核通过)──► active
                                                              │
                            (多次命中 + 相关 session 未在同一处翻车)│
                                                              ▼
                                                           verified
    │ (长期 last_used 老化 / 被新记忆覆盖 / 用户删)
    ▼
 demoted
```

**晋升和淘汰靠"使用反馈"自动完成,不靠用户手动把关**——这保住了全自主精神,又给了质量漏斗。`hit_count` + `last_used_at` 是状态机的事实依据。

### pitfall 强制 `trigger_key`(结构化)

pitfall 类记忆写入时**必须**带结构化触发键,例如:

```json
{ "tool": "shell", "command_pattern": "cargo test", "path_globs": ["app/src-tauri/*"] }
```

这让"工具执行前召回"从全文检索变成 `tool_name + tool_input` 精确匹配,几乎不误注。代价是写入端(remember tool + 旁路 reflection prompt)要多吐一个结构化字段——这个负担值得,它是"精确率优先"的工程兑现。

### 异步卫生 job(不阻塞主 loop)

后台定期对记忆库做:
- **dedup 合并**:相似记忆合并(同 trigger_key + 高文本相似度)
- **降权**:低 `hit_count` + 老 → `demoted`
- **冲突标记**:互相矛盾的两条记忆 → 标记冲突,注入时明示

写入端永远即时低门槛,库健康在背景维持。

### 写入安全网(防灌水 + 防泄漏)— 吸收自 spike-005

写入端虽然"全自主、不审批",但入库前必须有硬性安全网:

- **敏感信息过滤**:匹配正则 `(?i)(api[_-]?key|secret|password|token=|bearer)` → 拒绝并 warn,不入库。
- **路径泛化**:`/home/<user>/...` 等绝对路径 → 泛化为 `~`,避免本地用户名/隐私泄漏。
- **长度上限**:content ≤ 500 字符,超出拒绝,强制压缩成"一句可复用的坑描述"。
- **频率控制**:同 turn remember ≤ 3 次、同 session ≤ 50 条,超出按 hit_count 淘汰末尾。防灌水。
- **溯源字段**:`source_session` 细化为 `source_ref`(精确到 turn_id / tool_call_id),便于 debug / 回滚。

> 这套安全网替代了 spike-006 主张的"Tier 4 ask 用户确认写入"——本项目已定档 agent 全自主写,故用规则安全网 + 状态机漏斗替代人工审批。

## 4. 召回设计

两层召回,两套检索机制,各吃各的场景:

### 层 1 · session 开始背景召回

- **落点**:`chat_loop.rs:537` `build_instructions_blocks` 调用处(checklist injection 之后、provider.send 之前)
- **检索对象**:`status IN (active, verified)` 且 `kind IN (preference, fact)` 的 top-k
- **检索方式**:FTS5 全文检索(title + content + tags,bm25 排序)+ `scope`/`project_id` 过滤 — 吸收自 spike-006。FTS5 是 SQLite 内置、零额外依赖,比 LIKE 强、比向量简单,正好填补 v1"不上向量库"的中间档。(实现时需确认 sqlx/SQLite 启用 FTS5 feature)
- **注入方式**:转成 ephemeral ContentBlock,**追加进同一个 synthetic user message**,和指令文件共享 `cache_control: Ephemeral` 断点;token 硬上限 ≤ 500(吸收 spike-006)
- **缓存稳定性坑**:top-k 会因 hit_count 漂移而击穿 prompt cache → 同分按 `created_at` 固定排序,或自主记忆段单独打第二个 cache 断点(v1 倾向前者)

### 层 2 · 工具执行前实时召回(本系统的灵魂)

- **落点**:`permissions/check.rs:51` Tier 1 Hooks(当前 no-op)——**复用现有工具执行前拦截链,不另起炉灶**
- **检索对象**:`kind = pitfall` 且 `status IN (active, verified)`
- **检索方式**:用当前 `tool_name + tool_input` **精确匹配 `trigger_key`**(O(1),非向量、非 LIKE)
- **命中后分档处置**(本轮定档):

| status | 匹配强度 | 处置 | 效果 |
|---|---|---|---|
| `verified` | trigger_key 完全命中 | **软拦截重判**:回灌 pitfall 让 LLM 多想一轮,有机会调整命令再决定 | 真正"第一时间规避" |
| `active` / 弱匹配 | 部分命中 | **不阻断 + 注脚**:照常执行,把 pitfall 作为 tool_result 前置注脚回填 | 零 loop 改动,"下次注意"兜底 |

> 软拦截是整个设计里唯一"重"的地方(动 loop 结构、引入 hint round 延迟),只对 `verified` 开;`active` 先注脚兜底。

## 5. 存储 schema

走 DB,不走文件(现有 memory 模块是 mtime fence 读时检查,非 notify watcher → DB 省掉 watcher,且能关联 `source_ref` 回溯)。照搬 `db/subagent_runs.rs` 范式,`migrations.rs:55` 加表:

```sql
CREATE TABLE IF NOT EXISTS autonomous_memories (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,  -- 内部自增,FTS5 content_rowid 需要
    memory_id       TEXT NOT NULL UNIQUE,               -- UUID,对外引用
    scope           TEXT NOT NULL,              -- user | project | global
    project_id      TEXT,                       -- scope=project 时必填,FK projects.id
    kind            TEXT NOT NULL,              -- pitfall | preference | fact | decision
    status          TEXT NOT NULL,              -- candidate | active | verified | demoted
    title           TEXT NOT NULL,              -- 检索 + 展示用,写入时强制
    content         TEXT NOT NULL,              -- 正文(经验式措辞,非规则)
    tags            TEXT NOT NULL DEFAULT '[]', -- JSON array,session 开始召回的关键词
    trigger_key     TEXT,                       -- pitfall 专用:JSON 结构化触发键
    source_ref      TEXT,                       -- 溯源:turn_id / tool_call_id(吸收 spike-005)
    confidence      REAL NOT NULL DEFAULT 0.5,
    hit_count       INTEGER NOT NULL DEFAULT 0, -- 状态机晋升依据
    last_used_at    TEXT,
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL,
    demoted_reason  TEXT
);

-- FTS5 全文检索(session 开始召回用) — 吸收 spike-006
CREATE VIRTUAL TABLE IF NOT EXISTS autonomous_memories_fts USING fts5(
    title, content, tags,
    content='autonomous_memories', content_rowid='id'
);
-- 触发器:INSERT/UPDATE/DELETE 同步 FTS(实现时补)

CREATE INDEX IF NOT EXISTS idx_am_recall  ON autonomous_memories(scope, project_id, status, kind);
CREATE INDEX IF NOT EXISTS idx_am_trigger ON autonomous_memories(kind, status) WHERE trigger_key IS NOT NULL;
```

> id 自增 + memory_id UUID 分离:FTS5 的 `content_rowid` 必须是整数,故 UUID 对外、自增 id 对内(吸收 spike-006)。两套检索各管一类——pitfall 工具执行前召回走 `trigger_key` 精确匹配(不用 FTS5);preference/fact session 开始召回走 FTS5(不用 trigger_key)。

## 6. Hook 落点(精确到代码位置)

| 接入点 | 位置 | 复用的现有管线 |
|---|---|---|
| A · session 开始召回 | `chat_loop.rs:537`(`build_instructions_blocks` 调用处) | synthetic user message + cache_control 断点 |
| B · 工具执行前召回 | `permissions/check.rs:51` Tier 1 Hooks | 现有 5-tier 工具执行前拦截链 |
| C · 连续失败→成功监听 | `chat_loop.rs:1717`(`emit_tool_result` 处) | `ToolResultPayload`(含 is_error)信号源 |
| D · turn 结束事件检测 | `chat_loop.rs:1406`(`persist_turn` 处) | v1 预留,只做 C |
| E · DB 加表 | `migrations.rs:55` | projects / subagent_runs 建表范式 |

> 行号来自 spike 探查,实现时以实际为准(允许漂移)。

## 7. 数据流

```
chat_loop.rs:537  build_instructions_blocks
   │  ◄─ A: session 开始召回(preference/fact) → 追加进同一 synthetic msg
   ▼
for turn ──────────────────────────────────────────────────
   │ provider.send → ToolCall
   │
chat_loop.rs:1663  permissions::check
   │  ◄─ B: Tier1 工具执行前召回(pitfall,trigger_key 精确匹配)
   │       ├─ verified + 强命中 → 软拦截重判
   │       └─ active / 弱命中   → 放行(注脚)
   │ execute_tool
   │
chat_loop.rs:1717  emit_tool_result
   │  └─► C: 旁路状态机(连续失败→成功) ──► 旁路 reflection ──► 写 DB(active)
   │
chat_loop.rs:1406  persist_turn
   │  ◄─ D: 预留(session 级 reflection 扩展位)
   ▼
                                                          ┌─ 异步卫生 job: dedup / 降权 / 冲突标记
                                                          └─ 状态机: candidate→active→verified(靠 hit_count)
```

## 8. v1 边界(明确不做)

- ❌ 向量库 / embedding 检索(tag + trigger_key 精确匹配够 v1)
- ❌ LLM-judge 写入过滤(先用规则 + 状态机 + 用户 UI 把关)
- ❌ session 结束整体 reflection(C 点事件驱动已够贴场景)
- ❌ 全局(global)记忆层(user / project 两级先跑通)
- ❌ `recall_memory` 主动深挖 tool(留扩展位,v1 只做被动背景召回)

## 9. 落地切片(按价值可见性 + 依赖排,每步可独立合入)

| 步 | 内容 | 打通的是 |
|---|---|---|
| 1 | DB 表 + migrations + CRUD + Rust 结构体 | 存储底座,无 LLM |
| 2 | session 开始背景召回(A 点) | **召回骨架**:手写一条记忆→新 session 看到注入 |
| 3 | `remember` tool + 显式写入 + UI 查看 | **写入侧**闭环 |
| 4 | 工具执行前召回(B 点)+ 注脚档 | pitfall 召回,active 先兜底 |
| 5 | 事件驱动旁路 reflection(C 点) | **自动闭环**:失败→成功自动产出 pitfall |
| 6 | verified 软拦截 + 状态机晋升 + 卫生 job | 质量层,兑现"第一时间规避" |

> 步 2 单独就有可见效果(可手写记忆测召回),不用等闭环。

## 10. 待决问题 / 扩展位

- **软拦截 hint round 的 loop 改法**:verified 命中后,如何干净地把"提示 + 原 tool_use"变成让 LLM 重判的回合,而不破坏现有 turn 结构 / cancel 语义。步 6 实现时最需设计的细节。
- **卫生 job 去重算法**:吸收 spike-005 的具体阈值——content 做 Jaccard 相似度 > 0.7 视为重复,**hit_count 累加 + last_used_at 更新**而非新增;完全相同直接累加。合并时高 hit_count / 高 confidence 胜出。
- **前端集成落点**(吸收 spike-006):`stores/memory.ts` 加 `fetchMemories` / `deleteMemory` actions;`components/memory/MemoryPreview.vue` 扩展 runtime memories 列表 + 删除/pin;Tauri commands `list_memories` / `delete_memory`(`commands/memory.rs`)。复用现有 memory 组件脚手架。
- **召回 token 预算**:session 开始召回 ≤ 500 tokens(吸收 spike-006);条数 top-k 上限按 kind 分配(preference/fact 各占份额)。
- **system prompt"何时用"段**(吸收 spike-005):`build_instructions_blocks` 追加常量段教 LLM 何时 remember——"工具失败 ≥ 2 次同类"、"用户纠正"、"发现非显然项目约定";并明示"不记 API key / 隐私 / 临时路径"。这段对 remember tool 调用率关键。
- **扩展位**:`recall_memory` 主动深挖 tool、global scope、session 结束 reflection、向量检索——都留给 v2。

## 11. 与 spike-005 / spike-006 的吸收对比

三份同主题文档(spike-005 MiniMax-M3 / spike-006 deepseek-v4-pro / 本 007),本节记录从另两份吸收了什么、分歧在哪。

### 已吸收(并入上方设计)

| 吸收项 | 来源 | 并入位置 |
|---|---|---|
| 写入安全网(敏感过滤 / 路径泛化 / 长度上限 / 频率控制 / source_ref 溯源) | spike-005 §4 | §3 写入安全网 |
| FTS5 全文检索(session 开始召回,零依赖,bm25) | spike-006 §4.1/4.3 | §4 层 1 + §5 schema |
| memory_id UUID + 自增 id 分离(FTS5 rowid 需要) | spike-006 §4.1 | §5 schema |
| Jaccard > 0.7 去重合并阈值 | spike-005 §4.2 | §10 待决 |
| 召回 token ≤ 500 + 注入位置(checklist 后 / send 前) | spike-006 §4.3 | §4 层 1 + §10 |
| 前端集成落点(stores / MemoryPreview / IPC commands) | spike-006 §5 | §10 |
| system prompt"何时 remember"措辞 | spike-005 §3 | §10 待决 |

### 分歧(未吸收,记录理由)

| 分歧点 | spike-006 主张 | 本 007 立场 | 理由 |
|---|---|---|---|
| 写入权限 | Tier 4 ask(用户确认,同 web_fetch) | 全自主写 + 安全网 + 状态机 | 用户已定档全自主;人工审批破坏"自动规避"体验,用规则安全网替代 |
| 召回主力 | 每 turn FTS5 基于 last_user_message 注入 | session 开始 FTS5 + **工具执行前 trigger_key 精确召回** | 用户场景的坑触发在"跑命令那一刻",非用户消息;trigger_key 精确匹配比 FTS5 模糊召回更适合 pitfall |
| 主动性模型 | 纯 LLM tool(save/search) | 背景被动召回为主,remember tool 为辅 | 解决"未知的未知"——纯主动 recall 救不了"agent 不知道自己踩过坑" |

### 本 007 独有(另两份未覆盖)

- **工具执行前实时召回 + trigger_key 精确匹配**:真正对齐"第一时间规避"场景(另两份都只到 session/turn 级注入,没到"工具执行那一刻")。
- **记忆状态机 candidate→active→verified + 写入路径分化**(事件驱动直接 active / 自主写 candidate):系统化质量漏斗,替代 005 的频率控制 + 006 的"不做去重"。
- **软拦截分档**(verified 重判 / active 注脚):兑现"规避"vs"提醒"的两档体验。
- **"经验非规则"心智模型**:注入措辞降格、矛盾共存明示,应对全自主写入必然的错误归纳。

> 三份文档可视为互补:005 给了扎实的写入安全网细节,006 给了 FTS5 + 前端落点,007 给了召回时机 + 质量漏斗 + 心智模型。合并后是当前最完整方案。

## 关联文档

- [ARCHITECTURE — Agent Loop](./../ARCHITECTURE.md)(16 阶段请求生命周期)
- [STRUCTURE.md — memory/ permissions/ db/ 模块](./../STRUCTURE.md)
- spike-005 `agent-memory.md`、spike-006 `agent-autonomous-memory.md`(同主题另两份,见文末吸收对比)
- 实现时引用:`app/src-tauri/src/memory/loader.rs:537`、`agent/permissions/check.rs:51`、`agent/chat_loop.rs:1717`
