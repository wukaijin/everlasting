## 11. 风险与权衡

### 11.1 技术风险

| 风险 | 严重度 | 缓解 |
|---|---|---|
| breadcrumb 软门控,agent 绕过流程 | 中 | opt-in session 内 agent 配合度高;门控违规统一走协商档(Q3+S3:engine 主动 ask_user_question,见 §5.2.3/§6.6.2),非纯软提示 |
| worker 上下文注入(Q6 delegation 模板落地) | 中 | Phase 2 实施;模板 + task meta + 主 LLM 填委托,达成 Trellis jsonl 注入的等价效果(见 §6.6) |
| 沉淀闭环失效(agent 忘写 spec) | 中 | Phase 3 Rust 固定 hook(Q9 选 b)强制 done 时触发 |
| plugin 配置格式不稳定(过早抽象) | 中 | Phase 0-1 硬编码默认先验证,Phase 2 再外置;第二个 plugin 出现才逼出抽象 |
| workflow session token 预算(breadcrumb+元数据+delegation+recall 都进 messages) | 中 | 严格 append + cache_control + 预算;prd 全文不入。per-turn 估算(见下表) |
| 评审流需要新通讯架构(Q8 选 B) | 低(愿景阶段) | 延迟讨论;A 回合制先用 B6+L3b 落地,B 独立立项不阻塞 |

**token 预算 per-turn 估算**(M7 评审修正):

| 注入项 | per-turn 估算 | cache 命中时成本 |
|---|---|---|
| breadcrumb(state 模板) | ~300-700 tokens(按 §A.2 中文 breadcrumb 实测 250-500 字) | 0(append 到 messages[0],cache_control: None,不破坏缓存) |
| task.json metadata(title/summary/state) | ~50 tokens | 0(同 messages[0] block append) |
| delegation template(dispatch 时,见 §6.6.1) | ~200 tokens | 0(同上,仅 dispatch turn) |
| checklist items(task.json.items,agent 主动 read_file) | 按需,不入常驻 | N/A |
| memory recall(已有) | ≤[`RECALL_TOKEN_BUDGET`](../.trellis/spec/backend/memory.md) | 0(已有预算约束) |

**结论**:per-turn 注入(breadcrumb + task meta)~350-750 tokens,dispatch turn 额外 +200。全部 cache_control: None 不破坏 prompt cache breakpoint。可控,无需特殊预算机制。

### 11.2 工程权衡

**机制硬度 vs 灵活性**:opt-in session 内做硬约束,普通 session 零改动。代价:workflow session 行为分叉,要多写 spec。

**engine/content 分离时机**:Phase 0 就预留 plugin 接口(WorkflowDef struct + 三访问函数,见 §5.4),数据源先硬编码常量;Phase 2 换读 workflow.json。代价:Phase 0 多写点 struct;收益:Phase 2 外置 + 评审流加入是扩展不是重构,不碰已稳定的 engine 主体。

**沉淀质量 vs 自动化**:沉淀全靠 agent 写,不强制结构。代价:早期沉淀参差;长跑后有用 spec 浮出来。

---
