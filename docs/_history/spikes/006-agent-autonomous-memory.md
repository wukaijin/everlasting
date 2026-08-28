# spike-006: agent 自主跨 session 记忆 — 最小可行设计

**日期**: 2026-06-29
**状态**: 待评审（设计已对齐，未实施）
**作者**: deepseek-v4-pro
**依赖**:
- 无（新功能调研）
- 关联 `docs/spikes/005-agent-memory.md`（同主题早期讨论，本 spike 取代之，差异见 §9）

**预估耗时**:
- 调研 + 对齐: 已完成（本文档）
- 实施预估: 5-8 小时（DB migration + 2 tool + auto-injection + 前端扩展 + 测试）

---

## 1. 目标

把当前"加载 4 个静态指令文件"的 memory 模块，扩展为 agent 自主产生、可被 LLM 在决策时主动唤起的**跨 session 记忆能力**。

### 设想场景

> agent 多次因路径问题调用测试命令失败，最终成功后记住这个坑；在另一个 session 遇到时，能第一时间想到如何规避。

### 现状盘点

当前 `memory/` 模块只做**指令文件加载**：4 个 CLAUDE.md / AGENTS.md → 带 `cache_control: ephemeral` 注入 prompt。Agent 只读，不能写。

`MemoryKind::Session` 和 `MemoryKind::Runtime` 已在 `memory/types.rs` 留了类型占位，但从未被填充（`#[allow(dead_code)]`）。

BACKLOG 附录 A §3.4 有远期构想 "Runtime memory: agent 跨 session 长期记忆（可被 LLM 主动写）"，但未实施。

---

## 2. 核心决策

| 决策点 | 选择 | 理由 |
|---|---|---|
| **范围** | user + project 双 scope | schema 只差一列，建表和 FTS5 工作量完全一样 |
| **MVP 主动性** | LLM tool 调用 + harness FTS5 自动注入 | 两个 tool（save_memory / search_memory）+ 每 turn 自动 FTS5 检索 |
| **检索方式** | FTS5 关键词 + tag | MVP 不上向量检索；FTS5 是 SQLite 内置，零额外依赖 |
| **写入权限** | Tier 4 ask（和 web_fetch 同级） | agent 写长期持久化存储需用户确认，避免污染 |
| **去重** | 不做（MVP 最简单） | agent 先 search 再决定是否记新的；V2 可加 title 唯一 upsert |
| **前端** | MemoryPreview 扩展 + 可删除 | 用户可见/可控 runtime memories |

### 不做（明确延后到 V2）

- ❌ 旁路提取 agent
- ❌ 失败-成功模式自动检测
- ❌ Embedding 语义检索
- ❌ upsert 去重
- ❌ 记忆过期 / 衰减
- ❌ save_memory 自动触发（必须 LLM 显式调）

---

## 3. 通过标准（MVP 上线判定）

### 硬通过（全部满足 → MVP ship）
- [ ] `save_memory` / `search_memory` 两个 builtin tool 注册到 `tools/mod.rs`，LLM 可正常调用
- [ ] SQLite `memories` 表 + `memories_fts` FTS5 虚拟表 migration 落地，CRUD 跑通 Rust 单元测试
- [ ] `inject_relevant_memories()` 每 turn FTS5 自动检索 top 3，注入 ephemeral block
- [ ] 手动 spike 验证：agent 在连续 5 个真实场景中至少 4 次主动调用 `save_memory`，写入内容 review 合格
- [ ] 手动 spike 验证：agent 在遇到已知坑时，auto-injection 命中 + agent 直接规避（≥ 3/5）
- [ ] 关键词 + tag 检索在 100 条记忆内 top-5 召回率 ≥ 60%

### 可接受瑕疵（不阻塞）
- ⚠️ tag 由 LLM 自填，质量参差（FTS5 搜 content 兜底）
- ⚠️ 同义不同词漏召回（LLM 可调 `search_memory` 二次搜）
- ⚠️ agent 偶尔不调 save_memory（prompt 强化可缓解）

### 硬失败（任一 → 走回退方案）
- ❌ LLM 完全不调用 `search_memory`（tool description + prompt 都无效）
- ❌ save_memory 写入大量垃圾内容（频率控制 / 过滤失效）
- ❌ FTS5 召回 100 条内 top-5 命中率 < 30%
- ❌ 跨 project 泄漏（project_id WHERE 过滤被绕过）

---

## 4. 具体方案

### 4.1 SQLite 表结构

放在 `db/` 模块，新建 `db/memories.rs`。

```sql
CREATE TABLE memories (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    memory_id         TEXT    NOT NULL UNIQUE,  -- UUID，对外引用
    scope             TEXT    NOT NULL CHECK(scope IN ('user', 'project')),
    project_id        TEXT,                     -- scope='user' 时为 NULL
    title             TEXT    NOT NULL,
    content           TEXT    NOT NULL,
    tags              TEXT    NOT NULL DEFAULT '[]',  -- JSON 数组
    source_session_id TEXT,
    created_at        TEXT    NOT NULL,
    updated_at        TEXT    NOT NULL
);

CREATE VIRTUAL TABLE memories_fts USING fts5(
    title, content, tags,
    content='memories',
    content_rowid='id'
);

CREATE INDEX idx_memories_scope_project ON memories(scope, project_id);
CREATE INDEX idx_memories_created ON memories(created_at DESC);
```

**设计点**:
- `memory_id` UUID 对外引用，与内部自增 `id` 分离（FTS5 需要整数 content_rowid）
- `scope` 两值：`user`（跨项目）和 `project`（本项目）
- FTS5 触发器自动同步 INSERT / UPDATE / DELETE
- **不做** Jaccard 去重、use_count、confidence、source_kind 字段（MVP 最简单）

### 4.2 工具接口

#### `save_memory` — 写入记忆

```rust
// tools/save_memory.rs
{
  "scope": "project",          // "user" | "project"
  "title": "WSL cargo test PKG_CONFIG_PATH",
  "content": "在 WSL 下运行 cargo test 必须设置 PKG_CONFIG_PATH=...",
  "tags": ["wsl", "cargo", "pkg-config"]  // 可选
}
```

**行为**:
- scope="project" 时自动绑定当前 session 的 project_id
- 直接 INSERT，不检查重复
- 记录 `source_session_id`
- 返回 `{ memory_id, title }`

**权限**: Tier 4 ask（`ToolKind::Other`，和 web_fetch 同级）

**安全网**（入库前检查）:
- 敏感信息过滤：匹配 `(?i)(api[_-]?key|secret|password|token=|bearer)` → 拒绝 + warn
- 长度上限：content ≤ 2000 字符

#### `search_memory` — 检索记忆

```rust
// tools/search_memory.rs
{
  "query": "cargo test 路径问题",
  "scope": "project",          // 可选，不传则搜 user + 当前 project
  "limit": 5                   // 可选，默认 5
}
```

**行为**:
- `scope` 不传时自动搜 `scope='user'` + `(scope='project' AND project_id=当前项目)`
- FTS5 MATCH，按 bm25 排序
- 返回 `[{ memory_id, scope, title, content, tags, source_session_id, created_at }]`

**权限**: Tier 5 silent Allow（纯读操作）

### 4.3 自动注入（harness 侧）

在 `chat_loop.rs` 的注入阶段，checklist injection 之后、provider.send() 之前，新增：

```
inject_relevant_memories(db, project_id, &last_user_message_text)
  → FTS5 MATCH title+content+tags, limit 3
  → 格式化为 ephemeral ContentBlock:
    <relevant_memories>
    以下是你之前记住的相关经验：
    - [标题]: 正文
    - [标题]: 正文
    </relevant_memories>
  → 无匹配则跳过（不增加 token 开销）
```

**关键约束**:
- 注入位置：`cache_control: ephemeral` 断点**之后**，不影响 prompt caching
- Token 硬上限：注入内容 ≤ 500 tokens
- 不持久化到 messages（纯 ephemeral，和 B12 checklist 一样）

### 4.4 现有注入顺序中的位置

```
每 turn 注入顺序：
  load session → load project → load memory(4 static files)
  → load skill listing → load checklist
  → inject_relevant_memories  ← 新插在这里
  → head_sha refresh → provider.send
```

---

## 5. 与现有系统的集成点

| 集成点 | 改什么 | 文件 |
|---|---|---|
| DB | `memories` 表 + `memories_fts` 虚拟表 + migration | `db/migrations.rs`, `db/memories.rs` |
| DB | `insert_memory` / `search_memories` / `delete_memory` CRUD | `db/memories.rs` |
| Tool | `save_memory` tool（Tier 4 ask） | `tools/save_memory.rs` |
| Tool | `search_memory` tool（Tier 5 silent） | `tools/search_memory.rs` |
| Tool | 注册到 `builtin_tools()` | `tools/mod.rs` |
| Agent loop | `inject_relevant_memories()` per-turn 调用 | `agent/chat_loop.rs` |
| 前端 store | `fetchMemories` / `deleteMemory` actions | `stores/memory.ts` |
| 前端组件 | MemoryPreview 扩展 runtime memories 列表 | `components/memory/MemoryPreview.vue`（或新建组件） |
| IPC | `list_memories` / `delete_memory` commands | `commands/memory.rs`（或新建 `commands/memories.rs`） |

---

## 6. MVP 范围划线

### ✅ 做
- `save_memory` / `search_memory` 两个 tool
- SQLite 表 + FTS5 + CRUD
- 每 turn FTS5 自动注入 top 3
- 敏感信息过滤 + 长度上限
- 前端 MemoryPreview 扩展（查看 + 删除）
- Rust 单元测试（CRUD、project 隔离、敏感过滤）

### ❌ 不做
- 旁路提取 agent
- 失败-成功模式自动检测
- Embedding 语义检索
- upsert 去重
- 记忆过期 / 衰减
- 记忆晋升为指令文件

---

## 7. 执行步骤（评审通过后实施）

### Step 1: DB 落地（预估 1.5 小时）
- 新建 `db/memories.rs`：CRUD + 敏感过滤 + FTS5 触发器
- `db/migrations.rs` 加 v{N+1} migration
- `db/mod.rs` 导出
- 单元测试：插入 / FTS5 搜索 / 跨 project 隔离 / 敏感过滤拒绝

### Step 2: 工具实现（预估 2 小时）
- 新建 `tools/save_memory.rs` + `tools/search_memory.rs`
- `tools/mod.rs` 注册到 `builtin_tools()`
- 单元测试：长度上限拒绝 / project_id 绑定 / FTS5 召回

### Step 3: auto-injection（预估 1 小时）
- `agent/chat_loop.rs` 加 `inject_relevant_memories()`
- 集成测试：注入位置 / token 上限 / 空结果跳过

### Step 4: 前端（预估 1.5 小时）
- `stores/memory.ts` 扩展：`fetchMemories` / `deleteMemory`
- MemoryPreview 组件扩展：runtime memories 列表 + 删除按钮
- Tauri commands：`list_memories` / `delete_memory`

### Step 5: 质量验证（预估 1 小时）
```bash
cd app/src-tauri && PKG_CONFIG_PATH=... cargo test --lib
cd app/src-tauri && PKG_CONFIG_PATH=... cargo check
cd app && pnpm build   # vue-tsc --noEmit + vite build
```

### Step 6: 手动 spike（后置，1-2 小时）
5 个真实场景手动跑通：
1. WSL cargo test 路径问题 → agent 记住 PKG_CONFIG_PATH
2. 项目特有的 .env 变量约定 → agent 记住
3. 反复出现的 import 路径错误 → agent 记住
4. 用户纠正过的做法 → agent 记住
5. 新 session 遇到同类问题 → auto-injection 命中 + agent 规避

**判定**: save_memory 调用 ≥ 4/5，auto-injection 命中 ≥ 3/5

---

## 8. 失败 → 回退方案

| 现象 | 回退 |
|---|---|
| LLM 完全不调用 search_memory / save_memory | 退路 1: 在 chat_loop 每 N 轮强插 hint 提示；退路 2: 错误发生后强制注入记忆匹配结果 |
| save_memory 大量垃圾 | 退路 1: 收紧频率控制（同 session ≤ 10 条）；退路 2: 加去重（title 唯一） |
| FTS5 召回 < 30% | 退路 1: 扩为 content LIKE '%keyword%' 兜底；退路 2: 上 sqlite-vec 向量 |
| 跨 project 泄漏 | 退路: Rust 单元测试覆盖 project_id WHERE 过滤，工具层强校验 |
| 敏感信息泄漏 | 退路: 扩正则白名单，加 human review queue |

---

## 9. 与 spike-005 的差异

本 spike（006）取代 spike-005，主要差异：

| 差异点 | spike-005（MiniMax-M3） | spike-006（本设计） |
|---|---|---|
| scope | project-only | user + project 双 scope |
| 工具命名 | `remember` / `recall` | `save_memory` / `search_memory` |
| 自动注入 | 无（依赖 system prompt 教 LLM 调用） | 每 turn FTS5 自动注入 top 3 |
| 去重 | Jaccard 相似度 > 0.7 合并 | 不做（MVP 最简单） |
| 写入权限 | 无用户确认，安全网兜底 | Tier 4 ask（和 web_fetch 同级） |
| source_kind | 4 种枚举（tool_error 等） | 不记录（精简 MVP） |
| use_count / confidence | 有 | 无（精简 MVP） |
| 前端 | 暂不动 | MemoryPreview 扩展 + 可删除 |
| 频率控制 | 同 turn ≤ 3 次，同 session ≤ 50 条 | 简化为 content 长度上限 + 敏感过滤 |
| system prompt 注入 | `build_instructions_blocks()` 加常量段 | 不额外加 prompt（靠 tool description + auto-injection） |

**取代理由**: spike-005 在 2026-06-29 对话早期产生，后续 grill 中用户明确了不同倾向（双 scope、FTS5 自动注入、Tier 4 ask、不做去重）。本 spike 反映最终对齐的设计。

---

## 10. 关联文档

- `docs/spikes/005-agent-memory.md` — 同主题早期讨论（已被本 spike 取代）
- `docs/_archive/backlog-appendix-A.md` §3.4 — 远期 Runtime memory 构想
- `docs/ARCHITECTURE.md` — 16 阶段请求生命周期
- `docs/ROADMAP.md` — V2 路线图

---

## 评审 Checklist

评审者请重点确认:

- [ ] user + project 双 scope 是否合理？user 级跨项目污染风险如何缓解？
- [ ] FTS5 自动注入 top 3 会不会干扰 LLM 的正常推理（信息过载）？
- [ ] save_memory Tier 4 ask 弹窗会不会太频繁打断体验？是否考虑首次授权后 session 内记白名单？
- [ ] 不做去重在 100+ 条记忆后垃圾堆积风险多大？
- [ ] MemoryPreview 前端扩展的 UI 方案是否需要细化（Scope 过滤？按项目/时间排序？）
- [ ] 与 B12 checklist 的注入位置关系是否正确（checklist 在 memory injection 之前）？
