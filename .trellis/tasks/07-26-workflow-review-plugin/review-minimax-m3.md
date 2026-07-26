# workflow review plugin epic — 评审报告（MiniMax-M3）

> 评审对象：父任务 `07-26-workflow-review-plugin` + 子任务 C1 `07-26-subagent-resume` + C3 `07-26-review-plugin-pack` + C2 `07-26-review-viz`。
> 评审范围：整体合理性、任务拆分、关键决策风险、遗漏与盲点、可实施性。
> 评审日期：2026-07-26。

---

## 0. 代码锚点核查（先挑刺）

PRD 大量用「`xxx.rs:NNN`」锚定代码事实，但有几处与实际不符，会误导后续 design 与实施：

| PRD 声称 | 实际位置 | 问题 |
|---|---|---|
| `build_worker_messages`（`dispatch.rs:645`） | 实际定义在 `agent/subagent/mod.rs:642`；dispatch.rs:645 只是**调用点**附近的注释行 | 后续读者按锚点找实现会走错文件 |
| per-dispatch model override（`dispatch.rs:487/687`） | 实际在 `dispatch.rs:492 / 679` | 行号偏差，可改可不改 |
| `Coordination::SynthesisRound`（`dispatch.rs:1632/1653`） | 这两个行号在 dispatch.rs 中**不存在**（全文 ~2400 行）。实际定义在 `workflow/def.rs:127`；dispatch 路径零消费 | 「dispatch 路径零消费」说法本身对，但行号是**虚构的**，可信度打折 |
| `chat_loop.rs:681` 动态 model enum | 实际 682 起；偏差 1 行 | 轻微 |
| `db/migrations.rs:644/598`（final_text/model_display/transcript_json） | `model_display` 列在 line 740；`transcript_json` schema 在 line 1350 | 偏差 ~100 行 |

**结论**：行号是后续 design / 实施的「地基」。建议在父任务 PRD 中加一条「实施前精确 read + 更新所有行号」，否则 C1/C2/C3 拿到的契约层就是错的。

---

## 1. 整体合理性

### 1.1 核心价值假设未量化

PRD 隐含「多模型独立评审 + 主 LLM 修订 + 用户指挥收敛」是「需求评审」的正解。但：

- 多个强模型评同一份需求时，**80% 的发现会高度重合**（都提「清晰度不足」「边界模糊」「缺错误处理」），剩下 20% 分歧才真有价值。
- 没有「多模型 vs 单模型多轮」的量化对比，也没有对「重合度阈值」的工程化处理。
- 主 LLM 做「自然语言 → 结构化对比」是**最难的认知任务**——让一个模型把 4 份自由 markdown 提炼成可对比的结构，比让 4 个模型各自评审还难。这是反直觉设计，**需要先 spike 验证**。

### 1.2 替代方案缺失

PRD 没显式 reject 以下方案：

| 替代方案 | 优势 | 劣势 |
|---|---|---|
| A. 单模型 + 多轮迭代 | 不需要多模型基础设施 | 失去「跨模型分歧」价值 |
| B. SynthesisRound 引擎强制编排（Phase 2） | 引擎层 fan-out/gather 一致性 | 改动大、Phase 2 触发条件未定义 |
| C. 只做 `review-state.json` + 可视化，不引入多模型（用户手动开多 session） | 复杂度从引擎层降到数据层 | 牺牲自动化 |
| D. reviewer.md prompt 内加 `<output_format>` JSON 约束，让 reviewer 直接产结构化 | 跳过主 LLM 提炼层 | prompt 容易 break out of format |

**建议**：父任务 PRD 加「替代方案对比表」，明确 reject 理由。

### 1.3 与 dev 的边界没说清

- review session 与 dev session 共享 task（`resolve_current_task`），review 改 in-place PRD 后 dev 直接读——**正确**但**没说**。
- **review 半成品风险**：第 2 轮 reviewing 中主 LLM 崩了，PRD 已被部分修订，dev session 看到的就是半成品。建议加「修订原子化」约束（一次性提交，不允许半改退出）。
- review 在 dev 之前是硬假设，那用户先开 dev 干了半天再想插 review，能否回头改已存在 PRD？边界要明说。

---

## 2. 任务拆分与依赖

### 2.1 顺序 C1 → C3 → C2 逻辑成立，但 C1 粒度不对

依赖链：
- C1（resume）→ C3（resource pack 用 resume）→ C2（视图读 review-state.json）

但 C1 实际是独立 epic，不是 review 子任务：

- R1 resume 能力
- R2 消息持久化 vs 重建（架构级）
- R3 worktree 策略（按 agent 类型分）
- R4 API 兼容
- R5 适用范围验证
- 跨 session 限制（OQ3）

把它塞进 review 父子关系，会导致：

- review 进度被 C1 卡死
- C1 改动会波及整个 subagent 系统，**不该只为 review 服务**

**建议**：

- 把 C1 提升为**独立 epic**（subagent resume mechanism），独立验收
- review epic 接受 C1 是外部依赖
- 或者**先做不带 resume 的 review v1**：每轮 reviewer 全新派，接受 token 浪费，验证 review 流本身的价值

### 2.2 C3 R7（schema）是 C2 的硬契约——这点做对了

但风险：

- schema 在 PRD 中只是草稿，design.md 必须精确到 **JSON Schema + TypeScript type + Rust serde struct** 三端对齐
- 字段名（如 `verdict: pass / revise / reject`）要提前和前端 design system 对齐（颜色 / 图标）

### 2.3 父任务「不直接实施」定位模糊

父任务 PRD 只写了「最终集成验证」，缺：

- **集成测试场景清单**（只写了 1 条「完整 review 流跑通」，太粗略）
- 验收责任人

**建议**：父任务加**集成验证矩阵**（intake / reviewing 并发 / revising 修订 + 写 state / reported 收尾 / dev 接续读修订 prd，每路径列 happy + 2 个 unhappy）。

---

## 3. 关键决策的风险

### 决策 6：产物 = prd/design，砍 review.md

**有道理**：自产自销无意义，强制 review 改物料本身逼出可操作发现。

**被低估的风险**：

- **可追溯性丢失**：6 个月后想看「谁在第几轮提了哪个 issue」，PRD git history 不结构化
- **review-state.json 是否进 git / DB**：PRD 没明说。如果不进 git，跨 session 看历史依赖文件存在性；如果进 git，谁 commit、什么时机 commit？
- **半成品风险**：第 2 轮 reviewing 时主 LLM 崩了，PRD 已被部分修订，reviewer 第 3 轮看不到「修订起点」。建议加「修订原子化」约束。

### 决策 8：resume 硬前置

PRD 自己提了「resume 做不出来或延期，整个 epic 卡死」。但缓解只有「C1 是独立基建」。

C1 是**架构级改动**（持久化原始 messages vs 从 transcript 重建），不是几天能搞定：

- 方向 a（持久化原始 messages）：新增表 / 字段，要考虑 message 大小（>1MB）、token 截断、与 transcript 双写一致
- 方向 b（从 transcript 反向重建）：transcript 是 `TranscriptEntry[]`，要映射回 `ContentBlock`，涉及 tool_use / tool_result id 配对、cache_control 块重建，**极易出错**

**建议**：

- C1 必须在 review epic 开始**之前**有 PoC
- 或彻底放弃 resume 硬前置：v1 接受 token 浪费，resume 留 v2

### 决策 11：可视化数据基础 = 层次 2（主 LLM 提炼）

**这是整套设计最脆弱的一环**。

具体问题：

- **遗漏不报**：LLM 提炼时漏了一条 finding，**不会报错**。前端显示「本轮 3 个 finding」，实际还有 5 个被漏了。
- **维度漂移**：第 1 轮「清晰度 / 范围边界」，第 2 轮加「可测试性」，第 3 轮又改回——矩阵列对齐会乱。
- **verdict 主观性**：同一 finding，本轮 high，下轮可能 medium——review-state.json 不可信。
- **与 reviewer 原始 final_text 不一致**：用户展开 finding 看的是主 LLM 重写的话，不是 reviewer 原话。**UX 谎言**。

**缓解**：

1. 视图同时展示 review-state.json 提炼结果 **+** 链接到 reviewer 原始 final_text
2. 加 **verdict 一致性检查**：每轮写入前列出「本轮较上轮的变更」，让用户看到提炼过程的可信度
3. C3 验收加「人为注入测试」：reviewer final_text 含 5 个 finding，主 LLM 必须提炼出 5 个，漏 1 个就 fail

### 决策 5：主 LLM 一人多角（orchestrator + synthesizer + 修订者）

主 LLM 在 reviewing 派 reviewer，revising 综合 + 写 PRD + 写 review-state.json + askUserQuestion + 听用户指挥。**单轮里要干的事太多**，容易：

- 综合时漏 finding（决策 11 根因）
- 写 PRD 时忘记写 review-state.json
- askUserQuestion prompt 设计糟糕

**建议**：拆出 synthesizer subagent（只读 + 写 review-state.json），主 LLM 只做决策 + 修订 PRD。

---

## 4. 遗漏与盲点

### 4.1 模型 API 失败 / 超时

reviewing 并发派 N 个 reviewer，一个挂了怎么办？

- N-1 继续？
- 重试？
- Abort 整轮？

**建议**：wf-review-method 加「模型失败兜底」——单模型失败时 askUserQuestion 问「重试 / 跳过 / 替代 / abort 整轮」。默认策略：**≥2/3 成功进入 revising；<2/3 成功问用户**。

### 4.2 无限回环

`reviewing ↔ revising` 无 max-round cap，per-run-local loop detection（`loop_detection.rs`）**不跨轮**。

**建议**：

- workflow.json 加 `"max_rounds": 5`
- revising 达到上限强制 askUserQuestion「已收敛 / 放弃收敛 / 强行 reported」

### 4.3 resume 续接时 messages 含旧 PRD 内容

第 1 轮 reviewer 读过「prd §2 说 X」，第 2 轮主 LLM 修订把 §2 改成 Y，resume 续接的 reviewer messages 仍有「读到 X」的引用，reviewer 基于 X 评会困惑。

**建议**：resume 注入的「现状/变化/目的」**必须**显式列出 PRD 变更点（diff 或新 PRD 摘要），让 reviewer 先 update 上下文再评。

更稳妥：把 `clarification` 升级为结构化对象：

```jsonc
{
  "current_state": "当前 PRD 摘要",
  "changes_since_last": ["修改了目标和范围边界", "新增错误处理要求"],
  "this_round_purpose": "只检查修订是否解决上一轮 high severity findings"
}
```

### 4.4 review-state.json 写入时机与失败

- wf-synthesize 在 revising 末尾写 review-state.json，但**没有明确「revising 完成」vs「文件就绪」边界**
- 如果主 LLM 写文件失败（disk full / permission），revising 转 reported 后 C2 视图拿不到数据

**建议**：写文件成功才允许 state 转移；写失败留 revising 重试。

### 4.5 并发上限

`DELEGATION_MAX_CONCURRENT_CHILDREN` 默认 **3**，超过走 `OverLimit` 会拒绝执行。C3 的「模型多选」没给上限引导，用户选 4 个直接撞墙。

**建议**：

- UI 多选上限 ≤ 3
- 或超过上限自动分批串行
- 或部分成功策略：≥2/3 继续，<2/3 问用户

### 4.6 transcript_json 是截断展示格式，不是可续接 messages

`transcript_truncated=1` 超 4MB 只保留 head+tail。C1 R2「方向 b 从 transcript 重建」对长 run **实质不可行**。PRD 不应把两个方向当等价选项。

**建议**：默认选方向 a（持久化原始 LLM messages），方向 b 仅作 fallback。

### 4.7 review-state.json 是主 LLM 转述，无对账

视图（用户看到的「真相」）读的是主 LLM 提炼，与 reviewer 原始 final_text 可能不一致（漏记、张冠李戴、编 severity）。

**建议**：每条 finding 记 `source_run_id`，前端允许点击跳到原始 reviewer 产出。

### 4.8 dimensions 顶层单数组 vs 跨轮演化

C3 允许用户每轮增删维度，但 schema `dimensions` 是顶层单数组，无 per-round。

**建议**：每轮记自己的 `dimensions`，顶层保留 `dimension_catalog` 历史集合。

### 4.9 崩溃 / 状态恢复

daemon 在 reviewer 全部完成、未写 review-state.json 时崩溃：

- subagent_runs 留下 running 状态孤儿
- review-state.json 从未写入
- workflow state 与运行时 dispatch 状态不一致

**建议**：定义恢复策略；round 需要稳定 ID（不能只依赖递增数字）。

### 4.10 review/dev 共享 task，读写竞争

`resolve_current_task`（`inject.rs:229`）按 project 扫描，review/dev session 自动共享 task。若同时开 review（revising 写 PRD）和 dev（planning 读 PRD），就是同文件读写竞争。

**建议**：

- task 级 review/dev 锁
- 或 dev session 检测 review 状态并拒绝启动
- PRD 写入用临时文件 + rename 原子化

### 4.11 撕裂读（torn read）

前端轮询 vs 主 LLM 写文件，可能读到半截 JSON。LLM 本就可能写出非法 JSON。

**建议**：写入用 tmp + rename；前端解析失败进入「invalid」错误态。

### 4.12 模型 display_name 作 map key

`models: {<model>: ...}` 用 display_name 作键，重复选同模型会塌陷，重命名断裂。

**建议**：用稳定 model id（DB 已有），display_name 仅展示。

### 4.13 国际化未提

reviewer 输出、主 LLM 综合、review-state.json 字段都是中文吗？用户用英文 PRD 时整个流是否走英文？

**建议**：明确字段值语言跟随 PRD 语言；schema 字段名保持英文。

### 4.14 审查权限

用户在 reviewing 状态中途能否插话让 reviewer 停？还是只能在 revising 后用 askUserQuestion？

**建议**：定义 reviewing 中途插话的机制（cancel current run + 提前进入 revising）。

### 4.15 可观测性

每轮 token 用量、每 reviewer runtime、PR 命中率——这些 metrics 没规划。后续想优化「维度重合度」「max revisions」时没数据。

---

## 5. 可实施性 / 验收标准

### 5.1 大量模糊条目

| 条目 | 问题 |
|---|---|
| 父任务 AC「C1 落地：可用」 | 「可用」是模糊词。需要量化：3 个单测覆盖（空 history、错误 run_id、续接 messages 正确）|
| 父任务 AC「C2 落地：能呈现各 reviewer 发现」 | 「呈现」定性，无法判断完整 vs 漏 1/3 |
| C3 AC「revising：综合 + 修订 prd + 写 review-state.json + askUserQuestion」 | 4 件事并列，1 件没做怎么算 |
| C2 AC「review-state.json 每轮重写后视图增量更新」 | 「增量更新」标准？多快？错了怎么验？|

**建议**：每条 AC 改成 Given/When/Then 形式 + 可观测信号（DB 行数 / 文件存在 / DOM 节点 / SSE 事件类型）。

### 5.2 Open Questions 数量过多

C1 有 3 个 OQ、C3 有 3 个、C2 有 4 个。父任务自称「brainstorm 已收敛，无遗留」——**其实未收敛**。

**建议**：父任务加**Open Questions 总表**，列出跨子任务依赖的 OQ + 解决责任 + design deadline。

### 5.3 缺回滚 / 灰度

- review 插件上线后，怎么回退到只有 dev？
- builtin.rs 内置化后，review plugin 有 bug 怎么热禁用？
- 没 feature flag / 灰度策略。

**建议**：父任务加「回滚路径」段（disable 插件 / 不内置化，仅项目级）。

---

## 6. schema / API 契约完整性

### 6.1 review-state.json schema 严重不完整

PRD 中的样例 schema 缺：

1. **顶层**：`schema_version`（必须）、`task_id`、`session_id`、`created_at` / `updated_at`、`current_round`、`convergence`
2. **`verdict` 枚举**：`pass` / `pass_with_minor` / `revise` / `reject`（4 档，明确每档判定规则）
3. **`severity` 枚举**：`critical` / `high` / `medium` / `low` / `info`
4. **`location` 格式**：自由字符串（`prd.md §目标` 或 `prd.md:42`），前端只展示不解析
5. **`suggestion` 字段**：reviewer 建议的修订方向
6. **`seen_by` 字段**：去重与归并（多个 reviewer 发现同一问题时合并）
7. **稳定 `finding_id`**：`r{round}-m{model_slug}-{seq}`，支持 diff 跟踪
8. **`diff_from_previous`**：`resolved` / `new` / `still_open` 列表
9. **per-round `dimensions`**：每轮独立维度列表
10. **失败状态字段**：模型 run 状态 `completed` / `failed` / `timed_out` / `cancelled` / `truncated`

建议 schema 草案：

```jsonc
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$id": "everlasting://review-state.schema.json",
  "title": "ReviewState",
  "type": "object",
  "required": ["schema_version", "task_id", "session_id", "rounds", "current_round", "convergence"],
  "properties": {
    "schema_version": {"const": "1.0"},
    "task_id": {"type": "string"},
    "session_id": {"type": "string"},
    "created_at": {"type": "string", "format": "date-time"},
    "updated_at": {"type": "string", "format": "date-time"},
    "current_round": {"type": "integer", "minimum": 1},
    "convergence": {"enum": ["in_progress", "pass", "pass_with_minor", "revise", "reject", "abandoned"]},
    "rounds": {
      "type": "array",
      "items": {
        "type": "object",
        "required": ["round", "started_at", "prd_revision", "dimensions", "models", "diff_from_previous"],
        "properties": {
          "round": {"type": "integer", "minimum": 1},
          "started_at": {"type": "string", "format": "date-time"},
          "finished_at": {"type": "string", "format": "date-time"},
          "prd_revision": {"type": "integer", "description": "本轮评审时的 PRD 版本号"},
          "dimensions": {"type": "array", "items": {"type": "string"}, "minItems": 1},
          "models": {
            "type": "object",
            "additionalProperties": {"$ref": "#/$defs/ModelVerdict"}
          },
          "diff_from_previous": {
            "type": "object",
            "properties": {
              "resolved": {"type": "array", "items": {"type": "string"}, "description": "上一轮 finding_id 列表，已被本轮修订消解"},
              "new": {"type": "array", "items": {"type": "string"}, "description": "本轮新发现 finding_id 列表"},
              "still_open": {"type": "array", "items": {"type": "string"}}
            }
          }
        }
      }
    }
  },
  "$defs": {
    "ModelVerdict": {
      "type": "object",
      "required": ["model_id", "run_id", "status", "verdict", "findings"],
      "properties": {
        "model_id": {"type": "string", "description": "稳定模型 ID（DB 主键），不是 display_name"},
        "model_display": {"type": "string"},
        "run_id": {"type": "string", "description": "subagent_runs.id"},
        "status": {"enum": ["completed", "failed", "timed_out", "cancelled", "truncated"]},
        "verdict": {"enum": ["pass", "pass_with_minor", "revise", "reject"]},
        "summary": {"type": "string"},
        "findings": {
          "type": "array",
          "items": {
            "type": "object",
            "required": ["finding_id", "dimension", "severity", "issue"],
            "properties": {
              "finding_id": {"type": "string", "description": "stable id, format r{round}-m{model_slug}-{seq}"},
              "dimension": {"type": "string"},
              "severity": {"enum": ["critical", "high", "medium", "low", "info"]},
              "issue": {"type": "string"},
              "suggestion": {"type": "string"},
              "location": {"type": "string"},
              "source_run_id": {"type": "string"}
            }
          }
        }
      }
    }
  }
}
```

### 6.2 dispatch_subagent resume API

C1 R4 给的 `resume_from: <run_id>` 不够。建议：

```jsonc
{
  "resume_from": "<run_id>",
  "resume_clarification": {
    "current_state": "...",
    "changes_since_last": "...",
    "this_round_purpose": "..."
  }
}
```

错误码：

- `resume_run_not_found`
- `resume_run_still_running`（不允许 resume running）
- `resume_run_other_session`（跨 session 限制）
- `resume_run_other_project`（project 边界）

### 6.3 前后端事件契约

C2 R3「实时更新」没说机制（轮询 / IPC 推送 / 文件监听）。**必须显式定义**：

```typescript
// app/src/transport/events/review.ts
export type ReviewStateUpdatedEvent = {
  sessionId: string;
  taskId: string;
  path: string;
  currentRound: number;
  totalFindings: number;
  triggeredAt: string;
};
```

daemon 端点：

```text
GET /api/v1/tasks/:task_id/review-state
→ 200 { ok: true, state: ReviewState }
→ 404 { ok: false, error: "missing" }
→ 422 { ok: false, error: "invalid", detail: "..." }
```

前端三态：loading / missing / invalid + error toast。

---

## 7. 关键路径建议

如果是我来重排：

### Phase A：spike（先验证假设）

1. **C0-α（半天）**：跑 3 模型评同一份真实 PRD，统计 finding 重合度 + 人评主 LLM 提炼质量。
   - **重合度 > 70% 或提炼质量差 → 砍多模型流，做单模型多轮**。
2. **C0-β（半天）**：用 transcript 反向重建 messages，跑一个真实 resume 场景。
   - **失败 → 砍 resume 硬依赖，review v1 接受 token 浪费**。

### Phase B：基线 review v1（不带 resume）

C1 独立化为基建 epic；review v1 不依赖 C1，每轮 reviewer 全新派。

### Phase C：resume 接入

C1 落地后，C3 升级 reviewer 支持 resume。

### Phase D：可视化

C2 在 schema + 事件契约定稿后实施。

### Phase E：灰度与回滚

父任务加 review plugin 灰度开关，bug 时可热禁用。

---

## 8. 验收标准改写示例

### 8.1 C1 subagent resume

- **Given** 已完成 worker run `run-A`（status=completed，5 条 messages）
- **When** 主 LLM dispatch 新 worker，参数 `{ resume_from: "run-A", resume_clarification: {...} }`
- **Then** 新 worker 的初始 messages = `run-A` messages + 结构化 clarification
- **And** 不存在的 run_id 返回 `error.code = "resume_run_not_found"`
- **And** running 状态的 run 返回 `error.code = "resume_run_still_running"`
- **And** review reviewer resume **不**复用 worktree
- **And** dev implementer resume **复用**同一 worktree
- **And** 不带 `resume_from` 时行为与现状完全一致（snapshot 测试）

### 8.2 C2 review 可视化

- **Given** `review-state.json` fixture：3 轮 × 3 模型 × 12 findings（含 1 个失败模型）
- **When** 组件 `<ReviewMatrix taskId="..." />` 挂载
- **Then** 矩阵 3 行 × 3 列，verdict chip 正确，findings 数正确
- **And** 失败模型列灰显 + tooltip「此模型未完成」
- **And** 维度对比视图：选定「清晰度」时跨模型横向呈现 findings
- **And** finding 可展开看 issue + location + 跳转到 `run_id` 原始 final_text
- **And** 非 review session 不挂载
- **And** 非法 JSON 进入错误态 + toast

### 8.3 C3 集成验收（mock provider）

```text
intake → 选 2 模型 → reviewing 并发 2 run
→ revising 改 PRD → 写 review-state.json（schema validate pass）
→ 用户「再评」 → reviewing 第 2 轮（reviewer resume）
→ 用户「定稿」 → reported
→ 新 dev session 读修订后 PRD
```

每步断言：

- DB：`subagent_runs` 行数 / status
- 文件：`review-state.json` 存在 / schema 合法 / round 递增
- 事件：`review-state-updated` SSE 触发
- 状态：workflow state 转移序列
- PRD：内容被修改 + version 递增

---

## 9. 关键风险总览

| 风险 | 严重性 | 来源 |
|---|---|---|
| 数据时序矛盾：reviewing 阶段无 review-state.json | **致命** | 决策 11 + C3 R7 |
| resume 旧 PRD 上下文污染新一轮评审 | **致命** | 决策 8 |
| C1 延期卡死整个 epic | **致命** | 任务拆分 |
| 并发上限 3 与多选无限冲突 | **严重** | C3 R3 |
| 主 LLM 提炼漏 / 错 | **严重** | 决策 11 |
| 无限回环无 cap | **严重** | 决策 3 |
| review-state.json 重写而非 append，历史丢失 | **严重** | C3 R7 |
| 半成品 PRD 污染 dev session | 中 | 决策 6 |
| 维度跨轮演化无法表达 | 中 | C3 R7 |
| 崩溃恢复无策略 | 中 | C3 R3 |
| review/dev 共享 task 读写竞争 | 中 | C3 Background |
| schema 无版本号 | 中 | C3 R7 |
| 模型 display_name 作 key | 低 | C2 R1 |
| reviewer 200 turn 上限截断 | 低 | dispatch.rs:84 |

---

## 10. 一句话总结

> **方向正确，但当前 PRD 还是「产品愿景 + happy path」，还不是可安全实施的工程契约。**
> **建议先做两个 spike（多模型重合度 + resume PoC），并在 design 阶段解决数据时序、resume 版本一致性、失败恢复与 schema 版本化，再进入 C1/C3/C2。**

---

## 11. 相关文件路径

- 父任务 PRD：`.trellis/tasks/07-26-workflow-review-plugin/prd.md`
- C1 PRD：`.trellis/tasks/07-26-subagent-resume/prd.md`
- C3 PRD：`.trellis/tasks/07-26-review-plugin-pack/prd.md`
- C2 PRD：`.trellis/tasks/07-26-review-viz/prd.md`
- 代码锚点核对（用于修正行号）：
  - `app/src-tauri/src/agent/subagent/mod.rs:642`（`build_worker_messages` 实际定义）
  - `app/src-tauri/src/agent/subagent/dispatch.rs:492/679`（per-dispatch model override）
  - `app/src-tauri/src/agent/workflow/def.rs:127`（`Coordination::SynthesisRound`）
  - `app/src-tauri/src/db/migrations.rs:740/1350`（`model_display` / `transcript_json`）
  - `app/src-tauri/src/agent/chat_loop.rs:4269-4293`（并发上限）
  - `app/src-tauri/src/agent/subagent/truncate_summary.rs:32`（transcript 截断）
  - `app/src-tauri/src/agent/subagent/dispatch.rs:84,645`（max turns / build_worker_messages）
  - `app/src-tauri/src/agent/workflow/inject.rs:229`（resolve_current_task）
  - `app/src-tauri/src/agent/subagent/transcript.rs:18-27`（transcript 非 messages）