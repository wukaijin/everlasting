# spike-005: agent 自主跨 session 记忆能力(从指令文件到长期记忆)

**日期**: 2026-06-29
**状态**: 待评审(设计已对齐,未实施)
**作者**: MiniMax-M3
**依赖**:
- 无(本 spike 是新功能调研)
- 关联 `docs/spikes/2026-06-19-async-parallel-tool-research.md`(L3 subagent + worktree 调研,在调研中已识别"长期记忆"作为 L3 之外的独立命题)

**预估耗时**:
- 调研 + 文档:已完成(本文档)
- 实施预估:5-8 小时(Rust 端 + db migration + system prompt 注入 + 单元测试)
- 后置验证 spike:1-2 小时手动 LLM 跑通

---

## 目标

把当前"加载指令文件"的 memory 模块,扩展为 agent 自主产生、可被 LLM 在决策时主动唤起的**跨 session 记忆能力**。

### 设想场景

> agent 多次因路径问题调用测试命令失败,最终成功后记住这个坑;在另一个 session 遇到时,能第一时间想到如何规避。

### 现状盘点(2026-06-29)

当前 `memory/` 模块(`memory/loader.rs` / `watcher.rs` / `build_instructions_blocks()`)只做**指令文件加载**:4 个 CLAUDE.md / AGENTS.md 文件读取 → 带 `cache_control: ephemeral` 注入 system prompt。

| 轴 | 实现 | 性质 |
|---|---|---|
| **指令文件**(CLAUDE.md / AGENTS.md) | `memory/loader.rs` + `watcher.rs` + `build_instructions_blocks()` | **只读 + 注入 prompt** |
| **B12 checklist** | `tools/update_checklet.rs` + `stores/checklist.ts` | **session 内可变**,不跨 session |
| **审计日志** | `permissions/audit.rs` + `stores/audit.ts` | 跨 session 可查,**append-only 事件流**,非结构化记忆 |
| **skill 系统** | `skill/` | 手工维护的预置知识 |

**真正缺**:跨 session、agent 自主产生、结构化、可被 LLM 决策时主动唤起的记忆。

---

## 通过标准(MVP 上线判定)

### 硬通过(全部满足 → MVP ship)
- [ ] `remember` / `recall` 两个 builtin tool 注册到 `tools/mod.rs`,LLM 可正常调用
- [ ] SQLite `memories` 表 migration 落地,CRUD + 去重 + 敏感过滤跑通 Rust 单元测试
- [ ] `build_instructions_blocks()` 增加"何时 recall / remember"提示段
- [ ] 手动 spike 验证:主 agent 在**连续 5 个真实场景**中至少 4 次主动调用 recall(成功召回 ≥ 1 条)
- [ ] 手动 spike 验证:主 agent 在**连续 5 个真实场景**中至少 4 次主动调用 remember,且写入内容 review 后质量合格(一句可复用的坑描述)
- [ ] 关键词 + tag 检索在 100 条记忆内 top-5 召回率 ≥ 60%(LLM 二次筛选后)

### 可接受瑕疵(不阻塞,留打磨)
- ⚠️ LLM 偶尔不调用 recall(用 prompt 强化可缓解,不阻塞 MVP)
- ⚠️ tag 由 LLM 自填,质量参差(关键词 LIKE 兜底)
- ⚠️ 同义不同词的关键词检索漏召回(LLM 重试可救)

### 硬失败(任一 → 走"失败 → 回退方案")
- ❌ LLM 完全不调用 recall(tool description / prompt 强化都无效)
- ❌ remember 写入大量垃圾内容(频率控制 / 过滤失效)
- ❌ 关键词召回 100 条内 top-5 命中率 < 30%(LLM 二次筛选也救不回来)
- ❌ 跨项目污染(强 project_id WHERE 过滤被绕过)
- ❌ 写入敏感信息(API key / 隐私泄漏到 SQLite)

---

## 调研过程(已完成)

### 4 个核心决策(已与 Carlos-home 对齐)

| 决策点 | 选择 | 理由 |
|---|---|---|
| **范围** | project-level(强 project_id 隔离) | 跨项目污染问题天然规避;user-level 是 V2 增量 |
| **MVP 主动性** | agent 工具调用(LLM 主动) | 不挂隐式 hook,完全靠 prompt + tool description;可控可调试 |
| **检索方式** | 关键词 + tag,**不上向量检索** | MVP 简化;V2 看召回率决定 |
| **写入审核** | 无需用户 confirm 才落库 | 写入路径做"防污染"机制替代 UI confirm;LLM 自主决策不打断 |

### 已讨论并确认的设计点

1. **存储**:SQLite 主存,`memories` 表;不导出文件(动态记忆不需人类 review)
2. **工具接口**:
   - `remember(content, tags, source_kind)` — LLM 自填总结 + tags + 动机
   - `recall(query, tags?, limit=5)` — 自然语言 + 可选 tag 过滤
3. **System prompt 注入**:`build_instructions_blocks()` 加常量段教 LLM 何时用
4. **写入安全网**:敏感信息正则过滤 + 路径泛化 + 长度上限(500 字符)
5. **去重**:Jaccard 相似度 > 0.7 视为重复,use_count++ 而非新增
6. **频率控制**:同 turn ≤ 3 次,同 session ≤ 50 条(超出按 use_count 淘汰)

### 已识别的关键风险与缓解

| 风险 | 缓解 |
|---|---|
| LLM 不调用 recall | tool description 措辞强化 + prompt 明示"先查" |
| LLM 滥用 remember 灌水 | 频率控制 + 长度上限 + tag 强制 |
| 写入敏感信息 | 入库前正则过滤 + 路径泛化 |
| 跨项目误用 | 强 project_id WHERE 过滤,LLM 无 API 跨项目 |
| 关键词召回不准 | tag 字段强制 + LLM 二次筛选;V2 看真实召回率 |

---

## 具体方案

### 1. SQLite 表结构

放在 `db/` 模块,新建 `memories.rs`(子模块,与 `permissions.rs` / `subagent_runs.rs` 同级)。

```sql
CREATE TABLE memories (
  id            INTEGER PRIMARY KEY AUTOINCREMENT,
  project_id    INTEGER NOT NULL,
  content       TEXT    NOT NULL,
  tags          TEXT    NOT NULL DEFAULT '[]',  -- JSON 数组
  source_kind   TEXT    NOT NULL,                -- 'tool_error' | 'user_correction' | 'observation' | 'manual'
  source_ref    TEXT,                            -- 溯源:turn_id / session_id / tool_call_id
  confidence    REAL    NOT NULL DEFAULT 1.0,
  use_count     INTEGER NOT NULL DEFAULT 0,
  last_used_at  TEXT,                            -- ISO8601,衰减用
  created_at    TEXT    NOT NULL,
  FOREIGN KEY (project_id) REFERENCES projects(id) ON DELETE CASCADE
);

CREATE INDEX idx_memories_project ON memories(project_id, created_at DESC);
CREATE INDEX idx_memories_tags    ON memories(project_id, tags);
```

**关键设计点**:
- **强 project_id 隔离**:recall 时强制 `WHERE project_id = ?`,LLM 无法跨项目泄漏
- **`source_ref` 溯源**:每一笔反查"谁记的、什么时候记的",便于 debug / 回滚
- **scope 不加**:user-level V2 再说

### 2. 工具接口

注册到 `tools/mod.rs`,与 `use_skill` / `update_checklist` 同级。

#### 2.1 `remember` — 写入记忆

```rust
// tools/remember.rs
{
  "content": "在 WSL 下跑 cargo test 必须设 PKG_CONFIG_PATH,否则撞 gdk-pixbuf not found",
  "tags": ["wsl", "cargo", "build"],
  "source_kind": "tool_error"
}
```

**设计点**:
- `content` 是 LLM 自己写的**总结**(不是原始 tool 输出)—— LLM 必须把"3 行错误日志"压缩成"一句可复用的坑描述"
- `tags` 由 LLM 自填 —— 关键词检索的精确锚点(无向量检索时,tag 是召回关键)
- `source_kind` 让 LLM 标注动机 —— 便于 UI 区分"失败兜底记" vs "LLM 主动记"

#### 2.2 `recall` — 检索记忆

```rust
// tools/recall.rs
{
  "query": "WSL 下 cargo test 失败",
  "tags": ["wsl", "cargo"],
  "limit": 5
}
```

**返回**:
```
[
  {"content": "...", "tags": [...], "use_count": 3, "created_at": "..."},
  ...
]
```

**检索策略**(MVP):
- `WHERE project_id = ?` 强过滤
- `WHERE content LIKE '%query%' OR tags LIKE '%tag%'` 关键词匹配
- 排序:`use_count DESC, created_at DESC`
- **不做语义排序** —— LLM 自己从 top-N 里挑

### 3. System prompt 注入片段

在 `build_instructions_blocks()` 增加常量块(**不进 cache**,因为每次 turn 可能变):

```
你有长期记忆能力,可以调用 remember/recall 工具。

何时 recall:
- 遇到错误、不熟悉的工具、陌生的项目结构时,先 recall 看看历史有没有相关坑
- 用户说"之前怎么做"、"老办法"时

何时 remember:
- 工具调用失败 ≥ 2 次同类型,说明这是反复出现的坑,记下来
- 用户明确纠正了你的做法("不要这样,应该是...")
- 你发现了非显然的项目约定(构建命令、路径别名、环境变量等)

记什么:
- 一句话能说清的"坑"或"约定",不要记大段日志
- 标好 tags,便于下次召回
- 不要记:API key、用户隐私、临时路径、当前任务细节
```

**tool description 措辞强化**:`recall` 的 description 要比 `remember` 更"显眼"(LLM 默认不爱查):

> "**先查再行动**——遇到任何错误或陌生操作前,先 recall 项目级历史记忆。"

### 4. 写入端"安全网"

虽然不需要用户 confirm,但写入路径必须做:

#### 4.1 敏感信息过滤

`remember` 入库前过一道:
- 匹配正则 `(?i)(api[_-]?key|secret|password|token=|bearer)` → 拒绝并 warn
- 匹配绝对路径 `/home/<user>/...` 超过 1 个 → 自动泛化为 `~`
- 超过 500 字符 → 拒绝(强制 LLM 压缩)

#### 4.2 去重 / 合并

- 入库前用 `content` 做 Jaccard 相似度(简易字符串分词) > 0.7 → 视为重复,**use_count++ + last_used_at 更新**,不新增
- 完全相同 → 直接 use_count++

#### 4.3 频率控制

- 同一 session 同一 turn 内,`remember` 调用超过 3 次 → 第 4 次起拒绝,提示"够了,别刷"
- 同一 session 总记忆数上限 50 → 超出按 use_count 淘汰末尾

### 5. 与现有系统的集成点

| 集成点 | 改什么 | 文件 |
|---|---|---|
| `tools/mod.rs` | 注册 `remember` / `recall` 两个 builtin | `tools/mod.rs` |
| `memory/loader.rs` | `build_instructions_blocks()` 加 memory 提示段 | `memory/loader.rs` |
| `db/mod.rs` + `migrations.rs` | 新 migration 加 memories 表 | `db/` |
| `db/memories.rs` | 新子模块,CRUD + 去重 + 过滤 | `db/memories.rs` |
| `stores/`(前端) | 暂不动,前端 MVP 不暴露 | — |

---

## MVP 范围划线

### 做
- ✅ `remember` / `recall` 两个工具
- ✅ SQLite 表 + CRUD + 去重 + 敏感过滤
- ✅ system prompt 注入"何时用"
- ✅ 关键词 + tag 检索
- ✅ Rust 单元测试(remember 过滤、recall project 隔离、去重)

### 不做(明确延后到 V2)
- ❌ user-level scope
- ❌ 向量检索(看召回质量决定)
- ❌ UI 记忆管理页(先让 agent 自治)
- ❌ 记忆衰减 / 过期(先全量保留)
- ❌ 记忆晋升为指令文件(自动 + 人工都先不做)
- ❌ 失败兜底 hook(MVP 仅 LLM 主动;V2 看 remember 调用率决定要不要兜底)

---

## 执行步骤(评审通过后实施)

### Step 1:db 落地(预估 1.5 小时)

```bash
cd app/src-tauri
# 新建 db/memories.rs,实现 CRUD + 去重 + 敏感过滤
# db/migrations.rs 加 migration v{N+1}: memeries 表 + 索引
# db/mod.rs 导出 memories 模块
```

**单元测试**(`db/tests_memories.rs`):
- 插入 / 查询 / 更新 use_count / 删除
- 跨 project_id 隔离(用 test_pool 复制 6 份的现有模式,见 `db/tests.rs` 拆分后的 6 个 `*_tests.rs`)
- Jaccard 去重(content 相似度 > 0.7 视为同一条)
- 敏感信息正则拒绝(API key / token)

### Step 2:工具实现(预估 2 小时)

```bash
# 新建 tools/remember.rs / tools/recall.rs
# tools/mod.rs 注册到 builtin_tools()
# 写入路径:db::memories::insert + 过滤 + 频率控制
# 读取路径:db::memories::search(project_id, query, tags, limit)
```

**单元测试**(`tools/tests_remember.rs` / `tests_recall.rs`):
- remember 长度上限拒绝
- remember 频率上限拒绝(同一 session 第 4 次)
- recall 强 project_id 过滤
- recall 关键词 + tag 组合命中

### Step 3:prompt 注入(预估 30 分钟)

```rust
// memory/loader.rs::build_instructions_blocks()
// 在 4 个指令文件块后追加 memory 提示段(不进 cache_control)
```

### Step 4:质量验证(预估 1 小时)

```bash
# 后端
cd app/src-tauri && PKG_CONFIG_PATH=... cargo test --lib
cd app/src-tauri && PKG_CONFIG_PATH=... cargo check

# 前端 type-check
cd app && pnpm build
```

### Step 5:手动 spike(后置,1-2 小时)

5 个真实场景手动跑通(挑项目里典型的踩坑场景):
1. cargo test 在 WSL 下的路径问题
2. 某个 .env 变量命名约定
3. 某个项目特有的命令别名
4. 某个反复出现的 import 路径错误
5. 某个"用户上次纠正过我"的约定

**判定**:
- recall 命中率 ≥ 4/5
- remember 写入 review 合格率 ≥ 4/5(一句可复用 + tags 合理)

---

## 失败 → 回退方案

| 现象 | 回退 |
|---|---|
| LLM 完全不调用 recall(tool description + prompt 都无效) | 退路 1:在 `agent/chat_loop.rs` 每 N 轮强插一次 `<system-reminder>建议你先 recall 看看</system-reminder>`;退路 2:错误发生后强制注入 recall 提示(准 hook) |
| remember 大量垃圾 | 退路 1:收紧频率控制(turn 内 ≤ 1 次);退路 2:加 LLM-as-judge 后台审核(成本高,真不行才上) |
| 关键词召回 < 30% | 退路 1:把 `tags` 字段从 JSON 字符串改为多对多关联表,联合查询更准;退路 2:上 sqlite-vec 轻量向量(V2 决定) |
| 跨项目污染 | 退路 1:加 rust 单元测试覆盖 project_id WHERE 过滤;退路 2:从工具接口彻底禁掉无 project_id 的 recall 调用 |
| 敏感信息泄漏 | 退路 1:扩正则白名单(known secret patterns);退路 2:加 human review queue(破坏 MVP 无 confirm 原则,真不行才回退到这里) |

---

## 待评审关注点

以下 4 点是评审时建议重点讨论的:

1. **`source_kind` 4 种枚举够不够**
   - 当前:`tool_error` / `user_correction` / `observation` / `manual`
   - 疑问:是否要加 `pattern`(LLM 识别出通用模式)和 `preference`(用户表达偏好)?

2. **`tags` 由 LLM 自填 vs 强制枚举**
   - 当前:LLM 自填,更灵活但质量参差
   - 备选:全局枚举(如 `["build", "test", "deploy", "path", "wsl"]`),LLM 必须从枚举里选
   - 权衡:自填灵活 + LLM 二次筛选兜底 vs 枚举精确 + LLM 不爱填

3. **失败兜底 hook(MVP 不做,V2?)**
   - 当前:MVP 仅 LLM 主动
   - 备选:工具返回 error 时自动入队"待 remember 候选",LLM 下次 turn 决定是否记
   - 权衡:漏记风险 vs 增加主循环复杂度

4. **记忆上限 50 条 / session 是否合理**
   - 当前:超出按 use_count 淘汰末尾
   - 疑问:是 session 级还是 project 级?如果是 session 级,session 切换会丢大量记忆;project 级更稳但要全局容量控制

---

## 关联文档

- [CLAUDE.md §Architecture](./../../CLAUDE.md#architecture) —— 当前 memory 模块边界
- [`docs/spikes/2026-06-19-async-parallel-tool-research.md`](./2026-06-19-async-parallel-tool-research.md) —— 调研期间识别出"长期记忆"是独立于 L1/L2/L3 的命题
- [`docs/ARCHITECTURE.md`](./../ARCHITECTURE.md) —— 16 阶段请求生命周期(memory 注入是阶段 ③ 的一部分)
- [`docs/IMPLEMENTATION.md`](./../IMPLEMENTATION.md) —— 决策日志(memory 模块的演化历史)
- [`docs/ROADMAP.md`](./../ROADMAP.md) —— V2 路线图(memory 扩展项登记)

---

## 评审 Checklist

评审者请重点确认:

- [ ] 4 个核心决策(范围 / 主动性 / 检索方式 / 写入审核)是否需要调整?
- [ ] MVP 范围划线(做/不做)是否合理?
- [ ] 失败回退方案是否覆盖主要风险?
- [ ] 待评审关注点 4 个是否有明确倾向?
- [ ] 实施步骤预估耗时是否合理?
- [ ] 后置手动 spike 场景是否覆盖典型踩坑类型?

评审通过 → 进入实施,按 Step 1-5 推进,完成后回填本文件"实测结果"段。