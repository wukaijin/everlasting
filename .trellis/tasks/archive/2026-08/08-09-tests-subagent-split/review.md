# Review — 测试文件拆分:tests_subagent.rs(4270 行)

> 评审日期:2026-08-09。评审对象:`prd.md`(status=planning,实施前评审)。
> 方法:对 PRD 关键事实(`tests_subagent.rs` 行数、测试函数数与注解形态、`run_loop` helper 唯一性与调用数、10 簇函数归属与行号区间、`agent/mod.rs:75` 声明、基线 1662、收官表 ⏳ 行、批 1 OOS 表述、`run_chat_loop` 参数数、sibling hub 模式)逐条 `wc -l` / `grep` / `awk` 核验,并实测 `cargo test --lib`(1662 passed / 0 failed,21.79s)+ `cargo test --lib "agent::tests_subagent::"`(30 passed)+ `cargo fmt --check`(零警告)。

## 总体评价

PRD 结构完整、铁律与验收标准继承 sibling 三单且口径一致,**基线已实测核验属实**(1662 全绿、30 测试原样、fmt 干净)。拆分可行性无虞:任何合理切法下各簇均远 < 1200 行,AC1/AC2 稳。

**结论:可批准进入 design**,但建议先处理 2 个 P1(Background 注解形态计数错误 + 簇表区间与成员矛盾,均直接影响执行准确性),P2/P3 顺手改清。

## ✅ 核验通过(证据确凿)

| 声明 | 核验结果 |
|---|---|
| `tests_subagent.rs` = 4270 行 | **精确**(`wc -l` = 4270) |
| `agent/mod.rs:75` `pub mod tests_subagent;` 声明 | **精确**;目录化后零改动成立(Rust 自动解析 `tests_subagent/mod.rs`,sibling 同款已验证) |
| 30 个测试函数 | **精确**:`cargo test --lib "agent::tests_subagent::"` = 30 passed;函数名与簇表代表函数一一对应 |
| 基线 `cargo test --lib` = 1662 全绿 | **精确**:实测 1662 passed / 0 failed(AGENTS.md 的 ~1657 已陈旧,PRD 正确) |
| 唯一跨簇共享 helper `run_loop`@L2311(span 2311–2364) | **精确**:全文件仅此一个非测试顶层 fn;10 处调用(2576/2731/2862/2959/3095/3197/3281/3407/3528/3686)全部位于 l3a/l3b 区 |
| `run_chat_loop` 34 参签名债务(OOS 冻结) | **精确 34 个**(chat_loop.rs L315–582 签名逐一数出;与 directory-structure.md:92 一致) |
| 注解只有两种形态 | **不实,见 P1-1**(实测三种形态) |
| 收官表最后一行 ⏳(directory-structure.md:81) | **精确**;4 个测试任务 3/4 完成,本任务确为收官最后一单 |
| 批 1 OOS 划出"既有独立测试文件的拆分"(archive 08-07 prd.md:62) | **精确**;本任务是该行的落地 |
| sibling hub 模式:`#![cfg(test)]` + mod 声明 + re-export + `pub(super)` helper | **精确**(`tests_agent_loop/mod.rs` 实读吻合) |
| 簇文件 import 处理方式 | **R3 表述与实测不符,见 P2-2** |
| `cargo fmt --check` 零警告 | **精确**(exit 0) |

## ⚠️ 需修正的问题(按严重度排序)

### 🔴 P1-1 — 注解形态计数错误:"两种形态 15+15" 实际是三种形态 13+15+2

**位置**:`prd.md` Background L13("测试注解有两种形态:`#[tokio::test]`(串行路径,15 个)与 `#[tokio::test(flavor = "multi_thread")]`(需多线程运行时,15 个)")。

**问题**:实测注解为**三种形态**:
- 13 × `#[tokio::test]`(L47/171/405/553/1138/1286/1436/1568/2090/2201/2692/3148/3237)
- 15 × `#[tokio::test(flavor = "multi_thread")]`(L724/903/1769/2492/2790/2899/3005/3343/3469/3622/3787/3908/4080/4136/4225)
- **2 × 纯 `#[test]`**(L2375/2413,l3a unit 两个同步单测 `l3a_filter_tools_readonly_keeps_only_five_read_tools` / `l3a_classify_dispatch_batch_branches_correctly`,不依赖 tokio runtime)

"15+15=30" 只是总数凑巧;实际 13+15+2=30。影响:R2 的"不得改 flavor"只覆盖 `#[tokio::test]` ↔ multi_thread 这条轴,**没覆盖 `#[test]` ↔ `#[tokio::test]` 这条轴**——纯搬迁时最容易"顺手"把同步单测改写成 tokio 测试(或反向),正是铁律要防的行为漂移点。

**建议**:
- Background L13 改为三种形态计数(13 + 15 + 2),并点名两个 l3a unit 测试是纯 `#[test]` 同步单测
- R2 明确为"三种注解形态原样复制:`#[test]` / `#[tokio::test]` / `#[tokio::test(flavor = "multi_thread")]` 互不转换"

### 🔴 P1-2 — 簇表行号区间与簇成员矛盾:l3a/l3b concurrent 物理穿插

**位置**:`prd.md` Background 簇表 l3a concurrent(L2492–3342,6 个)与 l3b concurrent(L3343–3786,4 个)两行。

**问题**:`l3b_concurrent_general_purpose_workers_complete_shared`(fn@L2791,span 2791–2899 ≈ 108 行)的**物理位置在 l3a 区间内**——l3a concurrent 的 6 个测试并非连续区间,中间夹着一个 l3b 测试;l3b concurrent 区间实际只有 3/4 个测试。按"代表函数"归属无歧义,PRD 也有"切分以函数边界/注解边界为准"的 disclaimer,但**行号列字面读法会误导执行者**(尤其 subagent 按区间切分时)。连带预估簇行数失真:按簇归属实际切分 l3a_concurrent ≈ 743(非 851)、l3b_concurrent ≈ 552(非 444)。

**建议**:这两行把"行号区间"改为函数列表,或加注"`l3b_complete_shared` 物理位置在 l3a 区间内,切分按簇归属不按区间"。

### 🟡 P2-1 — l3a_unit 预估行数 187 失实

**位置**:`prd.md` Background 簇行数预估(l3a_unit 187)。

**问题**:簇区间 L2376–2491 实为 **116 行**;187 是把 `run_loop`(2305–2364,声明"提取到 hub 不计入任何簇")的物理区间算了进来,与自身声明自相矛盾。实际 ≈116–129 行,仍远 < 1200,无碍 AC1,但数字失实。

**建议**:改为 ~116(或注明含 run_loop 相邻注释块的粗估口径)。

### 🟡 P2-2 — R3 表述与 sibling 实际模式不符

**位置**:`prd.md` R3("簇文件经 `use super::*` + `use super::tests_common::*` 复用")。

**问题**:sibling(`tests_agent_loop/`)实际做法是**簇文件保留拆分前的显式命名 import 原样不动**(如 `use super::tests_common::{make_harness, test_messages, MockEmitter};` + 各自需要的 crate imports),hub 仅做 `#[allow(unused_imports)] use super::tests_common;` re-export 使原 import 继续解析——hub 注释原话即 "keep their imports unchanged ... (pure relocation)"。当前源文件 L8 也是显式命名导入,并非 glob。按 PRD 字面执行会引入 `use super::*` + glob,虽能编译但与范本/纯搬迁精神有偏差。

**建议**:R3 改述为"簇文件保留原 import 原样;hub re-export `tests_common`(`#[allow(unused_imports)] use super::tests_common;`),使 `use super::tests_common::...` 继续解析"。

### 🟢 P3-1 — R6 唯一靶点未点名

**位置**:`prd.md` R6。

**问题**:全仓非 archive 的 `tests_subagent.rs:LINE` 行号引用**只有一处**:`docs/IMPLEMENTATION/decisions-2026-06.md:711`(残留文档债注记,引 `tests_subagent.rs:1363/1669` 两处过时测试注释)。其余引用(STRUCTURE.md:209 目录树、tool-contract/registry 等代码注释)均为纯路径提及,无行号,R6 不涉及。不点名的话 sweep 时容易漏,且该处是历史 decisions 文档,需要先定口径(改符号引用 vs 豁免历史文档)。

**建议**:R6 显式列出该处并定处理方式(建议:改为符号引用指向具体测试函数名,或明确豁免历史 decisions 文档)。

## 🟦 其他备注(可不动)

- **R4 每簇独立 commit 与 implement.jsonl 引用范本不同源**:agent-loop 单(首个)用"主迁移一个 commit"(理由:目录化中间态编译不过),memories/sessions 两个**最近成功收官**任务均用"每簇独立 commit"——本 PRD 与后两者一致,策略正确。建议 implement.jsonl 补引 memories/sessions PRD 作同策略参照,避免执行时困惑。
- **参数数口径三处数字不矛盾**:`run_loop` doc 注释"23+"(指调用点需传的参数)、PRD"30+"(指 `run_chat_loop` 总参数)、spec"34"(精确值)——口径不同但均成立,无需改。
- **AGENTS.md "~1657" 过时**:实测 1662,PRD 正确;可在收尾 sweep 时顺手同步 AGENTS.md(与 sibling 三单的 P3 类备注一致)。
- 簇文件各需自带 crate imports(`crate::agent::chat_loop::run_chat_loop`、`MockProvider` 等,源文件 L9–15)——sibling 已证明该模式可行,R3 修订后自然覆盖。

## 复评建议

修订 P1-1 / P1-2 后可 `task.py start` 进入 design;P2-1 / P2-2 / P3-1 建议实施前单 commit 顺手合并修订(纯 doc 修,不动源码)。终验口径沿用 PRD:R5 的 30 前后不变 + 实测基线 1662(本评审已实测确认为当前真值)。
