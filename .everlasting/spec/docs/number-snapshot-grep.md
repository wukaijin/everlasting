# 文档数值快照改动必须全仓 grep 旧值

## 场景

同步 docs 中**实测数值快照**(IPC 命令数、AuditKind 变体数、数据库表数/列数、builtin 工具数)时。新特性落地常带来这类数值变化(如 IPC 107→108、AuditKind 28→29),且同一数值会出现在多个文档的多处位置。

## 规范

1. **先实测再落笔**:数值一律以代码实测为准(`tauri::generate_handler!` 注册数、`audit.rs` 枚举变体数、`migrations/schema.rs` 列、`#[tauri::command]` 计数),不沿用文档旧值。
2. **全仓 grep 旧值再改**:`grep -rn "<旧值>" docs/ STRUCTURE.md`(含 `_history` 之外的全部 docs + 根 STRUCTURE),逐处判断是「当前状态陈述」(必须改)还是「历史注记」(允许保留,如"08-31 为 107"的对比说明、当次同步纪要)。
3. **数值会散布的位置**(实测踩过):文档正文、ASCII 拓扑图/全景图、表头注释(`routes/ # N 个 ...`)、术语节、历史链注记。改主汇总处(如 STRUCTURE §5)之后必须清点其余。
4. check 阶段用 checker 子代理 grep 断言兜底(它抓到了主 LLM 漏掉的 4 处)。

## 反例

本次(09-01-a2-sandbox-docs-sync)只改了 STRUCTURE.md §5 汇总与树注释,漏掉 docs/ARCHITECTURE.md 34/104/921 行与 docs/CONTEXT.md 170 行的 IPC 107 当前状态陈述,被 checker 的 grep 断言抓出后补改。不要只改主汇总后就算同步完成。