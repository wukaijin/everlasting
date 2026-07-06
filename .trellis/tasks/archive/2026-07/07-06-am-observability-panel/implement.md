# Implement — V2-2+ 自主记忆可观测性与管理面板

> 配套 `prd.md` + `design.md`。ordered checklist + validation。inline 实施(非 sub-agent dispatch,跳过 implement.jsonl/check.jsonl gate)。**planning 已补全,下个 session 直接 task.py start → Phase A 实施。**

## 已确认事实(下个 session 直接用,无需重新探索)

- **ChatEvent enum 位置**:`app/src-tauri/src/llm/types.rs:341 pub enum ChatEvent`(Recall 变体加这里);`ChatEventPayload` 在 `state.rs:506`;`ChatEventSink` trait 在 `state.rs:559`(`emit_chat_event(&self, payload: &ChatEventPayload)`)。
- **migration 模式**:项目**无数字编号**,用 `add_column_if_missing` helper(先例 `migrations.rs:945` sessions / `:965` projects / `:986` providers / `:1101` subagent_runs)。SQLite < 3.35 不支持 `ADD COLUMN IF NOT EXISTS`,helper 走 `PRAGMA table_info` 检查。autonomous_memories 表在 `:743`,新列复用同款 helper。
- **`build_recall_text` 返 `Option<String>`**(`memory_recall.rs:77`),**不含 row 详情**。FTS 路 recall event 必须 sibling `build_recall_text_with_rows(pool, project_id, query) -> Option<(String, Vec<MemoryRow>)>`(重构:`build_recall_text` 包装它丢 rows;原 4 个测试零回归)。
- **`PitfallRecall` enum**(`permissions/check.rs:986`):`None / Footnote(String) / SoftBlock { hint, memory_id }`。`recall_pitfall` 在 `:1057`,内部调 `find_pitfalls_by_trigger_all_status`(`:1075 recall_pitfall_inner`)。**Footnote 只含拼接 String,无 row 详情**;pitfall 路 recall event 需扩展 `recall_pitfall` 返伴随 `Vec<MemoryRow>`(或 sibling `recall_pitfall_with_hits`),SoftBlock 已含 `memory_id` 可直接查 title。
- **时间戳**:`Utc::now().to_rfc3339()`(`db/memories.rs:60` import,`:518`/`:1352`/`:1521`/`:1576` 用)。
- **`MemoryRow`**:`db/memories.rs:213`(字段到 `:232 updated_at` + `demoted_reason`),加 `edited_by_user: bool` 字段;所有 SELECT 列表(`:524`/`:597`/`:641` 等 ~12 处)加列。
- **lib.rs 注册**:`lib.rs:149 invoke_handler(generate_handler![...])`,memory 命令在 `:239-244`,新 IPC 加 `:244` 后。
- **前端 chat-event listener**:`stores/streamController.ts:1578 unlistenChat = await listen<ChatEventPayload>("chat-event", ...)`,recall 分发加这里(过滤 `kind === "recall"`)。
- **`insert_memory` 安全网**:`db/memories.rs:499`,校验在 `:415`(char count)+ `:423`(sensitive regex,查 title+content)+ `:436`(sensitive path)。提取为 `validate_memory_text(title, content)`。
- **`update_status`**:`db/memories.rs:1483`,`StatusTransitionError` 在 `:1471`(Illegal/NotFound)。
- **参考组件**:`memory/MemoryModal.vue`(reka-ui Dialog 包 MemoryPreview,服务 layer),样式参考;`memory/MemoryPreview.vue` runtime section(row 扩展点)。

## 执行清单

### Phase A — 后端(DB + IPC + recall event)

- [ ] **A1** `db/migrations.rs`:复用 `add_column_if_missing` helper 加 `autonomous_memories.edited_by_user BOOLEAN NOT NULL DEFAULT 0`(默认 0,旧行回填)。
- [ ] **A2** `db/memories.rs`:提取 `validate_memory_text(title: &str, content: &str) -> Result<(), MemoryWriteError>`(从 `insert_memory` :415-436 重构 char count + sensitive regex + sensitive path),`insert_memory` 复用(回归基线,P4/P5 依赖)。
- [ ] **A3** `db/memories.rs`:新 `update_memory(pool, memory_id, title, content) -> Result<(), MemoryUpdateError>`(`validate_memory_text` + `edited_by_user=1` + `updated_at=Utc::now().to_rfc3339()`);`MemoryUpdateError` 复用 `MemoryWriteError` + `NotFound`。
- [ ] **A4** `db/memories.rs`:`MemoryRow`(213)加 `edited_by_user: bool`;所有 SELECT(约 12 处)+ list_autonomous_memories 映射加列。
- [ ] **A5** `commands/memory.rs`:IPC `update_autonomous_memory_status(memory_id: String, new_status: String, demoted_reason: Option<String>)` → 包 `db::update_status`;`StatusTransitionError::{Illegal,NotFound}` → `AppCommandError`。
- [ ] **A6** `commands/memory.rs`:IPC `update_autonomous_memory(memory_id: String, title: String, content: String)` → 包 `db::update_memory`。
- [ ] **A7** `llm/types.rs:341`:`ChatEvent` 加 `Recall { hits: Vec<RecallHit> }` 变体;新 `RecallHit { memory_id: String, title: String, kind: String, source: String }`(`source: "fts"|"pitfall"`)。确认 serde tag 命名(对齐现有 ChatEvent 序列化)。
- [ ] **A8** `agent/memory_recall.rs`:新 `build_recall_text_with_rows(pool, project_id, query) -> Option<(String, Vec<MemoryRow>)>`(重构 `build_recall_text` 为 wrapper:`.map(|(t, _)| t)`);保留原 4 测试零回归。`chat_loop.rs:1385` 改调它,`Some((_, rows))` 后 `sink.emit_chat_event(Recall { hits: rows.iter().map(RecallHit::from_fts).collect() })`。
- [ ] **A9** `agent/permissions/check.rs`:扩展 `recall_pitfall` 返 `(PitfallRecall, Vec<MemoryRow>)`(伴随命中的所有 pitfall rows;SoftBlock 的 memory_id 已在变体里),或新增 sibling `recall_pitfall_with_hits`。`chat_loop.rs:2453` + `:3205` 拿 rows 后 emit `Recall { hits: ..., source: "pitfall" }`;`PitfallRecall::None` 不 emit。
- [ ] **A10** `lib.rs:244` 后:注册 `update_autonomous_memory_status` + `update_autonomous_memory`。
- [ ] **A11** 后端测试:
  - `update_memory_roundtrip` / `update_memory_rejects_oversize` / `update_memory_rejects_sensitive` / `update_memory_rejects_sensitive_path` / `update_memory_sets_edited_by_user`
  - `insert_memory_still_safe_after_helper_extract`(回归 A2)
  - `build_recall_text_with_rows_returns_rows`(A8)+ 原 4 测试零回归
  - IPC `update_autonomous_memory_status` 合法/非法转换 roundtrip
  - `chat_loop_emits_recall_on_fts_hit`(A8)/ `chat_loop_emits_recall_on_pitfall_hit`(A9)集成测试
  - `worker_recall_does_not_reach_main_chat`(sink 隔离,AC7;SubagentBufferSink 不 forward)

### Phase B — 前端(store + UI)

- [ ] **B1** `stores/memory.ts`:`AutonomousMemory`(115)加 `editedByUser: boolean`;新 actions `updateMemoryStatus(id, status, reason?)` + `updateMemory(id, title, content)`(optimistic patch + IPC);`lastRecallHits` ref + 在 `streamController.ts:1578` listener 加 recall 分发(过滤 `kind === "recall"` → push hits;新 user message 清空)。
- [ ] **B2** `memory/MemoryPreview.vue`:runtime row 加 `hitCount` + `lastUsedAt` chip(`hitCount>0` 时)+ 点击 row(或"管理"按钮)打开 `RuntimeMemoryModal`。
- [ ] **B3** `memory/RuntimeMemoryModal.vue`(新建):reka-ui Dialog(参考 MemoryModal 样式 80vw/min640/max900/80vh);title input + content textarea + 统计区(hitCount/lastUsedAt/confidence/createdAt/sourceSessionId/sourceRef/editedByUser 徽标)+ 状态下拉(矩阵驱动合法目标,D6)+ 转 demoted 弹 reason + 保存/取消/delete。
- [ ] **B4** `chat/ChatPanel.vue`(或 MessageList 顶部):实时召回 chip(`lastRecallHits.length>0` 时),点开展开 title 列表(按 source 分组 fts/pitfall)。
- [ ] **B5** 前端测试:
  - store `updateMemoryStatus_happy_path` / `updateMemory_happy_path` / `recall_listener_accumulates_hits` / `recall_clears_on_new_user_message`
  - MemoryPreview `row_shows_hitCount_chip` / `row_click_opens_modal`
  - RuntimeMemoryModal `status_dropdown_only_lists_legal_targets`(矩阵驱动)/ `edit_save_calls_store` / `demote_prompts_reason`
  - ChatPanel `recall_chip_renders_on_hit`

### Phase C — 收尾

- [ ] **C1** spec:`memory.md` 加 "Scenario: V2-2+ Observability & Management" 段(update_memory / update_status IPC / recall event / RuntimeMemoryModal);`tool-contract.md` 加 update IPC 签名;`frontend/chat.md` 加召回 chip + modal。
- [ ] **C2** `docs/ROADMAP.md` §1.2 加 V2-2+ 行 + §2 第三档移项;`docs/IMPLEMENTATION.md` §4 ADR(D1-D7 + build_recall_text_with_rows / recall_pitfall 扩展决策)。
- [ ] **C3** 全量验证(见下)+ `cargo fmt`。

## Validation Commands

```bash
# Rust(WSL PKG_CONFIG_PATH,见 CLAUDE.md)
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib
cd app/src-tauri && cargo fmt --check

# 前端
cd app && pnpm test
cd app && pnpm build    # 含 vue-tsc --noEmit
```

## 风险文件 / 回滚点

见 `design.md` §6(回滚)+ §7(风险文件)。核心风险:
- `db/memories.rs` 安全网 helper 提取 → `insert_memory` 回归(P4/P5 依赖)— A11 回归测试兜底。
- `memory_recall.rs` `build_recall_text_with_rows` 重构 → 原 4 测试零回归(A8 wrapper 保持签名)。
- `permissions/check.rs` `recall_pitfall` 扩展返 rows → P5 soft-block 路径回归(A9,PitfallRecall 变体不变,只加伴随 rows)。
- `chat_loop.rs` 3 处 emit + worker 隔离 — A11 `worker_recall_does_not_reach_main_chat` 锁 AC7。
- `migrations.rs` add_column helper 复用 — A1 走既有模式。
