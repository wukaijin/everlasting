# P5 design.md — 技术设计

> child `06-29-am-p5-quality` · PRD [`prd.md`](./prd.md) · epic [`06-29-autonomous-memory`](../06-29-autonomous-memory/prd.md) · 设计源 [spike-007 §3/§4/§10](../../../docs/spikes/007-agent-autonomous-memory-plan.md)
> 接口底座由 P1 铺好（`bump_hit_count` / `update_status` 含转换矩阵+事务+`StatusTransitionError`，`db/memories.rs:1206` 注释明写 "P5 status-machine interfaces"），P5 只写**规则逻辑 + 新档（verified 软拦截）+ 新模块（卫生 job）**。

## 1. 范围与边界

三块质量机制（兑现 spike-007 §9 步 6）：
1. **verified 软拦截重判** —— pitfall 升 verified 且 `trigger_key` 完全命中 → 短路 `execute_tool`、回灌提示让 LLM 重判（动 loop，兑现"第一时间规避"）
2. **状态机自动晋升** —— `candidate → active → verified`，靠 `hit_count` + 存续时长；老化 → `demoted`
3. **异步卫生 job** —— Jaccard >0.7 dedup 合并 / 低命中+老降权 / 矛盾共存明示

**不做**（沿用 spike-007 §8 v1 边界）：向量检索 / LLM-judge 写入过滤 / global 记忆层 / 跨 session "翻车"持久追踪（P4 `FailureTracker` 是 session 内状态机，`auto_reflect.rs:17` 注释 "v1 accepts session-boundary reset"）。

## 2. 决策清单（4 待决项 + 死循环，全部拍板）

| # | 决策 | 定档 | 依据 |
|---|---|---|---|
| D1 | **软拦截死循环防护** | 每条 pitfall 每 session 软拦截 **1 次**；同 pitfall 再次命中 → 降级 active 注脚 + 正常执行 | session 级 `HashSet<memory_id>` 记账（同 `FailureTracker` 生命周期：loop 顶部建、退出销毁）。兑现"第一时间规避"又不卡 loop；尊重 agent 最终判断（"经验非规则"） |
| D2 | **晋升阈值** | candidate→active @ `hit_count≥2`；active→verified @ `hit_count≥5` **且** `created_at` 距今 ≥3 天 | v1 无跨 session 翻车信号，`hit_count`（召回命中次数）是主依据，"存续时长"代理"未翻车"。verified 触发软拦截（动 loop），门槛必须高 → 误拦/烦人风险最低 |
| D3 | **Jaccard 实现** | **char-trigram** 集合的 Jaccard，>0.7 视为重复 | 零依赖、语言无关（中英短句都吃）；content ≤500 char，trigram 足够；dedup 阈值 0.7 本身宽松，char-trigram 对中文短句的重叠捕获够用。fallback：若实测中文 dedup 漏合并，再评估引入分词（v2） |
| D4 | **卫生 job 触发** | **事件触发**：`insert_memory` 后按 `(scope,kind)` 计数取模触发（每 N 条跑一次 dedup/降权 pass）+ app 启动跑一次清理 | 项目**无现成长驻 `tokio::time::interval` task**（`tokio::spawn` 仅用于 fire-and-forget reflection / shell），自建 interval 要管生命周期+退出，成本不对称于"卫生 job 不需实时"。复用现有 spawn 范式 |

## 3. 关键纠正：两路 recall 的 filter 方向（推翻 P2 注释预期）

`memory_recall.rs:96` 注释预期 "P5 tightens back to `ActiveVerifiedOnly` once the state machine lands"；`check.rs:738` 现 filter `status=="active"`。**深入晋升路径后发现这两个"收紧"假设都与状态机矛盾**：

- candidate→active 的**唯一 v1 触发**是"被召回命中"（spike-007 §3：recall 命中 / UI 看到未删 / 复核通过 —— 后两者 v1 无埋点）。
- 若召回只查 active+verified，candidate 永远没机会被命中 → **永不晋升**。preference/fact 类无 `trigger_key`，只靠 session-start FTS 召回，断路尤甚。
- pitfall 类靠 pre-tool `trigger_key` 命中，但 `check.rs:738` filter 掉了 candidate → candidate pitfall 也断路。

**P5 改正方向（放宽，非收紧）**：

| recall 路 | 现状 filter | P5 改为 | 分档 |
|---|---|---|---|
| session-start FTS（`memory_recall.rs:96`） | `IncludeCandidate`（candidate+active+verified） | **保持 `IncludeCandidate`**（不收紧） | candidate 命中 → bump → 达 D2 阈值晋升，自然流出 candidate 池；噪音靠"快速晋升"控制，不靠 filter |
| pre-tool pitfall（`check.rs:738`） | `status=="active"` only | **candidate+active+verified** | verified+完全命中+未拦过 → **软拦截**；active → 注脚（现状）；candidate → 注脚+bump（晋升入口） |

> 即：`memory_recall.rs:96` 那行注释要随 P5 改写（filter 不动，注释纠正）。`RecallStatusFilter::ActiveVerifiedOnly` 仍保留枚举（P1 已定义，`db/memories.rs:731`），但 P5 不切到它。

## 4. 软拦截数据流（本 task 最复杂的部分）

**插入点**：`chat_loop.rs` 两 path 同构 —— parallel（`:1822` `recall_pitfall_footnote` 调用处）+ serial（`:2415`）。两条 path 都在 `permissions::check` 返回 Allow 之后、`execute_tool` 之前。

**新契约**：把 `recall_pitfall_footnote` 的 `Option<String>` 升级为分档 enum（或新增 sibling 函数，保留旧函数给注脚档）：

```text
enum PitfallRecall {
    None,
    Footnote(String),                          // active / candidate / 二次命中 → 现有注脚行为
    SoftBlock { hint: String, memory_id: ... } // verified + 完全命中 + 本 session 未拦过
}
```

**SoftBlock 命中时的回合改法**（兑现 spike-007 §10 "hint round 的 loop 改法"）：
1. **不调** `execute_tool`（短路，复用 `Decision::Deny` 在 `chat_loop.rs:1790/2270` 的"不执行+回填 tool_result"模式）。
2. 构造 `ContentBlock::ToolResult { content: hint, is_error: false }`，hint 措辞明示"⚠️ 此操作因历史 pitfall 被暂缓、**未实际执行**；请重新评估，调整命令后重试或确认继续"（`is_error=false` 避免 LLM 误判"工具坏了"换工具；语义是经验提示，不是错误）。
3. 记 `memory_id` 到 session 级 `HashSet`（D1 防循环记账）。
4. `bump_hit_count`（best-effort，同现状 spawn）。
5. `emit_tool_result`（前端 ToolCallCard 正常渲染提示型 result）。
6. tool_result 回填 → 进下一轮 `provider.send` → LLM 重判（调整命令 / 放弃 / 坚持）。

**二次同坑命中**（HashSet 已含该 memory_id）：降级 `Footnote` + 正常 `execute_tool`（即回到 active 注脚档）。**绝不**第二次短路 —— 这是 D1 的核心，保证不卡到 `MAX_TURNS`。

**完全命中 vs 弱匹配**（⚠️ 实现澄清，P5 落地时偏离 design 字面）：检索走 `find_pitfalls_by_trigger_all_status`（P5 放宽版，含 candidate/active/verified），按 `tool_name` + `command_pattern` 子串 + `path_globs` glob。design 字面说"完全命中 = 三者皆中"，但**内置工具探针对称性使该字面不可行**——Shell 不产 path 探针、Path 工具不产 command_pattern 探针，故"两字段都 `Some` 且匹配"对任何真实 tool_use 永不满足。`is_full_match`（`check.rs`）实际语义：**行上每个 `Some(_)` 字段都与探针匹配，且至少一个字段为 `Some`**（command_pattern 子串包含；path_globs 经 SQL glob filter 信任）。后果：宽泛 pitfall（两字段皆 `None`）永不 SoftBlock → 降级 Footnote，比字面更保守，保留"verified 高门槛"意图。锁定测试：`p5_recall_verified_full_match_returns_soft_block` / `p5_recall_verified_path_command_agnostic_returns_footnote`。

**cancel 语义**：软拦截不执行 tool，无 `execute_tool` 的 cancel/audit 不变量问题（`RULE-A-004` 不涉及）；下一轮 send 仍受 `CancellationToken` 管控。audit：软拦截不写 `tool_executed`（tool 没真跑），可写一条新的 `AuditKind`（可选，见 §7）。

## 5. 状态机晋升

**消费点**：`bump_hit_count` 已在 P2 session-start recall（`memory_recall.rs:135`）+ P3 pre-tool recall（`check.rs:753`）best-effort 调用。P5 在 bump 之后加 promotion 检查：

- 新增 `promote_if_eligible(pool, memory_id)`：同事务读回 `hit_count`/`status`/`created_at`，按 D2 阈值调 `update_status`。放在 `bump_hit_count` 内部（UPDATE hit_count 后同连接读回+判断+UPDATE status）以原子化，避免 bump 与 promote 之间的竞态（`update_status` 注释已点明 SQLite 串行写者兜底）。
  - `hit_count` 跨 2 且 `candidate` → `active`
  - `hit_count` 跨 5 且 `active` 且 `age≥3天` → `verified`
- 非法转换由 `update_status` 的矩阵 + `StatusTransitionError::Illegal` 兜底（如 demoted→verified 直接拒绝）。

## 6. 卫生 job（事件触发，D4）

**触发**：`insert_memory` 末尾，按 `(scope, kind)` 计数 `& N == 0`（N=10）→ `tokio::spawn` 跑一次 pass；app 启动（`lib.rs` setup）跑一次全量清理。fire-and-forget，失败 `warn!` 吞。

**三件事**：
1. **dedup 合并**（spike-007 §10）：同 `(scope, kind)` 下 —— pitfall 类按同 `trigger_key`（tool+command_pattern+path_globs）；pref/fact/decision 类按 content 的 **char-trigram Jaccard >0.7**（D3）。命中 → 保留 `confidence`/`hit_count` 高者（合并 hit_count 累加、`last_used_at` 取新），`delete_memory` 删冗余。
2. **降权**：`status IN (candidate, active)` 且 `last_used_at` 距今 >30 天 且 `hit_count < 2` → `update_status(Demoted, reason="aged_out")`。
3. **冲突共存明示**：v1 **不做** LLM 语义冲突检测（§1 边界）。pre-tool recall 多 pitfall 命中时 footnote 多 bullet（`check.rs:772` 已支持）即"共存明示"；session-start recall 多条命中同理（`memory_recall.rs:156` 多行）。无需单独 job。

## 7. 兼容性 / 回滚 / 测试影响

- **新增**：`PitfallRecall` enum / `promote_if_eligible` / 卫生 job 模块（新文件 `agent/memory_hygiene.rs` 或 `memory/hygiene.rs`）/ char-trigram util。**不破坏** P1/P2/P3/P4 公开接口。
- **改动点**：`recall_pitfall_footnote` 分档（或新增 sibling）→ `chat_loop.rs` 两 path 调用点改 2 处；`bump_hit_count` 内嵌 promotion；`memory_recall.rs:96` 注释纠正（filter 不动）；`lib.rs` setup 加启动清理。
- **测试影响**：
  - `memory_recall.rs:302 build_recall_text_surfaces_candidate_match` —— filter 不动，**仍通过**（candidate 继续被召回）。
  - `auto_reflect.rs` / `check.rs` 现有 pitfall 注脚测试 —— 注脚档行为不变，仍通过。
  - 新增：软拦截单测（verified+完全命中→SoftBlock）、死循环防护（二次命中→Footnote）、晋升阈值（hit 跨 2/5 + age）、char-trigram Jaccard、dedup 合并、降权。
- **回滚**：软拦截用 `const PITFALL_SOFT_BLOCK_ENABLED: bool = true` 开关（或 env），关掉即退回 P3 纯注脚行为（`PitfallRecall::SoftBlock` 永不返回）。
- **前端**：软拦截的 `ToolResult`（is_error=false、提示型 content）经现有 `emit_tool_result` → `ToolCallCard` 渲染，确认卡片正常显示提示（不当作 error 态）。实现时手动验证一次。

## 8. 权衡记录（一句话留痕）

- 软拦截 `is_error=false`（提示非错误）vs `true` → 选 false；死循环每坑拦 1 次 vs N 次 → 选 1 次；Jaccard trigram vs 分词 → 选 trigram；卫生 job 事件 vs 定时 → 选事件。四项理由见 §2，均取"最小复杂度满足 spike 真相"。
