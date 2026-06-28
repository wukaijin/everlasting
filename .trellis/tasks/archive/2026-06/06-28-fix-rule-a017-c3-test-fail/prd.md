# Fix RULE-A-017 — `agent_loop_c3_compaction_does_not_panic` deterministic fail

> P3 债收口。`.trellis/reviews/DEBT.md` RULE-A-017。
> 日期 2026-06-28。Owner: carlos。

## 背景

`cargo test --lib agent_loop_c3_compaction_does_not_panic` 在 main 上 deterministic
失败(957 passed + 1 failed)。journal Session 79/80/81(L3b PR 系列)反复出现
"1 fail = RULE-A-017 pre-existing"。每次收尾都被这条拖累,无法 `cargo test --lib`
全绿。

## 根因(逐行验证)

- **生产码语义正确,不动**:`RULE-A-002`(2026-06-14)把 C3 `compact_messages` 的
  `DegradationKind::StillOver` 从"继续发请求"改成 **fail-fast**
  (`chat_loop.rs:912-919` emit `Error` + `return`,避免 over-budget 请求 400)。
- **测试 setup 过时**:原测试用 `test_messages()`(仅 `["hello"]`)+ `context_window=10`
  (trigger=8, target=5)。`run_chat_loop` 在 `compact_messages`(line 871)之前会注入
  B5 instructions / skill listing(`chat_loop.rs:538/560/750`),把消息 vec 撑大,
  使 drop 后估算仍 > target 5 → 落 `StillOver` → emit Error(无 Done) → 两个 assert
  全挂。这与测试名 `does_not_panic` / 注释 "loop survives" 的意图**相反**。
- Stash 验证(06-27 L3b 执行期)确认 pre-existing、与 L3b 无关。

## 修复(只改测试,生产码零改动)

镜像旁边已 green 的 `agent_loop_c3_still_over_emits_error_and_skips_provider`
(它用 head[2]+middle+tail[huge]+window=1000 强制 `StillOver`)。本测试改成它的
**干净压缩镜像**:head[2 tiny protected] + middle[1 big droppable] + tail[1 tiny]
+ window=1000(trigger=800, target=500)。`big_middle`(~4.8KB≈1200 token)推
`tokens_before` 超 trigger 800 触发压缩;drop 后 head+tail≈10 token << target 500
→ `None`(safe-to-proceed)→ provider 被调、emit Done。

两测试形成 C3 双出口对称覆盖:
- `still_over` → `StillOver` → Error + abort(provider 不调)
- `does_not_panic` → `None` → 正常完成(provider 调一次,emit Done)

断言增强:加 `mock.call_count() == 1`(证明走 None 而非 StillOver 短路)。

## 验证

- `cargo test --lib agent_loop_c3_compaction_does_not_panic` → **ok**
- `cargo test --lib` 全量 → **958 passed; 0 failed**(之前 957+1fail),零回归。

## 收尾

- `DEBT.md` 删除 RULE-A-017(open 集合,闭合即删;通过 git log 追溯,非回填 hash)。
- P3 段计数 [3 items] → [2 items](剩 RULE-B-007 + RULE-C-008)。
- 不涉及生产码 / spec drift,无需 SPEC-DRIFT / ARCHITECTURE 改动。
