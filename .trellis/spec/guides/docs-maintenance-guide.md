# Docs Maintenance Thinking Guide

> 维护 `docs/` 与根目录 markdown 时的思考清单。教训来源：08-10 docs 过时审查任务（拆分/移动/归档后遗留 28 死锚 + 9 路径错误 + 12 条错卷条目）。

## When to Use

- 拆分/移动/重命名/归档任何 markdown 文档
- 批量更新文档事实（数字、路径、状态头）

## Checklist

### 1. 拆分 / 移动文档后，同批修相对路径

- [ ] 移动后所有相对链接补/减一层 `../`（`./DESIGN.md` → `../DESIGN.md`；`../.trellis/…` → `../../.trellis/…`）
- [ ] 标题状态后缀（如 `## 2 分类(2026-06-10 重排)`）会改变 GitHub 锚点 → 同步修所有 `#锚点` 引用，或把后缀移入正文
- [ ] 拆分后原文件变成 hub：列全 part 索引，`##` 级锚点失效的旧引用全部重定向
- [ ] **不要相信"上次已修过链接"的提交**——08-07 拆分后 d8be89f 声称修完，实际仍残留 28 死锚 + 9 路径错误，直至 08-10 审查才清零

### 2. 用脚本验证链接，不靠目测

```bash
python3 .trellis/tasks/08-10-docs-staleness-audit/scripts/check-links.py
# 复用方法：扫描范围 md 的相对链接 + 锚点可解析，0 失败
```

- [ ] 归档前 grep 全项目对旧路径的引用，列清单后一并修
- [ ] 归档后验证无活文档指向旧路径

### 3. 决策日志已退役(2026-09-13)

- `docs/IMPLEMENTATION/` 决策日志(按月分卷 ADR)已于 2026-09-13 停止维护并删除;历史 ADR 查 git 历史(`git log -- docs/IMPLEMENTATION/`)
- 设计决策改按领域沉淀到 `.trellis/spec/` 对应 spec 文件

### 4. 事实更新交叉验证

- [ ] 改数字前用代码实测（`grep -c "#\[tauri::command\]"` / 枚举变体数 / Cargo.toml 依赖），不信任旧文档互引
- [ ] 同义多文档间事实（工具数、AuditKind 类数、handler 数）以代码为准，一次全改
- [ ] 日期锚定的历史 ADR 中的旧数字**保留**（历史事实），只改描述"现状"的段落

### 5. 索引与归档

- [ ] 新增/归档文档后同步 `docs/README.md` 索引表（顶层活文档 vs `ls docs/*.md` 一一对应）
- [ ] 一次性/已消费文档归档到 `docs/_history/<YYYY-MM-DD>-<原名小写>.md`（沿用日期前缀惯例），不删除
- [ ] 历史目录 `docs/_history/`（含 spikes/ research/ deprecated/ reviews/ 子目录 + 日期前缀归档件）是记录性质，不做"新鲜度"审查；活文档一律不指向 `_history/` 之外的旧归档路径

## Related

- [docs/README.md](../../../docs/README.md) — 文档索引（本文档维护目标）
- check-links.py 脚本：`.trellis/tasks/08-10-docs-staleness-audit/scripts/check-links.py`
