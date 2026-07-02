# Implement — B9-C diff primitive

## 执行清单（有序）

1. **`<DiffPrimitive>`**（`components/chat/primitives/DiffPrimitive.vue`）✅ 已写
   - parsePatch 拆多文件 → FileDiff[]（path 清洗 + added/removed 计数 + status 推断 + patchToText 重组）
   - 复用 `<DiffView>` + 复制按钮
2. **registry 替换**（`uiPrimitiveRegistry.ts`）：`diff: DiffPrimitive`（MockPrimitive 保留为 fallback）
3. **`use_ui.rs` description**：补 diff 的 `diff_text` 字段说明
4. **测试**（`DiffPrimitive.test.ts`）
   - 单文件 diff → DiffView 渲染（含 +/- 行）
   - 多文件 diff → 多 file 卡片
   - path 清洗（a//b/ 去前缀）
   - added/removed 计数
   - 复制按钮 → clipboard.writeText + 反馈
   - 空/非法 diff_text → raw fallback（不崩）

## 验证命令

```bash
cd app && pnpm exec vue-tsc --noEmit
cd app && pnpm vitest run            # 含 DiffPrimitive.test + UiCard 无回归
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib use_ui   # description 改动
# 端到端(可选): pnpm tauri dev, LLM 调 use_ui({type:"diff",diff_text:"..."})
```

## 风险点 / 回滚

- **风险**：parsePatch round-trip（重组再 parse）边界情况。→ DiffView raw fallback + DiffPrimitive try/catch 双兜底。
- **风险**：DiffView 的 `:deep()` 样式覆盖（DiffPrimitive scoped 改 DiffView 内部）。→ 最小 :deep 覆盖（border/gap/max-height），不改 DiffView 本体。
- **回滚**：DiffPrimitive 新增；registry 单行回滚 MockPrimitive；description 文字回滚。

## Review gates

1. DiffPrimitive + registry 替换 → `vue-tsc` + `DiffPrimitive.test` 绿
2. use_ui description → `cargo test --lib use_ui` 绿
3. 端到端（LLM 调 diff → DiffView 渲染 + 复制）→ **Child C done**
