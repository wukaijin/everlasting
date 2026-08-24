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

## P1 — 重要(正确性 + 资源) [2 items]

### RULE-DB-001

- **Level**: P1
- **Subsystem**: DB
- **File**: `app/src-tauri/src/db/`(全层 + `migrations/`,无 backup / VACUUM INTO / 迁移前副本)
- **Description**: 全库数据(会话 / 消息 / 自主记忆 / 审计 / token 用量)仅单一 `everlasting.db`(实测 15M),零备份机制;一次坏盘 / 误删 / migration 写坏即数据归零
- **Fix**: daemon 启动或每日定时 `VACUUM INTO` 快照到 `~/.local/state/dev.everlasting.app/backups/`,保留最近 N 份(~50 行)
- **Owner**: carlos
- **Related Task**: null
- **Discovered In**: 2026-08-24 harness 缺口评估会话(非 formal review 文件)

### RULE-PERSIST-001

- **Level**: P1
- **Subsystem**: Agent Loop
- **File**: `app/src-tauri/src/agent/chat_loop/tools.rs:1779`(`finalize_turn` — 消息仅 turn 结束一次性落库,流式过程无逐块持久化;无 `run_status` / in_progress 启动恢复)
- **Description**: daemon 崩溃(kill -9 / 断电 / OOM)时当前 turn 已流式内容全部丢失,reload 后只剩半个 user 消息
- **Fix**: turn 开始落 `status=in_progress` 占位 + 流式过程周期性 checkpoint(每 N 块 upsert),启动检测 in_progress 恢复或提示(~150 行)
- **Owner**: carlos
- **Related Task**: null
- **Discovered In**: 2026-08-24 harness 缺口评估会话(非 formal review 文件)

## P2 — 健壮性 + 债务,中长期清理 [2 items]

### RULE-DAEMON-001

- **Level**: P2
- **Subsystem**: Daemon
- **File**: `scripts/daemon.sh:36`(`LOG_FILE=/tmp/everlasting-daemon.log`)+ `app/src-tauri/src/main.rs:4`(tracing stdout/stderr,无 RollingFile)
- **Description**: daemon 日志单文件写死 `/tmp`,重启即覆盖(排障永远缺"出事前"的日志)且单文件无限增长无轮转
- **Fix**: 日志移到 `~/.local/state/dev.everlasting.app/daemon.log` + 大小 / 日期轮转(tracing-appender 或启动时 mv 旧档)(~30 行)
- **Owner**: carlos
- **Related Task**: null
- **Discovered In**: 2026-08-24 harness 缺口评估会话(非 formal review 文件)

### RULE-CI-001

- **Level**: P2
- **Subsystem**: Cross
- **File**: `.github/workflows/ci.yml`(仅 cargo fmt --check + cargo test --lib,无 clippy)
- **Description**: E1(07-05)"clippy 留 follow-up(先本地清 warning 再加 gate)"至今未兑现;此后 +400 测试,clippy warning 回潮无机器把关
- **Fix**: 本地清 warning 后 CI 加 `cargo clippy -- -D warnings`(或先 `-- -W clippy::all` 观察)(~10 行 + 一次本地清理)
- **Owner**: carlos
- **Related Task**: null
- **Discovered In**: 2026-08-24 harness 缺口评估会话(非 formal review 文件)

## P3 — 轻微(文档/一致性) [0 items]

_无 open 项(RULE-B-007 + RULE-C-008 已 closed, 详见 git log)。_


---

## 优先级分布

| Level | Count | 说明 |
|---|---|---|
| P0 | 0 | 全部 closed(详见 git log) |
| P1 | 2 | 正确性 + 资源(数据丢失风险) |
| P2 | 2 | 健壮性 + 债务,中长期清理 |
| P3 | 0 | 文档 + 一致性,可延后 |
| **Total** | **4** | 当前 open items |

---

## 维护说明

- **每次 audit 必须 diff 本文件** 第一步
- **每次 PR merge 必须从本文件删除已 closed 债项**(本文件 = open 集合,通过 `git log` 追溯 closed)
- **每条 finding 闭合后从本文件删除**(无 status 字段,文件存在即 open)
- **子 task 创建时在本文件 Related Task 字段填 task 路径**
- **下次 audit 模板**: 第一段写 "DEBT.md diff 结果",已记录 finding 仅引用,新 finding 按模板加入
