# Review — 统一 token 预算表 + 关卡⑤硬卡(start 前评审门,2026-08-19)

> 评审对象:prd.md / design.md / implement.md(task.json status=planning)。
> 方法:三文档与真实代码逐点对照(锚点相对 `app/src-tauri/src/` 与 `app/src/`)。

## 总体结论

PRD 的诊断准确(@文件盲区、messages-only 触发线口径洞均属实),AC 可测,D1–D7 决策有据;design 的时序重排(D7)依赖核查成立(tools 过滤链只依赖 permissions + head_sha + StubRegistry,与压缩块零依赖,可安全挪到压缩判断前)。**当前不是 ready-to-start**:F1/F2 两个口径缺陷需在 start 前修掉,F3 过程门未满足(取决于实施平台),F4/F5 为低优先修正。

## 验证属实的关键锚点

- `prepare_loop_state`(init.rs:92)、memory_token 计数点(init.rs:499)、`inject_at_tokens`(init.rs:840)、digest 豁免 `!worker && !群聊`(init.rs:413-436)✅
- `drive_turn`(drive.rs:82)、head_sha 刷新(:180)、C3+ 压缩块(:215,tokens_pre :222)、摘要 postcheck 消费点(:311)、tools 过滤链 + stubify + 元工具 append(:752-826)、tools_token 估算(:845-851)、图片 resolve(:736)、`provider.send`(:891)✅
- `TRIGGER_RATIO=0.85` / `SUMMARY_POSTCHECK_RATIO=0.95`(context.rs:50/59)、机械 `compact_messages` 无条件(:281,口径 messages-only 属实)、read_file 50KB 截断(:40)、`IMAGES_TOKEN_FALLBACK_EACH=1600`(attachments.rs:309)✅
- `AuditKind` 加变体无 migration(audit kind 存 TEXT)、`ChatEvent::Retrying` 只读非持久化先例(llm/types/event.rs:156-216)✅
- TurnCard.vue:139-151 硬编码 `CONTEXT_WINDOW_REF=200_000` + 三 cell(:365-404)属实;`contextWindow` 已有前端字段(ChatInput.vue:167)✅

## 发现(按优先级)

### F1(高,design 缺陷)— 统一估算公式重复计数 memory

design §2:`estimate_request_tokens(...) = count_tokens(system) + count_tokens(memory 文本) + count_tokens(tools JSON) + estimate_messages_tokens(messages)`。

但 memory 指令块与 skill listing 已在 init 期作为合成消息插入 messages(init.rs:485-535),@文件也在 messages 里(init.rs:840),因此 `estimate_messages_tokens(messages)` 已含 memory + skill + @files + images + history。再加 `count_tokens(memory)` 是二次计数——与 design 自己对 @文件的口径自相矛盾(§2 明确写"正文在 messages 里已被 total 计入,不 double-count")。

后果:memory 块大时(指令文件 + digest 已加载节)总量虚高,可能吃光 0.95 预算线刻意留的 5% cl100k 偏差余量,比 provider 实际计量更早触发压缩/裁剪。

**修法**:总量 = `count_tokens(system) + count_tokens(tools_json) + estimate_messages_tokens(full messages)`(R4 五切片 + 历史残差口径正是这个,design 内部自洽);签名去掉 `memory_blocks` 参数,或显式传"不含合成头的 messages"。AC1 应把"各切片不重叠"写成断言。

### F2(中,design 缺口)— 裁剪后 trace 切片与实发量的口径

budget_gate 在 image resolve 后、send 前裁剪请求副本,而 trace 的 memory / at_files / images / tools_token 记的都是**预裁**计数。裁剪后实际发送的 memory(arm 3 回退)与 @文件正文(arm 1 占位)比 trace 记录小,provider 返回的 `context_input_tokens` 也会比预算行小。design 未定义 R4 预算行的"统一总量"是预裁还是实发。

**建议**:预算行显示**实发**(与 provider usage 可比),裁剪发生时由 audit 的 `tokens_freed` 补差;或明确"trace 落预裁 + audit 差值"。否则前端占比条会出现切片和 ≠ 总量的怪态。AC4 需补这句定义。

### F3(中,过程门)— implement.jsonl / check.jsonl 仍是 seed 行

两文件只有 `_example` 一行。ZCode 属 sub-agent dispatch 平台,workflow 1.3/1.4 要求 start 前两个 jsonl 各含 ≥1 条真实 spec/research 条目(seed 不算)。implement.md 的"start 前检查"写的是 "inline workflow,jsonl gate 不适用"——那是 Codex inline 的豁免,不适用于 ZCode。

- 若实施走 ZCode:先补条目(候选:`token-usage-tracking` / `agent-loop-architecture` / `database-guidelines` 等 spec)。
- 若确定走 Codex inline:可豁免,但 implement.md 需注明采用 inline 流程。

### F4(低,文档不一致)— 图片 pad 数值

PRD Risks 写"图片 pad 640/张近似",实际 context.rs:234 用 6400 个 `x` 字符(注释 ~1600 token),attachments.rs:309 也是 1600。640 应为 1600(或 6400 字符)之误,改掉避免 implementer 拿错值。

### F5(低,实现注意)— at_file_spans 偏移鲁棒性

arm 1 靠注入时记录的 [start,end] 偏移在 send 前替换,但注入后当前 turn user message 还会被 drive.rs:537 的 APPEND 组装追加内容。若偏移只对追加在末尾的情形有效,建议 spans 存 path + tokens、裁剪时按 manifest 重建定位,并在 AC3 加"注入后 message 又被 append 的轮次裁剪仍正确"用例。

### F6(核对)— AC8 的 ~1836 基线

静态计数当前 src 约 1028 `#[test]` + 834 `#[tokio::test]` ≈ 1862;AGENTS.md 记 08-13 实测 1689。1836 量级合理,但应以 start 时 main 实测为准(implement.md 已有此检查项,把具体数改为"以实测为准"即可)。

## 建议

1. 先修 F1(design §2 估算公式 + AC1 断言)、F2(定 R4 预算行口径 + AC4 补定义)、F4(改数字);
2. 按实施平台决定 F3(补 jsonl 或注明 inline 豁免);
3. F5 写进 implement.md PR1 测试项。

修完以上即具备 start 条件。
