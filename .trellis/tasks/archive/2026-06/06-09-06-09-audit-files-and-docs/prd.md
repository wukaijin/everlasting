# Step 8 — 代码重构与文档清理 (基于 audit + Opus 融合)

> Drafted: 2026-06-09 by Carlos + Claude (MiniMax-M3 session)
> Status: 融合 Opus 评审后, 准备 curate jsonl + task.py start
> Inputs:
> - 本地 audit: `.trellis/workspace/Carlos/audit-2026-06-09/{00-06}.md`
> - Opus 评审: `docs/_reviews/REVIEW-claude-opus-2026-06-09.md` (commit `db711c5`)
> - 融合结论: `.trellis/workspace/Carlos/audit-2026-06-09/06-synthesis-vs-opus.md`

## Goal

执行**新增 Step 8「代码重构与文档清理」**——在堆新功能（MCP 暴露 / 嵌入式终端 / daemon 化）之前，先把现有 6 个 >800 行文件 + 文档体系清理到位。Step 3b-2（rig-core 迁移）随本任务**正式废弃**，Step 5（WSL 体验优化）**降为可选**，腾出路线图空间。

## 路线图变更（采纳 Opus）

```
已完成:  1 ✅  2 ✅  3a ✅  3b-1 ✅  4 ✅  6a-多 Provider ✅
当前:    → Step 8: 代码重构与文档清理 ← 新增
暂缓:    Step 5 (WSL 体验, 降为可选)
废弃:    Step 3b-2 (rig-core 迁移) ← 弃用
远期:    Step 7 (daemon 化, v2 之后)
未来:    Step 6b (MCP 暴露, v1) + Step 5b (权限系统, v1)
```

## Scope (5 PR 串行)

| PR | 内容 | 新文件 | 工作量 |
|----|------|-------|--------|
| **8-PR1** | lib.rs 拆分 → `state.rs` + `commands/{config, providers, sessions, worktree, projects, cancel}.rs` + `agent/{chat, provider, system_prompt, thinking, helpers, tests}.rs` | 13 个 | 2-3h |
| **8-PR2** | db.rs 拆分 → `db/{mod, types, migrations, projects, sessions, providers, models, config, tests}.rs` | 9 个 | 1-2h |
| **8-PR3** | 前端组件拆分（ChatPanel → WorktreeChip + DiffModal；ModelsTab → ModelRow + ModelForm + DeleteModelConfirm）| 5 个新组件 | 2h |
| **8-PR4** | 文档更新（README/TECH/DESIGN/HANDOFF/BACKLOG/IMPLEMENTATION 校正到 06-09 状态） + 删除 9 个空 spec 文件 | 9-10 个文件改 + 9 个文件删 | 2h |
| **8-PR5** | 创建根目录 `STRUCTURE.md`（项目结构全景图）+ 拆 `llm-contract.md` 为 5 子文件 | 6 个新文件 | 1h |

**总工作量**: ~8-10h

## 显式不做 (Negative Scope)

| 项 | 不做的理由 |
|----|----------|
| **不拆 `wire.rs`** (1109 行) | 47% tests 比例（18 个 `#[test]`），拆解会破坏测试 1:1 镜像契约 |
| **不拆 `streamController.ts`** (796 行) | 高内聚 + 11 个 `it()` tests 覆盖；观察 06-09 stream bug 后续 |
| **不填实 9 个空 spec 文件** | 直接删除更减负；如需要从 git 历史恢复 |
| **不动 rig-core 迁移** | Step 3b-2 已废弃（自研 Provider trait + wire layer 已完整支持） |
| **不动 Step 5 WSL 体验** | 降为可选，路线图腾位给 Step 8 |

## Acceptance Criteria

- [ ] 8-PR1：`cargo check && cargo test --lib` 通过，`lib.rs` < 300 行，`commands/` + `agent/` 各文件 < 700 行
- [ ] 8-PR2：`cargo check && cargo test --lib` 通过，`db/mod.rs` < 100 行（仅 re-exports）
- [ ] 8-PR3：`pnpm build`（vue-tsc + vite）通过，`ChatPanel.vue` < 500 行，`ModelsTab.vue` < 400 行
- [ ] 8-PR4：路线图反映 Step 8 当前进行中 + Step 3b-2 废弃 + Step 5 降为可选；9 个空 spec 文件已删
- [ ] 8-PR5：根目录 `STRUCTURE.md` 创建，CLAUDE.md/README.md 引用更新；`llm-contract.md` 拆为 5 子文件
- [ ] 端到端：5 PR 全部合并后手动发一条消息无回归

## Definition of Done

- 5 PR 全部合并（按 8-PR1→5 顺序串行，每 PR 独立 commit + push + 验证）
- `cargo check && cargo test --lib` 全过
- `pnpm build`（vue-tsc + vite）全过
- 手动 e2e：发一条消息能正常流式
- `git log --oneline -30` 反映 5 PR 落地

## Technical Approach

### 8-PR1 lib.rs 拆分（关键设计）

**目标结构**：
```
src-tauri/src/
├── lib.rs              # ~120 行: mod 声明 + run() 入口 + init_tracing()
├── state.rs            # ~160 行: AppState, CancellationGuard, 事件 Payload
├── commands/           # IPC 表面（薄）
│   ├── mod.rs
│   ├── config.rs       # get_llm_config, get_home_dir
│   ├── providers.rs    # Provider/Model CRUD + test_model
│   ├── sessions.rs     # Session CRUD + diff_worktree
│   ├── worktree.rs     # attach/detach/delete + cancel_inflight
│   ├── projects.rs     # Project CRUD + pick_project_dir
│   └── cancel.rs       # cancel_chat
└── agent/              # 业务核心（厚）
    ├── mod.rs
    ├── chat.rs         # chat 命令 (Agent Loop 主函数) ~700 行
    ├── provider.rs     # resolve_chat_provider + PreFlightError
    ├── system_prompt.rs # build_system_prompt, lookup_head_sha
    ├── thinking.rs     # PendingThinking, flush_pending_thinking
    ├── helpers.rs      # tool_result_envelope, persist_turn_cwd, emit_chat_event
    └── tests.rs        # 全部 inline 测试
```

**关键原则**（Opus 提）：
- `commands/` 各文件只做 IPC 命令分发，不含业务逻辑
- `agent/` 是核心 Agent Loop，保持高内聚
- `state.rs` 是跨模块共享状态，避免循环依赖
- 每 PR 后 `cargo check && cargo test --lib` 验证

### 8-PR2 db.rs 拆分

**目标结构**：
```
src-tauri/src/db/
├── mod.rs          # ~30 行: pub re-exports + init_pool + run_migrations
├── types.rs        # ~230 行: 所有 row 类型、WorktreeState、ProviderProtocol
├── migrations.rs   # ~410 行: Schema 创建、ALTER TABLE 迁移、列探测
├── projects.rs     # ~290 行: Project CRUD
├── sessions.rs     # ~310 行: Session CRUD + worktree state + persist_turn
├── providers.rs    # ~160 行: Provider CRUD
├── models.rs       # ~190 行: Model CRUD
├── config.rs       # ~200 行: app_config KV + seed
└── tests.rs        # ~930 行: 全部测试
```

### 8-PR3 前端组件拆分

**ChatPanel.vue (957L) 拆解**：
- `WorktreeChip.vue` ~350 行（template + script + style，三态 chip + dropdown + clipboard）
- `DiffModal.vue` ~100 行（diff overlay 包装）
- 剩余 ChatPanel ~500 行（纯消息列表 + 输入框）

**ModelsTab.vue (954L) 拆解**：
- `ModelRow.vue` ~200 行（行显示 + 测试按钮 + 内联测试结果）
- `ModelForm.vue` ~350 行（Add/Edit 表单）
- `DeleteModelConfirm.vue` ~40 行
- 剩余 ModelsTab ~360 行（薄编排）

### 8-PR4 文档更新 + 9 个空 spec 删除

**文档更新**：
- `README.md`：加状态段（Step 8 进行中）+ STRUCTURE.md 引用
- `TECH.md`：rig-core 改"不采用"（采纳 Opus）
- `DESIGN.md`：MVP checklist 勾选 edit_file/grep/glob
- `HANDOFF.md`：更新到 06-09 状态（multi-provider 4 PR + 工具集扩展批次 + 6 个 bug 修复）
- `BACKLOG.md`：v3+ 移到附录，添加"已落地方向"段
- `IMPLEMENTATION.md`：§2.4 3b-2 标废弃，§2.7 多 Provider 标 ✅，§3 重写
- `CLAUDE.md`：Tech Stack 表校正（rig-core 改"未采用"）

**9 个空 spec 文件删除**：
- `.trellis/spec/backend/{directory-structure, quality-guidelines, logging-guidelines}.md`
- `.trellis/spec/frontend/{component-guidelines, directory-structure, quality-guidelines, hook-guidelines, type-safety}.md`
- 对应 `index.md` 移除引用

### 8-PR5 STRUCTURE.md + llm-contract 拆

**STRUCTURE.md**（根目录）：
- 13 节：顶层 / 前端 / 后端 / 依赖 / IPC 表面 / DB schema / 设计模式 / 数据流 / 环境变量 / 文档地图 / 一页式 ASCII / 维护建议 / 关系
- 来自 audit 04

**llm-contract.md 拆分**（Opus 提议）：
```
.trellis/spec/backend/
├── llm-contract.md           # ~400 行: 核心类型 + thinking 契约 + 反模式汇总
├── tool-contract.md          # ~300 行: 工具定义、ReadGuard、shell spill
├── worktree-contract.md      # ~400 行: attach/detach/delete + cancel + system prompt
├── multi-provider-contract.md # ~400 行: Provider trait + catalog + Anthropic/OpenAI 分发
└── test-model-contract.md    # ~100 行: test_model IPC
```

## Decision (ADR-lite)

**Context**: 项目运行至 2026-06-09，6 个源码文件 >800 行（lib.rs 3195 / db.rs 2862 / openai.rs 1150 / wire.rs 1109 / ChatPanel.vue 957 / ModelsTab.vue 954），god-module 风险已显化。同时文档体系（README/TECH/DESIGN/HANDOFF/BACKLOG/IMPLEMENTATION）部分滞后于实际进度（multi-provider 4 PR 实际已落地但文档未更新）。Claude Opus 4.8 在 2026-06-09 独立评审中提议新增 Step 8 作为当前最高优先级，融合本地 audit 与 Opus 评审后确认采纳。

**Decision**: 
1. 路线图加 Step 8（代码重构 + 文档清理），5 PR 串行，~8-10h 工作量
2. Step 3b-2（rig-core 迁移）正式废弃
3. Step 5（WSL 体验）降为可选
4. 9 个空 spec 文件删除（不填实）
5. wire.rs + streamController.ts 不拆（47%/11 个 it tests 锁定契约）
6. STRUCTURE.md 在根目录（更显眼）

**Consequences**:
- ✅ god-module 风险消除，IPC 表面与业务核心物理隔离
- ✅ 路线图与实际进度同步
- ✅ 测试 1:1 镜像契约不被破坏（wire.rs / streamController.ts）
- ⚠️ 5 PR 工作量 ~8-10h，需 1-2 周串行落地
- ⚠️ 路线图重排后，6b MCP / 5 终端等新功能推迟到 Step 8 完成后
- ⚠️ 删除 9 个空 spec 后，错误处理 / 日志等"应有"内容需在 STRUCTURE.md 章节中补充或开新 PR

## Out of Scope

- 实际执行 5 PR（由实施阶段完成，本任务仅规划）
- 6b MCP 暴露 / 5 嵌入式终端 / 5b 权限系统（Step 8 完成后开新 Trellis 任务）
- 3b-2 完整三栏 UI（已废弃，跟 rig-core 迁移一起）
- 4b Auto-commit（延后 P2）
- 6a-Ollama provider（延后 P2）
- 8 — Daemon 化评估窗口（P3，Step 8 完成后评估）

## PR-A Grill Decisions (锁定 2026-06-09)

| # | 决策 | 选择 | 理由 |
|---|------|------|------|
| 1 | CancellationGuard 归属 | **留在 state.rs** | 跟 AppState 生命周期绑定, 同文件修改窗口一致 |
| 2 | AppState 字段顺序 | **重排 + 接受 breaking change** | 现在动, 以后不动; 跨模块调用点一起改 |
| 3 | Provider catalog 初始化时机 | **8-PR1 同时初始化 catalog** | `AppState::load()` 调 `init_llm_client()` 时从 `db::list_providers()` 读全部 provider, 构建 `ProviderCatalog (HashMap<provider_id, Arc<dyn Provider>>)`. 8-PR1 完成时 chat command 能直接按 model_id 查 Provider |
| 4 | init_tracing 位置 | **抽到 main.rs** | tracing 是进程初始化关注点, 跟 GUI 进程生命周期绑定; `lib.rs::run()` 只调 run 套路 |
| 5 | 9 个空 spec 删除后"应有"内容 | **由 STRUCTURE.md §13 替代** | STRUCTURE.md 新增详节 (错误处理 / 日志 / 质量门 / 组件规约 / hooks 模式) 填这些; spec 骨架不复活 |

## Decisions Locked (Phase 1 收尾)

7 个 Q&A + 5 个 PR-A grill + Opus 7 个采纳点 + 路线图重排 = **19 个决策已锁**

## Technical Notes

- 审计包: `.trellis/workspace/Carlos/audit-2026-06-09/{00-06}.md` (7 docs, 2500+ lines)
- Opus 评审: `docs/_reviews/REVIEW-claude-opus-2026-06-09.md` (committed in `db711c5`)
- 路线图原 7 步 → 8 步（+Step 8，-Step 3b-2，Step 5 降级）
- 主要参考：本地 audit 02/03/04/05（事实细节密度）+ Opus 评审（结论决断力）
- 6 个 >800 行文件中 2 个不拆（wire.rs / streamController.ts），4 个拆
- 9 个空 spec 文件删除（采纳 Opus）
- STRUCTURE.md 在根目录（采纳 Opus，替代本地 audit 04 的 `docs/ARCHITECTURE-codebase.md`）
