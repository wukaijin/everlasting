# 实施质量评审 — 大文件拆分与文档同步

> 评审日期:2026-08-08(实施完成后)。对象:8 个 refactor commit(`fbb8126`→`0909923`,分支 `refactor/file-splitting`)。
> 方法:对照 PRD AC1-5、R1-R5、review.md 的 P1/P2/P3,逐项实测——行数/测试/clippy/fmt/链接/失效引用/纯搬迁。

## 总体评价:✅ 优秀,达标

实施**忠实执行了设计与评审意见**,核心硬约束全部达成:

- **R5 功能不变(硬约束)**:后端 `cargo test --lib` **1657 passed; 0 failed**,前端 `pnpm test` **1018 passed(65 files); 0 failed**。
- **clippy + fmt 零警告**(`cargo clippy --lib` exit 0;`cargo fmt --check` exit 0)。
- **AC1 行数**:7/8 源码文件 <1200;`dispatch.rs` 1472 行已透明豁免(用户批准,理由成立——`run_subagent` 单体函数主导);文档 hub 全部 <500、parts 全部 <1200。
- **Review P1/P2/P3 全部修正**:7 项问题逐一落地(见下表)。
- **AC4 文档结构**:6 个 hub + 49 个 part,49 个 hub→part 链接**零失效**(逐条核验)。

**唯一遗留瑕疵**:AC3 有 1 处 spec 内失效行号引用未覆盖(见 🔴 下文),建议修。

## ✅ Review 评审意见的执行核验

| 评审项 | 状态 | 核验证据 |
|---|---|---|
| 🔴 P1-1 `tests_group_chat.rs` 已存在冲突 | ✅ 已修 | 新建 `tests_group_chat_prompts.rs`(1029 行),原 `tests_group_chat.rs`(919 行)保持不动,正确避冲突 |
| 🔴 P1-2 `resolve_project_id`/`create_worker_worktree` 可见性 | ✅ 已修 | 两者均升 `pub(crate)`(`resolve.rs:187`、`worktree.rs:104`) |
| 🟡 P2-1 范围统计口径("9 个") | ✅ 已修 | PRD Background 改为"首批档位 1+2 的 8 个候选"+ 列出 ~14 个后续文件 |
| 🟡 P2-2 引用计数口径 | — | (R4 执行时已实际处理,口径问题不再阻塞) |
| 🟡 P2-3 `request_mode_change` Scenario 去重 | ✅ 已修 | part 11 现仅 1 个 Scenario 标题(原 2 个重复已合并) |
| 🟢 P3-1 `state-management.md` 空模板 | ✅ 已修 | 51→73 行,新增 9 处 Stream Controller 内容 |
| 🟢 P3-2 RULE-A-006 引用点 | ✅ 已修 | check.jsonl 已补 |

## ✅ AC 逐项核验

| AC | 状态 | 证据 |
|---|---|---|
| AC1 源码 <1200 行 | ✅(7/8 + 1 豁免) | wire/{mod,types,to_wire,from_wire}、group_chat_{loop,prompts}、loader/frontmatter/cache、openai/streaming、resolve/worktree/prep、streamController/streamRehydrate、messageCards/* 均 <1200;`dispatch.rs` 1472 行透明豁免(`run_subagent` 1312 行单体主导);`streamEvents.ts` 恰好 1200(见边界说明) |
| AC2 纯搬迁 | ✅ | 抽查 `chat_request_to_wire` 函数体 67 行 = 原文件 67 行;`moderator_system_prompt` 仅可见性 `pub`→`pub(crate)`;R1.3 return 块 6 项全保留(`events.handleChatEvent` 等 re-export) |
| AC3 无失效引用 | ⚠️ 基本达标 | 主要路径已更新(ARCHITECTURE.md 已改 `group_chat_prompts.rs`);**遗留 1 处 spec 失效行号**(见 🔴) |
| AC4 文档 <1200 + index 一致 | ✅ | 6 hub 均 <500(最大 83),49 part 均 <1200(最大 999),hub→part 49 链接零失效 |
| AC5 trellis-check 通过 | ✅(等价) | 后端 1657/0、前端 1018/0、clippy 0、fmt 0 全绿 |

## ⚠️ 遗留问题

### 🔴 唯一需修:`spec/frontend/chat.md` 仍引用 streamController.ts 失效行号【✅ 已于后续 commit 修正】

**位置**:`.trellis/spec/frontend/chat.md:833, 850, 851`(明确在 R4 范围内——是 spec)。

**问题**:该 spec 引用 `streamController.ts:1364`、`:1381`、`:1400` 具体行号,但:
- `streamController.ts` 现仅 **1121 行**,这些行号已超出文件边界;
- `handleToolCall` 等函数已迁到 `streamEvents.ts:685`。

属 AC3("全仓 grep 确认无文档仍引用被搬走的旧路径/旧行号")的**未覆盖项**。

**建议**:把这三处行号引用改为符号引用(删行号,保留函数名,或指向 `streamEvents.ts`)。例:`handleToolCall`(`streamController.ts:1364`)→ `handleToolCall`(`streamEvents.ts`,经 streamController re-export)。

**附带**:`docs/IMPLEMENTATION/decisions-2026-07.md:222` 引用 `streamController.ts:1216-1222`,行号也已失效。决策日志性质偏历史,但属活跃 docs/,建议同步。

### 🟢 边界说明:`streamEvents.ts` 恰好 1200 行

AC1 字面是 "<1200",`streamEvents.ts` 实测**恰好 1200**。严格按字面是边界违反(等于)。但该文件是单一工厂函数 `createStreamEventHandlers`,结构内聚,再拆意义不大。建议要么接受,要么把 AC1 措辞改为 "≤1200"。

### 🟢 文档行数小误差

PRD AC1 写 `dispatch.rs` 豁免理由为 "1467 行",实测 **1472 行**(差 5 行)。非功能问题,但豁免记录的数字未与最终状态同步,建议核对后修正。

## ✅ 做得好的地方

1. **测试可见性处理干净**:所有迁出函数按 R1.2 升 `pub(crate)`,无公开 API 泄漏,无 `pub` 滥用。
2. **hub+parts 执行到位**:原路径全保留(引用不失效),parts 命名一致(`NN-name` / `scenario-*` / `pattern-*`),hub 总览+索引结构清晰。
3. **豁免透明**:`dispatch.rs` 超标未隐瞒,在 PRD AC1 显式记录理由 + 用户批准,与 `chat_loop.rs` 同策略,可追溯。
4. **额外收益**:顺带填实了 `state-management.md`(P3-1),不只拆分还补了真实 spec。
5. **commit 粒度合理**:8 个 refactor 各自独立可 revert,顺序按 design(最干净→最复杂),每个 commit message 含拆分明细。

## 修正建议优先级

| 级别 | 项 | 必修? |
|---|---|---|
| 🔴 | `spec/frontend/chat.md` + `IMPLEMENTATION/decisions-2026-07.md` 的 streamController.ts 失效行号 | 是(AC3 遗漏) |
| 🟢 | `streamEvents.ts` 1200 行边界 → 改 AC 措辞为 ≤1200 或接受 | 否 |
| 🟢 | PRD AC1 的 dispatch.rs "1467" → "1472" 同步 | 否 |
