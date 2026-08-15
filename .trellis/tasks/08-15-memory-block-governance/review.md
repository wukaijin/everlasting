# Review — memory 指令块窗口治理(2026-08-15,Phase 1.4 评审门)

> 评审对象:prd.md / design.md / implement.md / research/code-map-20260815.md / implement.jsonl / check.jsonl。
> 方法:工件通读 + 对 research 关键行号与设计论断做代码抽查(行号为本日实测)。

## 结论

**工件完整、方案可执行,建议先修 P1(节粒度矛盾)+ P2(§3.4 cache 论断),连同 P3 三处补行,再 `task.py start`。** 无需重选型:方案 A(分级注入 + 按需拉取)与既有 C7D 模式同构,风险面(模型不拉、摘要质量)已有开关兜底 + AC3 实测。

## 工件完整性

| 检查项 | 状态 |
|------|------|
| prd.md / design.md / implement.md 三件套 | ✅ 齐全(复杂任务要求) |
| research/code-map-20260815.md | ✅ 含实测行号 + fence 陷阱提示 |
| implement.jsonl / check.jsonl 真实条目(ready gate) | ✅ 6 + 3 条,无 seed 行 |
| PR 划分 + 回滚点 | ✅ PR1 独立可 revert;PR2 开关兜底 |
| 验收标准可测 | ✅ AC1-AC6 均带数值或断言 |

## 发现

### P1 必须修:节粒度规则与"7 节"自相矛盾(design §3.2 / implement Step 2.1)

- §3.2 边界规则写 `^#{1,3} ` 都是节边界,示例列 `### 核心数据流` 为独立节;但目录头示例写 "7 sections"、implement Step 2.1 快照断言写 "7 节"。按自己的规则,repo CLAUDE.md = 7 个 `##` + 2 个 `###` + Preamble = **10 节,非 7 节**。
- 连带缺口:**嵌套节 key 的规范化未定义**。`### 核心数据流` 是独立 key 还是 `Architecture` 子树?工具参数 `CLAUDE.md#节标题` 遇同名标题跨父节时如何消歧?
- 建议(任选,关键是规则/示例/快照/工具 key 四处一致):**顶层 `##` 为节单元**(`###` 随父节整体返回),key = 标题文本,最简且合"7 节"直觉;或保留 `#{1,3}` 但 key 用完整标题路径并写明碰撞策略。

### P2 建议修:design §3.4 "每次拉取至多一次 prefix miss"与代码自身 cache 模型矛盾

- drive.rs:253-263(B12 注释)模型:断点之后 append 的内容不影响 memory cache 窗口("Appending keeps the checklist AFTER the memory breakpoint so the memory cache window stays intact")。按此模型,粘性 body 增长在 banner 断点之后,**不产生 prefix miss**;按另一解读(banner 为唯一断点、body 本不在缓存前缀内)结论相同——body 变化对 cache 零影响。
- **AC4 用实测裁决而非假设,方向正确,验收标准不必改**;仅把 §3.4 断言改写为"cache 影响以 AC4 实测为准,机制上 body 位于断点之后、预计零到一次 miss",防实现者带错误预期调优。
- 另:RULE-A-005 / I1 cache 契约实际只存在于代码注释(init.rs:453-471、drive.rs B12),`.trellis/spec/backend/memory.md` 无此内容——check.jsonl 将其列为 memory.md 来源不准确;implement/check 清单补一句指向上述注释,WP3 沉淀时把 cache 模型写进 spec。

### P3 补一行即可

1. **`["all"]` 语义含糊**(design §3.3):数组只有 `"all"` 无层名时,是全层所有节还是全部 4 层?定义清楚。
2. **AC5 "逐字节一致"应覆盖 tools[] 列表**:gate off 时 `load_memory_sections` def 不得 append(implement Step 2.3 已写"gate 同源",AC5 断言补半句)。
3. **粘性 registry 陈旧 key**:session 内文件被改(mtime fence 刷新)→ 节重命名 → registry 旧 key 失效。注入侧静默跳过不存在的 key(工具报错附新节列表自愈),design 补一句容错语义。

## 代码抽查结论(全部成立)

| 断言 | 结果 |
|------|------|
| loader.rs:204 `load_for_session` / :347 `build_instructions_blocks`,banner 为唯一 `cache_control: Ephemeral` 块 | ✅ |
| init.rs:378-379 唯一注入点(经典/群聊/worker 共用 prepare_loop_state) | ✅ |
| drive.rs:604-609 tools_token 计算、:860 Done 处 upsert(非致命 warn) | ✅ |
| stub.rs:33/105/138 STUB_CANDIDATES / load_tool_schemas_def / StubRegistry(delete_session 挂点 :166) | ✅ |
| drive.rs:537/589 gate = `stub_on && !effective_is_worker && !is_group_chat`;chat_loop.rs:613 每 request 读开关 | ✅ |
| schema.rs:959 `tools_token INTEGER`、:981 幂等 backfill;trace.rs:48/71/281/300 | ✅(upsert 参数实为 :76,±10 行内) |
| turnTrace.ts:129 / TurnCard.vue:166 toolsPct,null 隐藏 | ✅(computed 实为 :166,±10 行内) |
| gate 落点标志可达:init.rs:306 算 effective_is_worker;is_group_chat 可由 session_type 推导 | ✅ |
| AC4 数据源:turn-smoke.sh:113 已输出 cache_read_input_tokens,`--keep` 已支持 | ✅ |
| trace.test.ts 存在(implement Step 1.3 引用) | ✅ |

行号基线整体准确(±10 行漂移,文档已声明"实现前重扫",可接受)。

## AC 预算复核

- AC2:digest 按 10 节 ×(标题 + ≤120 chars 首句)≈ 0.5k + AGENTS 1.3k + wrapper/banner 0.2k ≈ **2.0k ≤ 2500** ✅;CLAUDE.md 单层降幅 ~93% ≥ 80% ✅。
- AC5:gate off 分支注入逐字节一致,单测断言合成消息文本;worker/群聊零改动路径同断言 ✅。
- AC6:测试面(fence 状态机 / digest 生成 / registry 生命周期 / upsert 扩参)覆盖最高 bug 密度点 ✅。

## 建议动作

1. design.md:定死节粒度与 key 规范化(P1)、改写 §3.4 cache 论断(P2)、补 `"all"` 语义 + 陈旧 key 容错(P3-1/3)。
2. implement.md:快照断言数字与 P1 决策同步;AC5 断言补 tools[] 覆盖(P3-2);check 清单补 init.rs/drive.rs 注释指引。
3. 完成后走 1.4 `task.py start`;PR1 先行,基线记 `research/baseline.md`。
