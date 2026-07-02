# Design — B9-C diff primitive

> 技术 design（精简，复用 DiffView 是核心）。需求见 `prd.md`，父决策见 parent D4。

## 1. 数据流

```
LLM → use_ui({type:"diff", diff_text: "<unified diff string>"})
  ↓
<UiCard> registry dispatch → <DiffPrimitive :primitive>
  ↓
parsePatch(diff_text) → ParsedPatch[]（每文件一个）
  ↓ map
FileDiff[] = [{ path(洗 a//b/), status, added, removed, diff_text(重组) }]
  ↓
<DiffView :files>（复用：行级着色 + 折叠 + raw fallback）
```

## 2. 关键点

- **复用 DiffView**：DiffView 接 `FileDiff[]`（每 file 含 `diff_text`），内部 `parsePatch` 渲染。DiffPrimitive 负责"unified 字符串 → FileDiff[]"转换，不重写渲染。
- **patchToText 重组**：parsePatch 拆出每 patch 后，重组为单文件 unified diff 字符串给 DiffView（DiffView 再 parsePatch 一次）。round-trip 便宜，保留 DiffView 的 raw fallback 路径。
- **path 清洗**：git unified diff 的 `+++ b/foo.rs` → path `foo.rs`（去 `a/`/`b/`）。
- **status 推断**：纯加 → added，纯删 → deleted，混合 → modified（给 DiffView 的着色）。
- **多文件**：一个 diff_text 含多个 patch → 多 FileDiff → DiffView 渲染多卡片。
- **复制**：复制原始 diff_text（整段），非重组后。

## 关键设计决策

| 决策 | 选择 | 理由 | 否决的备选 |
|---|---|---|---|
| 渲染 | 复用 DiffView（FileDiff[] 转换） | 行级着色/折叠/raw fallback 已成熟，零重复 | 自己渲染 +/-（重写） |
| 输入形态 | 单 `diff_text` 字符串 | LLM 易生成（一段 unified diff） | 结构化 files 数组（LLM 拆分负担） |
| 多文件 | parsePatch 拆多 patch → 多 FileDiff | 一个 diff 多文件天然支持 | 只渲染 patches[0]（丢文件） |
| 应用动作 | 不做（D4） | edit_file+权限⑨+DiffView 已覆盖修改确认 | 复用 edit_file 路径写回（与 edit_file 模型冲突） |

## 兼容性 / 回归

- DiffView 零改动（DiffPrimitive 适配其 FileDiff[] 契约）。
- registry 替换是单行；MockPrimitive 保留为 fallback。
- 不改 markdown / hljs 管线（Child B 的）。

## 风险点

- **parsePatch 与 DiffView 的二次 parse**：round-trip 重组 diff_text 再 parse，极端 malformed 输入可能两次结果不一致。→ DiffView 有 raw fallback 兜底；DiffPrimitive 也有 try/catch fallback。
- **path 清洗误伤**：`a/`/`b/` 前缀启发式，非 git diff（如手工 unified）可能 path 为 `diff`。可接受（fallback 显示）。
