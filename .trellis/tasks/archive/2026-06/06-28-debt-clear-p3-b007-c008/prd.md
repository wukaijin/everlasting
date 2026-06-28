# Clear remaining P3 debt — RULE-B-007 + RULE-C-008

> P3 债清零。`.trellis/reviews/DEBT.md` 最后 2 条。
> 日期 2026-06-28。Owner: carlos。

## 背景

RULE-A-017 闭合后(见上一 task `06-28-fix-rule-a017-c3-test-fail`),DEBT 仅剩 2 条
P3(文档/一致性),且都是「决策类」open question(Fix 字段 = "评估保留 or 移除" /
"决定硬前置 or 维持")。本 task 逐条调研现状 → 确认决策 → 从 DEBT 删除。**零代码改动**
(两条现状均已正确,且都有项目级决策背书)。

## RULE-B-007 — Background Mode 空壳 → **维持(有意保留预留位)**

- **DEBT 原描述**:`mode.rs:26-28` `mode_system_prefix` 的 `Mode::Background` 占位
  字符串,`#[allow(dead_code)]`。
- **调研结论**:
  - `Mode::Background` **wire 完整,非 dead code**:`db/types.rs:212`(enum) +
    `:221/:234`(DB 序列化 `"background"`) + `commands/permissions.rs:93`(IPC wire) +
    `tests_mode.rs:8,77`(测试覆盖)。
  - `mode.rs` 实际**无** `#[allow(dead_code)]` 残留 — DEBT 描述过时不准确。
  - `mode_system_prefix` 的 Background arm 是 `match mode` **穷尽性必须**(不能删)。
  - CLAUDE.md 顶部 + ROADMAP §4.2 明确:「`Background` enum 留位但 UI 不暴露」。
- **决策**:**保留**。这是有意保留的预留位(未来后台/daemon 模式用),wire 完整,
  UI 未暴露。删 enum 会推翻已记录的项目决策 + 破坏 DB/IPC wire,无收益。
- **动作**:仅删 DEBT 条目。

## RULE-C-008 — grill Q4 AGENTS.md 物理顺序前置 → **维持(wrapper 标签即 Q4 决策)**

- **DEBT 原描述**:`loader.rs:321` 仍按 CLAUDE→AGENTS 顺序,优先级仅靠
  `<primary>`/`<reference>` wrapper 标签;「软提示 vs 硬提示,标签可能已足够」。
- **调研结论**:
  - `loader.rs:360-374` **已按 grill Q4 实现**:AGENTS.md 包
    `<primary instructions>`(为 Everlasting 写)、CLAUDE.md 包 `<reference>`
    (Claude-Code interop),注释 line 360-361 明确 "per the B5 review §3 Q4 decision"。
  - grill Q4 的**结论就是 wrapper 标签**(语义硬标记,非软提示),不是物理顺序前置。
  - 物理前置(AGENTS 排 CLAUDE 前)反而破坏 User→Project 层级 + 与 cache breakpoint
    顺序耦合。
- **决策**:**维持现状**。DEBT 条目(06-14 记录)是对 Q4 的误读 — 实现已是 Q4 结论。
- **动作**:仅删 DEBT 条目。

## 验证

- 无代码改动 → 无需跑测试。
- DEBT.md:P3 段 [2 items] → [0 items];表格 P3 2→0、Total 2→0。
- **DEBT.md 当前 open items = 0**(P0/P1/P2/P3 全 0)。

## 收尾

- `DEBT.md` 删除 RULE-B-007 + RULE-C-008(open 集合,闭合即删,通过 git log 追溯)。
- 不涉及代码 / spec drift,无需 SPEC-DRIFT / ARCHITECTURE 改动。
- 两条决策依据留痕于本 prd + commit message + journal(防未来 audit 复述)。
