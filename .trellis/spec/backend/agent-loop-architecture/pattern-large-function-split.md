# Pattern: Large-Function Split (A 类单体重构)

> 2026-08-08,首个 A 类专项(`08-08-a-class-dispatch-split`,dispatch.rs 1472 行 → hub 469 行 + 7 子模块)。后续 A 类专项(`anthropic.rs` / `sink.rs` / `chat_loop.rs`)复用本模式。

## Problem

> 1000+ 行单体函数无法用"纯搬迁"处理(B 类):代码块间共享大量局部状态,直接挪文件会破坏借用/顺序。`run_subagent` 1295 行、25 参数、~24 个内部阶段即此类。

## Solution:两阶段拆分

### Phase 1 — 提取(函数化,原地)

- 按**连续代码段**切分阶段(每个阶段函数严格对应一段连续代码;若逻辑阶段跨不连续段,拆成两个连续段函数,禁止"穿越中间代码合并")。
- 每个阶段函数返回**阶段输出 struct**(用户拍板,非纯参数传递、非单一 ctx struct);struct 字段 `pub(crate)`,owned 类型。
- **关键技巧**:调用处用 `let StructName { field1, field2, ... } = stage_fn(...).await;` 解构回原局部变量名 → **后续代码零改动**(原变量名/类型继续可用;`&str` 借用在字段 owned 化后 shadow 回 `&str`)。
- 每个阶段一个独立 commit,commit 后立即 `cargo test --lib` 全量(行为零变化的锚点)。
- 早返路径:`Result<T, (String, bool, bool, Option<i32>)>` 形态,`Err` 与函数返回 tuple 同形;早返 `is_error` 恒为 `true`(不变量)。

### Phase 2 — 拆分(文件移动,单 commit)

- `foo.rs` 保留为 hub(文档 + `pub(crate) mod x;` + `#[allow(unused_imports)] pub(crate) use x::*;` 全量 re-export),子文件进 `foo/` 目录 —— 既有调用方路径(`crate::...::foo::sym`)与测试的 `use super::foo::*` 零改动解析。
- hub 的 use 块精确裁剪到 hub 自身所需(阶段函数移走后原 use 大多 unused)。
- 被子模块消费的 hub 私有项(如 `const`/`fn`)须升 `pub(crate)`。

## Gotchas(实测,2026-08-08)

> **Warning**:`#[allow(unused_imports)]` 只作用于其后**单个 use 语句**。子模块批量复制 use 集时必须用文件级 inner attribute `#![allow(unused_imports)]`(放文件头 doc 之后),否则每个多余 use 各报一条 unused import 警告。

> **Warning**:子模块(`foo/x.rs`)里原文件的 `use super::prep::...` 必须改 `use super::super::prep::...`(super 现在指向 `foo` 而非上一级);同样,引用 hub 的符号用 `use super::sym;`。

> **Warning**:段切分时,主函数前的陈旧 doc 注释/属性(如 `#[allow(clippy::too_many_arguments)]`)若落在最后一个阶段段与主函数之间,会被段切割吞进子文件——检查并归还主函数,否则 clippy `too_many_arguments` 回归。

> **Warning**:函数行数验收口径:25+ 参数签名(含每参数历史注释)是冻结债务,不计入"函数体 ≤250 行";awk 从签名起测会虚高 ~40%。

> **Warning**:测试迁出(内联 `mod tests` → 同级 `tests_*.rs`)时:文件级 `use super::*` 改 `use super::<hub>::*` 只传播 **pub 项**——hub 的私有 use 导入(如 `ThinkingConfig`)与私有 fn(如 `parse_anthropic_usage`)不传播,需显式 import 或升 `pub(crate)`;`mod tests` 内的 `use super::*` 保持原样(指向测试文件文件级作用域);测试文件需在**父模块**(非 hub)声明 `mod tests_xxx;`(文件位置决定声明位置)。

## Variant:stream!/闭包生成器(无 yield 纯函数提取,08-08 anthropic 专项)

`async_stream::stream!` / 含 `yield` 的生成器函数无法按"阶段输出 struct"模式提取——`yield` 只能在宏体内。

**模式**:把每个事件 match 臂提取为**无 yield 纯函数**:

- 有即时产出的事件臂 → `fn handle_x(data: &str, state: &mut ...) -> Option<ChatEvent>`,宏体内统一 `if let Some(ev) = handle_x(...) { yield Ok(ev); }`(yield 点收敛,事件顺序逐一对应原分支)。
- 纯状态转换臂(无 yield)→ 返回 `()`。
- 含 `yield Err` 的 IO 段(HTTP 发送)→ `async fn -> Result<T, LlmError>`,宏体 `match { Err(e) => { yield Err(e); return; } }`。

**收益**:宏体从 ~430 行降到 ~90 行(骨架 + 分发 + yield 点);事件状态机(`BlockState`)与 handler 成为可独立单测的纯逻辑(零覆盖 → 补 5 个测试,1657 → 1662)。

## 验收(dispatch.rs 实例)

- `run_subagent` 函数体 171 行(7 个阶段调用 + 解构 + 胶水),签名 118 行冻结。
- `dispatch.rs` hub 469 行:`run_subagent` 主体 + `check_workflow_role_gate` + re-export。
- `dispatch/{parse,plan,prepare,model,register,drive,finalize}.rs` 143–448 行。
- 1657 测试全绿,`clippy --lib --tests` + `fmt --check` 零警告。
