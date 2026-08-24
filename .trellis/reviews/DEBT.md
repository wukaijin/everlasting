# Review Backlog — 跨 review 债务整合

> ## ⚠️ 强制约定(2026-06-23 强化)
>
> 本文件**只记录当前 open 技术债**。
>
> - finding 解决后必须**从本文件删除**(通过 `git log` 追溯,不在此保留 closed 条目)
> - **严禁**记录任何日志 / 流水 / 决策历史 / 降级说明 / 收尾路径建议 / 子 task 编排 / Feature Follow-ups / Re-evaluation Log / 历史最后更新
> - 上述内容走 journal 或独立 spec 文档,**不允许污染本文件**

---

> **目的**: 集中追踪所有 review(审计 / SPEC 对照 / 历史 review)的 finding,避免下次 audit 重新独立复述
>
> **基线审计**: `docs/_reviews/REVIEW-agent-loop-full-audit-2026-06-14.md`(commit `a4fb302`)
>
> **创建**: 2026-06-14(由 `.trellis/tasks/06-14-review-debt-consolidation` 启动)

---

## 新增 finding 流程

> **重要**: 任何新 audit / review / spec 对照,**第一步必须 diff 本文件**。

### 添加新 finding

```markdown
### RULE-{Subsystem}-{Seq}

- **Level**: P0 | P1 | P2 | P3
- **Subsystem**: Agent Loop | Permission | Memory | Provider | Tools | Cross
- **File**: `path/to/file.rs:LINE`
- **Description**: 一句话描述问题
- **Fix**: 修复方向(行数估算)
- **Owner**: carlos | 待分配
- **Related Task**: `.trellis/tasks/XX-YY-name` 或 null
- **Discovered In**: `docs/_reviews/REVIEW-XXX.md`
```

### 流程规则

1. **不重新展开已记录 finding**: 新 audit 中遇到已记录的 RULE-X-XXX,**只标一行** `// See DEBT.md §RULE-X-XXX`,不重新描述 file:line 和影响
2. **闭合时**: PR merge 后从本文件删除该 finding;通过 `git log` 追溯;**本文件 = open 集合**
3. **优先级重审**: 每次 audit 可重新评估,如需降级或合并,在 finding 描述中注明理由
4. **ID 一旦分配不变**: 即使 finding 后续证明不是问题,ID 不重新分配(已删除的 ID 可留空)

### 复述检测

如果新 audit 复述了某条 finding 但未引用 DEBT.md:
- **轻度**: review 本身不扣分,但应在结论段标注"漏查 DEBT.md"
- **重度**: 如果是 P0/P1 漏查,review 应被打回修订

---

> **本文件仅记录当前 open 债项**。已 closed 条目不在此保留;通过 git log 追溯。

## P1 — 重要(正确性 + 资源) [0 items]

> 全部 closed(RULE-PERSIST-001 于 2026-08-24 由 `.trellis/tasks/08-24-p1-turn-crash-recovery` 闭合,详见 git log)。

## P2 — 健壮性 + 债务,中长期清理 [4 items]

### RULE-CI-001

- **Level**: P2
- **Subsystem**: Cross
- **File**: `.github/workflows/ci.yml`(仅 cargo fmt --check + cargo test --lib,无 clippy)
- **Description**: E1(07-05)"clippy 留 follow-up(先本地清 warning 再加 gate)"至今未兑现;此后 +400 测试,clippy warning 回潮无机器把关
- **Fix**: 本地清 warning 后 CI 加 `cargo clippy -- -D warnings`(或先 `-- -W clippy::all` 观察)(~10 行 + 一次本地清理)
- **Owner**: carlos
- **Related Task**: null
- **Discovered In**: 2026-08-24 harness 缺口评估会话(非 formal review 文件)

### RULE-FM-001

- **Level**: P2
- **Subsystem**: Tools
- **File**: `app/src-tauri/src/resource_loader.rs:160/195`、`app/src-tauri/src/skill/loader/frontmatter.rs:48/79`、`app/src-tauri/src/agent/subagent/frontmatter.rs:85/116`
- **Description**: frontmatter 解析器 3 份复制(`parse_frontmatter` + `apply_kv` 逐字相同,仅字段不同;`parse_tools_array` / `parse_allowed_tools` 并存);测试断言也复制(`apply_kv_ignores_comments_blank_unknown` 在 resource_loader.rs:558 + skill/tests_loader.rs:55 两处)——一个行为变更 = 改 3 实现 + ≥3 测试
- **Fix**: 在 resource_loader 之上抽泛型 `parse_md_resource<T>(content, T::default, T::apply_kv)`,三 loader 各留 `apply_kv` + 字段定义(~半天)
- **Owner**: carlos
- **Related Task**: null
- **Discovered In**: 2026-08-24 群聊 session `702e6ec8…`(讨论本项目不足,代码事实验证)

### RULE-TESTPOOL-001

- **Level**: P2
- **Subsystem**: Cross
- **File**: `app/src-tauri/src/`(grep "fn test_pool" 命中 15 处:db/sessions_tests/mod.rs:23、db/subagent_runs_tests.rs:28、db/messages_tests.rs:22、projects/store.rs:176、db/memories_tests/mod.rs:23、commands/tests_resolve_mode_change.rs:56、agent/tests_common.rs:157、tools/tests_merge_worker.rs:15、db/search_tests.rs:18、db/usage_tests.rs:12、db/providers_tests.rs:21、db/permissions_tests.rs:24、db/trace.rs:498、db/projects_tests.rs:23、agent/subagent/tests_dispatch.rs:255)
- **Description**: in-memory 测试池构建函数 15 处手写复制(connect + PRAGMA foreign_keys + migrations::run),migrations 变更要同步 15 处
- **Fix**: 抽 `db/test_support.rs` 共享 `test_pool()`,15 处改调用(~120 行净删)
- **Owner**: carlos
- **Related Task**: null
- **Discovered In**: 2026-08-24 群聊 session `702e6ec8…`(讨论本项目不足,代码事实验证)

### RULE-ARGS-001

- **Level**: P2
- **Subsystem**: Agent Loop
- **File**: `app/src-tauri/src/agent/chat_loop.rs:319`(`run_chat_loop` 18 参)、`chat_loop/drive.rs:82`(drive_turn)、`chat_loop/tools.rs:1779`(finalize_turn);grep "too_many_arguments" 全库 43 处
- **Description**: 项目穿状态的方式是线性参数管道(每次新 feature 往既有签名追加参数 + 注释块);chat_loop 已物理拆成 `chat_loop/{drive,init,tools}.rs` 但超长签名原样存活在子文件——拆分只做了形式没做实质
- **Fix**: parameter object 重构(provider / cancel / cache / subagent 套件聚类),目标 43 处非单函数(中等 epic 量级)
- **Owner**: carlos
- **Related Task**: null
- **Discovered In**: 2026-08-24 群聊 session `702e6ec8…`(讨论本项目不足,代码事实验证)

## P3 — 轻微(文档/一致性) [1 items]

### RULE-DOC-001

- **Level**: P3
- **Subsystem**: Cross
- **File**: `app/src-tauri/src/agent/chat_loop/drive.rs:107`(参数注释块 `08-20-turn-usage-event-quota-view WP2` 等)、`docs/CONTEXT.md`(与 CLAUDE.md "当前状态" 段重复)
- **Description**: 参数注释块把 git log 已记录的 feature 名 + 日期 + commit hash 重复进代码注释,形成双 source of truth(注释会被 feature 重命名牵动,gir log 是只读稳定副本);CLAUDE.md "当前状态"段与 ROADMAP 重复且每轮注入付 token 税
- **Fix**: 参数注释收敛为一句用途说明,历史走 git log;CLAUDE.md 状态段改派生生成(git log / 代码现状 / 既有 memory 管道)
- **Owner**: carlos
- **Related Task**: null
- **Discovered In**: 2026-08-24 群聊 session `702e6ec8…`(讨论本项目不足,代码事实验证)


---

## 优先级分布

| Level | Count | 说明 |
|---|---|---|
| P0 | 0 | 全部 closed(详见 git log) |
| P1 | 0 | 全部 closed(RULE-PERSIST-001 2026-08-24 闭合) |
| P2 | 4 | 健壮性 + 债务,中长期清理 |
| P3 | 1 | 文档 + 一致性,可延后 |
| **Total** | **5** | 当前 open items |

---

## 维护说明

- **每次 audit 必须 diff 本文件** 第一步
- **每次 PR merge 必须从本文件删除已 closed 债项**(本文件 = open 集合,通过 `git log` 追溯 closed)
- **每条 finding 闭合后从本文件删除**(无 status 字段,文件存在即 open)
- **子 task 创建时在本文件 Related Task 字段填 task 路径**
- **下次 audit 模板**: 第一段写 "DEBT.md diff 结果",已记录 finding 仅引用,新 finding 按模板加入
