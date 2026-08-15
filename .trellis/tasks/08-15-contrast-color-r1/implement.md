# 执行计划 · contrast-color-r1

依赖:PRD WP1-WP6。所有 grep 基线为 2026-08-15 会话实测,实现时**必须重扫**(行号会漂)。

## Step 0 · 重扫 call site 清单(先于任何改动)

```bash
# accent 作文字(注意:前缀匹配会带出 accent-hover/accent-muted,逐条看语义)
grep -rn "color: var(--color-accent" app/src --include="*.vue"
# 错误文案(tool-error 作 color)
grep -rn "color: var(--color-tool-error)" app/src --include="*.vue"
# 彩底组件内的 muted 文字(memory 三件套优先)
grep -rn "color: var(--color-text-muted)" app/src/components/memory
# 11px mono 高频元数据位
grep -rn "var(--text-xs)" app/src --include="*.vue" | grep -iv "border\|padding"
```

基线快照(2026-08-15):
- accent 文字:`MessageItem.vue:1103`(markdown 链接,最重要)、`DiscussionSummaryCard.vue:72,174`、`EmptyProjectState.vue:155,217,338`、`WorktreeChip.vue:271,300`、`HiddenProjectsMenu.vue:140,254`、`BrowserHeader.vue:74`、`RuntimeMemoryModal.vue:601,751`。
- 同 grep 还会带出 `border-color: var(--color-accent...)` 行(`RuntimeMemoryModal 715,788,805,862`、`EmptyProjectState 192,347` 等)——**这些是图形用途,3:1 达标,不动**。

## Step 1 · WP1 + WP2 + WP4(token 层,一个小 PR)

1. `style.css` `@theme`:加 `--color-accent-text: #7c9aff`、`--color-tool-error-text: #f87171`,带注释注明"文字专用,填充仍用 500 档"与来源任务。
2. `--color-bg-hover` 6%→10%、`--color-bg-active` 10%→14%(同文件两行)。
3. Step 0 清单逐条替换 accent 文字 / 错误文案为对应新 token;每替换一处确认是 `color:` 属性。
4. 自查:`grep -rn "color: var(--color-accent)$\|color: var(--color-accent);"` 剩余处应均为非文字语义。

## Step 2 · WP3 + WP5(定向 sweep)

1. memory 彩底组件 muted→secondary(Step 0 第三条 grep 结果全量,均在 accent-muted/彩底上)。
2. WP5 候选从下面这些**高频注视位**里挑,逐条列入下方勾选框,不确定的留 muted:
   - [x] `Sidebar.vue:280` `.sidebar__title`(SESSIONS 头,11px semibold)
   - [x] `SessionGroupHeader.vue:94,106` 分组 label + count(本周/更早行)
   - [x] `MessageItemFooter.vue:338` `.msg__latency`(消息耗时标签)
   - [x] `MemoryPreview.vue:814` `.runtime-memory__timestamp`(VLM 两轮点名的 memory 卡时间戳)
   - [x] `ChatInputHintRow.vue:158` `.chat-input__hint`(输入区常驻提示)
   - 保留 muted:SessionGroupHeader chevron、各 close/delete 图标按钮(交互图形)、
     MemoryModal/RuntimeMemoryModal chip 与 stat-label(低频/有 surface 底)、`(edited)` 角标。
   - ToolCallHeader `__duration` 实现前已是 secondary,无需动。
3. 每处替换后在浏览器里目验层级仍读得出 primary > secondary > muted。

## Step 3 · 验证(对应 PRD AC)

1. `cd app && pnpm test && pnpm build`。
2. 数值复算(skill 单行命令)记入 `research/verify-contrast.md`:accent-text × surface/elevated、error-text × surface/elevated/accent-muted、secondary × accent-muted ≥4.5。
3. 样本页复跑:`node scripts/ui-review-specimen.mjs --out out/ui-review/<ts>-contrast-r1 --shoot`,确认 A/B 章 FAIL 格消失(AC2),D 章候选已删。
4. 全量复跑:`./scripts/ui-review.sh`,报告与 `research/baseline-full-20260815-151036.md` 对比(AC3);hover 判读参考 C 章新样本。
5. 真机复核 R2.3(荧光感)与 R4.2(叠加强度)。

## Step 4 · WP6 spec 沉淀

1. `design-tokens.md` 新 Decision 段(2026-08-15 对比度评审):两 token 用途边界 + "填充 500/文字 400"原则 + 彩底最低 secondary + 彩底禁紫/蓝字 + 最小可辨 ΔL(hover/active)。同步更新 Token 表格(Text 段、Tool colors 段、State Tints 段)。
2. `scripts/ui-review-specimen.mjs` 删 CANDIDATES 对应条目(D 章自动消失)。

## PR 划分建议

- PR1:Step 1(token + 机械替换,小而稳)
- PR2:Step 2 + Step 4(sweep + 文档)
- Step 3 验证各自跟随所属 PR;AC3 全量复跑放 PR2 后。
