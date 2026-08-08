# A类单体重构:subagent sink 拆分 — Implement

## 执行策略

无单函数单体(sink.rs 是大 struct + 大 trait impl),拆分 = 可见性准备 → impl 块移动 + 测试迁出 → 文档 sweep。每个逻辑变更独立 commit。

## 执行结果(2026-08-08 完成)

- Commit 1(可见性):`6210b13` 14 字段 + record 升 pub(crate);每步 1662 全绿。
- Commit 2(拆分):`a47f874` hub(493 行 ≤500 ✓)+ `sink/events.rs`(346,impl ChatEventSink 6 方法整体平移)+ 测试迁出 `tests_sink.rs`(877);thread_local TEST_COLLECTOR 升 pub(crate);测试文件级显式补 import(Arc/StdMutex/TranscriptEntry/TokenUsage/payload 类型);clippy/fmt 零警告。
- Commit 3(文档 sweep):`adc4aab` 11 处 `sink.rs:LINE` 行号引用改符号(含 sink.rs 自引用、event_sink.rs 注释、research 文档 4 处);源码注释清零。
- **spec 沉淀**:`pattern-large-function-split.md` 追加 "Variant:大 struct + 大 trait impl(可见性拆分)"。
- **与 design 的偏差**:切分脚本需额外处理 impl 收尾 `}` 归属(struct/impl 边界处 `}\n\n` 模式匹配陷阱——修复脚本 find 首个匹配误删 struct 收尾,已修正)。

## 验证命令

```bash
cd /usr/local/code/github/everlasting/app/src-tauri
export PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig"
cargo check --lib                          # 每个 commit 后
cargo test --lib                           # 每个 commit 后:全量 1662 基线
cargo fmt                                  # 每 commit 前
cargo clippy --lib --tests && cargo fmt --check   # 拆分 commit 后:零警告终验
```

## Ordered Checklist

### Phase A:可见性准备(独立 commit)

1. **[commit] 字段/方法可见性** — struct 14 字段 `pub(crate)`;`record` 升 `pub(crate)`。纯可见性变更,无行为变化。
   - 验证:`cargo check --lib` + `cargo test --lib`(1662 全绿)+ 确认 `pub` API 面零变化(AC5)

### Phase B:拆分(单 commit)

2. **[commit] events.rs + hub re-export + 测试迁出** —
   - 建 `sink/events.rs`:`impl ChatEventSink for SubagentBufferSink` 全部 6 方法整体平移(逐行核对锁序与 emit 顺序)
   - hub(sink.rs):`pub(crate) mod events;` + `#[allow(unused_imports)] pub(crate) use events::*;`;`SubagentBufferSink` 定义保留 hub
   - 内联 `mod tests` 迁出 `tests_sink.rs`,声明加 `subagent/mod.rs`;**文件级 import 照搬 tests_dispatch.rs 先例**:
     ```rust
     #![cfg(test)]
     #[allow(unused_imports)]
     use super::sink::*;      // 文件级,必须带 allow(unused_imports)——
                              // 否则 lib 构建(非 test)报未用导入
     mod tests { use super::*; }   // 模块级指向文件作用域,保持原样
     ```
   - 验证:`cargo check --lib`(mod.rs `pub use sink::SubagentBufferSink` 路径零改动)→ `cargo test --lib`(1662 全绿,30 个 sink 测试全在)→ clippy + fmt 零警告

### Phase C:文档 sweep

3. **[commit] 引用 sweep** — grep `sink.rs:[0-9]` 于 `.trellis/spec/`、`docs/`、`app/src-tauri/src/`(排除 `/_reviews/|/decisions-20|/archive/|/_deprecated/`;research 文档**在范围内**)。已实测命中 11 处(用户 2026-08-08 复核):
   - `docs/REMOTE-ACCESS-RESEARCH.md:102,106,119,661` — `sink.rs:698` / `sink.rs:279`(emit_permission_ask / record 的旧行号)→ `sink.rs::emit_permission_ask` / `sink/events.rs::record` 等符号引用
   - `docs/REMOTE-ACCESS-ROADMAP.md:115,132` — 同上
   - `docs/research/subagent-scheduling-communication-survey.md:61,62` — `sink.rs:238-281` / `sink.rs:608-659` → `sink.rs::new_with_collector` / `sink.rs::emit_permission_ask` 符号引用
   - `app/src-tauri/src/agent/subagent/event_sink.rs:50` — `sink.rs:1126` 注释 → `tests_sink.rs::*` 符号引用
   - `app/src-tauri/src/agent/subagent/sink.rs:1620` — **sink.rs 自身 doc 自引用**(`sink.rs:421-530`),拆分后行号必变 → 改 `sink/events.rs::emit_chat_event` 符号引用
   - 残留核验:上述 grep 应无输出

### Phase D:收尾

4. 终验:`cargo test --lib` 全绿(1662)+ `cargo clippy --lib --tests` + `cargo fmt --check` 零警告
5. squash merge 回 main → `task.py archive` → 复验 `cargo test --lib`

## 风险提示(design §5)

- 方法整体平移禁止顺手重构(锁序/emit 顺序逐行核对)
- 测试迁出:`mod tests` 内 `use super::*` 保持原样(指向文件级作用域);文件级才改 `use super::sink::*`
- `app_handle` dead_code 字段保留(不因可见性调整触发新 dead_code 警告——字段已 `#[allow(dead_code)]`)

## Review Gates

- [ ] 用户评审 prd/design/implement 通过
- [ ] `task.py start` 后实施
- [ ] 每 commit 独立可回滚(AC3)
- [ ] 终验三绿(AC4)+ 锁序/emit 顺序核对(AC7)
