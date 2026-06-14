# Reviews Index

> 项目级 review 跟踪文件索引

## 文件清单

| 文件 | 目的 | 当前 finding 数 |
|---|---|---|
| [DEBT.md](./DEBT.md) | 代码层 finding 跟踪(跨 review 债务整合) | 36 open + 1 wontfix |
| [SPEC-DRIFT.md](./SPEC-DRIFT.md) | spec 与实现偏离追踪(spec drift) | 2 active + 4 待审 |

## 使用流程

### 给 reviewer / 下次 audit

1. **第一步**: diff 本目录两个文件
2. **第二步**: 已记录的 finding / drift 仅引用,不重新展开
3. **第三步**: 新 finding 按 DEBT.md 顶部"新增 finding 流程"段添加
4. **第四步**: 新 drift 按 SPEC-DRIFT.md 顶部"分类"段添加

### 给实施者(PR 关闭 finding)

1. PR merge 后立即更新对应 RULE 条目的:
   - `Status`: open → closed
   - `Closed At`: commit hash
   - `Related PR`: #N
2. 如发现优先级需重评,在 `Re-evaluation Log` 段记录

### 给 spec 维护者

1. 修订 spec PR 必须先在 SPEC-DRIFT.md 标"resolved by PR #N"
2. spec 文档修订完成后,本文件条目移到 "历史已解决" 段

## 相关元任务

- `.trellis/tasks/06-14-review-debt-consolidation/` — review 整合发起 task
- `docs/_reviews/REVIEW-agent-loop-full-audit-2026-06-14.md` — 当前基线审计
- `docs/_reviews/REVIEW-sse-agent-loop-2026-06-12.md` — 历史 review(债务源)
- `docs/_reviews/REVIEW-a2-b7-permission-mode-plan-2026-06-13.md` — 历史 review(大部分已解决)
- `docs/_reviews/REVIEW-b5-memory-grill-2026-06-10.md` — 历史 review(7/9 已解决)

---

**创建**: 2026-06-14 by carlos