# Docs Maintenance Thinking Guide

> 维护 `docs/` 与根目录 markdown 时的思考清单。教训来源：08-10 docs 过时审查任务（拆分/移动/归档后遗留 28 死锚 + 9 路径错误 + 12 条错卷条目）。

## When to Use

- 拆分/移动/重命名/归档任何 markdown 文档
- 批量更新文档事实（数字、路径、状态头）
- 向决策日志追加条目

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

### 3. 决策日志按月分卷纪律

- `docs/IMPLEMENTATION/decisions-YYYY-MM.md` 是**按月份分卷的只追加日志**
- [ ] 新条目写进**条目日期对应**的分卷，不是当前编辑的分卷（06 卷曾堆积 12 条 07/08 月条目）
- [ ] 错卷归位：按日期移回正确分卷，原卷留一行"X 条已归位（日期）"备注；git 全程可回溯

### 4. 只追加日志禁用字符级脚本编辑

- [ ] 对 decisions 卷做编辑用 Edit/Write 工具整块替换，**不要**用脚本做字符拼接——08-10 曾一次拼接事故损坏整个 07 卷，靠 `git checkout` 恢复重做
- [ ] 事后验证：`git diff` 逐条核对无文本丢失（对比条目数 + 关键行字节一致）

### 5. 事实更新交叉验证

- [ ] 改数字前用代码实测（`grep -c "#\[tauri::command\]"` / 枚举变体数 / Cargo.toml 依赖），不信任旧文档互引
- [ ] 同义多文档间事实（工具数、AuditKind 类数、handler 数）以代码为准，一次全改
- [ ] 日期锚定的历史 ADR 中的旧数字**保留**（历史事实），只改描述"现状"的段落

### 6. 索引与归档

- [ ] 新增/归档文档后同步 `docs/README.md` 索引表（顶层活文档 vs `ls docs/*.md` 一一对应）
- [ ] 一次性/已消费文档归档到 `docs/_archive/<YYYY-MM-DD>-<原名小写>.md`（沿用日期前缀惯例），不删除
- [ ] 历史目录（_archive/_deprecated/_reviews/research/spikes）是记录性质，不做"新鲜度"审查

## Related

- [docs/README.md](../../../docs/README.md) — 文档索引（本文档维护目标）
- check-links.py 脚本：`.trellis/tasks/08-10-docs-staleness-audit/scripts/check-links.py`
