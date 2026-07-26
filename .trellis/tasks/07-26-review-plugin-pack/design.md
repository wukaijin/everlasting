# Design: review plugin resource pack (C3)

> 父任务：`07-26-workflow-review-plugin`
> 配套：`prd.md`
> 依赖：C1（reviewer resume）—— reviewer.md 的 resume 行为依赖 C1 落地。
> 本 design 落定 PRD Open Questions（reviewer 输出细则 / schema 细则 / resume clarification 模板）+ 给出 workflow.json/reviewer.md/skill 的具体形态。

## 0. 调研事实（C3 内置化扩展点）

| 事实 | 位置 | 扩展方式 |
|---|---|---|
| `builtin_plugin_skills` 硬编码 `if workflow_name != "dev"` | `skill/loader.rs:463` | 扩成 match "dev"\|"review"，加 `BUILTIN_REVIEW_SKILLS` 分支 |
| `BUILTIN_DEV_SKILLS`/`BUILTIN_DEV_AGENTS` 常量 + `BUILTIN_PLUGIN_NAMES=&["dev"]` | `workflow/builtin.rs` | 追加 `BUILTIN_REVIEW_*` 常量组 + 清单加 "review" |
| `builtin_workflow_json` match "dev" | `workflow/builtin.rs` | 加 "review" 分支 |
| subagent loader 同模式硬编码 dev | `agent/subagent/loader.rs`（BuiltinPlugin 处理） | 同样扩 review 分支 |
| dev workflow.json 各字段写法（breadcrumb/delegation_templates/coordination/gather_strategy） | `resources/builtin-workflow/dev/workflow.json` | review 照此结构写 |
| skill frontmatter：`name`/`description`/`allowed-tools` | dev skills/*.md | review skills 照此 |
| dispatch_subagent 的 model enum（主 LLM 发现模型的渠道） | `subagent/mod.rs:381` schema | wf-review-prep 指引主 LLM 从此 enum 读模型 |

## 1. workflow.json（4 state 带回环）

PRD R1 已给骨架。design 补充各字段具体内容（参考 dev 风格）：

```jsonc
{
  "name": "review",
  "description": "多模型评审流:评审需求/计划 → 修订 → 回环重评,过程可视化,用户指挥收敛",
  "states": ["intake", "reviewing", "revising", "reported"],
  "initial": "intake",
  "transitions": [
    {"from": "intake", "to": "reviewing", "requires_user_confirm": true},
    {"from": "reviewing", "to": "revising", "requires_user_confirm": true},
    {"from": "revising", "to": "reviewing", "requires_user_confirm": true},
    {"from": "revising", "to": "reported", "requires_user_confirm": true}
  ],
  "roles_by_state": {
    "intake": [],
    "reviewing": ["reviewer"],
    "revising": [],
    "reported": []
  },
  "breadcrumb": {
    "intake": "[Wf · intake · review] 读 current_task(prd/design/progress)理解评审对象;从 dispatch_subagent 的 model enum 看可用模型;askUserQuestion 让用户多选评审模型;按任务种类推荐维度并确认。完成后请用户确认转 reviewing",
    "reviewing": "[Wf · reviewing · review] 按 wf-review-method 选定的维度,并发派多个 reviewer(各不同模型,用 dispatch_subagent 的 model 参数 + resume_from 续接上轮)。等全部返回后请用户确认转 revising。若某模型失败,标注缺失(写 review-state.json 的 status=failed)",
    "revising": "[Wf · reviewing · review] 综合 N 份评审(按维度横向对比、triage adopt/reject);修订 prd/design;写 <task>/review-state.json(含 change_log + convergence_note);askUserQuestion 问用户「再评一轮还是定稿」。再评→回 reviewing;定稿→reported",
    "reported": "[Wf · reported · review] prd 已修订就绪。用户可切 dev session 实施(dev 读同一 prd,task 共享)"
  },
  "delegation_templates": {
    "reviewer": "你是 review workflow 的 reviewer 子代理。当前 task: {title}\nSummary: {summary}\nState: reviewing\n相关 spec 路径: {relevant_specs}\n\n请按确认的评审维度评审 prd/design,有权读项目代码做「设计 vs 实现一致性」检查。输出按维度分节 + 总体结论。"
  },
  "coordination": "synthesis_round",
  "gather_strategy": {"reviewing": ["reviewer"]}
}
```

**设计要点**：
- breadcrumb 是主 LLM 的 per-turn 指引，把 PRD 决策（模型从 enum 发现、resume 续接、写 review-state.json、triage）都点进去。
- delegation_templates.reviewer 是 dispatch 时注入 worker messages[0] 的角色模板（dev 的 researcher/implementer/checker 同模式）。
- `coordination: synthesis_round` 作 prompt 提示（B2-α，引擎不强制）。

## 2. reviewer.md（角色，PRD R2 落定）

```markdown
---
name: reviewer
description: "评审子代理 — 读 prd/design + 项目代码 + 按维度给评审意见"
# 只读(无写工具),不需要隔离 worktree —— 同 dev researcher 理由。
# model: 留空,由 dispatch_subagent 的 model 参数(per-dispatch override)主导。
---

# review workflow · reviewer

你是 review workflow 的 reviewer 子代理。当前 task: {title}
State: reviewing

## 目标

按确认的评审维度评审 prd/design,产出结构化评审意见。**只评审不修改** — 你的输出给主 LLM 综合。

## 评审范围

- **读 prd/design**(评审主对象):task 目录下的 prd.md / design.md / progress.md
- **读项目代码**(设计 vs 实现一致性):可用 read/grep/glob 探索 codebase,判断设计是否可行、是否与现有实现冲突
- **不修改任何文件**(只读角色)

## 输出格式(便于主 LLM 综合时按维度横向对比)

按本轮确认的维度逐节输出,每节含发现 + severity + 建议:

### 维度1: <维度名>
- [severity: high/medium/low] <具体问题> — <location,如 prd.md§2>
- [severity: ...] ...
- 建议: <修订方向>

### 维度2: <维度名>
...

## 总体结论
<通过 / 有条件通过 / 打回> + 一句话理由

## 约束

- ✅ 可使用:read_file / grep / glob / list_dir / web_fetch
- ❌ 不修改任何文件
- ❌ 不 dispatch 子代理
- **若上轮对话引用与当前文件内容矛盾,以当前文件为准**(resume 续接的 stale context 处理)
```

**输出格式说明**：reviewer 输出是自由 markdown（层次 2 决策,主 LLM 在 revising 提炼成 review-state.json）。上面的格式是「软约束」——reviewer 遵守得越好,主 LLM 提炼越准;但即使不完美,主 LLM 兜底。severity/location 字段让主 LLM 能结构化提取。

## 3. skills（4 个，PRD R3 落定）

### wf-overview（review 全貌）

参考 dev wf-overview 结构。核心内容:
- review 整体流程图(4 state 带回环)
- 主 LLM 的 orchestrator 职责(intake 准备 / reviewing 派 reviewer / revising 综合修订 / reported 收尾)
- 多模型评审心智:N 个不同模型同题对比,价值在分歧
- task 共享机制(review/dev 共享 current_task,prd 是衔接物)

### wf-review-prep（intake，模型发现 + 维度确认）

**模型发现**（PRD R3 关键澄清）:主 LLM **无法直接调 `list_models` 内部 API**。发现模型的渠道是 `dispatch_subagent` 工具的 `model` 参数 enum（`chat_loop.rs:681` 已构建动态 enum,display_name 来自 list_models 快照）。skill 指引主 LLM:
1. 看 dispatch_subagent 的 model enum → 得到可用模型 display_name 列表
2. askUserQuestion 让用户多选(建议跨 provider,但非强制)
3. 多选结果存内存,reviewing 派 reviewer 时用这些 display_name 作 model override

**维度推荐**(维度推荐器):按任务种类推荐基础维度组合 + 可选维度池 + askUserQuestion 确认增删(详见 PRD R3 表)。

### wf-review-method（reviewing，维度推荐器细则）

维度推荐表(PRD R3 已给)的核心实现。按 current_task 的 prd/title 判断任务种类 → 推荐维度。可选维度池补充「其他可能性」。

### wf-synthesize（revising，综合 + triage + 写 review-state.json）

最复杂的 skill。核心内容:
1. **综合方法**:按维度横向对比 N 份 reviewer 输出,标注分歧(同维度不同模型看法),提炼共识
2. **triage 决策**(评审回流):每条 finding 标 adopt/reject/defer + reason;reject 要对照已知约束(brainstorm 决策、项目既定方向)——评审者常缺决策上下文,主 LLM 带上下文判断
3. **修订 prd**:据 adopt 的 finding 修订 prd/design(主 LLM 有写工具)
4. **写 review-state.json**(C2 数据源,见 §4 schema)
5. **convergence 评估**:本轮 vs 上轮 finding 数/severity 趋势,主动建议定稿(软引导,非硬 cap)
6. **askUserQuestion**:问用户「再评一轮还是定稿」,附 convergence_note

## 4. review-state.json schema（PRD R7 + 评审回流，C2 跨任务契约）

PRD R7 已给 schema（含 schema_version/finding_id/source_run_id/triage/change_log/convergence_note/status/models_present/per-round dimensions/model_id key）。design 补充写入流程:

### 写入时机

wf-synthesize 在 revising state,主 LLM 完成综合+修订后,**用 review-only `emit_review_state_updated` 工具写 `<task>/review-state.json`**(不用通用 write_file,见下)。每轮 revising 重写整个文件,rounds 数组累积。

### 写入约束（评审回流：write_file 不原子 + 事件发送点）

- **原子化 + 事件发送（MiniMax 方案 iii）**：`write_file` 工具实际是 `tokio::fs::write`(`write_file.rs:163`),**非 tmp+rename**,中途读会拿到半截 JSON。采用新建 review-only `emit_review_state_updated` 工具(仿 `ask_user_question` 模式,拿 `ChatEventSink`),内部:
  1. tmp + rename 原子写 review-state.json(仿 `task.rs:373 write_task`)
  2. 写成功后发 `review-state-updated` 事件(解决 C2 事件发送点,零 dev 污染 —— 工具只在 review workflow 可见,用 `filter_tools_for_workflow` gate)
  - 一举解决原子性 + 事件发送点 + 零 dev 污染。wf-synthesize 指引主 LLM 调此工具而非 write_file。
- **纯 JSON**:工具内部校验主 LLM 提供的 JSON 字符串合法性(serde_json::from_str 校验),非法则工具返回错误让主 LLM 重写。
- **schema 合法**:wf-synthesize skill 强调按 schema 写字段名/枚举值。

### 字段值语言

- schema 字段名:英文（schema 约定）
- 字段值（issue/suggestion/change_log/convergence_note）:跟随 prd 语言（prd 中文则中文,英文则英文）

## 5. builtin.rs 内置化扩展（PRD R4）

### 新增资源目录

```
app/src-tauri/resources/builtin-workflow/review/
├── workflow.json          # §1 内容
├── agents/reviewer.md     # §2 内容
└── skills/
    ├── wf-overview/SKILL.md
    ├── wf-review-prep/SKILL.md
    ├── wf-review-method/SKILL.md
    └── wf-synthesize/SKILL.md
```

### builtin.rs 扩展

```rust
pub const BUILTIN_REVIEW_WORKFLOW_JSON: &str =
    include_str!("../../../resources/builtin-workflow/review/workflow.json");

pub const BUILTIN_REVIEW_SKILLS: &[(&str, &str)] = &[
    ("wf-overview", include_str!("...wf-overview/SKILL.md")),
    ("wf-review-prep", include_str!("...wf-review-prep/SKILL.md")),
    ("wf-review-method", include_str!("...wf-review-method/SKILL.md")),
    ("wf-synthesize", include_str!("...wf-synthesize/SKILL.md")),
];

pub const BUILTIN_REVIEW_AGENTS: &[(&str, &str)] = &[
    ("reviewer", include_str!("...agents/reviewer.md")),
];

pub const BUILTIN_PLUGIN_NAMES: &[&str] = &["dev", "review"];  // 追加 review

pub fn builtin_workflow_json(name: &str) -> Option<&'static str> {
    match name {
        "dev" => Some(BUILTIN_DEV_WORKFLOW_JSON),
        "review" => Some(BUILTIN_REVIEW_WORKFLOW_JSON),  // 追加
        _ => None,
    }
}
```

### skill/loader.rs + subagent/loader.rs 扩展

`builtin_plugin_skills`（skill/loader.rs:462）从 `if workflow_name != "dev"` 改为 match:
```rust
fn builtin_plugin_skills(workflow_name: &str) -> Vec<SkillResource> {
    let skills: &[(&str, &str)] = match workflow_name {
        "dev" => crate::agent::workflow::BUILTIN_DEV_SKILLS,
        "review" => crate::agent::workflow::BUILTIN_REVIEW_SKILLS,
        _ => return Vec::new(),
    };
    skills.iter().filter_map(|(slug, body)| {
        let mut res = parse_skill_content(body, slug, SkillSource::BuiltinPlugin)?;
        res.path = PathBuf::from(format!("<builtin>/{workflow_name}/skills/{slug}/SKILL.md"));
        Some(res)
    }).collect()
}
```

subagent loader 的 BuiltinPlugin 处理同模式扩展（dev + review）。

## 6. dev skill 衔接指引（PRD R5）

改 dev 的 `wf-brainstorm` / `wf-overview`,加一句:「prd 可能已被 review session 修订过,planning 注意读最新 prd」。

具体位置:dev skills/wf-brainstorm/SKILL.md 的「写 prd.md」段开头;dev skills/wf-overview/SKILL.md 的「planning」段。

## 7. 影响面 + 回归风险

### 改动文件
- 新增:`resources/builtin-workflow/review/*`（workflow.json + reviewer.md + 4 skill）
- 改:`workflow/builtin.rs`（追加 review 常量组 + NAMES + match 分支）
- 改:`skill/loader.rs`（builtin_plugin_skills 扩 match）
- 改:`agent/subagent/loader.rs`（BuiltinPlugin 处理扩 match）
- 改:`resources/builtin-workflow/dev/skills/wf-brainstorm.md` + `wf-overview.md`（衔接指引）
- 同步:`.everlasting/workflow/review/`（项目示例,人工同步）

### 回归风险
- **现有 dev plugin 不受影响**:BUILTIN_PLUGIN_NAMES 加 review 是并集;list_plugins 返回多一个名字;dev 路径完全不变。
- **内置化测试**:builtin.rs 单测要加 review 组（workflow.json 过 validate + skills/agents body 非空 + frontmatter 含 name）。
- **schema 契约**:review-state.json schema 是 C2 硬依赖,本任务的 schema 定稿后 C2 才能 implement。

### 单测
- `BUILTIN_REVIEW_WORKFLOW_JSON` 过 `validate()` + 4 state + 回环 transition 合法
- `BUILTIN_REVIEW_SKILLS` 4 个 body 非空 + frontmatter 含 name
- `BUILTIN_REVIEW_AGENTS` reviewer body 非空 + frontmatter 含 name + model 留空
- `list_plugins` 返回含 "review"
- `builtin_plugin_skills("review")` 返回 4 个 SkillResource
- `builtin_workflow_json("review")` 返回 Some

## 8. Open Questions 落定

1. ✅ **reviewer 输出细则**（PRD OQ1）:见 §2,维度分节 + severity/location/建议,自由 markdown 软约束（主 LLM 兜底提炼）。
2. ✅ **schema 细则**（PRD OQ2）:PRD R7 已给完整 schema,本 design §4 补写入流程 + 语言策略。
3. ✅ **resume clarification 模板**（PRD OQ3）:依赖 C1 的 resume API（resume_clarification: current_state/changes_since_last/this_round_purpose）。wf-synthesize 指引主 LLM 在派下一轮 reviewer 时构造此 clarification（current_state=修订后 prd 摘要,changes_since_last=本轮修订点,this_round_purpose=验证上轮 high severity 是否解决）。
