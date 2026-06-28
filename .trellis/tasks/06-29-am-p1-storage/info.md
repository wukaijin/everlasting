# P1 交接说明(给新 session / 实施 agent)

> 本文件补充 P1 prd 没写、但冷启动实施必须知道的项目现状与启动步骤。
> prd(需求/AC/schema)以 `prd.md` 为准;本文件只补"怎么动手"。

## 启动步骤

1. **切 active task**:当前 active 是 `am-p5-quality`(建 epic 时的副作用)。执行 P1 前先切:
   ```
   python3 ./.trellis/scripts/task.py start 06-29-am-p1-storage
   ```
   (jsonl 已 curate,Phase 1.3 可跳过——start 后 trellis-implement subagent 会自动注入下方 spec)
2. 进 Phase 2,按 prd `Implementation Plan`:**PR1a(表+CHECK+索引)→ Open Q#1 FTS5 验证 → PR1b(FTS5+触发器)→ PR2 → PR3 → PR4**
3. PR1a 零风险先落,FTS5 验证结果决定 PR1b 走 FTS5 还是 LIKE 退路。

## 项目现状(prd 没提,必读)

- **uuid 只有 v4 feature**(`app/src-tauri/Cargo.toml:34` `uuid = { features = ["v4","serde"] }`)。prd Open Q#3 倾向 v7 → 需改 Cargo.toml 加 `"v7"`(留 `"v4"` 兼容旧代码或一并切 v7)。uuid crate 1.x 支持 v7。
- **`PRAGMA foreign_keys = ON` 已在 `init_pool` 设**(`db/migrations.rs:46`)。prd 决定 `project_id` **不加 FK CASCADE**——故不受影响;但看到这行别误以为必须加 REFERENCES。
- **migration 是顺序 `CREATE TABLE IF NOT EXISTS` 制,非版本号数组**(`run_migrations` 顺序执行,幂等)。加新表 = 在 `run_migrations` 里追加一段 `CREATE TABLE IF NOT EXISTS autonomous_memories (...)` + 索引,参考 projects 段(`db/migrations.rs:57-97`)。**SQLite 不支持 ALTER ADD CHECK**,length/enum CHECK 必须建表时定档(prd B1)。
- **`db/mod.rs` 是 facade,re-export 子模块**。加 memories:
  - `pub mod memories;` + `pub mod memories_tests;`(参考 `mod.rs:58-72` 现有声明)
  - CRUD 函数范式参考 `db/subagent_runs.rs`;`mod.rs:86-88` 有注释——subagent_runs 用 `pub mod` 不 `pub use` 避免冲突,memories 同理。
- **sqlx 非 bundled**(`Cargo.toml:33` features 无 `bundled-sqlite`),链接系统 sqlite → **FTS5 是否可用取决于系统 sqlite 编译**(Ubuntu 通常启用 FTS5,但 tokenizer 默认 `unicode61` 对中文不友好)。必须验证,见下。

## FTS5 验证具体操作(Open Q#1,blocking 第一步)

在 `db/memories_tests.rs`(或临时测试)里:
1. `CREATE VIRTUAL TABLE t USING fts5(title, content)` 能否成功 → 验 FTS5 启用
2. 插入中文行 → `SELECT ... WHERE title MATCH '中文词'` 能否命中 → 验 tokenizer(默认 `unicode61` 对中文按字符边界,可能需改 `tokenize='trigram'`)
3. 若 FTS5 不可用 → 退路:`content LIKE '%kw%' OR tags LIKE '%kw%'`(`tool_name` 精确匹配不受影响,prd 已注)
4. 若中文召回差 → 换 `tokenize='trigram'`(对中文/子串友好)

跑测试(PKG_CONFIG_PATH 必须设,否则撞 gdk-pixbuf not found):
```
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib memories
```

## 已 curate 的 jsonl(Phase 1.3 已完成,新 session 可跳过)

- **implement.jsonl**:database-guidelines / subagent-runs-schema / memory / backend-index / code-reuse-guide / spike-007
- **check.jsonl**:database-guidelines / backend-index / code-reuse + cross-layer guide

## 不在本 session 实施

P1 仅规划就绪(prd v3 + jsonl + 本 info)。实施由新 session `task.py start` 后进 Phase 2。本 session 不写 Rust 代码。
