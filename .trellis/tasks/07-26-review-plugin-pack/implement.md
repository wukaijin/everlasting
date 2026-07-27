# Implement: review plugin resource pack (C3)

> 父任务：`07-26-workflow-review-plugin`
> 配套：`prd.md` + `design.md`
> 依赖：C1（reviewer resume）—— reviewer.md 的 resume 行为依赖 C1 落地，但 C3 的资源包可先写（resume 是 dispatch 时 C1 提供的能力，workflow.json/builtin 不阻塞）。
> 本文件是 ordered execution checklist + 验证命令 + 风险点。

## 执行顺序

资源文件 → builtin.rs 扩展 → loader 扩展 → dev skill 衔接 → 测试。先写纯资源（不依赖代码），再接内置化机制。

---

## Phase 1：资源文件（纯文件，无代码依赖）

### 步骤 1.1 — 新建 builtin-workflow/review/ 目录结构

**新增文件**：
```
app/src-tauri/resources/builtin-workflow/review/
├── workflow.json           # design.md §1 内容
├── agents/reviewer.md      # design.md §2 内容
└── skills/
    ├── wf-overview/SKILL.md
    ├── wf-review-prep/SKILL.md
    ├── wf-review-method/SKILL.md
    └── wf-synthesize/SKILL.md
```

每个 skill 文件顶部 frontmatter（参考 dev skills）：
```markdown
---
name: wf-overview   # 或 wf-review-prep/wf-review-method/wf-synthesize
description: <一句话>
allowed-tools: []
---
```

**关键内容来源**：
- workflow.json：design.md §1 完整 jsonc
- reviewer.md：design.md §2 完整内容（含只读 + 读代码 + stale context 提示 + 输出格式）
- wf-overview：design.md §3 第一条（流程图 + orchestrator 职责 + 多模型心智 + task 共享）
- wf-review-prep：design.md §3 第二条（**模型从 dispatch_subagent 的 model enum 发现** + askUserQuestion 多选 + 维度推荐）
- wf-review-method：design.md §3 第三条（维度推荐表细则）
- wf-synthesize：design.md §3 第四条（综合 + triage + 修订 prd + 写 review-state.json + convergence + askUserQuestion）—— 最复杂，参考 C3 design.md §4 写入约束

**验证**：
```bash
# workflow.json 合法性（手动 jq 或后续 Phase 5 单测覆盖）
cat app/src-tauri/resources/builtin-workflow/review/workflow.json | python3 -m json.tool > /dev/null
# frontmatter 含 name（手动 head）
for f in app/src-tauri/resources/builtin-workflow/review/skills/*/SKILL.md app/src-tauri/resources/builtin-workflow/review/agents/reviewer.md; do echo "=== $f ==="; head -3 "$f"; done
```

---

## Phase 2：builtin.rs 扩展

### 步骤 2.1 — 追加 review 常量组 + NAMES + match 分支

**文件**：`app/src-tauri/src/agent/workflow/builtin.rs`

**改动**（design.md §5）：
1. 加 `BUILTIN_REVIEW_WORKFLOW_JSON`（include_str! review/workflow.json）
2. 加 `BUILTIN_REVIEW_SKILLS`（4 个 (slug, body) 元组）
3. 加 `BUILTIN_REVIEW_AGENTS`（reviewer 元组）
4. `BUILTIN_PLUGIN_NAMES` 追加 `"review"`：`&["dev", "review"]`
5. `builtin_workflow_json` match 加 `"review" => Some(BUILTIN_REVIEW_WORKFLOW_JSON)`

**验证**：
```bash
cd app/src-tauri && cargo build --lib 2>&1 | tail -5
# 期望：include_str! 路径正确，编译通过
```

---

## Phase 3：loader 扩展（skill + subagent）

### 步骤 3.1 — skill/loader.rs builtin_plugin_skills 扩 match

**文件**：`app/src-tauri/src/skill/loader.rs:462`

**改动**（design.md §5）：从 `if workflow_name != "dev"` 改为 match "dev"|"review"，参考 design.md §5 伪代码。

### 步骤 3.2 — subagent/loader.rs BuiltinPlugin 扩 match

**文件**：`app/src-tauri/src/agent/subagent/loader.rs`（BuiltinPlugin 处理处）

**改动**：同 3.1 模式，从硬编码 "dev" 扩成 match "dev"|"review"，加 `BUILTIN_REVIEW_AGENTS` 分支。

**验证**（3.1 + 3.2）：
```bash
cd app/src-tauri && cargo test --lib builtin_plugin 2>&1 | tail -10
cd app/src-tauri && cargo test --lib loader 2>&1 | tail -10
# 期望：现有 dev 测试不破坏；review 路径能解析
```

---

## Phase 4：dev skill 衔接指引

### 步骤 4.1 — 改 dev wf-brainstorm + wf-overview

**文件**：
- `app/src-tauri/resources/builtin-workflow/dev/skills/wf-brainstorm/SKILL.md`
- `app/src-tauri/resources/builtin-workflow/dev/skills/wf-overview/SKILL.md`

**改动**（design.md §6）：在 wf-brainstorm 的「写 prd.md」段开头、wf-overview 的 planning 段，加一句：「prd 可能已被 review session 修订过，planning 注意读最新 prd」。

同步改项目示例 `.everlasting/workflow/dev/skills/` 对应文件（同 dev 约定，人工同步）。

**验证**：手动 read 确认文案加入；不影响 dev 现有行为（只是 prompt 多一句）。

---

## Phase 5：项目示例同步 + 单测

### 步骤 5.1 — 同步项目示例

**新增**：`.everlasting/workflow/review/`（workflow.json + agents/reviewer.md + 4 skill），内容与 builtin 源一致（同 dev 约定，人工同步，README 注明 source of truth 在 builtin 源）。

### 步骤 5.2 — builtin.rs 单测

**文件**：`app/src-tauri/src/agent/workflow/builtin.rs`（tests mod）

**改动**：参考 dev 的单测，加 review 组：
- `builtin_review_workflow_json_validates`：解析 + validate() + 4 state + 回环 transition 合法
- `builtin_review_skills_nonempty_with_frontmatter`：4 个 body 非空 + frontmatter 含 name
- `builtin_review_agents_nonempty_with_frontmatter`：reviewer body 非空 + frontmatter 含 name + model 字段缺失（留空）

### 步骤 5.3 — list_plugins + loader 单测

- `list_plugins` 返回含 "review"（现有测试断言可能要更新，参考 07-09 builtin 任务对 dev 测试的调整方式）
- `builtin_plugin_skills("review")` 返回 4 个 SkillResource
- `builtin_workflow_json("review")` 返回 Some

**验证**：
```bash
cd app/src-tauri && cargo test --lib builtin 2>&1 | tail -10
cd app/src-tauri && cargo test --lib list_plugins 2>&1 | tail -10
```

---

## Phase 6：全量验证

```bash
cd app/src-tauri
cargo test --lib 2>&1 | tail -20
cargo clippy --lib --tests -- -D warnings 2>&1 | tail -10
```

**回归重点**：
- dev plugin 完全不受影响（BUILTIN_PLUGIN_NAMES 加 review 是并集）。
- 现有 builtin/loader 测试全绿。
- review workflow.json 过 validate（4 state + 回环 transition + 空 roles 合法，C1 design 已验证引擎支持）。

---

## 风险点

### R1（中）：wf-synthesize skill 复杂度
**问题**：wf-synthesize 要教主 LLM 综合 + triage + 修订 prd + 写 review-state.json + convergence + askUserQuestion,是 4 个 skill 里最复杂的。prompt 写不好主 LLM 会漏步骤（漏写 review-state.json / 漏 triage）。
**缓解**：skill 用清晰的步骤清单（① 综合 ② triage ③ 修订 prd ④ 写 review-state.json ⑤ convergence 评估 ⑥ askUserQuestion），每步给判断标准。implement 时可参考 dev wf-check 的结构化写法。

### R2（中）：review-state.json schema 与 C2 的跨任务契约
**问题**：C3 写 schema、C2 读 schema,任一方字段名/枚举值变动要同步另一方。C2 的 TS 类型（C2 design §7）必须与 C3 R7 schema 一一对应。
**缓解**：C3 定稿 schema 后,C2 implement 时严格按 C3 schema 写 TS 类型。建议 C3 schema 字段名/枚举值一旦定稿不再改（schema_version 字段为未来演化留位）。

### R3（低）：reviewer.md 输出格式软约束不可靠
**问题**：reviewer 是自由 markdown 输出,不保证按 §2 格式。主 LLM 提炼时可能因 reviewer 输出混乱而漏 finding。
**缓解**：这是层次 2 的已知 trade-off（层次 1 做不到,层次 3 过度工程）。reviewer.md prompt 尽量明确格式 + 主 LLM 兜底提炼。C2 的 source_run_id 跳转让用户能对比原话,是安全网。

---

## Follow-up（C3 范围外）

- resume 接入：reviewer.md 提到 resume 续接,实际行为依赖 C1 落地。**C1 已合并(703ab7d)**,reviewer 可用 `resume_from` 续接。
- review-state.json 写入原子化:本任务用通用 write_file(非原子,tokio::fs::write)。若未来 C2 前端读到半截 JSON 成实际问题,再考虑:(a) 新建 review-only `emit_review_state_updated` 工具(tmp+rename 原子写 + 发事件),或 (b) 把 write_file 全局改为 tmp+rename。两者都是另立 task,不在 C3。design §4 已记录此决策。

---

## 验证命令汇总

```bash
cd app/src-tauri
cargo build --lib                              # Phase 2 编译
cargo test --lib builtin_plugin                # Phase 3 loader
cargo test --lib loader                        # Phase 3 loader
cargo test --lib builtin                       # Phase 5 单测
cargo test --lib list_plugins                  # Phase 5 单测
cargo test --lib                               # Phase 6 全量
cargo clippy --lib --tests -- -D warnings      # Phase 6 lint
```
